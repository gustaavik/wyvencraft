//! Networking for [`InGameState`]: applying what arrived, deciding what to say.
//!
//! Transport itself lives behind [`Session`](crate::state::session::Session) —
//! this module never touches a socket. It splits into three parts:
//!
//! - [`InGameState::pump_network`] drives one frame: drain, apply, speak, flush.
//! - `apply_*` interpret one [`Inbound`] against the world, the players and the
//!   mobs. They are ordinary `&mut self` methods, testable against a
//!   [`FakeSession`](crate::state::session::FakeSession).
//! - The free functions convert between the in-memory model and the protocol's
//!   wire types; several are `pub(super)` so construction, persistence, and
//!   tests in sibling modules can reuse them.

use std::collections::HashMap;
use std::time::Duration;

use glam::Vec3;

use super::mobs::{self, RemoteMob};
use super::{
    HOST_PLAYER_ID, INVENTORY_SYNC_INTERVAL, InGameState, STATS_INTERVAL, WORLD_SYNC_BATCH,
};
use crate::core::{BlockId, BlockPos};
use crate::entity::Arrow;
use crate::inventory::{ARMOR_SIZE, ARMOR_START, Inventory, ItemId, ItemRegistry, RecipeBook};
use crate::net::{
    Channel, ClientMessage, NetItemStack, PlayerId, PlayerRestore, RecipeData, RemotePlayer,
    ServerMessage,
};
use crate::save::{ItemStackData, PlayerData, PlayerRecords};
use crate::state::session::Inbound;

impl InGameState {
    /// Drive networking for one frame: drain the transport and apply what
    /// arrived, then say this frame's piece and flush.
    pub(super) fn pump_network(&mut self, dt: f32) {
        let duration = Duration::from_secs_f32(dt.max(1.0e-4));

        // Survival stats are low-frequency; throttle them to keep the wire quiet.
        self.peers.stats_timer += dt;
        let send_stats = self.peers.stats_timer >= STATS_INTERVAL;
        if send_stats {
            self.peers.stats_timer = 0.0;
        }

        for msg in self.session.poll(duration) {
            self.apply_inbound(msg);
        }

        if self.session.is_authority() {
            self.broadcast_authority_state(send_stats);
        } else {
            self.report_to_host(dt, send_stats);
        }
        self.session.flush();
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
        let restored = self
            .save
            .records
            .0
            .get(&identity)
            .map(|record| record_to_restore(record, &self.content.items));
        let spawn = restored
            .as_ref()
            .map(|r| r.position)
            .unwrap_or_else(|| self.player.position.to_array());
        if restored.is_some() {
            log::info!("player {} rejoined; restoring saved state", pid.0);
        }

        let welcome = ServerMessage::Welcome {
            seed: self.world.seed(),
            your_id: pid,
            spawn,
            time_of_day: self.day_cycle.time_of_day(),
            game_mode: self.player.mode,
            content_hash: self.content.hash,
            recipes: recipes_to_wire(&self.recipes, &self.content.items),
            restored,
        };
        self.session.send_to(pid, &welcome, Channel::Reliable);

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

        self.peers.identities.insert(pid, identity);
        if let Some(account) = account {
            self.peers.accounts.insert(pid, account);
        }
        self.peers
            .players
            .insert(pid, RemotePlayer::new(pid, name, Vec3::from_array(spawn)));
    }

    /// Tell everyone a peer is really here, and bring it up to date on what
    /// everyone is wearing.
    ///
    /// Deliberately *not* part of admitting a peer. A status probe from the
    /// server browser connects with a valid ticket and gets a `Welcome` like
    /// anyone else, but it never asks for the world — so it never reaches here,
    /// and nobody playing ever sees it come and go. See `Peers::announced`.
    fn announce_player(&mut self, pid: PlayerId) {
        if !self.peers.announced.insert(pid) {
            return;
        }
        let Some(name) = self.peers.players.get(&pid).map(|rp| rp.name.clone()) else {
            return;
        };

        self.session.broadcast(
            &ServerMessage::PlayerJoined { id: pid, name },
            Channel::Reliable,
        );

        let equipment: Vec<(PlayerId, [Option<u16>; ARMOR_SIZE])> = self
            .peers
            .equipment
            .iter()
            .map(|(&id, &armor)| (id, armor))
            .collect();
        for (id, armor) in equipment {
            self.session.send_to(
                pid,
                &ServerMessage::PlayerEquipment { id, armor },
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
            online: (self.peers.announced.len() + 1) as u32,
            max: (crate::net::MAX_CLIENTS + 1) as u32,
            content_hash: self.content.hash,
        };
        self.session.send_to(pid, &status, Channel::Reliable);
    }

    /// Snapshot a leaving player so their state survives a rejoin, then drop
    /// every trace of them from this session.
    fn forget_player(&mut self, pid: PlayerId) {
        // A peer nobody was told about is a peer nobody has to be told left —
        // and, more importantly, one whose account must not be written over. A
        // status probe never plays, so recording it would replace a real
        // player's saved position and vitals with the spawn values it was
        // handed a moment earlier.
        if !self.peers.announced.contains(&pid) {
            self.peers.remove(pid);
            return;
        }

        record_remote(
            &mut self.save.records,
            &self.peers.identities,
            &self.peers.players,
            &self.peers.inventories,
            &self.content.items,
            pid,
        );
        self.peers.remove(pid);
        self.session
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
                if let Some(rp) = self.peers.players.get_mut(&pid) {
                    rp.push_snapshot(Vec3::from_array(position), yaw, pitch);
                }
            }
            ClientMessage::Break { pos } => self.apply_client_edit(pos, BlockId::AIR),
            ClientMessage::Place { pos, block } => self.apply_client_edit(pos, block),
            ClientMessage::Stats {
                health,
                hunger,
                saturation,
            } => {
                if let Some(rp) = self.peers.players.get_mut(&pid) {
                    rp.health = health;
                    rp.hunger = hunger;
                    rp.saturation = saturation;
                }
            }
            ClientMessage::SetMode(m) => {
                if let Some(rp) = self.peers.players.get_mut(&pid) {
                    rp.mode = m;
                }
            }
            ClientMessage::SyncInventory { slots, selected } => {
                if let Some(rp) = self.peers.players.get_mut(&pid) {
                    rp.armor = armor_from_slots(&slots);
                }
                self.peers.inventories.insert(pid, (slots, selected));
            }
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
        }
    }

    /// Apply a client's block edit and echo the result to everyone.
    fn apply_client_edit(&mut self, pos: BlockPos, block: BlockId) {
        if self.world.set_block(pos, block).is_some() {
            self.fluids.block_changed(pos);
            self.session.broadcast(
                &ServerMessage::BlockChanged { pos, block },
                Channel::Reliable,
            );
        }
    }

    /// Replay the world's existing edits and current mob population to a
    /// joining player, so they see what's already there.
    fn replay_world_state(&mut self, pid: PlayerId) {
        let edits = self.world.collect_edits();
        log::debug!("replaying {} world edits to player {}", edits.len(), pid.0);
        for batch in edits.chunks(WORLD_SYNC_BATCH) {
            self.session.send_to(
                pid,
                &ServerMessage::WorldEdits {
                    edits: batch.to_vec(),
                },
                Channel::Chunk,
            );
        }
        let spawned: Vec<ServerMessage> = self
            .mobs
            .live
            .iter()
            .map(|mob| ServerMessage::MobSpawned {
                id: mob.id.0,
                kind: mob.kind_name.clone(),
                position: mob.position.to_array(),
            })
            .collect();
        for msg in spawned {
            self.session.send_to(pid, &msg, Channel::Reliable);
        }
    }

    /// Damage a client's swing lands, from the item in the hotbar slot they
    /// last reported selected.
    ///
    /// `SyncInventory` is throttled and client-reported, so this can lag a
    /// weapon swap by a beat and a dishonest client could claim a better sword.
    /// That is the same trust the host already extends to `ClientMessage::Stats`
    /// for health and hunger; an unknown or empty slot falls back to the fist.
    fn client_melee_damage(&self, pid: PlayerId) -> f32 {
        self.peers
            .inventories
            .get(&pid)
            .and_then(|(slots, selected)| slots.get(*selected as usize)?.as_ref())
            .and_then(|stack| self.content.items.tool(ItemId(stack.item)))
            .and_then(|tool| tool.damage)
            .unwrap_or(mobs::PLAYER_ATTACK_DAMAGE)
    }

    /// Validate a client's melee swing against their last known position, then
    /// apply it with kill credit. The outcome reaches clients via `mob_events`.
    fn apply_client_attack(&mut self, pid: PlayerId, mob_id: u64) {
        let Some(attacker) = self.peers.players.get(&pid).map(|rp| rp.position()) else {
            return;
        };
        let damage = self.client_melee_damage(pid);
        if let Some(mob) = self.mobs.live.iter_mut().find(|m| m.id.0 == mob_id)
            && mobs::attack_in_range(attacker, mob.position)
        {
            let to_mob = mob.position - attacker;
            let push = Vec3::new(to_mob.x, 0.0, to_mob.z).normalize_or_zero()
                * mobs::KNOCKBACK_PUSH
                + Vec3::Y * mobs::KNOCKBACK_LIFT;
            mob.damage(damage, push);
            mob.last_attacker = Some(pid.0);
            let health = mob.health;
            self.peers
                .mob_events
                .push(ServerMessage::MobHurt { id: mob_id, health });
        }
    }

    // --- Client: authoritative updates ----------------------------------------------

    /// Apply one authoritative update from the host.
    fn apply_update(&mut self, msg: ServerMessage) {
        let local_id = self.session.local_id();
        match msg {
            // The welcome is consumed during construction, not here.
            ServerMessage::Welcome { .. } => {}
            // Only the server browser's probe ever asks for one, and it is not
            // an `InGameState` — a playing client seeing this is the host
            // answering a question nobody in the world asked.
            ServerMessage::Status { .. } => {}
            ServerMessage::PlayerJoined { id, name } if id != local_id => {
                self.peers
                    .players
                    .entry(id)
                    .or_insert_with(|| RemotePlayer::new(id, name, Vec3::ZERO));
            }
            ServerMessage::PlayerLeft { id } => {
                self.peers.players.remove(&id);
            }
            ServerMessage::PlayerState {
                id,
                position,
                yaw,
                pitch,
            } if id != local_id => {
                self.peers
                    .entry(id, Vec3::from_array(position))
                    .push_snapshot(Vec3::from_array(position), yaw, pitch);
            }
            ServerMessage::BlockChanged { pos, block } => {
                // apply_edit (not set_block) so an edit whose chunk hasn't
                // streamed in yet is buffered and applied when it loads.
                self.world.apply_edit(pos, block);
            }
            ServerMessage::WorldEdits { edits } => {
                let count = edits.len();
                for (pos, block) in edits {
                    self.world.apply_edit(pos, block);
                }
                log::debug!("applied {count} world-state edits on join");
            }
            ServerMessage::PlayerStats {
                id,
                health,
                hunger,
                mode,
            } if id != local_id => {
                self.peers
                    .entry(id, Vec3::ZERO)
                    .set_stats(health, hunger, mode);
            }
            ServerMessage::PlayerEquipment { id, armor } if id != local_id => {
                self.peers.entry(id, Vec3::ZERO).armor = armor;
            }
            ServerMessage::MobSpawned { id, kind, position } => {
                match self.content.entities.find(&kind) {
                    Some(k) if k.mob.is_some() => {
                        log::debug!("replicating mob {id} ({kind}) from host");
                        self.mobs
                            .remote
                            .insert(id, RemoteMob::new(k, Vec3::from_array(position)));
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
                    if let Some(mob) = self.mobs.remote.get_mut(&id) {
                        mob.push_snapshot(Vec3::from_array(position), yaw);
                    }
                }
            }
            ServerMessage::MobHurt { .. } => {
                // Reserved for hurt feedback (flash/sound); the authoritative
                // outcome arrives as MobDespawned.
            }
            ServerMessage::MobDespawned { id, killed_by } => {
                if let Some(mob) = self.mobs.remote.remove(&id)
                    && killed_by == Some(local_id)
                {
                    // This player made the kill: roll the loot locally.
                    let (kind, position) = (mob.kind_name().to_string(), mob.position());
                    self.pop_drops_for(&kind, id, position);
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
                self.mobs.arrows.push(Arrow::new(
                    Vec3::from_array(position),
                    Vec3::from_array(velocity),
                    0.0,
                    gravity,
                    lifetime,
                ));
            }
            ServerMessage::PlayerDamaged { id, amount } if id == local_id => {
                self.damage_local_player(amount);
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
            self.player.position.to_array(),
            self.player.yaw,
            self.player.pitch,
        )];
        snapshots.extend(
            self.peers
                .players
                .iter()
                .map(|(pid, rp)| (*pid, rp.position().to_array(), rp.yaw, rp.pitch)),
        );
        for (id, position, yaw, pitch) in snapshots {
            self.session.broadcast(
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
                self.player.health,
                self.player.hunger,
                self.player.mode,
            )];
            stats.extend(
                self.peers
                    .players
                    .values()
                    .map(|rp| (rp.id, rp.health, rp.hunger, rp.mode)),
            );
            for (id, health, hunger, mode) in stats {
                self.session.broadcast(
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

        // Armor, only when it changes (the host's own from its inventory, each
        // remote's from its last inventory sync).
        let mut equip: Vec<(PlayerId, [Option<u16>; ARMOR_SIZE])> =
            vec![(HOST_PLAYER_ID, armor_ids(&self.inventory))];
        equip.extend(self.peers.players.iter().map(|(pid, rp)| (*pid, rp.armor)));
        for (id, armor) in equip {
            if self.peers.equipment.get(&id) != Some(&armor) {
                self.peers.equipment.insert(id, armor);
                self.session.broadcast(
                    &ServerMessage::PlayerEquipment { id, armor },
                    Channel::Reliable,
                );
            }
        }

        // Mob lifecycle events queued by this frame's simulation (spawns,
        // hurts, deaths, arrows, remote-player damage), then one batched
        // unreliable movement snapshot for all live mobs.
        for msg in std::mem::take(&mut self.peers.mob_events) {
            self.session.broadcast(&msg, Channel::Reliable);
        }
        if !self.mobs.live.is_empty() {
            let states = ServerMessage::MobStates {
                mobs: self
                    .mobs
                    .live
                    .iter()
                    .map(|m| (m.id.0, m.position.to_array(), m.yaw))
                    .collect(),
            };
            self.session.broadcast(&states, Channel::Unreliable);
        }
    }

    /// Client: report our own state to the host.
    fn report_to_host(&mut self, dt: f32, send_stats: bool) {
        // Ask the host to replay the world's existing edits, once connected.
        if !self.peers.world_state_requested && self.session.is_connected() {
            self.session
                .request(&ClientMessage::RequestWorldState, Channel::Reliable);
            self.peers.world_state_requested = true;
        }

        self.session.request(
            &ClientMessage::Move {
                position: self.player.position.to_array(),
                yaw: self.player.yaw,
                pitch: self.player.pitch,
            },
            Channel::Unreliable,
        );
        if send_stats {
            self.session.request(
                &ClientMessage::Stats {
                    health: self.player.health,
                    hunger: self.player.hunger,
                    saturation: self.player.saturation,
                },
                Channel::Reliable,
            );
        }

        // Report the inventory (throttled, only on change) so the host can
        // persist it in the world save.
        self.peers.inventory_sync_timer += dt;
        if self.peers.inventory_sync_timer >= INVENTORY_SYNC_INTERVAL {
            self.peers.inventory_sync_timer = 0.0;
            let changed = self
                .peers
                .last_synced_inventory
                .as_ref()
                .is_none_or(|last| {
                    last.slots() != self.inventory.slots()
                        || last.selected_index() != self.inventory.selected_index()
                });
            if changed {
                let (slots, selected) = inventory_to_wire(&self.inventory);
                self.session.request(
                    &ClientMessage::SyncInventory { slots, selected },
                    Channel::Reliable,
                );
                self.peers.last_synced_inventory = Some(self.inventory.clone());
            }
        }
    }

    /// Propagate a local block edit: the authority asserts it, a client asks.
    pub(super) fn broadcast_local_edit(&mut self, pos: BlockPos, block: BlockId) {
        if self.session.is_authority() {
            self.session.broadcast(
                &ServerMessage::BlockChanged { pos, block },
                Channel::Reliable,
            );
        } else {
            let msg = if block.is_air() {
                ClientMessage::Break { pos }
            } else {
                ClientMessage::Place { pos, block }
            };
            self.session.request(&msg, Channel::Reliable);
        }
    }

    /// Tell the host the local player's game mode changed (a no-op on the
    /// authority — the host advertises its mode via `PlayerStats`/`Welcome`).
    pub(super) fn broadcast_mode_change(&mut self) {
        let mode = self.player.mode;
        if !self.session.is_authority() {
            self.session
                .request(&ClientMessage::SetMode(mode), Channel::Reliable);
        }
    }

    pub(super) fn net_status(&self) -> String {
        self.session.status(self.peers.count())
    }

    /// Swap in a different networking role. Tests use this to drive host and
    /// client logic through a fake transport; production wiring sets it in
    /// `setup`.
    #[cfg(test)]
    pub(super) fn set_session(&mut self, session: Box<dyn crate::state::session::Session>) {
        self.session = session;
    }
}

/// Snapshot one connected player into the host's persistent per-identity
/// records. A free function over the individual fields so it can be called from
/// inside `pump_network`'s borrow of `self.net`.
pub(super) fn record_remote(
    records: &mut PlayerRecords,
    identities: &HashMap<PlayerId, u64>,
    remote_players: &HashMap<PlayerId, RemotePlayer>,
    remote_inventories: &HashMap<PlayerId, (Vec<Option<NetItemStack>>, u32)>,
    items: &ItemRegistry,
    pid: PlayerId,
) {
    let Some(&identity) = identities.get(&pid) else {
        return;
    };
    let Some(rp) = remote_players.get(&pid) else {
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

/// Worn armor item ids for the wire, from an inventory's armor slots.
fn armor_ids(inventory: &Inventory) -> [Option<u16>; ARMOR_SIZE] {
    inventory.equipped_armor().map(|slot| slot.map(|id| id.0))
}

/// Worn armor item ids from a wire inventory snapshot's armor slots.
fn armor_from_slots(slots: &[Option<NetItemStack>]) -> [Option<u16>; ARMOR_SIZE] {
    std::array::from_fn(|i| {
        slots
            .get(ARMOR_START + i)
            .and_then(|slot| slot.map(|s| s.item))
    })
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
        })
        .collect()
}

/// Convert the local inventory to its wire form for `SyncInventory`.
fn inventory_to_wire(inventory: &Inventory) -> (Vec<Option<NetItemStack>>, u32) {
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

/// Convert a saved record back to wire form for a returning client's `Welcome`.
/// Item names this build no longer knows are dropped.
fn record_to_restore(record: &PlayerData, items: &ItemRegistry) -> PlayerRestore {
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
    }
}

/// Rebuild a recipe book from a host's wire data. Recipes naming items this
/// build doesn't know are skipped with a warning (mismatched versions).
pub(super) fn recipes_from_wire(data: &[RecipeData], items: &ItemRegistry) -> RecipeBook {
    let resolved = data
        .iter()
        .filter_map(|r| {
            crate::inventory::crafting::resolve_named(&r.output, r.count, &r.ingredients, items)
        })
        .collect();
    RecipeBook::from_recipes(resolved)
}

#[cfg(test)]
mod tests {
    use std::net::UdpSocket;
    use std::sync::Arc;
    use std::time::Instant;

    use wyven_auth::{AccountState, AuthClient, FakeAuthClient, KeyCache};

    use super::*;
    use crate::content::GameContent;
    use crate::core::GameMode;
    use crate::net::status::{NetStatusProbe, StatusOutcome, StatusProbe};
    use crate::net::{Client, Host, TicketJoin, host_config};
    use crate::state::session::{FakeHandle, FakeSession};
    use crate::world::block::blocks;

    /// An in-game state driven by a fake session, plus the handle to script it.
    fn host_session() -> (InGameState, FakeHandle) {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        let session = FakeSession::host();
        let handle = session.handle();
        state.set_session(Box::new(session));
        (state, handle)
    }

    fn client_session(local: PlayerId) -> (InGameState, FakeHandle) {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        let session = FakeSession::client(local);
        let handle = session.handle();
        state.set_session(Box::new(session));
        (state, handle)
    }

    /// Put `name` in the first hotbar slot and select it.
    fn hold(state: &mut InGameState, name: &str) {
        let id = state
            .content
            .items
            .find(name)
            .unwrap_or_else(|| panic!("{name}"));
        state
            .inventory
            .set_slot(0, Some(state.content.items.full_stack(id)));
        state.inventory.set_selected(0);
    }

    /// A swing is worth whatever the held item says, and a tool with no
    /// `damage` component — or an empty hand — is worth a bare fist.
    #[test]
    fn a_local_swing_takes_its_damage_from_the_held_item() {
        let (mut state, _handle) = host_session();

        hold(&mut state, "iron_sword");
        assert_eq!(state.melee_damage(), 6.0, "iron_sword");
        hold(&mut state, "wooden_sword");
        assert_eq!(state.melee_damage(), 4.0, "wooden_sword");
        hold(&mut state, "iron_axe");
        assert_eq!(state.melee_damage(), 5.0, "iron_axe");

        hold(&mut state, "iron_pickaxe");
        assert_eq!(
            state.melee_damage(),
            mobs::PLAYER_ATTACK_DAMAGE,
            "a pickaxe is no better than a fist"
        );

        state.inventory.set_slot(0, None);
        assert_eq!(
            state.melee_damage(),
            mobs::PLAYER_ATTACK_DAMAGE,
            "an empty hand is a fist"
        );
    }

    /// The host resolves a client's swing against the inventory that client
    /// last reported, not against the host's own held item.
    #[test]
    fn a_clients_swing_takes_its_damage_from_their_reported_inventory() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        handle.deliver(Inbound::Joined {
            player: pid,
            identity: 42,
            account: None,
        });
        state.pump_network(1.0 / 60.0);

        // The host is holding nothing special; the client reports an iron sword.
        assert_eq!(
            state.client_melee_damage(pid),
            mobs::PLAYER_ATTACK_DAMAGE,
            "nothing reported yet, so a fist"
        );

        let sword = state.content.items.find("iron_sword").expect("iron_sword");
        let mut slots = vec![None; 3];
        slots[2] = Some(NetItemStack {
            item: sword.0,
            count: 1,
            durability: state.content.items.max_durability(sword),
        });
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::SyncInventory { slots, selected: 2 },
        });
        state.pump_network(1.0 / 60.0);

        assert_eq!(
            state.client_melee_damage(pid),
            6.0,
            "the client's iron sword"
        );
        assert_eq!(
            state.melee_damage(),
            mobs::PLAYER_ATTACK_DAMAGE,
            "the host's own swing is unaffected"
        );
    }

    /// A first-time joiner is welcomed with this world's seed and their new id,
    /// announced to everyone once it asks for the world, and registered as a
    /// remote player.
    #[test]
    fn a_joining_player_is_welcomed_and_announced() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        handle.deliver(Inbound::Joined {
            player: pid,
            identity: 42,
            account: None,
        });
        // What a real client sends on its first connected frame, and what marks
        // this peer as a player rather than a status probe.
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::RequestWorldState,
        });

        state.pump_network(1.0 / 60.0);

        let net = handle.lock();
        let welcome = net.messages_to(pid);
        let Some(ServerMessage::Welcome {
            seed,
            your_id,
            restored,
            ..
        }) = welcome.first()
        else {
            panic!("the joiner must receive a Welcome, got {welcome:?}");
        };
        assert_eq!(*seed, state.world.seed());
        assert_eq!(*your_id, pid);
        assert!(
            restored.is_none(),
            "a first-time joiner has nothing to restore"
        );
        assert!(
            net.broadcasts()
                .iter()
                .any(|m| matches!(m, ServerMessage::PlayerJoined { id, .. } if *id == pid)),
            "everyone is told about the join"
        );
        drop(net);
        assert!(state.peers.players.contains_key(&pid));
        assert_eq!(state.peers.identities.get(&pid), Some(&42));
    }

    /// The server browser's probe holds a real ticket and gets a real
    /// `PlayerId`, so the only thing keeping a Refresh from reading as a join to
    /// everyone playing is that it never asks for the world.
    #[test]
    fn a_status_query_is_answered_without_announcing_anybody() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        handle.deliver(Inbound::Joined {
            player: pid,
            identity: 42,
            account: None,
        });
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::RequestStatus,
        });

        state.pump_network(1.0 / 60.0);

        let net = handle.lock();
        let Some(ServerMessage::Status {
            online,
            max,
            content_hash,
            ..
        }) = net
            .messages_to(pid)
            .into_iter()
            .find(|m| matches!(m, ServerMessage::Status { .. }))
        else {
            panic!("the probe must be told the status");
        };
        assert_eq!(*online, 1, "the host itself is on the server");
        assert_eq!(*max, (crate::net::MAX_CLIENTS + 1) as u32);
        assert_eq!(*content_hash, state.content.hash);
        assert!(
            !net.broadcasts()
                .iter()
                .any(|m| matches!(m, ServerMessage::PlayerJoined { .. })),
            "nobody playing should see a status query"
        );
    }

    /// The dangerous half of the same story: a probe is handed spawn-fresh
    /// vitals in its `Welcome`, so recording it on the way out would overwrite
    /// the real player's saved health, hunger and position for that account.
    #[test]
    fn a_peer_that_never_played_does_not_overwrite_its_accounts_saved_state() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        let identity = 7;
        let saved = PlayerData {
            position: [12.0, 65.0, -8.0],
            yaw: 1.5,
            pitch: 0.2,
            flying: false,
            health: 3.0,
            hunger: 4.0,
            saturation: 0.0,
            selected_slot: 3,
            slots: vec![None; crate::inventory::TOTAL_SLOTS],
        };
        state.save.records.0.insert(identity, saved.clone());

        handle.deliver(Inbound::Joined {
            player: pid,
            identity,
            account: None,
        });
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::RequestStatus,
        });
        state.pump_network(1.0 / 60.0);
        handle.deliver(Inbound::Left { player: pid });
        state.pump_network(1.0 / 60.0);

        let record = state.save.records.0.get(&identity).expect("still recorded");
        assert_eq!(record.health, saved.health, "vitals were rewritten");
        assert_eq!(record.position, saved.position, "position was rewritten");
        assert!(
            !handle
                .lock()
                .broadcasts()
                .iter()
                .any(|m| matches!(m, ServerMessage::PlayerLeft { .. })),
            "nobody was told they arrived, so nobody is told they left"
        );
    }

    /// A returning identity gets its saved position and inventory handed back
    /// in the `Welcome`, rather than starting fresh.
    #[test]
    fn a_returning_player_gets_their_saved_state_back() {
        let (mut state, handle) = host_session();
        let bread = state.content.items.find("bread").unwrap();
        let identity = 7;

        // This world remembers them from a previous session.
        let mut slots = vec![None; crate::inventory::TOTAL_SLOTS];
        slots[3] = Some(ItemStackData {
            id: "bread".to_string(),
            count: 5,
            durability: None,
        });
        state.save.records.0.insert(
            identity,
            PlayerData {
                position: [12.0, 65.0, -8.0],
                yaw: 1.5,
                pitch: 0.2,
                flying: false,
                health: 14.0,
                hunger: 11.0,
                saturation: 2.0,
                selected_slot: 3,
                slots,
            },
        );

        let pid = PlayerId(1);
        handle.deliver(Inbound::Joined {
            player: pid,
            identity,
            account: None,
        });
        state.pump_network(1.0 / 60.0);

        let net = handle.lock();
        let welcome = net.messages_to(pid);
        let Some(ServerMessage::Welcome {
            spawn, restored, ..
        }) = welcome.first()
        else {
            panic!("expected a Welcome");
        };
        let restored = restored.as_ref().expect("a returning player is restored");
        assert_eq!(
            *spawn,
            [12.0, 65.0, -8.0],
            "they resume where they left off"
        );
        assert_eq!(restored.health, 14.0);
        assert_eq!(restored.selected, 3);
        let stack = restored.slots[3].expect("their bread survives the round trip");
        assert_eq!(stack.item, bread.0);
        assert_eq!(stack.count, 5);
    }

    /// A leaving player is snapshotted into the persistent records (so a rejoin
    /// restores them) and dropped from the live session.
    #[test]
    fn a_leaving_player_is_recorded_and_forgotten() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        handle.deliver(Inbound::Joined {
            player: pid,
            identity: 99,
            account: None,
        });
        // Asking for the world is what makes them a player rather than a passing
        // status query, and so what makes them worth recording.
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::RequestWorldState,
        });
        state.pump_network(1.0 / 60.0);

        // They report an inventory, then disconnect.
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::SyncInventory {
                slots: vec![None; crate::inventory::TOTAL_SLOTS],
                selected: 2,
            },
        });
        handle.deliver(Inbound::Left { player: pid });
        state.pump_network(1.0 / 60.0);

        assert!(
            !state.peers.players.contains_key(&pid),
            "dropped from the session"
        );
        assert!(!state.peers.identities.contains_key(&pid));
        assert_eq!(
            state.save.records.0.get(&99).map(|r| r.selected_slot),
            Some(2),
            "their state is kept against their identity for a rejoin"
        );
        assert!(
            handle
                .lock()
                .broadcasts()
                .iter()
                .any(|m| matches!(m, ServerMessage::PlayerLeft { id } if *id == pid)),
            "everyone is told about the departure"
        );
    }

    /// The host applies a client's edit to its own world and echoes it, which
    /// is what makes the host authoritative over terrain.
    #[test]
    fn a_client_edit_request_is_applied_and_echoed() {
        let (mut state, handle) = host_session();
        let pos = BlockPos::new(1, 80, 1);
        state.world.set_block(pos, blocks::STONE);

        handle.deliver(Inbound::Request {
            player: PlayerId(1),
            msg: ClientMessage::Break { pos },
        });
        state.pump_network(1.0 / 60.0);

        assert!(
            state.world.block_at(pos).is_air(),
            "the host applied the break"
        );
        assert!(
            handle.lock().broadcasts().iter().any(|m| matches!(
                m,
                ServerMessage::BlockChanged { pos: p, block } if *p == pos && block.is_air()
            )),
            "and echoed it to every peer"
        );
    }

    /// The same local edit means different things by role: the authority
    /// asserts it, a client can only ask.
    #[test]
    fn a_local_edit_is_asserted_by_a_host_and_requested_by_a_client() {
        let pos = BlockPos::new(4, 70, 4);

        let (mut host, host_net) = host_session();
        host.broadcast_local_edit(pos, blocks::STONE);
        assert!(
            host_net
                .lock()
                .broadcasts()
                .iter()
                .any(|m| matches!(m, ServerMessage::BlockChanged { pos: p, .. } if *p == pos)),
            "a host asserts the edit"
        );
        assert!(host_net.lock().requests().is_empty(), "and asks no one");

        let (mut client, client_net) = client_session(PlayerId(2));
        client.broadcast_local_edit(pos, blocks::STONE);
        let net = client_net.lock();
        assert!(
            net.requests()
                .iter()
                .any(|m| matches!(m, ClientMessage::Place { pos: p, .. } if *p == pos)),
            "a client requests a placement, got {:?}",
            net.requests()
        );
        assert!(net.broadcasts().is_empty(), "and asserts nothing");
    }

    /// Reach is validated host-side: a client's `Attack` lands only when they
    /// were actually next to the mob. Both directions matter — a test that only
    /// checked the rejection would pass even if attacks never applied at all.
    #[test]
    fn a_client_attack_is_reach_validated() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        handle.deliver(Inbound::Joined {
            player: pid,
            identity: 1,
            account: None,
        });
        state.pump_network(1.0 / 60.0);
        // The joiner is placed at the host's position (no saved record).
        let attacker = state.peers.players[&pid].position();

        // In reach: the swing lands.
        let near = state.spawn_mob("cow", attacker).expect("cow spawns");
        let full_health = state.mobs.live[0].health;
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Attack { id: near.0 },
        });
        state.pump_network(1.0 / 60.0);
        assert!(
            state.mobs.live[0].health < full_health,
            "a swing from next to the mob lands"
        );

        // Out of reach: the same message does nothing.
        let hurt_health = state.mobs.live[0].health;
        state.mobs.live[0].position = attacker + Vec3::new(500.0, 0.0, 0.0);
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Attack { id: near.0 },
        });
        state.pump_network(1.0 / 60.0);
        assert_eq!(
            state.mobs.live[0].health, hurt_health,
            "the same swing from 500 blocks away is rejected"
        );
    }

    /// A client applies the host's block edits verbatim — no validation, the
    /// host is truth.
    #[test]
    fn a_client_applies_the_hosts_edits() {
        let (mut state, handle) = client_session(PlayerId(2));
        let pos = BlockPos::new(-3, 90, 7);

        handle.deliver(Inbound::Update(ServerMessage::BlockChanged {
            pos,
            block: blocks::STONE,
        }));
        state.pump_network(1.0 / 60.0);

        assert_eq!(state.world.block_at(pos), blocks::STONE);
    }

    /// A client tells the host where it is, so other players see it move.
    #[test]
    fn a_client_reports_its_position_to_the_host() {
        let (mut state, handle) = client_session(PlayerId(2));
        state.player.position = Vec3::new(3.0, 71.0, -5.0);
        state.pump_network(1.0 / 60.0);

        let net = handle.lock();
        assert!(
            net.requests().iter().any(|m| matches!(
                m,
                ClientMessage::Move { position, .. } if *position == [3.0, 71.0, -5.0]
            )),
            "the client reports its position"
        );
        assert!(
            net.requests()
                .iter()
                .any(|m| matches!(m, ClientMessage::RequestWorldState)),
            "and asks for the world's existing edits exactly once on connect"
        );
        assert_eq!(net.flushes, 1, "one flush per frame");
    }

    fn now_unix() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// A port nothing else is on, found by letting the OS pick one and handing
    /// it straight back.
    fn free_port() -> u16 {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("a spare port");
        socket.local_addr().expect("bound").port()
    }

    fn temp_keys(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "wyven-authkeys-{tag}-{}-{:?}.toml",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    /// The whole status path over a real socket: a host bound on loopback
    /// verifying a real Ed25519 ticket, and a probe that connects, asks, is
    /// answered, and leaves.
    ///
    /// Every other test here fakes the transport away, which is the right
    /// default — but the one thing this feature adds that cannot be faked is
    /// that a probe *is* a client, and that a host will answer one without
    /// treating it as a player.
    #[test]
    fn a_probe_reaches_a_real_host_and_is_answered_without_joining_it() {
        let keys_path = temp_keys("probe");
        // Stamped against the wall clock, because the host on the other end of
        // the socket checks its own: the double's fixed default sits years in
        // the future and would be refused as "not valid yet".
        let auth: Arc<dyn AuthClient> = Arc::new(
            FakeAuthClient::new()
                .with_account("gustav", "hunter2")
                .with_account("mira", "hunter2")
                .issuing_at(now_unix()),
        );
        KeyCache::at(&keys_path)
            .store(&auth.public_keys().expect("the double publishes keys"))
            .expect("keys are cached");

        // A host on a real port, refusing anyone it cannot verify — exactly the
        // gate a live server runs behind.
        let port = free_port();
        let content = GameContent::builtin();
        let ours = content.hash;
        let host = Host::bind(port, 4242, host_config(), TicketJoin::at(&keys_path))
            .expect("binds loopback");
        assert!(host.can_verify(), "the host must be able to check tickets");
        let mut server = InGameState::new_host(content, 4242, host, GameMode::Survival);

        let account = AccountState::new();
        account.sign_in(auth.login("gustav", "hunter2").expect("signs in"));
        let mut probe = NetStatusProbe::with_client(&account, Arc::clone(&auth));
        probe.begin(vec![format!("127.0.0.1:{port}")]);

        let dt = Duration::from_millis(16);
        let deadline = Instant::now() + Duration::from_secs(10);
        let outcome = loop {
            server.pump_network(dt.as_secs_f32());
            if let Some((_, outcome)) = probe.poll(dt).into_iter().next() {
                break outcome;
            }
            assert!(Instant::now() < deadline, "the probe never got an answer");
            std::thread::sleep(dt);
        };

        let StatusOutcome::Online(status) = outcome else {
            panic!("expected the host to answer, got {outcome:?}");
        };
        // Counted while the probe is still connected, which is the point: the
        // probe holds a `PlayerId` at this moment and must not be one of the
        // players the row reports.
        assert_eq!(status.online, 1, "only the host is in the world");
        assert_eq!(status.max, (crate::net::MAX_CLIENTS + 1) as u32);
        assert_eq!(status.content_hash, ours);
        assert!(!status.name.is_empty(), "a row needs something to show");

        // --- and the other half: a real client still announces itself ---
        //
        // Announcing moved off the connect event and onto the first request for
        // the world, which is the change that makes a probe invisible. This is
        // the half that has to keep working: a peer that *does* ask for the
        // world must still be counted, or the browser would report every server
        // as empty.
        //
        // A second account, because a netcode id is derived from the account and
        // netcode admits each id once: one person cannot be playing on a server
        // and querying it in the same breath.
        let player_account = AccountState::new();
        player_account.sign_in(auth.login("mira", "hunter2").expect("signs in"));
        let ticket = crate::net::ticket::issue(&player_account, auth.as_ref(), now_unix())
            .expect("a ticket for the player");
        let mut player = Client::connect(
            format!("127.0.0.1:{port}").parse().expect("loopback"),
            player_account.netcode_id().expect("signed in"),
            crate::net::PROTOCOL_ID,
            Some(ticket.slot),
        )
        .expect("connects");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut asked = false;
        loop {
            server.pump_network(dt.as_secs_f32());
            player.pump(dt).expect("still connected");
            let _ = player.receive();
            if !asked && player.is_connected() {
                player.send(&ClientMessage::RequestWorldState, Channel::Reliable);
                asked = true;
            }
            let _ = player.flush();
            if asked && server.peers.announced.len() == 1 {
                break;
            }
            assert!(Instant::now() < deadline, "the client was never announced");
            std::thread::sleep(dt);
        }

        probe.begin(vec![format!("127.0.0.1:{port}")]);
        let deadline = Instant::now() + Duration::from_secs(10);
        let outcome = loop {
            server.pump_network(dt.as_secs_f32());
            player.pump(dt).expect("still connected");
            let _ = player.flush();
            if let Some((_, outcome)) = probe.poll(dt).into_iter().next() {
                break outcome;
            }
            assert!(Instant::now() < deadline, "the second probe got no answer");
            std::thread::sleep(dt);
        };
        let StatusOutcome::Online(status) = outcome else {
            panic!("expected the host to answer again, got {outcome:?}");
        };
        assert_eq!(status.online, 2, "the host and the player who joined");

        player.disconnect();
        let _ = std::fs::remove_file(&keys_path);
    }
}
