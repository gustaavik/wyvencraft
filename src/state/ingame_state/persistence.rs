//! World persistence: writing the save handle and restoring a returning
//! client's player state from the host.

use glam::Vec3;

use super::net::record_remote;
use super::{InGameState, NetRole};
use crate::inventory::{ItemId, ItemStack, TOTAL_SLOTS};
use crate::net::{PlayerId, PlayerRestore};
use crate::save::{MobsData, PlayerData, WorldData, WorldSnapshot};

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
    /// named world). Clients and ephemeral worlds hold the null repository, so
    /// they bail before doing the real work of capturing a snapshot.
    pub(super) fn save_world(&mut self) {
        if !self.save.is_persistent() {
            return;
        }
        debug_assert!(
            !matches!(self.net, NetRole::Client { .. }),
            "clients never hold a persistent repository"
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
        let snapshot = WorldSnapshot {
            world: &world,
            player: &player,
            players: &self.player_records,
            mobs: &mobs,
            game_mode: self.player.mode,
            spawn: self.spawn.to_array(),
            time_of_day: self.day_cycle.time_of_day(),
        };
        match self.save.store(&snapshot) {
            Ok(()) => log::info!(
                "saved world '{}' ({} edits, {} player records, {} mobs)",
                self.save.world_name(),
                world.edits.len(),
                self.player_records.0.len(),
                mobs.mobs.len()
            ),
            Err(err) => log::error!("failed to save world '{}': {err}", self.save.world_name()),
        }
    }

    /// Swap in a different persistence destination. Tests use this to capture a
    /// save without a `saves/` directory; production wiring sets it in `setup`.
    #[cfg(test)]
    pub(super) fn set_repository(&mut self, repo: Box<dyn crate::save::WorldRepository>) {
        self.save = repo;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::GameContent;
    use crate::core::{BlockPos, GameMode};
    use crate::save::InMemoryWorldRepository;
    use crate::world::block::blocks;

    /// The null repository is what clients and ephemeral worlds hold: calling
    /// `save_world` on one must be a silent no-op, not a panic or a write.
    #[test]
    fn an_ephemeral_world_never_saves() {
        let mut state = InGameState::new(GameContent::builtin(), 3, GameMode::Survival);
        assert!(
            !state.save.is_persistent(),
            "a world built without a save handle holds the null repository"
        );
        state.save_world(); // must not panic
        assert_eq!(state.save.world_name(), "(unsaved)");
    }

    /// A persistent repository receives the session's live state — terrain
    /// edits, player, mobs, and the metadata that lives in `level.toml`.
    #[test]
    fn saving_captures_the_live_session_state() {
        let mut state = InGameState::new(GameContent::builtin(), 11, GameMode::Creative);
        let repo = InMemoryWorldRepository::default();
        let log = repo.log();
        state.set_repository(Box::new(repo));

        let edit = BlockPos::new(2, 100, -3);
        state.world.set_block(edit, blocks::STONE);
        state.player.position = Vec3::new(1.0, 70.0, 2.0);
        state
            .spawn_mob("zombie", Vec3::new(4.0, 70.0, 4.0))
            .expect("zombie spawns");

        state.save_world();

        let log = log.lock().unwrap();
        assert_eq!(log.writes, 1);
        let stored = log.last.as_ref().expect("a snapshot was stored");
        assert_eq!(stored.game_mode, GameMode::Creative);
        assert_eq!(stored.player.position, [1.0, 70.0, 2.0]);
        assert_eq!(stored.mobs.mobs.len(), 1);
        assert!(
            stored.world.edits.iter().any(|&(pos, _)| pos == edit),
            "the terrain edit reached the repository"
        );
    }
}
