//! Networking for [`InGameState`]: applying what arrived, deciding what to say.
//!
//! Transport itself lives behind [`Session`](crate::application::session::Session) —
//! this module never touches a socket. It splits into three parts:
//!
//! - [`InGameState::pump_network`] drives one frame: drain, apply, speak, flush.
//! - `apply_*` interpret one [`Inbound`] against the world, the players and the
//!   mobs. They are ordinary `&mut self` methods, testable against a
//!   [`FakeSession`](crate::application::session::FakeSession).
//! - The free functions convert between the in-memory model and the protocol's
//!   wire types; several are `pub(super)` so construction, persistence, and
//!   tests in sibling modules can reuse them.

use std::collections::HashMap;
use std::time::Duration;

use glam::Vec3;

use super::mobs;
use super::{
    HOST_PLAYER_ID, INVENTORY_SYNC_INTERVAL, InGameState, STATS_INTERVAL, WORLD_SYNC_BATCH,
};
use crate::application::ecs::Ecs;
use crate::application::ecs::components::{Health, Kind, Mob, RemotePlayer, Transform};
use crate::application::ecs::systems::mobs as mob_systems;
use crate::application::ecs::systems::players;
use crate::application::ecs::{With, spawn};
use crate::application::session::Inbound;
use crate::domain::core::{BlockId, BlockPos};
use crate::domain::entity::MobId;
use crate::domain::inventory::crafting::{KnownItems, NamedRecipe, resolve_named};
use crate::domain::inventory::{ARMOR_START, Inventory, ItemId, ItemRegistry, RecipeBook, Tool};
use crate::infrastructure::net::{
    Channel, ClientMessage, Equipment, NetItemStack, PlayerId, PlayerRestore, RecipeData,
    ServerMessage,
};
use crate::infrastructure::save::{ItemStackData, PlayerData, PlayerRecords};

impl InGameState {
    /// Drive networking for one frame: drain the transport and apply what
    /// arrived, then say this frame's piece and flush.
    pub(super) fn pump_network(&mut self, dt: f32) {
        let duration = Duration::from_secs_f32(dt.max(1.0e-4));

        // Survival stats are low-frequency; throttle them to keep the wire quiet.
        self.net.peers.stats_timer += dt;
        let send_stats = self.net.peers.stats_timer >= STATS_INTERVAL;
        if send_stats {
            self.net.peers.stats_timer = 0.0;
        }

        for msg in self.net.session.poll(duration) {
            self.apply_inbound(msg);
        }

        if self.net.session.is_authority() {
            self.broadcast_authority_state(send_stats);
        } else {
            // A client decides nothing, so it has nothing to tell anyone.
            self.sim.outbox.clear();
            self.report_to_host(dt, send_stats);
        }
        self.net.session.flush();
    }

    /// Apply one message from the network. Requests arrive on the authority and
    /// are validated here; updates arrive on a client and are already truth.
    fn apply_inbound(&mut self, inbound: Inbound) {
        match inbound {
            Inbound::Joined {
                player,
                identity,
                account,
            } => self.welcome_player(player, identity, account),
            Inbound::Left { player } => self.forget_player(player),
            Inbound::Request { player, msg } => self.apply_request(player, msg),
            Inbound::Update(msg) => self.apply_update(msg),
        }
    }

    // --- Host: connection lifecycle -------------------------------------------------

    /// Admit a joining peer: hand back their saved state if this world
    /// remembers them, and give them a replica to be tracked by.
    ///
    /// It does *not* announce them — see [`InGameState::announce_player`].
    fn welcome_player(
        &mut self,
        pid: PlayerId,
        identity: u64,
        account: Option<wyven_auth::AccountIdentity>,
    ) {
        let restored = self.save.records.0.get(&identity).map(|record| {
            let known = self.save.discovery.players.get(&identity);
            record_to_restore(
                record,
                known.map_or(&[], Vec::as_slice),
                &self.content.rules.items,
            )
        });
        let spawn = restored
            .as_ref()
            .map(|r| r.position)
            .unwrap_or_else(|| self.sim.player.position.to_array());
        if restored.is_some() {
            log::info!("player {} rejoined; restoring saved state", pid.0);
        }

        let welcome = ServerMessage::Welcome {
            seed: self.sim.world.seed(),
            your_id: pid,
            spawn,
            time_of_day: self.sim.day_cycle.time_of_day(),
            game_mode: self.sim.player.mode,
            content_hash: self.content.rules.hash(),
            recipes: recipes_to_wire(&self.sim.recipes, &self.content.rules.items),
            restored,
        };
        self.net.session.send_to(pid, &welcome, Channel::Reliable);

        // The name comes from the player's verified ticket, not from anything
        // they typed or the host made up. `PlayerJoined` already carried a name
        // field — it was just never given a real one to carry.
        //
        // The fallback only fires where there is nobody to verify (the
        // singleplayer session, or a test): a host on a real socket refuses
        // joins it cannot verify, so a connected peer always has an account.
        let name = account
            .as_ref()
            .map(|account| account.username.clone())
            .unwrap_or_else(|| format!("Player {}", pid.0));

        self.net.peers.identities.insert(pid, identity);
        if let Some(account) = account {
            self.net.peers.accounts.insert(pid, account);
        }
        players::remove(&mut self.sim.ecs, pid);
        spawn::remote_player(
            &mut self.sim.ecs,
            RemotePlayer::new(pid, name, Vec3::from_array(spawn)),
        );
    }

    /// Drop everything this session knows about a peer that has gone: their
    /// entity, and what the host still owed them.
    fn forget_peer(&mut self, pid: PlayerId) {
        players::remove(&mut self.sim.ecs, pid);
        self.net.peers.remove(pid);
    }

    /// Tell everyone a peer is really here, and bring it up to date on what
    /// everyone is wearing.
    ///
    /// Deliberately *not* part of admitting a peer. A status probe from the
    /// server browser connects with a valid ticket and gets a `Welcome` like
    /// anyone else, but it never asks for the world — so it never reaches here,
    /// and nobody playing ever sees it come and go. See `Peers::announced`.
    fn announce_player(&mut self, pid: PlayerId) {
        if !self.net.peers.announced.insert(pid) {
            return;
        }
        let Some(name) = players::get(&self.sim.ecs, pid).map(|rp| rp.name.clone()) else {
            return;
        };

        self.net.session.broadcast(
            &ServerMessage::PlayerJoined { id: pid, name },
            Channel::Reliable,
        );

        let equipment: Vec<(PlayerId, Equipment)> = self
            .net
            .peers
            .equipment
            .iter()
            .map(|(&id, &equipment)| (id, equipment))
            .collect();
        for (id, equipment) in equipment {
            self.net.session.send_to(
                pid,
                &ServerMessage::PlayerEquipment { id, equipment },
                Channel::Reliable,
            );
        }
    }

    /// Answer a status query: what the server browser puts in a row.
    ///
    /// Sent only to the peer that asked. Nothing else happens — no announcement,
    /// no world state, no record — which is the whole reason a probe can ask.
    fn report_status(&mut self, pid: PlayerId) {
        // The host counts as one of the players, and as one of the slots: it is
        // in the world and it is occupying capacity, so a row reading "1/17" on
        // an empty server is the truth rather than an off-by-one.
        let status = ServerMessage::Status {
            name: self.save.world_name().to_string(),
            // Announced players, not connected peers: a probe gives itself a
            // replica in `peers` like anyone else, and must not count itself
            // (nor the other probes refreshing their lists at the same moment).
            online: (self.net.peers.announced.len() + 1) as u32,
            max: (crate::infrastructure::net::MAX_CLIENTS + 1) as u32,
            content_hash: self.content.rules.hash(),
        };
        self.net.session.send_to(pid, &status, Channel::Reliable);
    }

    /// Keep what a client has discovered, so its next `Welcome` hands it back.
    ///
    /// Merged rather than replaced: the set only ever grows, and merging means
    /// two reports arriving out of order (`Channel::Reliable` is unordered)
    /// cannot lose anything. Kept by stable identity, like the player records.
    fn record_discovery(&mut self, pid: PlayerId, wire: &[u16]) {
        let Some(&identity) = self.net.peers.identities.get(&pid) else {
            return;
        };
        let items = &self.content.rules.items;
        let entry = self.save.discovery.players.entry(identity).or_default();
        let mut known = KnownItems::from_ids(entry, items);
        if known.merge(&KnownItems::from_wire(wire, items)) {
            *entry = known.to_ids(items);
        }
    }

    /// Snapshot a leaving player so their state survives a rejoin, then drop
    /// every trace of them from this session.
    fn forget_player(&mut self, pid: PlayerId) {
        // A peer nobody was told about is a peer nobody has to be told left —
        // and, more importantly, one whose account must not be written over. A
        // status probe never plays, so recording it would replace a real
        // player's saved position and vitals with the spawn values it was
        // handed a moment earlier.
        if !self.net.peers.announced.contains(&pid) {
            self.forget_peer(pid);
            return;
        }

        record_remote(
            &mut self.save.records,
            &self.net.peers.identities,
            &self.sim.ecs,
            &self.net.peers.inventories,
            &self.content.rules.items,
            pid,
        );
        self.forget_peer(pid);
        self.net
            .session
            .broadcast(&ServerMessage::PlayerLeft { id: pid }, Channel::Reliable);
    }

    // --- Host: client requests ------------------------------------------------------

    /// Validate and apply one client request. Everything a client can ask for
    /// is checked here — this is the authority's only inbound surface.
    fn apply_request(&mut self, pid: PlayerId, msg: ClientMessage) {
        match msg {
            ClientMessage::Move {
                position,
                yaw,
                pitch,
            } => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.push_snapshot(Vec3::from_array(position), yaw, pitch);
                }
            }
            ClientMessage::Break { pos } => {
                if self.client_may_break(pid, pos) {
                    self.apply_client_edit(pos, BlockId::AIR);
                }
            }
            ClientMessage::Place { pos, block } => self.apply_client_edit(pos, block),
            ClientMessage::Stats {
                health,
                hunger,
                saturation,
            } => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.health = health;
                    rp.hunger = hunger;
                    rp.saturation = saturation;
                }
            }
            ClientMessage::SetMode(m) => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.mode = m;
                }
            }
            ClientMessage::SyncInventory { slots, selected } => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.equipment = equipment_from_slots(&slots, selected);
                }
                self.net.peers.inventories.insert(pid, (slots, selected));
            }
            ClientMessage::SyncKnown { items } => self.record_discovery(pid, &items),
            // The only place a command is ever parsed and run: the host knows
            // who is authorized, so the host decides.
            ClientMessage::Chat(text) => self.dispatch_chat(pid, text),
            ClientMessage::RequestWorldState => {
                // The first thing a real client asks for, and the thing a status
                // probe never asks for — so this is where a connected peer
                // becomes a player everyone else can see.
                self.announce_player(pid);
                self.replay_world_state(pid);
            }
            ClientMessage::RequestStatus => self.report_status(pid),
            ClientMessage::Attack { id } => self.apply_client_attack(pid, id),
            ClientMessage::UseBlock {
                pos,
                slots,
                selected,
            } => {
                // Adopt the snapshot first, so the use is judged against
                // exactly what the client held when it clicked.
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.equipment = equipment_from_slots(&slots, selected);
                }
                self.net.peers.inventories.insert(pid, (slots, selected));
                self.use_block(pid, pos);
            }
        }
    }

    /// Whether a client's tool meets the tier of the block it broke — the
    /// progression gate, checked here too so a modified client cannot mine a
    /// later biome's ore on day one. Read off the inventory it last reported,
    /// the same trust `client_melee_damage` extends. A refused break is undone
    /// on the client by echoing the block back, since it already applied the
    /// edit optimistically.
    fn client_may_break(&mut self, pid: PlayerId, pos: BlockPos) -> bool {
        let existing = self.sim.world.block_at(pos);
        let harvest = self.content.rules.blocks.get(existing).harvest.as_ref();
        let tool = self
            .net
            .peers
            .inventories
            .get(&pid)
            .and_then(|(slots, selected)| slots.get(*selected as usize)?.as_ref())
            .and_then(|stack| {
                self.content
                    .rules
                    .items
                    .component::<Tool>(ItemId(stack.item))
            });
        if crate::domain::inventory::meets_tier(harvest, tool) {
            return true;
        }
        log::info!("player {} broke {pos:?} below its tier; undoing", pid.0);
        self.net.session.send_to(
            pid,
            &ServerMessage::BlockChanged {
                pos,
                block: existing,
            },
            Channel::Reliable,
        );
        false
    }

    /// Apply a client's block edit and echo the result to everyone.
    fn apply_client_edit(&mut self, pos: BlockPos, block: BlockId) {
        if self.sim.world.set_block(pos, block).is_some() {
            self.sim.fluids.block_changed(pos);
            self.net.session.broadcast(
                &ServerMessage::BlockChanged { pos, block },
                Channel::Reliable,
            );
        }
    }

    /// Replay the world's existing edits and current mob population to a
    /// joining player, so they see what's already there.
    fn replay_world_state(&mut self, pid: PlayerId) {
        let edits = self.sim.world.collect_edits();
        log::debug!("replaying {} world edits to player {}", edits.len(), pid.0);
        for batch in edits.chunks(WORLD_SYNC_BATCH) {
            self.net.session.send_to(
                pid,
                &ServerMessage::WorldEdits {
                    edits: batch.to_vec(),
                },
                Channel::Chunk,
            );
        }
        let spawned: Vec<ServerMessage> = self
            .sim
            .ecs
            .query::<(&MobId, &Kind, &Transform, With<Mob>)>()
            .map(|(_, (id, kind, transform, ()))| ServerMessage::MobSpawned {
                id: id.0,
                kind: kind.name.clone(),
                position: transform.position.to_array(),
            })
            .collect();
        for msg in spawned {
            self.net.session.send_to(pid, &msg, Channel::Reliable);
        }
        self.net.session.send_to(
            pid,
            &ServerMessage::Progression(self.sim.progression.clone()),
            Channel::Reliable,
        );
    }

    /// Damage a client's swing lands, from the item in the hotbar slot they
    /// last reported selected.
    ///
    /// `SyncInventory` is throttled and client-reported, so this can lag a
    /// weapon swap by a beat and a dishonest client could claim a better sword.
    /// That is the same trust the host already extends to `ClientMessage::Stats`
    /// for health and hunger; an unknown or empty slot falls back to the fist.
    fn client_melee_damage(&self, pid: PlayerId) -> f32 {
        self.net
            .peers
            .inventories
            .get(&pid)
            .and_then(|(slots, selected)| slots.get(*selected as usize)?.as_ref())
            .and_then(|stack| {
                self.content
                    .rules
                    .items
                    .component::<Tool>(ItemId(stack.item))
            })
            .and_then(|tool| tool.damage)
            .unwrap_or(mobs::PLAYER_ATTACK_DAMAGE)
    }

    /// Validate a client's melee swing against their last known position, then
    /// apply it with kill credit. The outcome reaches clients via `mob_events`.
    fn apply_client_attack(&mut self, pid: PlayerId, mob_id: u64) {
        let Some(attacker) = players::get(&self.sim.ecs, pid).map(|rp| rp.position()) else {
            return;
        };
        let damage = self.client_melee_damage(pid);
        let target = mob_systems::find(&self.sim.ecs, MobId(mob_id)).and_then(|entity| {
            let at = self.sim.ecs.get::<Transform>(entity)?.position;
            Some((entity, at))
        });
        if let Some((entity, at)) = target
            && mobs::attack_in_range(attacker, at)
        {
            let to_mob = at - attacker;
            let push = Vec3::new(to_mob.x, 0.0, to_mob.z).normalize_or_zero()
                * mobs::KNOCKBACK_PUSH
                + Vec3::Y * mobs::KNOCKBACK_LIFT;
            let Some(health) = mob_systems::hit(&mut self.sim.ecs, entity, damage, push, pid.0)
            else {
                return;
            };
            self.sim.emit(ServerMessage::MobHurt { id: mob_id, health });
        }
    }

    // --- Client: authoritative updates ----------------------------------------------

    /// Apply one authoritative update from the host.
    fn apply_update(&mut self, msg: ServerMessage) {
        let local_id = self.net.session.local_id();
        // The biome/boss loop's messages are handled beside the code that
        // sends them; whatever they decline comes back to be matched here.
        let Some(msg) = self.apply_progression_update(msg) else {
            return;
        };
        let Some(msg) = self.apply_boss_update(msg) else {
            return;
        };
        match msg {
            // The welcome is consumed during construction, not here.
            ServerMessage::Welcome { .. } => {}
            // Only the server browser's probe ever asks for one, and it is not
            // an `InGameState` — a playing client seeing this is the host
            // answering a question nobody in the world asked.
            ServerMessage::Status { .. } => {}
            ServerMessage::PlayerJoined { id, name } if id != local_id => {
                if players::find(&self.sim.ecs, id).is_none() {
                    spawn::remote_player(
                        &mut self.sim.ecs,
                        RemotePlayer::new(id, name, Vec3::ZERO),
                    );
                }
            }
            ServerMessage::PlayerLeft { id } => {
                players::remove(&mut self.sim.ecs, id);
            }
            ServerMessage::PlayerState {
                id,
                position,
                yaw,
                pitch,
            } if id != local_id => {
                players::entry(&mut self.sim.ecs, id, Vec3::from_array(position)).push_snapshot(
                    Vec3::from_array(position),
                    yaw,
                    pitch,
                );
            }
            ServerMessage::BlockChanged { pos, block } => {
                // apply_edit (not set_block) so an edit whose chunk hasn't
                // streamed in yet is buffered and applied when it loads.
                self.sim.world.apply_edit(pos, block);
            }
            ServerMessage::WorldEdits { edits } => {
                let count = edits.len();
                for (pos, block) in edits {
                    self.sim.world.apply_edit(pos, block);
                }
                log::debug!("applied {count} world-state edits on join");
            }
            ServerMessage::PlayerStats {
                id,
                health,
                hunger,
                mode,
            } if id != local_id => {
                players::entry(&mut self.sim.ecs, id, Vec3::ZERO).set_stats(health, hunger, mode);
            }
            ServerMessage::PlayerEquipment { id, equipment } if id != local_id => {
                players::entry(&mut self.sim.ecs, id, Vec3::ZERO).equipment = equipment;
            }
            ServerMessage::MobSpawned { id, kind, position } => {
                match self.content.rules.entities.find(&kind) {
                    Some(k) if k.mob.is_some() => {
                        log::debug!("replicating mob {id} ({kind}) from host");
                        spawn::replica(&mut self.sim.ecs, k, MobId(id), Vec3::from_array(position));
                    }
                    // Shouldn't happen (the content hash gates divergent builds),
                    // but degrade gracefully.
                    _ => log::warn!("host spawned unknown mob kind {kind:?}; ignoring"),
                }
            }
            ServerMessage::MobStates { mobs } => {
                for (id, position, yaw) in mobs {
                    // Unknown ids are fine: an unreliable snapshot can outrun
                    // its reliable MobSpawned.
                    if let Some(entity) = mob_systems::find(&self.sim.ecs, MobId(id))
                        && let Some(transform) = self.sim.ecs.get_mut::<Transform>(entity)
                    {
                        transform.position = Vec3::from_array(position);
                        transform.yaw = yaw;
                    }
                }
            }
            ServerMessage::MobHurt { id, health } => {
                // Mirrored for the boss bar; the authoritative outcome of a
                // killing blow still arrives as MobDespawned.
                if let Some(entity) = mob_systems::find(&self.sim.ecs, MobId(id))
                    && let Some(mirror) = self.sim.ecs.get_mut::<Health>(entity)
                {
                    mirror.current = health;
                }
            }
            ServerMessage::MobDespawned { id, killed_by } => {
                let gone = mob_systems::find(&self.sim.ecs, MobId(id)).and_then(|entity| {
                    let kind = self.sim.ecs.get::<Kind>(entity)?.name.clone();
                    let position = self.sim.ecs.get::<Transform>(entity)?.position;
                    self.sim.ecs.despawn(entity);
                    Some((kind, position))
                });
                if let Some((kind, position)) = gone
                    && killed_by == Some(local_id)
                {
                    // This player made the kill: roll the loot locally.
                    self.sim.pop_loot(&kind, id, position);
                }
            }
            ServerMessage::ArrowSpawned {
                position,
                velocity,
                gravity,
                lifetime,
            } => {
                // Visual-only on clients: damage is host-side, so the local
                // copy carries none.
                crate::application::ecs::spawn::arrow(
                    &mut self.sim.ecs,
                    Vec3::from_array(position),
                    Vec3::from_array(velocity),
                    0.0,
                    gravity,
                    lifetime,
                );
            }
            ServerMessage::PlayerDamaged { id, amount } if id == local_id => {
                self.sim.damage_local_player(amount);
            }
            ServerMessage::Chat { from, kind, text } => self.show_remote_chat(from, kind, text),
            ServerMessage::GrantItems { to, stacks } if to == local_id => {
                self.apply_granted_items(&stacks);
            }
            ServerMessage::Teleport { to, position } if to == local_id => {
                self.apply_teleport(position);
            }
            _ => {}
        }
    }

    // --- Outgoing -------------------------------------------------------------------

    /// Host: publish this frame's authoritative state.
    fn broadcast_authority_state(&mut self, send_stats: bool) {
        // Player snapshots: the host's own, then every remote's.
        let mut snapshots = vec![(
            HOST_PLAYER_ID,
            self.sim.player.position.to_array(),
            self.sim.player.yaw,
            self.sim.player.pitch,
        )];
        snapshots.extend(
            players::all(&self.sim.ecs)
                .map(|rp| (rp.id, rp.position().to_array(), rp.yaw, rp.pitch)),
        );
        for (id, position, yaw, pitch) in snapshots {
            self.net.session.broadcast(
                &ServerMessage::PlayerState {
                    id,
                    position,
                    yaw,
                    pitch,
                },
                Channel::Unreliable,
            );
        }

        // Periodic authoritative vitals for the host and every remote player.
        if send_stats {
            let mut stats = vec![(
                HOST_PLAYER_ID,
                self.sim.player.health,
                self.sim.player.hunger,
                self.sim.player.mode,
            )];
            stats.extend(
                players::all(&self.sim.ecs).map(|rp| (rp.id, rp.health, rp.hunger, rp.mode)),
            );
            for (id, health, hunger, mode) in stats {
                self.net.session.broadcast(
                    &ServerMessage::PlayerStats {
                        id,
                        health,
                        hunger,
                        mode,
                    },
                    Channel::Reliable,
                );
            }
        }

        // What everyone is wearing and holding, only when it changes (the host's
        // own from its inventory, each remote's from its last inventory sync).
        let mut equip: Vec<(PlayerId, Equipment)> =
            vec![(HOST_PLAYER_ID, equipment_of(&self.sim.inventory))];
        equip.extend(players::all(&self.sim.ecs).map(|rp| (rp.id, rp.equipment)));
        for (id, equipment) in equip {
            if self.net.peers.equipment.get(&id) != Some(&equipment) {
                self.net.peers.equipment.insert(id, equipment);
                self.net.session.broadcast(
                    &ServerMessage::PlayerEquipment { id, equipment },
                    Channel::Reliable,
                );
            }
        }

        // Mob lifecycle events queued by this frame's simulation (spawns,
        // hurts, deaths, arrows, remote-player damage), then one batched
        // unreliable movement snapshot for all live mobs.
        // Only a host has anyone to tell; elsewhere the events are dropped
        // here, exactly where they used to be dropped at the moment of emission.
        let events = std::mem::take(&mut self.sim.outbox);
        if self.net.session.serves_peers() {
            for msg in events {
                self.net.session.broadcast(&msg, Channel::Reliable);
            }
        }
        if self.sim.ecs.count::<Mob>() > 0 {
            let states = ServerMessage::MobStates {
                mobs: self
                    .sim
                    .ecs
                    .query::<(&MobId, &Transform, With<Mob>)>()
                    .map(|(_, (id, t, ()))| (id.0, t.position.to_array(), t.yaw))
                    .collect(),
            };
            self.net.session.broadcast(&states, Channel::Unreliable);
        }
    }

    /// Client: report our own state to the host.
    fn report_to_host(&mut self, dt: f32, send_stats: bool) {
        // Ask the host to replay the world's existing edits, once connected.
        if !self.net.peers.world_state_requested && self.net.session.is_connected() {
            self.net
                .session
                .request(&ClientMessage::RequestWorldState, Channel::Reliable);
            self.net.peers.world_state_requested = true;
        }

        self.net.session.request(
            &ClientMessage::Move {
                position: self.sim.player.position.to_array(),
                yaw: self.sim.player.yaw,
                pitch: self.sim.player.pitch,
            },
            Channel::Unreliable,
        );
        if send_stats {
            self.net.session.request(
                &ClientMessage::Stats {
                    health: self.sim.player.health,
                    hunger: self.sim.player.hunger,
                    saturation: self.sim.player.saturation,
                },
                Channel::Reliable,
            );
        }

        // Report the inventory (throttled, only on change) so the host can
        // persist it in the world save.
        self.net.peers.inventory_sync_timer += dt;
        if self.net.peers.inventory_sync_timer >= INVENTORY_SYNC_INTERVAL {
            self.net.peers.inventory_sync_timer = 0.0;
            let changed = self
                .net
                .peers
                .last_synced_inventory
                .as_ref()
                .is_none_or(|last| {
                    last.slots() != self.sim.inventory.slots()
                        || last.selected_index() != self.sim.inventory.selected_index()
                });
            if changed {
                let (slots, selected) = inventory_to_wire(&self.sim.inventory);
                self.net.session.request(
                    &ClientMessage::SyncInventory { slots, selected },
                    Channel::Reliable,
                );
                self.net.peers.last_synced_inventory = Some(self.sim.inventory.clone());
            }
        }
    }

    /// Propagate a local block edit: the authority asserts it, a client asks.
    pub(super) fn broadcast_local_edit(&mut self, pos: BlockPos, block: BlockId) {
        if self.net.session.is_authority() {
            self.net.session.broadcast(
                &ServerMessage::BlockChanged { pos, block },
                Channel::Reliable,
            );
        } else {
            let msg = if block.is_air() {
                ClientMessage::Break { pos }
            } else {
                ClientMessage::Place { pos, block }
            };
            self.net.session.request(&msg, Channel::Reliable);
        }
    }

    /// Tell the host the local player's game mode changed (a no-op on the
    /// authority — the host advertises its mode via `PlayerStats`/`Welcome`).
    pub(super) fn broadcast_mode_change(&mut self) {
        let mode = self.sim.player.mode;
        if !self.net.session.is_authority() {
            self.net
                .session
                .request(&ClientMessage::SetMode(mode), Channel::Reliable);
        }
    }

    pub(super) fn net_status(&self) -> String {
        self.net
            .session
            .status(self.sim.ecs.count::<RemotePlayer>())
    }

    /// Swap in a different networking role. Tests use this to drive host and
    /// client logic through a fake transport; production wiring sets it in
    /// `setup`.
    #[cfg(test)]
    pub(super) fn set_session(&mut self, session: Box<dyn crate::application::session::Session>) {
        self.sim.authoritative = session.is_authority();
        self.net.session = session;
    }
}

/// Snapshot one connected player into the host's persistent per-identity
/// records. A free function over the individual fields so it can be called from
/// inside `pump_network`'s borrow of `self.net`.
pub(super) fn record_remote(
    records: &mut PlayerRecords,
    identities: &HashMap<PlayerId, u64>,
    ecs: &Ecs,
    remote_inventories: &HashMap<PlayerId, (Vec<Option<NetItemStack>>, u32)>,
    items: &ItemRegistry,
    pid: PlayerId,
) {
    let Some(&identity) = identities.get(&pid) else {
        return;
    };
    let Some(rp) = players::get(ecs, pid) else {
        return;
    };
    // A client that never reported an inventory keeps its previous record's.
    let (slots, selected) = match remote_inventories.get(&pid) {
        Some((slots, selected)) => (wire_slots_to_ids(slots, items), *selected),
        None => match records.0.get(&identity) {
            Some(prev) => (prev.slots.clone(), prev.selected_slot),
            // Never reported an inventory and no history: don't record at all,
            // so a rejoin starts fresh (starter kit) instead of empty-handed.
            None => return,
        },
    };
    records.0.insert(
        identity,
        PlayerData {
            position: rp.position().to_array(),
            yaw: rp.yaw,
            pitch: rp.pitch,
            flying: false,
            health: rp.health,
            hunger: rp.hunger,
            saturation: rp.saturation,
            selected_slot: selected,
            slots,
        },
    );
}

/// What an inventory is wearing and holding, for the wire.
fn equipment_of(inventory: &Inventory) -> Equipment {
    Equipment {
        armor: inventory.equipped_armor().map(|slot| slot.map(|id| id.0)),
        held: inventory.selected_stack().map(|stack| stack.item.0),
    }
}

/// The same, read out of a wire inventory snapshot: a client reports its slots
/// and which one it has selected, so the host never has to be told separately
/// what that client is holding.
fn equipment_from_slots(slots: &[Option<NetItemStack>], selected: u32) -> Equipment {
    Equipment {
        armor: std::array::from_fn(|i| {
            slots
                .get(ARMOR_START + i)
                .and_then(|slot| slot.map(|s| s.item))
        }),
        held: slots
            .get(selected as usize)
            .and_then(|slot| slot.map(|s| s.item)),
    }
}

/// Serialize the recipe book back to item ids for the `Welcome` message.
pub(super) fn recipes_to_wire(book: &RecipeBook, items: &ItemRegistry) -> Vec<RecipeData> {
    book.recipes()
        .iter()
        .map(|recipe| RecipeData {
            output: items.get(recipe.output).id.clone(),
            count: recipe.count as u32,
            ingredients: recipe
                .ingredients
                .iter()
                .map(|&(item, n)| (items.get(item).id.clone(), n))
                .collect(),
            station: recipe.station.clone(),
        })
        .collect()
}

/// Convert the local inventory to its wire form for `SyncInventory`.
pub(super) fn inventory_to_wire(inventory: &Inventory) -> (Vec<Option<NetItemStack>>, u32) {
    let slots = inventory
        .slots()
        .iter()
        .map(|slot| {
            slot.map(|stack| NetItemStack {
                item: stack.item.0,
                count: stack.count,
                durability: stack.durability,
            })
        })
        .collect();
    (slots, inventory.selected_index() as u32)
}

/// Convert wire inventory slots to the id-based on-disk form. Numeric ids out
/// of this build's registry range (mismatched peer) become empty slots.
fn wire_slots_to_ids(
    slots: &[Option<NetItemStack>],
    items: &ItemRegistry,
) -> Vec<Option<ItemStackData>> {
    slots
        .iter()
        .map(|slot| {
            slot.and_then(|s| {
                ((s.item as usize) < items.len()).then(|| ItemStackData {
                    id: items.get(ItemId(s.item)).id.clone(),
                    count: s.count,
                    durability: s.durability,
                })
            })
        })
        .collect()
}

/// Convert a saved record back to wire form for a returning client's `Welcome`,
/// with the items it had discovered (`known`, string ids). Item names this
/// build no longer knows are dropped.
fn record_to_restore(record: &PlayerData, known: &[String], items: &ItemRegistry) -> PlayerRestore {
    PlayerRestore {
        position: record.position,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        hunger: record.hunger,
        saturation: record.saturation,
        slots: record
            .slots
            .iter()
            .map(|slot| {
                slot.as_ref().and_then(|s| {
                    items.find(&s.id).map(|id| NetItemStack {
                        item: id.0,
                        count: s.count,
                        durability: s.durability,
                    })
                })
            })
            .collect(),
        selected: record.selected_slot,
        known_items: KnownItems::from_ids(known, items).to_wire(),
    }
}

/// Rebuild a recipe book from a host's wire data. Recipes naming items this
/// build doesn't know are skipped with a warning (mismatched versions).
pub(super) fn recipes_from_wire(
    data: &[RecipeData],
    items: &ItemRegistry,
    stations: &[String],
) -> RecipeBook {
    let resolved = data
        .iter()
        .filter_map(|r| {
            let named = NamedRecipe {
                output: &r.output,
                count: r.count,
                ingredients: &r.ingredients,
                station: r.station.as_deref(),
            };
            resolve_named(&named, items, stations)
        })
        .collect();
    RecipeBook::from_recipes(resolved)
}

#[cfg(test)]
mod tests;
