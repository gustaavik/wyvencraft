//! Loading the rules: every `assets/*.toml` that feeds the content hash.
//!
//! Reads through the [`AssetSource`] port, so the same path serves the real
//! `assets/` directory, the builtins-only build, and test fixtures. Loading is
//! fail-soft: a missing or invalid file logs a warning and falls back to its
//! embedded builtin, so the game always boots — and each file falls back on its
//! own, so one bad file never costs more than itself.
//!
//! The block and item files also describe how things *look* — texture names,
//! models, display names. Those are reported out of band in [`RuleVisuals`]
//! rather than stored on `Block`/`Item`, because `Block` and `Item` feed the
//! content hash and a purely visual difference must never refuse a join.
//! Turning them into textures is presentation's job.

use std::sync::Arc;

use wyven_assets::{AssetSource, load_or_builtin};

use crate::domain::content::{self, Registries};
use crate::domain::entity::{EntityRegistry, SpawnConfig};
use crate::domain::inventory::ItemRegistry;
use crate::domain::inventory::item::ItemVisuals;
use crate::domain::world::BlockRegistry;
use crate::domain::world::block::{BUILTIN_BLOCKS, BlockVisuals};
use crate::domain::world::generation::WorldGenConfig;
use crate::domain::world::structure::StructureConfig;

pub const BLOCKS_PATH: &str = "assets/blocks.toml";
pub const ITEMS_PATH: &str = "assets/items.toml";
pub const ENTITIES_PATH: &str = "assets/entities.toml";
pub const WORLDGEN_PATH: &str = "assets/worldgen.toml";
pub const STRUCTURES_PATH: &str = "assets/structures.toml";
pub const SPAWNING_PATH: &str = "assets/spawning.toml";

/// What the block and item files said about appearance, handed on unparsed
/// for presentation to resolve.
#[derive(Default)]
pub struct RuleVisuals {
    pub blocks: BlockVisuals,
    pub items: ItemVisuals,
}

/// Build every registry from `source`, in dependency order: blocks back the
/// placeable items, entities gate the spawn rules, blocks and worldgen place
/// the structures.
pub fn load_registries(source: &dyn AssetSource) -> (Registries, RuleVisuals) {
    let mut block_visuals = BlockVisuals::default();
    let blocks = Arc::new(load_or_builtin(
        source,
        BLOCKS_PATH,
        "blocks",
        &mut block_visuals,
        BlockRegistry::from_toml_with_models,
        |visuals| {
            BlockRegistry::from_toml_with_models(BUILTIN_BLOCKS, visuals)
                .expect("embedded blocks.toml must parse")
        },
        |reg| format!("{} blocks", reg.len()),
    ));
    let mut item_visuals = ItemVisuals::default();
    let items = Arc::new(load_or_builtin(
        source,
        ITEMS_PATH,
        "items",
        &mut item_visuals,
        |text, visuals| ItemRegistry::from_toml_with_visuals(text, &blocks, visuals),
        |visuals| {
            *visuals = ItemVisuals::default();
            ItemRegistry::from_blocks(&blocks)
        },
        |reg| format!("{} items", reg.len()),
    ));
    // A block asking for a tool kind nothing is would be slow for everyone,
    // and — if it insists on one — undroppable forever. `world` sits below
    // `inventory` and cannot see the tools; here is the first place that
    // sees both.
    for (block, kind) in content::unknown_harvest_tools(&blocks, &items) {
        log::warn!("block {block:?}: wants tool kind {kind:?}, which no item declares");
    }
    let entities = Arc::new(load_or_builtin(
        source,
        ENTITIES_PATH,
        "entities",
        &mut (),
        |text, _| EntityRegistry::from_toml(text),
        |_| EntityRegistry::builtin(),
        |reg| format!("{} entity kinds", reg.len()),
    ));
    let worldgen = Arc::new(load_or_builtin(
        source,
        WORLDGEN_PATH,
        "worldgen",
        &mut (),
        |text, _| WorldGenConfig::from_toml(text, &blocks),
        |_| WorldGenConfig::builtin(&blocks),
        |_| "worldgen config".to_string(),
    ));
    let structures = Arc::new(load_or_builtin(
        source,
        STRUCTURES_PATH,
        "structures",
        &mut (),
        |text, _| StructureConfig::from_toml(text, &blocks, &worldgen),
        |_| StructureConfig::builtin(&blocks, &worldgen),
        |config| format!("{} structures", config.all().len()),
    ));
    let spawning = Arc::new(load_or_builtin(
        source,
        SPAWNING_PATH,
        "spawning",
        &mut (),
        |text, _| SpawnConfig::from_toml(text, &entities, &worldgen),
        |_| SpawnConfig::builtin(&entities, &worldgen),
        |config| format!("{} spawn rules", config.entries.len()),
    ));
    // The loop's references cross files that load in an order where their
    // targets cannot yet be seen; this is the first point that sees all.
    for problem in content::dangling_references(&blocks, &items, &entities, &structures) {
        log::warn!("{problem}");
    }
    for (block, tier) in content::unreachable_tiers(&blocks, &items) {
        log::warn!("block {block:?}: needs a tier {tier} tool, and no tool reaches it");
    }

    let registries = Registries::new(blocks, items, entities, worldgen, structures, spawning);
    let visuals = RuleVisuals {
        blocks: block_visuals,
        items: item_visuals,
    };
    (registries, visuals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyven_assets::{EmbeddedSource, MapSource};

    /// A source with no files is the builtins, exactly — the promise
    /// [`Registries::builtin`] makes without doing any loading.
    #[test]
    fn loading_nothing_is_the_builtin_registries() {
        let (loaded, _) = load_registries(&EmbeddedSource);
        assert_eq!(loaded.hash(), Registries::builtin().hash());
    }

    /// Worldgen is strict — any bad name rejects the whole file — and a
    /// rejected file must leave the builtin in place rather than a half-parse.
    #[test]
    fn a_rejected_worldgen_file_falls_back_to_the_builtin() {
        let source = MapSource::new().with(WORLDGEN_PATH, "[[biome]]\nid = 7\n");
        let (loaded, _) = load_registries(&source);
        assert_eq!(loaded.hash(), Registries::builtin().hash());
    }

    /// The block file's appearance fields are reported, not dropped: one entry
    /// per block, so presentation can index them by `BlockId`.
    #[test]
    fn block_visuals_are_reported_per_block() {
        let (loaded, visuals) = load_registries(&EmbeddedSource);
        assert_eq!(visuals.blocks.display_names.len(), loaded.blocks.len());
    }
}
