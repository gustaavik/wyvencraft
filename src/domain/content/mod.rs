//! The rules the game is made of, as one value.
//!
//! [`Registries`] is every definition two peers must agree on to share a
//! world — blocks, items, entity kinds, worldgen, structures and spawn rules —
//! and the fingerprint of them that the multiplayer `Welcome` compares.
//!
//! It holds **nothing visual**. Textures, models, icons, display names and
//! sounds are [`crate::presentation::content::Visuals`] and friends, a
//! separate value, so "a visual difference must never refuse a join" is a fact
//! about the types rather than a convention every loader has to remember.

use std::sync::Arc;

use crate::domain::entity::{EntityRegistry, SpawnConfig};
use crate::domain::inventory::ItemRegistry;
use crate::domain::world::BlockRegistry;
use crate::domain::world::generation::WorldGenConfig;
use crate::domain::world::structure::StructureConfig;

pub mod references;

pub use references::{dangling_references, unknown_harvest_tools, unreachable_tiers};

/// Every gameplay-affecting definition, with its fingerprint.
///
/// The hash is computed once, by [`Registries::new`], from exactly the values
/// it is stored beside — there is no way to build one whose hash describes
/// different content.
#[derive(Debug, Clone)]
pub struct Registries {
    pub blocks: Arc<BlockRegistry>,
    pub items: Arc<ItemRegistry>,
    pub entities: Arc<EntityRegistry>,
    pub worldgen: Arc<WorldGenConfig>,
    /// Shrines and boss altars. Part of the terrain, so part of the hash:
    /// peers placing different structures would be walking different worlds.
    pub structures: Arc<StructureConfig>,
    /// Mob spawn rules.
    pub spawning: Arc<SpawnConfig>,
    hash: u64,
}

impl Registries {
    pub fn new(
        blocks: Arc<BlockRegistry>,
        items: Arc<ItemRegistry>,
        entities: Arc<EntityRegistry>,
        worldgen: Arc<WorldGenConfig>,
        structures: Arc<StructureConfig>,
        spawning: Arc<SpawnConfig>,
    ) -> Self {
        let hash = content_hash(
            &blocks,
            &items,
            &entities,
            &worldgen,
            &structures,
            &spawning,
        );
        Self {
            blocks,
            items,
            entities,
            worldgen,
            structures,
            spawning,
            hash,
        }
    }

    /// The compiled-in definitions, and nothing else — exactly what loading
    /// from a source that supplies no files produces, since every loader falls
    /// back to these same builtins.
    pub fn builtin() -> Self {
        let blocks = Arc::new(BlockRegistry::with_builtins());
        let items = Arc::new(ItemRegistry::from_blocks(&blocks));
        let entities = Arc::new(EntityRegistry::builtin());
        let worldgen = Arc::new(WorldGenConfig::builtin(&blocks));
        let structures = Arc::new(StructureConfig::builtin(&blocks, &worldgen));
        let spawning = Arc::new(SpawnConfig::builtin(&entities, &worldgen));
        Self::new(blocks, items, entities, worldgen, structures, spawning)
    }

    /// Fingerprint of every definition here. Exchanged in the multiplayer
    /// `Welcome`: raw block/item ids cross the wire, so a session between
    /// peers with divergent content would silently corrupt worlds —
    /// mismatches refuse to join instead.
    pub fn hash(&self) -> u64 {
        self.hash
    }
}

/// FNV-1a over a canonical rendering of the definitions. The `Debug`
/// representations cover every gameplay-affecting field deterministically —
/// all collections are ordered `Vec`s, and an item's capability components are
/// held in key order for exactly this reason — which is the fidelity the
/// mismatch check needs.
fn content_hash(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    entities: &EntityRegistry,
    worldgen: &WorldGenConfig,
    structures: &StructureConfig,
    spawning: &SpawnConfig,
) -> u64 {
    let repr =
        format!("{blocks:?}|{items:?}|{entities:?}|{worldgen:?}|{structures:?}|{spawning:?}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in repr.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::world::block::BUILTIN_BLOCKS;

    /// Rebuild with one registry swapped, keeping the rest builtin.
    fn with(
        base: &Registries,
        blocks: Option<Arc<BlockRegistry>>,
        structures: Option<Arc<StructureConfig>>,
        spawning: Option<Arc<SpawnConfig>>,
    ) -> Registries {
        Registries::new(
            blocks.unwrap_or_else(|| base.blocks.clone()),
            base.items.clone(),
            base.entities.clone(),
            base.worldgen.clone(),
            structures.unwrap_or_else(|| base.structures.clone()),
            spawning.unwrap_or_else(|| base.spawning.clone()),
        )
    }

    /// The content hash is stable across loads of identical definitions (it
    /// gates multiplayer sessions) and reacts to any definition change.
    #[test]
    fn content_hash_is_stable_and_sensitive() {
        let a = Registries::builtin();
        let b = Registries::builtin();
        assert_eq!(
            a.hash(),
            b.hash(),
            "identical content must hash identically"
        );
        assert_ne!(a.hash(), 0);

        let tweaked = BUILTIN_BLOCKS.replace("hardness = 1.5", "hardness = 9.0");
        let blocks = Arc::new(BlockRegistry::from_toml(&tweaked).unwrap());
        assert_ne!(
            with(&a, Some(blocks), None, None).hash(),
            a.hash(),
            "a changed definition must change the hash"
        );

        // Spawn rules gate multiplayer too: divergent rules = divergent hash.
        use crate::domain::entity::spawning::BUILTIN_SPAWNING;
        let tweaked = BUILTIN_SPAWNING.replace("max_mobs = 40", "max_mobs = 99");
        let spawning =
            Arc::new(SpawnConfig::from_toml(&tweaked, &a.entities, &a.worldgen).unwrap());
        assert_ne!(
            with(&a, None, None, Some(spawning)).hash(),
            a.hash(),
            "changed spawn rules must change the hash"
        );

        // Structures are terrain: a moved shrine is a different world.
        use crate::domain::world::structure::BUILTIN_STRUCTURES;
        let tweaked = BUILTIN_STRUCTURES.replace("chance_per_mille = 650", "chance_per_mille = 10");
        let moved = Arc::new(StructureConfig::from_toml(&tweaked, &a.blocks, &a.worldgen).unwrap());
        assert_ne!(
            with(&a, None, Some(moved), None).hash(),
            a.hash(),
            "changed structures must change the hash"
        );
    }
}
