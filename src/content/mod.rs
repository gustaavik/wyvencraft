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
//! so the game always boots. Where the text comes from is a [`ContentSource`],
//! so the same load path serves the real `assets/` directory, the
//! builtins-only build, and test fixtures.

pub mod source;

use std::sync::Arc;

use crate::core::Direction;
use crate::entity::kind::VisualSpec;
use crate::entity::{EntityRegistry, SpawnConfig};
use crate::inventory::ItemRegistry;
use crate::model::{ModelId, ModelRegistry, ModelSpec};
use crate::render::TileRegistry;
use crate::world::block::{
    BUILTIN_BLOCKS, BlockModel, BlockModelSpec, BlockRegistry, model_hitbox,
};
use crate::world::generation::WorldGenConfig;

use source::load_or_builtin;
pub use source::{ContentSource, EmbeddedSource, FsSource, MapSource};

/// How an item is drawn as a 2D icon in the inventory and hotbar. Computed once
/// at content load and indexed by `ItemId`, so the UI never touches block or
/// tile registries at draw time. Kept off [`crate::inventory::Item`] on purpose:
/// texture assignment is visual-only and must not feed [`content_hash`], which
/// gates multiplayer joins.
#[derive(Debug, Clone, Copy)]
pub enum ItemIcon {
    /// A placeable solid block, drawn as a shaded isometric cube.
    Cube { top: u32, left: u32, right: u32 },
    /// Anything else (tools, food, armor, fluids), drawn as one flat tile.
    Flat(u32),
    /// An item with a file-loaded model, drawn from its cell of the pre-rendered
    /// icon sheet (see [`crate::render::icons`]). The cell index is the
    /// [`ModelId`], so the sheet and the model registry share an ordering.
    Model(ModelId),
}

/// A loaded model plus the placement its data file asked for. Resolving the
/// path to a [`ModelId`] once at load keeps the per-frame path a plain index.
#[derive(Debug, Clone, Copy)]
pub struct ItemModel {
    pub id: ModelId,
    pub scale: f32,
    /// Model-space rotation in radians (`ModelSpec` authors it in degrees).
    pub rotation: glam::Vec3,
    pub offset: glam::Vec3,
}

/// All loaded content registries, shared across the app.
pub struct GameContent {
    /// Texture name → atlas tile assignments plus the CPU-side atlas pixels
    /// (uploaded once by the renderer at startup).
    pub tiles: TileRegistry,
    pub blocks: Arc<BlockRegistry>,
    pub items: Arc<ItemRegistry>,
    pub entities: Arc<EntityRegistry>,
    pub worldgen: Arc<WorldGenConfig>,
    /// Mob spawn rules (`assets/spawning.toml`).
    pub spawning: Arc<SpawnConfig>,
    /// Every model file referenced by an entity visual or an item, parsed once.
    pub models: Arc<ModelRegistry>,
    /// 2D icon for each item, indexed by `ItemId` (see [`ItemIcon`]).
    pub item_icons: Vec<ItemIcon>,
    /// 3D model for each item, indexed by `ItemId`: what a held or dropped
    /// stack is drawn as. Like [`ItemIcon`], kept off `Item` so it never feeds
    /// [`GameContent::hash`].
    pub item_models: Vec<Option<ItemModel>>,
    /// 3D model for each block, indexed by `BlockId`: a block with one is
    /// meshed by baking that model into its cell instead of six atlas-textured
    /// cube faces. Kept off `Block` for the same reason as [`ItemModel`].
    pub block_models: Vec<Option<BlockModel>>,
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
const SPAWNING_PATH: &str = "assets/spawning.toml";

impl GameContent {
    /// Load content from `assets/` (CWD-relative, like recipes and saves),
    /// falling back to the embedded builtin copies. Never fails.
    pub fn load() -> Arc<Self> {
        Self::from_source(&FsSource)
    }

    /// The embedded builtin content only — used by tests and as the fallback.
    pub fn builtin() -> Arc<Self> {
        Self::from_source(&EmbeddedSource)
    }

    /// Build every registry from `source`, in dependency order: blocks name the
    /// tiles and back the placeable items, entities gate the spawn rules. Each
    /// file falls back to its builtin independently, so one bad file never
    /// costs more than itself.
    pub fn from_source(source: &dyn ContentSource) -> Arc<Self> {
        let mut tiles = TileRegistry::with_engine_tiles();

        // Blocks own the tile registry: both the parsed and the builtin path
        // must register their textures into the *same* `tiles`, or the tile
        // indices baked into block faces won't match the atlas built below.
        // Block models ride out on the same `ctx` channel as the tile registry,
        // and for the same reason item models do: they are visual-only and must
        // stay out of `content_hash`.
        let mut block_ctx = BlockCtx {
            tiles: &mut tiles,
            models: Vec::new(),
        };
        let blocks = Arc::new(load_or_builtin(
            source,
            BLOCKS_PATH,
            "blocks",
            &mut block_ctx,
            |text, ctx| BlockRegistry::from_toml_with_models(text, ctx.tiles, &mut ctx.models),
            builtin_blocks,
            |reg| format!("{} blocks", reg.len()),
        ));
        let block_model_specs = block_ctx.models;
        // Item models ride out on the `ctx` channel rather than on `Item`
        // itself: they are visual-only and must stay out of `content_hash`.
        let mut item_model_specs: Vec<Option<ModelSpec>> = Vec::new();
        let items = Arc::new(load_or_builtin(
            source,
            ITEMS_PATH,
            "items",
            &mut item_model_specs,
            |text, specs| ItemRegistry::from_toml_with_models(text, &blocks, specs),
            |specs| {
                specs.clear();
                ItemRegistry::from_blocks(&blocks)
            },
            |reg| format!("{} items", reg.len()),
        ));
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
        let spawning = Arc::new(load_or_builtin(
            source,
            SPAWNING_PATH,
            "spawning",
            &mut (),
            |text, _| SpawnConfig::from_toml(text, &entities),
            |_| SpawnConfig::builtin(&entities),
            |config| format!("{} spawn rules", config.entries.len()),
        ));

        // Models load last: they are named by the entity and item definitions,
        // so the registries above have to exist first. Each path is parsed once
        // however many definitions share it.
        let mut models = ModelRegistry::new();
        for kind in entities.iter() {
            if let VisualSpec::Model(spec) = &kind.visual {
                models.load(&spec.path, source);
            }
        }
        // A block and the item that places it typically name the same file;
        // `ModelRegistry::load` memoises by path, so they share one parse, one
        // `ModelId`, one GPU texture and one 3D-icon cell.
        let block_models: Vec<Option<BlockModel>> = block_model_specs
            .iter()
            .map(|entry| {
                let entry = entry.as_ref()?;
                let id = models.load(&entry.spec.path, source)?;
                // The hitbox is measured from the placed geometry, so it can
                // never drift from what the block actually looks like. The
                // model registry is borrowed here, before it is wrapped in an
                // `Arc`, which is the only reason this can't live in `world`.
                let placed = placed_bounds(models.get(id)?, entry);
                Some(BlockModel {
                    id,
                    scale: entry.spec.scale,
                    rotation: entry.spec.rotation(),
                    offset: entry.spec.offset(),
                    random_yaw: entry.random_yaw,
                    hitbox: model_hitbox(placed),
                })
            })
            .collect();
        // One entry per item, even if the items file fell back to its builtin
        // and left the spec list empty — this vector is indexed by `ItemId`.
        item_model_specs.resize(items.len(), None);
        let item_models: Vec<Option<ItemModel>> = item_model_specs
            .iter()
            .map(|spec| {
                let spec = spec.as_ref()?;
                Some(ItemModel {
                    id: models.load(&spec.path, source)?,
                    scale: spec.scale,
                    rotation: spec.rotation(),
                    offset: spec.offset(),
                })
            })
            .collect();

        let item_icons = build_item_icons(&mut tiles, &blocks, &items, &item_models);
        let hash = content_hash(&blocks, &items, &entities, &worldgen, &spawning);
        Arc::new(Self {
            tiles,
            blocks,
            items,
            entities,
            worldgen,
            spawning,
            models: Arc::new(models),
            item_icons,
            item_models,
            block_models,
            hash,
        })
    }
}

/// Resolve an icon for every item. A placeable solid block becomes an isometric
/// cube from its own face tiles (so new blocks get an icon for free); everything
/// else — tools, food, armor, and fluids — resolves its name to one flat tile.
fn build_item_icons(
    tiles: &mut TileRegistry,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    item_models: &[Option<ItemModel>],
) -> Vec<ItemIcon> {
    items
        .iter()
        .map(|(id, item)| {
            // An item with a model is drawn as that model, rendered once into
            // the icon sheet — it has geometry and a texture of its own, and no
            // atlas tile could represent it.
            if let Some(model) = item_models.get(id.0 as usize).copied().flatten() {
                return ItemIcon::Model(model.id);
            }
            // A cube reads wrong for fluids, so only truly solid blocks get one.
            if let Some(block_id) = item.place_block {
                let block = blocks.get(block_id);
                if block.is_visible() && block.fluid.is_none() {
                    return ItemIcon::Cube {
                        top: block.textures.tile(Direction::PosY),
                        left: block.textures.tile(Direction::NegZ),
                        right: block.textures.tile(Direction::PosX),
                    };
                }
            }
            ItemIcon::Flat(tiles.resolve(&item.name).tile)
        })
        .collect()
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
    spawning: &SpawnConfig,
) -> u64 {
    let repr = format!("{blocks:?}|{items:?}|{entities:?}|{worldgen:?}|{spawning:?}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in repr.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// A model's bounds after its `[block.model]` placement, in block-local `0..1`
/// coordinates — the same transform `world::meshing` bakes with, minus the yaw
/// and the translation to the cell (which `model_hitbox` handles by staying
/// square and centred).
fn placed_bounds(model: &crate::model::Model, spec: &BlockModelSpec) -> (glam::Vec3, glam::Vec3) {
    let transform = crate::model::mesh::placement(
        // The cell's horizontal centre but its *floor* — exactly the origin
        // `world::meshing::culled` bakes at, or the box would sit half a block
        // above the geometry.
        glam::Vec3::new(0.5, 0.0, 0.5),
        0.0,
        0.0,
        spec.spec.scale,
        spec.spec.rotation(),
        spec.spec.offset(),
    );
    let (lo, hi) = model.bounds;
    // Transform all eight corners: a rotation can turn the box, so the extremes
    // are not simply the transformed `lo`/`hi`.
    let corners = [
        glam::Vec3::new(lo.x, lo.y, lo.z),
        glam::Vec3::new(hi.x, lo.y, lo.z),
        glam::Vec3::new(lo.x, hi.y, lo.z),
        glam::Vec3::new(hi.x, hi.y, lo.z),
        glam::Vec3::new(lo.x, lo.y, hi.z),
        glam::Vec3::new(hi.x, lo.y, hi.z),
        glam::Vec3::new(lo.x, hi.y, hi.z),
        glam::Vec3::new(hi.x, hi.y, hi.z),
    ];
    corners
        .iter()
        .map(|&c| transform.transform_point3(c))
        .fold(None, |acc: Option<(glam::Vec3, glam::Vec3)>, p| match acc {
            Some((lo, hi)) => Some((lo.min(p), hi.max(p))),
            None => Some((p, p)),
        })
        .expect("eight corners")
}

/// What the block loader mutates on both the parse and the fallback path: the
/// shared tile registry, plus the `[block.model]` specs it reports back.
struct BlockCtx<'a> {
    tiles: &'a mut TileRegistry,
    models: Vec<Option<BlockModelSpec>>,
}

fn builtin_blocks(ctx: &mut BlockCtx<'_>) -> BlockRegistry {
    BlockRegistry::from_toml_with_models(BUILTIN_BLOCKS, ctx.tiles, &mut ctx.models)
        .expect("embedded blocks.toml must parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_content_loads() {
        let content = GameContent::builtin();
        assert!(!content.blocks.is_empty());
        assert!(!content.items.is_empty());
    }

    /// Models come off the real `assets/` tree: every path the shipped
    /// definitions name must resolve to a real, non-empty model. Which export
    /// they name is deliberately not asserted — the two formats describe the
    /// same object and are meant to be swappable in the data files.
    #[test]
    fn models_named_by_definitions_are_loaded_and_resolved() {
        let content = GameContent::load();
        assert!(
            !content.models.is_empty(),
            "the shipped content names models"
        );
        for kind in content.entities.iter() {
            if let VisualSpec::Model(spec) = &kind.visual {
                let id = content
                    .models
                    .find(&spec.path)
                    .unwrap_or_else(|| panic!("{} names an unloadable model", kind.name));
                assert!(
                    content
                        .models
                        .get(id)
                        .is_some_and(|m| m.triangle_count() > 0)
                );
            }
        }

        let sword = content.items.find("vine sword").expect("vine sword item");
        let model = content.item_models[sword.0 as usize].expect("vine sword has a model");
        assert_eq!(model.scale, 0.35);
        let loaded = content
            .models
            .get(model.id)
            .expect("model is in the registry");
        assert_eq!(loaded.triangle_count(), 252);

        // `item_models` is indexed by `ItemId`, so it must cover every item even
        // though almost none of them declare a model.
        assert_eq!(content.item_models.len(), content.items.len());
    }

    /// Every `[block.model]` in the shipped data must resolve to real geometry,
    /// and — because a block and the item that places it name the same file —
    /// both must land on the *same* `ModelId`, so the pair costs one parse, one
    /// GPU texture and one icon cell rather than two.
    #[test]
    fn block_models_load_and_share_their_items_model_id() {
        let content = GameContent::load();

        // Indexed by `BlockId`, so it must cover every block.
        assert_eq!(content.block_models.len(), content.blocks.len());

        let mut declared = Vec::new();
        for (id, block) in content.blocks.iter() {
            let Some(model) = content.block_models[id.0 as usize] else {
                continue;
            };
            let loaded = content
                .models
                .get(model.id)
                .unwrap_or_else(|| panic!("{}: model id is not in the registry", block.name));
            assert!(
                loaded.triangle_count() > 0,
                "{}: model has no geometry",
                block.name
            );
            assert!(model.scale > 0.0, "{}: non-positive scale", block.name);

            let item = content
                .items
                .find(&block.name)
                .unwrap_or_else(|| panic!("{}: no placeable item", block.name));
            let item_model = content.item_models[item.0 as usize]
                .unwrap_or_else(|| panic!("{}: item declares no model", block.name));
            assert_eq!(
                item_model.id, model.id,
                "{}: block and item parsed the file twice",
                block.name
            );
            declared.push(block.name.clone());
        }

        assert_eq!(
            declared,
            ["blue bells", "red flower", "red mushroom", "brown mushroom"],
            "shipped model-backed blocks"
        );
    }

    /// Ground cover must not be targetable across the whole cell it stands in.
    /// The boxes are derived from the models, so this also catches a model
    /// re-export that quietly changed size.
    #[test]
    fn ground_cover_hitboxes_are_smaller_than_their_cells() {
        let content = GameContent::load();
        // (name, width, height) as measured off the shipped models. A golden:
        // a re-export that changes a plant's size should show up here rather
        // than as a crosshair that quietly stops feeling right.
        let expected = [
            ("blue bells", 0.83, 0.99),
            ("red flower", 0.49, 0.63),
            ("red mushroom", 0.41, 0.56),
            ("brown mushroom", 0.83, 0.63),
        ];
        for (name, width, height) in expected {
            let id = content.blocks.find(name).expect("shipped block");
            let hitbox = content.block_models[id.0 as usize]
                .expect("has a model")
                .hitbox;
            let size = hitbox.max - hitbox.min;
            assert!(
                (size.x - width).abs() < 0.02 && (size.y - height).abs() < 0.02,
                "{name}: {size:?} is not {width} x {height}"
            );
            // Square in plan, so `random_yaw` cannot leave it lopsided...
            assert!((size.x - size.z).abs() < 1e-5, "{name}: not square in plan");
            // ...centred, standing on the cell floor, and inside the cell...
            assert!(
                (size.x * 0.5 + hitbox.min.x - 0.5).abs() < 1e-5,
                "{name}: off-centre"
            );
            assert_eq!(hitbox.min.y, 0.0, "{name}: floating");
            assert!(
                hitbox.min.x >= 0.0 && hitbox.max.x <= 1.0,
                "{name}: escapes its cell"
            );
            // ...smaller than the cell it stands in, which is the whole point...
            assert!(size.x < 1.0 && size.y < 1.0, "{name}: fills its cell");
            // ...and big enough to actually click on.
            assert!(
                size.x >= 0.1 && size.y >= 0.1,
                "{name}: {size:?} too small to hit"
            );
        }
    }

    /// A block with neither `textures` nor `[block.model]` would render as the
    /// magenta marker on all six faces. Rejecting the file is louder, and the
    /// caller still falls back to the builtin blocks.
    #[test]
    fn a_block_without_textures_or_a_model_is_rejected() {
        let mut tiles = TileRegistry::with_engine_tiles();
        let bad = r#"
            [[block]]
            name = "ghost"
            render = "opaque"
            solid = true
            hardness = 1.0
            material = "stone"
        "#;
        let err = BlockRegistry::from_toml(bad, &mut tiles).expect_err("must not parse");
        assert!(err.contains("ghost"), "{err}");
    }

    /// Every `[item.model]` in the shipped data must resolve to real geometry.
    ///
    /// This reads the actual `assets/` tree, so it is the check that catches a
    /// mistyped path, a Blockbench export the loader cannot read, or a model
    /// saved without its texture — all of which otherwise degrade quietly to a
    /// magenta icon at runtime.
    #[test]
    fn every_item_model_in_the_shipped_data_loads() {
        let content = GameContent::load();
        let mut declared = Vec::new();

        for (id, item) in content.items.iter() {
            let index = id.0 as usize;
            let Some(model) = content.item_models[index] else {
                continue;
            };
            let loaded = content
                .models
                .get(model.id)
                .unwrap_or_else(|| panic!("{}: model id is not in the registry", item.name));
            assert!(
                loaded.triangle_count() > 0,
                "{}: model has no geometry",
                item.name
            );
            assert!(model.scale > 0.0, "{}: non-positive scale", item.name);
            // An item with a model always icons as that model.
            assert!(
                matches!(content.item_icons[index], ItemIcon::Model(drawn) if drawn == model.id),
                "{}: does not icon as its model",
                item.name
            );
            declared.push(item.name.clone());
        }

        // The twelve tiered tools, the vine sword, and the four ground-cover
        // blocks whose items are drawn as their own model.
        assert_eq!(declared.len(), 17, "declared item models: {declared:?}");
    }

    /// The shipped items file with every model path pointed somewhere else.
    /// Extension-agnostic on purpose: the data files are meant to be able to
    /// name either export, and a fixture keyed to one of them would quietly
    /// stop substituting anything the day the other is chosen.
    fn items_with_repointed_models() -> String {
        let base = crate::inventory::item::BUILTIN_ITEMS;
        let repointed = base.replace("assets/models/vine_sword", "assets/models/elsewhere");
        assert_ne!(base, repointed, "fixture substituted nothing");
        repointed
    }

    /// A model is visual-only: swapping one must not change the fingerprint that
    /// gates multiplayer joins, or two players with differently-drawn swords
    /// would be refused a shared world.
    #[test]
    fn item_models_do_not_feed_the_content_hash() {
        // Both sides come from the same source kind, so the model path is the
        // only thing that differs between them.
        let base = GameContent::from_source(
            &MapSource::new().with(ITEMS_PATH, crate::inventory::item::BUILTIN_ITEMS),
        );
        let repointed = GameContent::from_source(
            &MapSource::new().with(ITEMS_PATH, items_with_repointed_models()),
        );
        assert_eq!(
            base.items.len(),
            repointed.items.len(),
            "the items themselves are unchanged"
        );
        assert_eq!(base.hash, repointed.hash);
    }

    /// A model path that does not resolve degrades that one item's appearance
    /// and nothing else — the world still boots.
    #[test]
    fn an_unloadable_model_leaves_the_rest_of_the_content_intact() {
        let content = GameContent::from_source(
            &MapSource::new().with(ITEMS_PATH, items_with_repointed_models()),
        );
        let sword = content.items.find("vine sword").expect("item still exists");
        assert!(content.item_models[sword.0 as usize].is_none());
        assert!(!content.items.is_empty());
        assert!(content.models.is_empty(), "nothing resolved");
    }

    /// An empty source serves no files, so every registry must fall back to its
    /// builtin — i.e. `from_source` over nothing is exactly `builtin()`. This is
    /// what lets the two constructors share one code path.
    #[test]
    fn an_empty_source_is_the_builtin_content() {
        let empty = GameContent::from_source(&MapSource::new());
        let builtin = GameContent::builtin();
        assert_eq!(empty.hash, builtin.hash);
        assert_eq!(empty.blocks.len(), builtin.blocks.len());
        assert_eq!(empty.items.len(), builtin.items.len());
    }

    /// Definitions really do come from the source: a fixture adding a block
    /// yields a registry containing it, with its item and icon derived.
    ///
    /// The fixture extends the builtin blocks rather than replacing them,
    /// because `worldgen.toml` names concrete blocks ("wood", "stone", ...) and
    /// resolving it against a registry missing them is a hard error by design.
    #[test]
    fn definitions_are_read_from_the_source() {
        let blocks = format!(
            "{BUILTIN_BLOCKS}\n\
             [[block]]\n\
             name = \"testonium\"\n\
             render = \"opaque\"\n\
             solid = true\n\
             hardness = 1.0\n\
             material = \"stone\"\n\
             textures = \"stone\"\n"
        );
        let content = GameContent::from_source(&MapSource::new().with(BLOCKS_PATH, blocks));

        let builtin = GameContent::builtin();
        assert_eq!(
            content.blocks.len(),
            builtin.blocks.len() + 1,
            "the fixture's block is registered on top of the builtins"
        );
        assert!(content.blocks.find("testonium").is_some());
        // The auto-generated placeable item comes with it, and every item
        // resolved an icon — proving the fixture's textures reached the same
        // TileRegistry the atlas is built from.
        assert!(content.items.find("testonium").is_some());
        assert_eq!(content.item_icons.len(), content.items.len());
        assert_ne!(content.hash, builtin.hash, "new block changes the hash");
    }

    /// Fail-soft is per file: a malformed blocks.toml costs only the blocks,
    /// and the other four registries still load from the source.
    #[test]
    fn a_malformed_file_falls_back_alone() {
        let source = MapSource::new()
            .with(BLOCKS_PATH, "this is not valid toml {{{")
            .with(
                ENTITIES_PATH,
                crate::entity::kind::BUILTIN_ENTITIES
                    .replace("max_health = 20.0", "max_health = 17.0"),
            );
        let content = GameContent::from_source(&source);

        let builtin = GameContent::builtin();
        assert_eq!(
            content.blocks.len(),
            builtin.blocks.len(),
            "bad blocks.toml falls back to the builtin blocks"
        );
        // ...while the entities file, which parsed fine, was still honoured.
        assert_ne!(
            content.hash, builtin.hash,
            "the tweaked entities file must still take effect"
        );
    }

    /// Worldgen is strict by design (an unknown block name would silently
    /// generate the wrong terrain), so a bad name rejects the whole file.
    #[test]
    fn unknown_worldgen_block_rejects_the_file() {
        let bad = crate::world::generation::config::BUILTIN_WORLDGEN
            .replace("bedrock = \"bedrock\"", "bedrock = \"no such block\"");
        let content = GameContent::from_source(&MapSource::new().with(WORLDGEN_PATH, bad));
        let builtin = GameContent::builtin();
        assert_eq!(
            content.hash, builtin.hash,
            "a rejected worldgen file leaves the builtin content in place"
        );
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
        let spawning = Arc::new(SpawnConfig::builtin(&entities));
        assert_ne!(
            content_hash(&blocks, &items, &entities, &worldgen, &spawning),
            a.hash,
            "a changed definition must change the hash"
        );

        // Spawn rules gate multiplayer too: divergent rules = divergent hash.
        use crate::entity::spawning::BUILTIN_SPAWNING;
        let tweaked = BUILTIN_SPAWNING.replace("max_mobs = 40", "max_mobs = 99");
        let spawning = Arc::new(SpawnConfig::from_toml(&tweaked, &entities).unwrap());
        let blocks = Arc::new(builtin_blocks(&mut BlockCtx {
            tiles: &mut TileRegistry::with_engine_tiles(),
            models: Vec::new(),
        }));
        let items = Arc::new(ItemRegistry::from_blocks(&blocks));
        let worldgen = Arc::new(WorldGenConfig::builtin(&blocks));
        assert_ne!(
            content_hash(&blocks, &items, &entities, &worldgen, &spawning),
            a.hash,
            "changed spawn rules must change the hash"
        );
    }
}
