//! Data-driven game content.
//!
//! [`GameContent`] owns the registries loaded from the TOML files under
//! `assets/` (blocks today; items, entities, and worldgen follow the same
//! pattern). It is loaded once at app startup and shared via `Arc` — the
//! renderer needs the texture set before any game state exists, and every
//! session (singleplayer, host, client) reads the same definitions.
//!
//! Loading is fail-soft, following the recipes-file precedent: a missing or
//! invalid file logs a warning and falls back to the embedded builtin copy,
//! so the game always boots.

use std::sync::Arc;

use crate::entity::EntityRegistry;
use crate::inventory::ItemRegistry;
use crate::render::TileRegistry;
use crate::world::block::{BUILTIN_BLOCKS, BlockRegistry};
use crate::world::generation::WorldGenConfig;

/// All loaded content registries, shared across the app.
pub struct GameContent {
    /// Texture name → atlas tile assignments plus the CPU-side atlas pixels
    /// (uploaded once by the renderer at startup).
    pub tiles: TileRegistry,
    pub blocks: Arc<BlockRegistry>,
    pub items: Arc<ItemRegistry>,
    pub entities: Arc<EntityRegistry>,
    pub worldgen: Arc<WorldGenConfig>,
    /// Fingerprint of every gameplay-affecting definition. Exchanged in the
    /// multiplayer `Welcome`: raw block/item ids cross the wire, so a session
    /// between peers with divergent content would silently corrupt worlds —
    /// mismatches refuse to join instead. Texture pixels are excluded
    /// (visual-only divergence is harmless).
    pub hash: u64,
}

const BLOCKS_PATH: &str = "assets/blocks.toml";
const ITEMS_PATH: &str = "assets/items.toml";
const ENTITIES_PATH: &str = "assets/entities.toml";
const WORLDGEN_PATH: &str = "assets/worldgen.toml";

impl GameContent {
    /// Load content from `assets/` (CWD-relative, like recipes and saves),
    /// falling back to the embedded builtin copies. Never fails.
    pub fn load() -> Arc<Self> {
        let mut tiles = TileRegistry::with_engine_tiles();
        let blocks = Arc::new(load_blocks(&mut tiles));
        let items = Arc::new(load_items(&blocks));
        let entities = Arc::new(load_entities());
        let worldgen = Arc::new(load_worldgen(&blocks));
        let hash = content_hash(&blocks, &items, &entities, &worldgen);
        Arc::new(Self {
            tiles,
            blocks,
            items,
            entities,
            worldgen,
            hash,
        })
    }

    /// The embedded builtin content only — used by tests and as the fallback.
    pub fn builtin() -> Arc<Self> {
        let mut tiles = TileRegistry::with_engine_tiles();
        let blocks = Arc::new(builtin_blocks(&mut tiles));
        let items = Arc::new(ItemRegistry::from_blocks(&blocks));
        let entities = Arc::new(EntityRegistry::builtin());
        let worldgen = Arc::new(WorldGenConfig::builtin(&blocks));
        let hash = content_hash(&blocks, &items, &entities, &worldgen);
        Arc::new(Self {
            tiles,
            blocks,
            items,
            entities,
            worldgen,
            hash,
        })
    }
}

/// FNV-1a over a canonical rendering of the definitions. The `Debug`
/// representations cover every gameplay-affecting field deterministically
/// (all collections are ordered `Vec`s), which is exactly the fidelity the
/// mismatch check needs.
fn content_hash(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    entities: &EntityRegistry,
    worldgen: &WorldGenConfig,
) -> u64 {
    let repr = format!("{blocks:?}|{items:?}|{entities:?}|{worldgen:?}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in repr.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn builtin_blocks(tiles: &mut TileRegistry) -> BlockRegistry {
    BlockRegistry::from_toml(BUILTIN_BLOCKS, tiles).expect("embedded blocks.toml must parse")
}

fn load_blocks(tiles: &mut TileRegistry) -> BlockRegistry {
    let text = match std::fs::read_to_string(BLOCKS_PATH) {
        Ok(text) => text,
        Err(err) => {
            log::info!("could not read {BLOCKS_PATH} ({err}); using builtin blocks");
            return builtin_blocks(tiles);
        }
    };
    let registry = match BlockRegistry::from_toml(&text, tiles) {
        Ok(reg) => reg,
        Err(err) => {
            log::warn!("failed to parse {BLOCKS_PATH}: {err}; using builtin blocks");
            return builtin_blocks(tiles);
        }
    };
    log::info!("loaded {} blocks from {BLOCKS_PATH}", registry.len());
    registry
}

fn load_worldgen(blocks: &BlockRegistry) -> WorldGenConfig {
    let text = match std::fs::read_to_string(WORLDGEN_PATH) {
        Ok(text) => text,
        Err(err) => {
            log::info!("could not read {WORLDGEN_PATH} ({err}); using builtin worldgen");
            return WorldGenConfig::builtin(blocks);
        }
    };
    match WorldGenConfig::from_toml(&text, blocks) {
        Ok(config) => {
            log::info!("loaded worldgen config from {WORLDGEN_PATH}");
            config
        }
        Err(err) => {
            log::warn!("failed to parse {WORLDGEN_PATH}: {err}; using builtin worldgen");
            WorldGenConfig::builtin(blocks)
        }
    }
}

fn load_entities() -> EntityRegistry {
    let text = match std::fs::read_to_string(ENTITIES_PATH) {
        Ok(text) => text,
        Err(err) => {
            log::info!("could not read {ENTITIES_PATH} ({err}); using builtin entities");
            return EntityRegistry::builtin();
        }
    };
    match EntityRegistry::from_toml(&text) {
        Ok(reg) => {
            log::info!("loaded {} entity kinds from {ENTITIES_PATH}", reg.len());
            reg
        }
        Err(err) => {
            log::warn!("failed to parse {ENTITIES_PATH}: {err}; using builtin entities");
            EntityRegistry::builtin()
        }
    }
}

fn load_items(blocks: &BlockRegistry) -> ItemRegistry {
    let text = match std::fs::read_to_string(ITEMS_PATH) {
        Ok(text) => text,
        Err(err) => {
            log::info!("could not read {ITEMS_PATH} ({err}); using builtin items");
            return ItemRegistry::from_blocks(blocks);
        }
    };
    match ItemRegistry::from_toml(&text, blocks) {
        Ok(reg) => {
            log::info!("loaded {} items from {ITEMS_PATH}", reg.len());
            reg
        }
        Err(err) => {
            log::warn!("failed to parse {ITEMS_PATH}: {err}; using builtin items");
            ItemRegistry::from_blocks(blocks)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_content_loads() {
        let content = GameContent::builtin();
        assert!(content.blocks.len() > 0);
        assert!(content.items.len() > 0);
    }

    /// The content hash is stable across loads of identical definitions (it
    /// gates multiplayer sessions) and reacts to any definition change.
    #[test]
    fn content_hash_is_stable_and_sensitive() {
        let a = GameContent::builtin();
        let b = GameContent::builtin();
        assert_eq!(a.hash, b.hash, "identical content must hash identically");
        assert_ne!(a.hash, 0);

        let mut tiles = TileRegistry::with_engine_tiles();
        let tweaked = BUILTIN_BLOCKS.replace("hardness = 1.5", "hardness = 9.0");
        let blocks = Arc::new(BlockRegistry::from_toml(&tweaked, &mut tiles).unwrap());
        let items = Arc::new(ItemRegistry::from_blocks(&blocks));
        let entities = Arc::new(EntityRegistry::builtin());
        let worldgen = Arc::new(WorldGenConfig::builtin(&blocks));
        assert_ne!(
            content_hash(&blocks, &items, &entities, &worldgen),
            a.hash,
            "a changed definition must change the hash"
        );
    }
}
