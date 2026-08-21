//! Serialized forms of the game state, decoupled from the live types.
//!
//! Blocks and items are referenced by *name* through a palette / per-slot
//! strings: numeric `BlockId`/`ItemId` are registry-insertion-order indices, so
//! raw ids on disk would silently corrupt a world whenever a block or item is
//! added in the middle of a registry. Unknown names on load are skipped with a
//! warning (the same fail-soft policy as the recipe wire sync).

use std::collections::HashMap;

use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::core::{BlockId, BlockPos};
use crate::entity::Player;
use crate::inventory::{Inventory, ItemRegistry, ItemStack, TOTAL_SLOTS};
use crate::world::{BlockRegistry, World};

/// The world's block-edit overlay: everything that diverges from the terrain
/// the seed regenerates. `edits` index into the block-name `palette`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorldData {
    pub palette: Vec<String>,
    pub edits: Vec<(BlockPos, u16)>,
}

impl WorldData {
    /// Snapshot a world's edit overlay, building the name palette on the fly.
    pub fn from_world(world: &World, registry: &BlockRegistry) -> Self {
        let mut palette: Vec<String> = Vec::new();
        let mut index_of: HashMap<BlockId, u16> = HashMap::new();
        let edits = world
            .collect_edits()
            .into_iter()
            .map(|(pos, block)| {
                let index = *index_of.entry(block).or_insert_with(|| {
                    palette.push(registry.get(block).name.to_string());
                    (palette.len() - 1) as u16
                });
                (pos, index)
            })
            .collect();
        Self { palette, edits }
    }

    /// Map the palette back onto this build's registry. Edits naming blocks the
    /// build doesn't know are dropped with one warning per name.
    pub fn resolve(&self, blocks: &BlockRegistry) -> Vec<(BlockPos, BlockId)> {
        let ids: Vec<Option<BlockId>> = self
            .palette
            .iter()
            .map(|name| {
                let id = blocks.find(name);
                if id.is_none() {
                    log::warn!("save references unknown block '{name}'; skipping its edits");
                }
                id
            })
            .collect();
        self.edits
            .iter()
            .filter_map(|&(pos, index)| {
                ids.get(index as usize)
                    .copied()
                    .flatten()
                    .map(|id| (pos, id))
            })
            .collect()
    }
}

/// One inventory slot on disk (item referenced by name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStackData {
    pub name: String,
    pub count: u8,
    pub durability: Option<u16>,
}

/// A player + inventory snapshot. Used both for the save owner (`player.dat`)
/// and per-identity multiplayer records (`players.dat`). Transient fields
/// (velocity, on_ground, perspective, fall bookkeeping) deliberately reset on
/// load; the game mode lives in `level.toml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerData {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub flying: bool,
    pub health: f32,
    pub hunger: f32,
    pub saturation: f32,
    pub selected_slot: u32,
    pub slots: Vec<Option<ItemStackData>>,
}

impl PlayerData {
    /// Snapshot the local player and inventory (item ids → names).
    pub fn capture(player: &Player, inventory: &Inventory, items: &ItemRegistry) -> Self {
        Self {
            position: player.position.to_array(),
            yaw: player.yaw,
            pitch: player.pitch,
            flying: player.flying,
            health: player.health,
            hunger: player.hunger,
            saturation: player.saturation,
            selected_slot: inventory.selected_index() as u32,
            slots: inventory
                .slots()
                .iter()
                .map(|slot| {
                    slot.map(|stack| ItemStackData {
                        name: items.get(stack.item).name.clone(),
                        count: stack.count,
                        durability: stack.durability,
                    })
                })
                .collect(),
        }
    }

    /// Restore onto a freshly built player/inventory. Overwrites *all* slots
    /// (clearing the starter kit); unknown item names become empty slots.
    ///
    /// A save written before armor existed carries only the 36 storage slots;
    /// the missing armor entries read back as `None`, so old worlds load with an
    /// unarmored player instead of failing the version check.
    pub fn apply(&self, player: &mut Player, inventory: &mut Inventory, items: &ItemRegistry) {
        player.teleport(Vec3::from_array(self.position));
        player.yaw = self.yaw;
        player.pitch = self.pitch;
        player.flying = self.flying;
        player.health = self.health;
        player.hunger = self.hunger;
        player.saturation = self.saturation;
        for index in 0..TOTAL_SLOTS {
            let stack = self.slots.get(index).and_then(|slot| {
                slot.as_ref().and_then(|data| {
                    let id = items.find(&data.name);
                    if id.is_none() {
                        log::warn!("save references unknown item '{}'; dropping it", data.name);
                    }
                    id.map(|item| ItemStack {
                        item,
                        count: data.count,
                        durability: data.durability,
                    })
                })
            });
            inventory.set_slot(index, stack);
        }
        inventory.set_selected(self.selected_slot as usize);
    }
}

/// Per-identity player records a host keeps for its world, keyed by the stable
/// netcode client id. Lets a returning client get its inventory/position back.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlayerRecords(pub HashMap<u64, PlayerData>);

/// One saved mob (`mobs.dat`). Kind by name (the save convention); transient
/// state (velocity, brain, cooldowns, mob ids) deliberately resets on load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MobData {
    pub kind: String,
    pub position: [f32; 3],
    pub health: f32,
    /// Under the daylight-despawn rule (so dawn still reaps it after a load).
    pub night_spawned: bool,
}

/// The world's live mob population (`mobs.dat`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MobsData {
    pub mobs: Vec<MobData>,
}

impl MobsData {
    /// Snapshot the live mobs for the save.
    pub fn from_mobs(mobs: &[crate::entity::Mob]) -> Self {
        Self {
            mobs: mobs
                .iter()
                .map(|mob| MobData {
                    kind: mob.kind_name.clone(),
                    position: mob.position.to_array(),
                    health: mob.health,
                    night_spawned: mob.night_spawned,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::GameMode;
    use crate::world::block::blocks;
    use crate::world::{NoiseGenerator, WorldGenerator};
    use std::sync::Arc;

    fn test_registries() -> (Arc<BlockRegistry>, ItemRegistry) {
        let blocks = Arc::new(BlockRegistry::with_builtins());
        let items = ItemRegistry::from_blocks(&blocks);
        (blocks, items)
    }

    #[test]
    fn world_edits_roundtrip_through_the_palette() {
        let (registry, _) = test_registries();
        let generator: Arc<dyn WorldGenerator> = Arc::new(NoiseGenerator::new(42));
        let mut world = World::new(generator, registry.clone());

        let edits = [
            (BlockPos::new(3, 200, 5), blocks::STONE),
            (BlockPos::new(-20, 190, 40), blocks::GLASS),
            (BlockPos::new(3, 201, 5), blocks::STONE),
        ];
        for &(pos, block) in &edits {
            world.ensure_chunk(pos.chunk());
            assert!(world.set_block(pos, block).is_some());
        }

        let data = WorldData::from_world(&world, &registry);
        assert_eq!(data.palette.len(), 2, "palette dedups block names");

        // Round-trip through bincode like the .dat files do.
        let bytes = bincode::serde::encode_to_vec(&data, bincode::config::standard()).unwrap();
        let (back, _): (WorldData, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).unwrap();

        let mut resolved = back.resolve(&registry);
        resolved.sort();
        let mut expected = world.collect_edits();
        expected.sort();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn unknown_palette_names_drop_only_their_edits() {
        let (registry, _) = test_registries();
        // Palette order deliberately differs from registry id order, plus a name
        // this build doesn't know — simulating a registry reshuffle across builds.
        let data = WorldData {
            palette: vec!["glass".into(), "no_such_block".into(), "dirt".into()],
            edits: vec![
                (BlockPos::new(0, 100, 0), 0),
                (BlockPos::new(1, 100, 0), 1),
                (BlockPos::new(2, 100, 0), 2),
            ],
        };
        let resolved = data.resolve(&registry);
        assert_eq!(
            resolved,
            vec![
                (BlockPos::new(0, 100, 0), blocks::GLASS),
                (BlockPos::new(2, 100, 0), blocks::DIRT),
            ]
        );
    }

    #[test]
    fn player_snapshot_roundtrips_and_drops_unknown_items() {
        let (_, items) = test_registries();
        let kinds = crate::entity::EntityRegistry::builtin();
        let mut player = Player::new(
            Vec3::new(10.0, 70.0, -4.0),
            GameMode::Survival,
            kinds.player(),
        );
        player.yaw = 1.2;
        player.pitch = -0.3;
        player.health = 11.0;
        player.hunger = 9.0;
        player.saturation = 2.0;
        let mut inventory = Inventory::new();
        inventory.set_slot(
            0,
            Some(ItemStack::with_durability(
                items.find("wooden pickaxe").unwrap(),
                33,
            )),
        );
        inventory.set_slot(9, Some(ItemStack::new(items.find("bread").unwrap(), 3)));
        inventory.set_selected(5);

        let mut data = PlayerData::capture(&player, &inventory, &items);
        // A record from another build may name an item we don't have.
        data.slots[1] = Some(ItemStackData {
            name: "netherite doohickey".into(),
            count: 1,
            durability: None,
        });

        let mut restored_player = Player::new(Vec3::ZERO, GameMode::Survival, kinds.player());
        let mut restored_inventory = Inventory::new();
        // Pre-fill a slot to prove apply() clears slots the snapshot left empty.
        restored_inventory.set_slot(20, Some(ItemStack::new(items.find("apple").unwrap(), 2)));
        data.apply(&mut restored_player, &mut restored_inventory, &items);

        assert_eq!(restored_player.position, player.position);
        assert_eq!(restored_player.yaw, player.yaw);
        assert_eq!(restored_player.health, player.health);
        assert_eq!(restored_player.saturation, player.saturation);
        assert_eq!(
            restored_inventory.slot(0),
            Some(ItemStack::with_durability(
                items.find("wooden pickaxe").unwrap(),
                33
            ))
        );
        assert_eq!(
            restored_inventory.slot(9),
            Some(ItemStack::new(items.find("bread").unwrap(), 3))
        );
        assert_eq!(restored_inventory.slot(1), None, "unknown item dropped");
        assert_eq!(restored_inventory.slot(20), None, "stale slot cleared");
        assert_eq!(restored_inventory.selected_index(), 5);
    }
}
