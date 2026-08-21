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

    /// Admit a joining player: hand back their saved state if this world
    /// remembers them, announce them, and bring them up to date on everyone's
    /// current gear (unchanging equipment isn't otherwise re-broadcast).
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

        let joined = ServerMessage::PlayerJoined {
            id: pid,
            name: name.clone(),
        };
        self.session.broadcast(&joined, Channel::Reliable);

        self.peers.identities.insert(pid, identity);
        if let Some(account) = account {
            self.peers.accounts.insert(pid, account);
        }
        self.peers
            .players
            .insert(pid, RemotePlayer::new(pid, name, Vec3::from_array(spawn)));

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

    /// Snapshot a leaving player so their state survives a rejoin, then drop
    /// every trace of them from this session.
    fn forget_player(&mut self, pid: PlayerId) {
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
            ClientMessage::RequestWorldState => self.replay_world_state(pid),
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
        if let Some(mob) = self.mobs.iter_mut().find(|m| m.id.0 == mob_id)
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
                        self.remote_mobs
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
                    if let Some(mob) = self.remote_mobs.get_mut(&id) {
                        mob.push_snapshot(Vec3::from_array(position), yaw);
                    }
                }
            }
            ServerMessage::MobHurt { .. } => {
                // Reserved for hurt feedback (flash/sound); the authoritative
                // outcome arrives as MobDespawned.
            }
            ServerMessage::MobDespawned { id, killed_by } => {
                if let Some(mob) = self.remote_mobs.remove(&id)
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
                self.arrows.push(Arrow::new(
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
        if !self.mobs.is_empty() {
            let states = ServerMessage::MobStates {
                mobs: self
                    .mobs
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
        Some((slots, selected)) => (wire_slots_to_names(slots, items), *selected),
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

/// Serialize the recipe book back to item names for the `Welcome` message.
pub(super) fn recipes_to_wire(book: &RecipeBook, items: &ItemRegistry) -> Vec<RecipeData> {
    book.recipes()
        .iter()
        .map(|recipe| RecipeData {
            output: items.get(recipe.output).name.clone(),
            count: recipe.count as u32,
            ingredients: recipe
                .ingredients
                .iter()
                .map(|&(item, n)| (items.get(item).name.clone(), n))
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

/// Convert wire inventory slots to the name-based on-disk form. Ids out of this
/// build's registry range (mismatched peer) become empty slots.
fn wire_slots_to_names(
    slots: &[Option<NetItemStack>],
    items: &ItemRegistry,
) -> Vec<Option<ItemStackData>> {
    slots
        .iter()
        .map(|slot| {
            slot.and_then(|s| {
                ((s.item as usize) < items.len()).then(|| ItemStackData {
                    name: items.get(ItemId(s.item)).name.clone(),
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
                    items.find(&s.name).map(|id| NetItemStack {
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
    use super::*;
    use crate::content::GameContent;
    use crate::core::GameMode;
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

        hold(&mut state, "iron sword");
        assert_eq!(state.melee_damage(), 6.0, "iron sword");
        hold(&mut state, "wooden sword");
        assert_eq!(state.melee_damage(), 4.0, "wooden sword");
        hold(&mut state, "iron axe");
        assert_eq!(state.melee_damage(), 5.0, "iron axe");

        hold(&mut state, "iron pickaxe");
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

        let sword = state.content.items.find("iron sword").expect("iron sword");
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
    /// announced to everyone, and registered as a remote player.
    #[test]
    fn a_joining_player_is_welcomed_and_announced() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        handle.deliver(Inbound::Joined {
            player: pid,
            identity: 42,
            account: None,
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
            name: "bread".to_string(),
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
        let full_health = state.mobs[0].health;
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Attack { id: near.0 },
        });
        state.pump_network(1.0 / 60.0);
        assert!(
            state.mobs[0].health < full_health,
            "a swing from next to the mob lands"
        );

        // Out of reach: the same message does nothing.
        let hurt_health = state.mobs[0].health;
        state.mobs[0].position = attacker + Vec3::new(500.0, 0.0, 0.0);
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Attack { id: near.0 },
        });
        state.pump_network(1.0 / 60.0);
        assert_eq!(
            state.mobs[0].health, hurt_health,
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
}
