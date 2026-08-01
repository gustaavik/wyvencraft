//! World persistence: writing the save handle and restoring a returning
//! client's player state from the host.

use glam::Vec3;

use super::net::record_remote;
use super::{InGameState, NetRole};
use crate::inventory::{ItemId, ItemStack, TOTAL_SLOTS};
use crate::net::{PlayerId, PlayerRestore};
use crate::save::{MobsData, PlayerData, WorldData};

impl InGameState {
    /// Apply the saved state the host handed back in its `Welcome` (this client
    /// played this world before). Replaces the starter kit wholesale.
    pub(super) fn apply_restore(&mut self, restore: &PlayerRestore) {
        self.player.teleport(Vec3::from_array(restore.position));
        self.player.yaw = restore.yaw;
        self.player.pitch = restore.pitch;
        self.player.health = restore.health;
        self.player.hunger = restore.hunger;
        self.player.saturation = restore.saturation;
        for index in 0..TOTAL_SLOTS {
            let stack = restore.slots.get(index).and_then(|slot| {
                slot.and_then(|s| {
                    ((s.item as usize) < self.items.len()).then_some(ItemStack {
                        item: ItemId(s.item),
                        count: s.count,
                        durability: s.durability,
                    })
                })
            });
            self.inventory.set_slot(index, stack);
        }
        self.inventory.set_selected(restore.selected as usize);
        // Don't immediately echo the restored inventory back to the host.
        self.last_synced_inventory = Some(self.inventory.clone());
        log::info!("restored player state from host at {:?}", restore.position);
    }

    /// Persist the world if this session owns one (singleplayer or host of a
    /// named world). Clients and ephemeral worlds are no-ops by construction.
    pub(super) fn save_world(&mut self) {
        if self.save.is_none() {
            return;
        }
        debug_assert!(
            !matches!(self.net, NetRole::Client { .. }),
            "clients never hold a save handle"
        );
        // Fold currently connected players into the persistent records first.
        let connected: Vec<PlayerId> = self.remote_players.keys().copied().collect();
        for pid in connected {
            record_remote(
                &mut self.player_records,
                &self.remote_identities,
                &self.remote_players,
                &self.remote_inventories,
                &self.items,
                pid,
            );
        }
        let world = WorldData::from_world(&self.world);
        let player = PlayerData::capture(&self.player, &self.inventory, &self.items);
        let mobs = MobsData::from_mobs(&self.mobs);
        let save = self.save.as_mut().expect("checked above");
        save.meta.game_mode = self.player.mode;
        save.meta.spawn = self.spawn.to_array();
        save.meta.time_of_day = self.day_cycle.time_of_day();
        match save.write(&world, &player, &self.player_records, &mobs) {
            Ok(()) => log::info!(
                "saved world '{}' ({} edits, {} player records, {} mobs)",
                save.meta.name,
                world.edits.len(),
                self.player_records.0.len(),
                mobs.mobs.len()
            ),
            Err(err) => log::error!("failed to save world '{}': {err}", save.meta.name),
        }
    }
}
