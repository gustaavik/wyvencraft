//! Per-frame network pump and wire (de)serialization for [`InGameState`].
//!
//! The free functions convert between the in-memory model and the protocol's
//! wire types; several are `pub(super)` so construction, persistence, and tests
//! in sibling modules can reuse them.

use std::collections::HashMap;
use std::time::Duration;

use glam::Vec3;

use super::mobs::{self, RemoteMob};
use super::{
    HOST_PLAYER_ID, INVENTORY_SYNC_INTERVAL, InGameState, NetRole, STATS_INTERVAL, WORLD_SYNC_BATCH,
};
use crate::core::{BlockId, BlockPos};
use crate::entity::Arrow;
use crate::inventory::{ARMOR_SIZE, ARMOR_START, Inventory, ItemId, ItemRegistry, RecipeBook};
use crate::net::{
    Channel, ClientMessage, NetItemStack, PlayerId, PlayerRestore, RecipeData, RemotePlayer,
    ServerMessage,
};
use crate::save::{ItemStackData, PlayerData, PlayerRecords};

impl InGameState {
    /// Drive networking for one frame: process incoming, broadcast local state.
    pub(super) fn pump_network(&mut self, dt: f32) {
        let duration = Duration::from_secs_f32(dt.max(1.0e-4));
        let position = self.player.position.to_array();
        let yaw = self.player.yaw;
        let pitch = self.player.pitch;
        let mode = self.player.mode;
        let health = self.player.health;
        let hunger = self.player.hunger;
        let saturation = self.player.saturation;

        // Survival stats are low-frequency; throttle them to keep the wire quiet.
        self.stats_timer += dt;
        let send_stats = self.stats_timer >= STATS_INTERVAL;
        if send_stats {
            self.stats_timer = 0.0;
        }

        // Clients report their inventory (throttled, only on change) so the
        // host can persist it in the world save.
        let mut inventory_sync = None;
        if matches!(self.net, NetRole::Client { .. }) {
            self.inventory_sync_timer += dt;
            if self.inventory_sync_timer >= INVENTORY_SYNC_INTERVAL {
                self.inventory_sync_timer = 0.0;
                let changed = self.last_synced_inventory.as_ref().is_none_or(|last| {
                    last.slots() != self.inventory.slots()
                        || last.selected_index() != self.inventory.selected_index()
                });
                if changed {
                    inventory_sync = Some(inventory_to_wire(&self.inventory));
                    self.last_synced_inventory = Some(self.inventory.clone());
                }
            }
        }

        // One-shot initial world-state request (client only). Captured here and
        // written back after the match to avoid borrowing `self` while `self.net` is.
        let need_world_request = !self.world_state_requested;
        let mut requested_world_state_now = false;
        // Client-side effects that need `&mut self` and so must run after the
        // match releases its borrow of `self.net`: loot from mobs this player
        // killed, and mob damage addressed to this player.
        let mut my_kills: Vec<(String, u64, Vec3)> = Vec::new();
        let mut incoming_damage = 0.0f32;

        match &mut self.net {
            NetRole::Singleplayer => {}
            NetRole::Host(host) => {
                host.pump(duration);
                let seed = self.world.seed();
                let time_of_day = self.day_cycle.time_of_day();

                for cid in host.take_joined() {
                    if let Some(pid) = host.player_id(cid) {
                        // The netcode client id doubles as the player's stable
                        // identity: returning players get their saved state back.
                        let identity: u64 = cid;
                        let restored = self
                            .player_records
                            .0
                            .get(&identity)
                            .map(|record| record_to_restore(record, &self.items));
                        let spawn = restored.as_ref().map(|r| r.position).unwrap_or(position);
                        if restored.is_some() {
                            log::info!("player {} rejoined; restoring saved state", pid.0);
                        }
                        let name = format!("Player {}", pid.0);
                        host.send(
                            cid,
                            &ServerMessage::Welcome {
                                seed,
                                your_id: pid,
                                spawn,
                                time_of_day,
                                game_mode: mode,
                                content_hash: self.content_hash,
                                recipes: recipes_to_wire(&self.recipes, &self.items),
                                restored,
                            },
                            Channel::Reliable,
                        );
                        host.broadcast(
                            &ServerMessage::PlayerJoined {
                                id: pid,
                                name: name.clone(),
                            },
                            Channel::Reliable,
                        );
                        self.remote_identities.insert(pid, identity);
                        self.remote_players
                            .insert(pid, RemotePlayer::new(pid, name, Vec3::from_array(spawn)));
                        // Bring the newcomer up to date on everyone's current gear
                        // (unchanging equipment isn't otherwise re-broadcast).
                        for (&id, &armor) in &self.equipment_broadcast {
                            host.send(
                                cid,
                                &ServerMessage::PlayerEquipment { id, armor },
                                Channel::Reliable,
                            );
                        }
                    }
                }
                for pid in host.take_left() {
                    // Snapshot the leaving player so their state survives a rejoin.
                    record_remote(
                        &mut self.player_records,
                        &self.remote_identities,
                        &self.remote_players,
                        &self.remote_inventories,
                        &self.items,
                        pid,
                    );
                    self.remote_players.remove(&pid);
                    self.remote_identities.remove(&pid);
                    self.remote_inventories.remove(&pid);
                    self.equipment_broadcast.remove(&pid);
                    host.broadcast(&ServerMessage::PlayerLeft { id: pid }, Channel::Reliable);
                }

                for (pid, msg) in host.receive() {
                    match msg {
                        ClientMessage::Move {
                            position,
                            yaw,
                            pitch,
                        } => {
                            if let Some(rp) = self.remote_players.get_mut(&pid) {
                                rp.push_snapshot(Vec3::from_array(position), yaw, pitch);
                            }
                        }
                        ClientMessage::Break { pos } => {
                            if self.world.set_block(pos, BlockId::AIR).is_some() {
                                self.fluids.block_changed(pos);
                                host.broadcast(
                                    &ServerMessage::BlockChanged {
                                        pos,
                                        block: BlockId::AIR,
                                    },
                                    Channel::Reliable,
                                );
                            }
                        }
                        ClientMessage::Place { pos, block } => {
                            if self.world.set_block(pos, block).is_some() {
                                self.fluids.block_changed(pos);
                                host.broadcast(
                                    &ServerMessage::BlockChanged { pos, block },
                                    Channel::Reliable,
                                );
                            }
                        }
                        ClientMessage::Stats {
                            health,
                            hunger,
                            saturation,
                        } => {
                            if let Some(rp) = self.remote_players.get_mut(&pid) {
                                rp.health = health;
                                rp.hunger = hunger;
                                rp.saturation = saturation;
                            }
                        }
                        ClientMessage::SetMode(m) => {
                            if let Some(rp) = self.remote_players.get_mut(&pid) {
                                rp.mode = m;
                            }
                        }
                        ClientMessage::SyncInventory { slots, selected } => {
                            if let Some(rp) = self.remote_players.get_mut(&pid) {
                                rp.armor = armor_from_slots(&slots);
                            }
                            self.remote_inventories.insert(pid, (slots, selected));
                        }
                        ClientMessage::Chat(_) => {}
                        ClientMessage::RequestWorldState => {
                            let edits = self.world.collect_edits();
                            log::debug!(
                                "replaying {} world edits to player {}",
                                edits.len(),
                                pid.0
                            );
                            for batch in edits.chunks(WORLD_SYNC_BATCH) {
                                host.send_to_player(
                                    pid,
                                    &ServerMessage::WorldEdits {
                                        edits: batch.to_vec(),
                                    },
                                    Channel::Chunk,
                                );
                            }
                            // ... and the current mob population, so the
                            // joiner sees what already roams the world.
                            for mob in &self.mobs {
                                host.send_to_player(
                                    pid,
                                    &ServerMessage::MobSpawned {
                                        id: mob.id.0,
                                        kind: mob.kind_name.clone(),
                                        position: mob.position.to_array(),
                                    },
                                    Channel::Reliable,
                                );
                            }
                        }
                        ClientMessage::Attack { id } => {
                            // Validate reach against the attacker's last known
                            // position, then apply with kill credit; the hurt
                            // (or death) event reaches clients via mob_events.
                            let Some(attacker) =
                                self.remote_players.get(&pid).map(|rp| rp.position())
                            else {
                                continue;
                            };
                            if let Some(mob) = self.mobs.iter_mut().find(|m| m.id.0 == id)
                                && mobs::attack_in_range(attacker, mob.position)
                            {
                                let to_mob = mob.position - attacker;
                                let push = Vec3::new(to_mob.x, 0.0, to_mob.z).normalize_or_zero()
                                    * mobs::KNOCKBACK_PUSH
                                    + Vec3::Y * mobs::KNOCKBACK_LIFT;
                                mob.damage(mobs::PLAYER_ATTACK_DAMAGE, push);
                                mob.last_attacker = Some(pid.0);
                                self.mob_events.push(ServerMessage::MobHurt {
                                    id,
                                    health: mob.health,
                                });
                            }
                        }
                    }
                }

                // Broadcast authoritative player snapshots.
                host.broadcast(
                    &ServerMessage::PlayerState {
                        id: HOST_PLAYER_ID,
                        position,
                        yaw,
                        pitch,
                    },
                    Channel::Unreliable,
                );
                let snapshots: Vec<_> = self
                    .remote_players
                    .iter()
                    .map(|(pid, rp)| (*pid, rp.position().to_array(), rp.yaw, rp.pitch))
                    .collect();
                for (id, position, yaw, pitch) in snapshots {
                    host.broadcast(
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
                    host.broadcast(
                        &ServerMessage::PlayerStats {
                            id: HOST_PLAYER_ID,
                            health,
                            hunger,
                            mode,
                        },
                        Channel::Reliable,
                    );
                    let stats: Vec<_> = self
                        .remote_players
                        .values()
                        .map(|rp| (rp.id, rp.health, rp.hunger, rp.mode))
                        .collect();
                    for (id, health, hunger, mode) in stats {
                        host.broadcast(
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

                // Broadcast armor whenever it changes (the host's own from its
                // inventory, each remote's from its last inventory sync).
                let mut equip: Vec<(PlayerId, [Option<u16>; ARMOR_SIZE])> =
                    vec![(HOST_PLAYER_ID, armor_ids(&self.inventory))];
                equip.extend(self.remote_players.iter().map(|(pid, rp)| (*pid, rp.armor)));
                for (id, armor) in equip {
                    if self.equipment_broadcast.get(&id) != Some(&armor) {
                        self.equipment_broadcast.insert(id, armor);
                        host.broadcast(
                            &ServerMessage::PlayerEquipment { id, armor },
                            Channel::Reliable,
                        );
                    }
                }

                // Mob lifecycle events queued by this frame's simulation
                // (spawns, hurts, deaths, arrows, remote-player damage), then
                // one batched unreliable movement snapshot for all live mobs.
                for msg in std::mem::take(&mut self.mob_events) {
                    host.broadcast(&msg, Channel::Reliable);
                }
                if !self.mobs.is_empty() {
                    host.broadcast(
                        &ServerMessage::MobStates {
                            mobs: self
                                .mobs
                                .iter()
                                .map(|m| (m.id.0, m.position.to_array(), m.yaw))
                                .collect(),
                        },
                        Channel::Unreliable,
                    );
                }
                host.flush();
            }
            NetRole::Client { client, local_id } => {
                let local_id = *local_id;
                if let Err(err) = client.pump(duration) {
                    log::warn!("client pump error: {err}");
                }
                // Ask the host to replay the world's existing edits, once connected.
                if need_world_request && client.is_connected() {
                    client.send(&ClientMessage::RequestWorldState, Channel::Reliable);
                    requested_world_state_now = true;
                }
                client.send(
                    &ClientMessage::Move {
                        position,
                        yaw,
                        pitch,
                    },
                    Channel::Unreliable,
                );
                if send_stats {
                    client.send(
                        &ClientMessage::Stats {
                            health,
                            hunger,
                            saturation,
                        },
                        Channel::Reliable,
                    );
                }
                if let Some((slots, selected)) = inventory_sync.take() {
                    client.send(
                        &ClientMessage::SyncInventory { slots, selected },
                        Channel::Reliable,
                    );
                }
                for msg in client.receive() {
                    match msg {
                        ServerMessage::Welcome { .. } => {}
                        ServerMessage::PlayerJoined { id, name } if id != local_id => {
                            self.remote_players
                                .entry(id)
                                .or_insert_with(|| RemotePlayer::new(id, name, Vec3::ZERO));
                        }
                        ServerMessage::PlayerLeft { id } => {
                            self.remote_players.remove(&id);
                        }
                        ServerMessage::PlayerState {
                            id,
                            position,
                            yaw,
                            pitch,
                        } if id != local_id => {
                            self.remote_players
                                .entry(id)
                                .or_insert_with(|| {
                                    RemotePlayer::new(
                                        id,
                                        format!("Player {}", id.0),
                                        Vec3::from_array(position),
                                    )
                                })
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
                            self.remote_players
                                .entry(id)
                                .or_insert_with(|| {
                                    RemotePlayer::new(id, format!("Player {}", id.0), Vec3::ZERO)
                                })
                                .set_stats(health, hunger, mode);
                        }
                        ServerMessage::PlayerEquipment { id, armor } if id != local_id => {
                            self.remote_players
                                .entry(id)
                                .or_insert_with(|| {
                                    RemotePlayer::new(id, format!("Player {}", id.0), Vec3::ZERO)
                                })
                                .armor = armor;
                        }
                        ServerMessage::MobSpawned { id, kind, position } => {
                            match self.entities.find(&kind) {
                                Some(k) if k.mob.is_some() => {
                                    log::debug!("replicating mob {id} ({kind}) from host");
                                    self.remote_mobs
                                        .insert(id, RemoteMob::new(k, Vec3::from_array(position)));
                                }
                                // Shouldn't happen (the content hash gates
                                // divergent builds), but degrade gracefully.
                                _ => log::warn!("host spawned unknown mob kind {kind:?}; ignoring"),
                            }
                        }
                        ServerMessage::MobStates { mobs } => {
                            for (id, position, yaw) in mobs {
                                // Unknown ids are fine: an unreliable snapshot
                                // can outrun its reliable MobSpawned.
                                if let Some(mob) = self.remote_mobs.get_mut(&id) {
                                    mob.push_snapshot(Vec3::from_array(position), yaw);
                                }
                            }
                        }
                        ServerMessage::MobHurt { .. } => {
                            // Reserved for hurt feedback (flash/sound); the
                            // authoritative outcome arrives as MobDespawned.
                        }
                        ServerMessage::MobDespawned { id, killed_by } => {
                            if let Some(mob) = self.remote_mobs.remove(&id)
                                && killed_by == Some(local_id)
                            {
                                // This player made the kill: roll the loot
                                // locally (deferred; needs &mut self).
                                my_kills.push((mob.kind_name().to_string(), id, mob.position()));
                            }
                        }
                        ServerMessage::ArrowSpawned {
                            position,
                            velocity,
                            gravity,
                            lifetime,
                        } => {
                            // Visual-only on clients: damage is host-side, so
                            // the local copy carries none.
                            self.arrows.push(Arrow::new(
                                Vec3::from_array(position),
                                Vec3::from_array(velocity),
                                0.0,
                                gravity,
                                lifetime,
                            ));
                        }
                        ServerMessage::PlayerDamaged { id, amount } if id == local_id => {
                            incoming_damage += amount;
                        }
                        _ => {}
                    }
                }
                let _ = client.flush();
            }
        }

        if requested_world_state_now {
            self.world_state_requested = true;
        }
        for (kind, id, position) in my_kills {
            self.pop_drops_for(&kind, id, position);
        }
        if incoming_damage > 0.0 {
            self.damage_local_player(incoming_damage);
        }
    }

    /// Propagate a local block edit to the network (host broadcasts, client requests).
    pub(super) fn broadcast_local_edit(&mut self, pos: BlockPos, block: BlockId) {
        match &mut self.net {
            NetRole::Singleplayer => {}
            NetRole::Host(host) => {
                host.broadcast(
                    &ServerMessage::BlockChanged { pos, block },
                    Channel::Reliable,
                );
            }
            NetRole::Client { client, .. } => {
                let msg = if block.is_air() {
                    ClientMessage::Break { pos }
                } else {
                    ClientMessage::Place { pos, block }
                };
                client.send(&msg, Channel::Reliable);
            }
        }
    }

    /// Tell the host the local player's game mode changed (no-op for host /
    /// singleplayer — the host advertises its mode via `PlayerStats`/`Welcome`).
    pub(super) fn broadcast_mode_change(&mut self) {
        let mode = self.player.mode;
        if let NetRole::Client { client, .. } = &mut self.net {
            client.send(&ClientMessage::SetMode(mode), Channel::Reliable);
        }
    }

    pub(super) fn net_status(&self) -> String {
        match &self.net {
            NetRole::Singleplayer => "singleplayer".to_string(),
            NetRole::Host(host) => format!("host ({} players)", host.player_count()),
            NetRole::Client { .. } => format!("client ({} remote)", self.remote_players.len()),
        }
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
