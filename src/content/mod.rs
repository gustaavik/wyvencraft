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

use std::sync::Arc;

use crate::core::Direction;
use crate::entity::kind::VisualSpec;
use crate::entity::{EntityRegistry, SpawnConfig};
use crate::inventory::ItemRegistry;
use crate::model::{ModelId, ModelRegistry, ModelSpec, blockjson};
use crate::render::TileRegistry;
use crate::render::block_textures::{self, AnimatedLayers, BlockTextureSet, Strip};
use crate::world::block::{
    BUILTIN_BLOCKS, BlockJsonSpec, BlockModel, BlockModelSpec, BlockRegistry, BlockVisuals,
    FaceTextures, FluidVisual, model_hitbox,
};
use crate::world::blockmodel::BakedBlockModel;
use crate::world::generation::WorldGenConfig;
use crate::world::meshing::BlockModels;
use wyven_assets::decode_png;

// The byte-source port lives in `wyven_assets` — the model loaders need it
// too, and they sit below game content. Re-exported under the old name so
// every caller and fixture keeps reading.
use wyven_assets::load_or_builtin;
pub use wyven_assets::{AssetSource as ContentSource, EmbeddedSource, FsSource, MapSource};

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
    /// The 256×256 textures Blockbench-authored blocks sample, one array layer
    /// each. Uploaded once by the renderer alongside the atlas.
    pub block_textures: BlockTextureSet,
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
    /// Blockbench-authored geometry for each block, indexed by `BlockId`. The
    /// direction every block is moving in; also kept off `Block`.
    pub baked_models: Vec<Option<BakedBlockModel>>,
    /// Atlas tiles derived from a Blockbench block's own textures, indexed by
    /// `BlockId` — see [`GameContent::face_textures`], which is how everything
    /// outside the chunk mesh should read them.
    pub block_face_tiles: Vec<Option<FaceTextures>>,
    /// The animation strip each fluid block draws from, indexed by `BlockId`
    /// and covering the auto-registered flowing blocks too. Kept off `Block`
    /// for the same reason as [`BlockModel`].
    pub fluid_textures: Vec<Option<FluidTexture>>,
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
        let mut tiles = crate::art::tile_registry();

        // Blocks own the tile registry: both the parsed and the builtin path
        // must register their textures into the *same* `tiles`, or the tile
        // indices baked into block faces won't match the atlas built below.
        // Block models ride out on the same `ctx` channel as the tile registry,
        // and for the same reason item models do: they are visual-only and must
        // stay out of `content_hash`.
        let mut block_ctx = BlockCtx {
            tiles: &mut tiles,
            visuals: BlockVisuals::default(),
        };
        let blocks = Arc::new(load_or_builtin(
            source,
            BLOCKS_PATH,
            "blocks",
            &mut block_ctx,
            |text, ctx| BlockRegistry::from_toml_with_models(text, ctx.tiles, &mut ctx.visuals),
            builtin_blocks,
            |reg| format!("{} blocks", reg.len()),
        ));
        let BlockVisuals {
            models: block_model_specs,
            json: block_json_paths,
            fluids: fluid_visuals,
        } = block_ctx.visuals;
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

        // Blocks authored in Blockbench. Each `.json` is parsed, its textures
        // take layers of the shared array, and the geometry is baked into quads
        // the chunk mesher can place with a translation. Fail-soft like every
        // other content load: a bad model costs its own block's appearance (it
        // falls back to whatever `textures` it declared) and nothing else.
        let mut block_textures = BlockTextureSet::new();
        let mut baked_models: Vec<Option<BakedBlockModel>> = Vec::new();
        let mut block_face_tiles: Vec<Option<FaceTextures>> = Vec::new();
        for spec in &block_json_paths {
            let baked = spec
                .as_ref()
                .and_then(|spec| load_block_model(spec, source, &mut block_textures));
            block_face_tiles.push(
                baked
                    .as_ref()
                    .map(|m| derive_face_tiles(m, &block_textures, &mut tiles)),
            );
            baked_models.push(baked);
        }

        // Fluids draw from an animation strip rather than a model: the frames
        // take a run of array layers each, and the mesher steps through them.
        let mut fluid_textures: Vec<Option<FluidTexture>> = Vec::new();
        for (id, visual) in fluid_visuals.iter().enumerate() {
            let fluid = visual
                .as_ref()
                .and_then(|visual| load_fluid_texture(visual, source, &mut block_textures));
            // The inventory icon and the dropped-item cube still sample the
            // atlas, so a fluid needs the same 16-pixel stand-in a Blockbench
            // block gets — its first still frame, which is the frame the block
            // spends most of its time looking like.
            if let Some(tex) = &fluid
                && block_face_tiles[id].is_none()
                && let Some(image) = block_textures.layer(tex.still.first)
            {
                let tile = tiles
                    .insert(
                        &format!("blockmodel:{}", tex.still.first),
                        block_textures::to_atlas_tile(image),
                    )
                    .tile;
                block_face_tiles[id] = Some(FaceTextures::uniform(tile));
            }
            fluid_textures.push(fluid);
        }

        let item_icons =
            build_item_icons(&mut tiles, &blocks, &items, &item_models, &block_face_tiles);
        let hash = content_hash(&blocks, &items, &entities, &worldgen, &spawning);
        Arc::new(Self {
            tiles,
            block_textures,
            blocks,
            items,
            entities,
            worldgen,
            spawning,
            models: Arc::new(models),
            item_icons,
            item_models,
            block_models,
            baked_models,
            block_face_tiles,
            fluid_textures,
            hash,
        })
    }

    /// The model geometry the chunk mesher needs, borrowed from this content.
    pub fn block_models(&self) -> BlockModels<'_> {
        BlockModels {
            models: &self.models,
            blocks: &self.block_models,
            baked: &self.baked_models,
            fluids: &self.fluid_textures,
        }
    }

    /// The six atlas tiles standing in for `block` outside the chunk mesh — its
    /// inventory icon, and the little cube a dropped stack is drawn as.
    ///
    /// A Blockbench-authored block has these derived from its own model
    /// textures; every other block uses the tiles `blocks.toml` named for it.
    /// Kept here rather than written back onto `Block` because `Block` feeds
    /// [`content_hash`], and a derived tile index would then refuse a join over
    /// a difference that is purely visual.
    pub fn face_textures(&self, block: crate::core::BlockId) -> FaceTextures {
        self.block_face_tiles
            .get(block.0 as usize)
            .copied()
            .flatten()
            .unwrap_or(self.blocks.get(block).textures)
    }
}

/// Where a fluid block's animation lives in the block texture array.
///
/// Both columns of the strip are resolved even for a source block: the
/// auto-registered flowing blocks share this entry, and which column a face
/// takes is a per-face decision the mesher makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidTexture {
    /// Column 1 — top and bottom faces, and every face of a source block.
    pub still: AnimatedLayers,
    /// Column 0 — the side faces of a flowing block.
    pub flowing: AnimatedLayers,
    pub fps: u8,
    /// `tintindex` into the biome colours; `2` is water.
    pub tint: Option<u8>,
}

/// Column of the strip each of the two states reads. Documented in
/// `assets/blocks.toml`; a one-column strip collapses both onto column 0.
const FLOWING_COLUMN: u32 = 0;
const STILL_COLUMN: u32 = 1;

/// Load a fluid's animation strip into the block texture array, or warn and
/// give up on it — the block then falls back to whatever `textures` it
/// declared, which for water is the magenta marker.
fn load_fluid_texture(
    visual: &FluidVisual,
    source: &dyn ContentSource,
    textures: &mut BlockTextureSet,
) -> Option<FluidTexture> {
    let spec = &visual.spec;
    let path = spec.path.as_str();
    let bytes = match source.read_bytes(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!("could not read fluid texture {path}: {err}");
            return None;
        }
    };
    let image = match decode_png(&bytes) {
        Ok(image) => image,
        Err(err) => {
            log::warn!("could not decode fluid texture {path}: {err}");
            return None;
        }
    };
    let frames = u32::from(spec.frames);
    // A one-column strip has no separate still art, so both states read it;
    // `resolve_strip` rejects a column past the end rather than guessing.
    let two_columns = image
        .height()
        .checked_div(frames)
        .is_some_and(|size| size > 0 && image.width() > size);
    let still_column = if two_columns {
        STILL_COLUMN
    } else {
        FLOWING_COLUMN
    };
    // The flowing blocks of a fluid share their source's strip, so this runs
    // once per block but allocates layers only the first time.
    let before = textures.len();
    let strip = |column| Strip {
        column,
        frames,
        alpha: spec.opacity,
    };
    let flowing = textures.resolve_strip(path, &image, strip(FLOWING_COLUMN));
    let still = textures.resolve_strip(path, &image, strip(still_column));
    if still == AnimatedLayers::MISSING || flowing == AnimatedLayers::MISSING {
        return None;
    }
    if textures.len() > before {
        log::info!(
            "loaded fluid texture {path} ({} frames per column at {} fps)",
            spec.frames,
            spec.fps
        );
    }
    Some(FluidTexture {
        still,
        flowing,
        fps: spec.fps,
        tint: spec.tint,
    })
}

/// Parse one Blockbench block model and bake it, or warn and give up on it.
fn load_block_model(
    spec: &BlockJsonSpec,
    source: &dyn ContentSource,
    textures: &mut BlockTextureSet,
) -> Option<BakedBlockModel> {
    let path = spec.path.as_str();
    let bytes = match source.read_bytes(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::warn!("could not read block model {path}: {err}");
            return None;
        }
    };
    let dir = path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");
    match blockjson::load(&bytes, dir, source) {
        Ok(model) => {
            let baked = BakedBlockModel::bake(&model, textures, spec.random_yaw);
            log::info!(
                "loaded block model {path} ({} quads, {} textures)",
                baked.quads.len(),
                model.textures.len()
            );
            Some(baked)
        }
        Err(err) => {
            log::warn!("could not load block model {path}: {err}");
            None
        }
    }
}

/// Reduce a baked model's face textures to one atlas tile each.
///
/// Keyed by `"blockmodel:<layer>"` so two blocks covering a face with the same
/// texture share a tile rather than each burning one of the atlas's few free
/// slots. A face nothing covers keeps tile 0 — a model with no bottom has no
/// honest bottom to show on an icon.
fn derive_face_tiles(
    model: &BakedBlockModel,
    textures: &BlockTextureSet,
    tiles: &mut TileRegistry,
) -> FaceTextures {
    let mut faces = [0u32; 6];
    for (face, layer) in faces.iter_mut().zip(model.face_layers()) {
        let Some(layer) = layer else { continue };
        let Some(image) = textures.layer(layer) else {
            continue;
        };
        *face = tiles
            .insert(
                &format!("blockmodel:{layer}"),
                block_textures::to_atlas_tile(image),
            )
            .tile;
    }
    FaceTextures(faces)
}

/// Resolve an icon for every item. A placeable solid block becomes an isometric
/// cube from its own face tiles (so new blocks get an icon for free); everything
/// else — tools, food, armor, and fluids — resolves its name to one flat tile.
fn build_item_icons(
    tiles: &mut TileRegistry,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    item_models: &[Option<ItemModel>],
    block_face_tiles: &[Option<FaceTextures>],
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
                    // A Blockbench-authored block's tiles are derived from its
                    // own model textures; everything else uses what
                    // `blocks.toml` named. Same shape either way, so the icon
                    // keeps the familiar isometric cube rather than needing a
                    // second orientation in the 3D icon sheet.
                    let faces = block_face_tiles
                        .get(block_id.0 as usize)
                        .copied()
                        .flatten()
                        .unwrap_or(block.textures);
                    return ItemIcon::Cube {
                        top: faces.tile(Direction::PosY),
                        left: faces.tile(Direction::NegZ),
                        right: faces.tile(Direction::PosX),
                    };
                }
            }
            // A fluid has no cube icon but does have derived tiles; preferring
            // them keeps it off the name lookup, which has no art to find.
            let derived = item.place_block.and_then(|block_id| {
                block_face_tiles
                    .get(block_id.0 as usize)
                    .copied()
                    .flatten()
                    .map(|faces| faces.tile(Direction::PosY))
            });
            match derived {
                Some(tile) => ItemIcon::Flat(tile),
                None => ItemIcon::Flat(tiles.resolve(&item.name).tile),
            }
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
    visuals: BlockVisuals,
}

fn builtin_blocks(ctx: &mut BlockCtx<'_>) -> BlockRegistry {
    BlockRegistry::from_toml_with_models(BUILTIN_BLOCKS, ctx.tiles, &mut ctx.visuals)
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
        let mut tiles = crate::art::tile_registry();
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
    /// The shipped water strip must actually load: a mistyped path or a
    /// mis-shaped PNG degrades water to the magenta marker, since it no longer
    /// declares any atlas `textures` to fall back to.
    #[test]
    fn the_shipped_water_animation_loads_for_every_fluid_block() {
        let content = GameContent::load();
        let water = content.blocks.find("water").expect("shipped block");
        let tex = content.fluid_textures[water.0 as usize].expect("water is animated");
        assert_eq!(tex.still.frames, 64);
        assert_eq!(tex.flowing.frames, 64);
        assert_ne!(
            tex.still.first, tex.flowing.first,
            "the two columns are separate runs of layers"
        );
        assert_eq!(tex.tint, Some(2), "water takes the biome water colour");

        // The strip's own alpha is a placeholder; `opacity` is what decides how
        // much of the riverbed shows through, since a body of water is one
        // blended sheet however deep it is.
        let frame = content
            .block_textures
            .layer(tex.still.first)
            .expect("still frame");
        let alpha = frame.pixels[3];
        assert!(
            (200..255).contains(&alpha),
            "water should read as water, not as glass: alpha {alpha}"
        );

        // The inventory icon and the dropped-item cube still sample the atlas,
        // so a fluid needs a real stand-in tile there rather than the marker.
        let faces = content.face_textures(water);
        assert_ne!(
            faces.tile(crate::core::Direction::PosY),
            0,
            "water must not fall back to the missing-texture tile"
        );

        // The auto-registered flowing blocks share the source's entry, or a
        // spreading stream would fall back to the marker halfway down a hill.
        for level in 1..=7 {
            let id = content.blocks.flowing(0, level);
            assert_eq!(
                content.fluid_textures[id.0 as usize],
                Some(tex),
                "water flow {level}"
            );
        }
    }

    /// A fluid whose strip cannot be read must not fail the load — it degrades
    /// to the block's own `textures`, like every other content failure.
    #[test]
    fn an_unreadable_fluid_strip_degrades_to_no_animation() {
        let blocks = BUILTIN_BLOCKS.replace(
            "assets/textures/water_flow.png",
            "assets/textures/no_such_fluid.png",
        );
        let content = GameContent::from_source(&MapSource::new().with(BLOCKS_PATH, &blocks));
        let water = content.blocks.find("water").expect("declared");
        assert!(content.fluid_textures[water.0 as usize].is_none());
        assert_eq!(
            content.blocks.len(),
            BlockRegistry::with_builtins().len(),
            "the rest of the block set is untouched"
        );
    }

    /// Every block naming a `block_model` must actually have baked geometry —
    /// a mistyped path or an unreadable export otherwise degrades quietly to an
    /// invisible block, which is far harder to notice than a broken icon.
    #[test]
    fn every_blockbench_block_in_the_shipped_data_loads() {
        let content = GameContent::load();
        let modelled: Vec<&str> = content
            .baked_models
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_some())
            .map(|(id, _)| {
                content
                    .blocks
                    .get(crate::core::BlockId(id as u16))
                    .name
                    .as_str()
            })
            .collect();
        assert_eq!(
            modelled,
            vec![
                "stone",
                "dirt",
                "grass",
                "sand",
                "oak log",
                "oak leaves",
                "gravel",
                "coal ore",
                "iron ore",
                "copper ore",
                "cobblestone",
                "cornflower",
            ],
            "the blocks migrated to Blockbench so far"
        );

        // Solid terrain cubes must fill their cell so neighbours can cull
        // against them. The other two legitimately do not: leaves are a cutout,
        // and a cornflower is two crossed planes.
        let see_through = ["oak leaves", "cornflower"];

        for (id, baked) in content.baked_models.iter().enumerate() {
            let Some(baked) = baked else { continue };
            let name = content
                .blocks
                .get(crate::core::BlockId(id as u16))
                .name
                .as_str();
            assert!(!baked.quads.is_empty(), "{name}: no geometry");
            let expected = !see_through.contains(&name);
            assert_eq!(
                baked.occludes, [expected; 6],
                "{name}: wrong occlusion for what it is"
            );
            for quad in &baked.quads {
                assert_ne!(quad.layer, 0, "{name}: sampling the missing-texture layer");
            }
        }
    }

    /// A flower is two crossed planes, so the crosshair must not reach it from
    /// the far corner of its cell — and its box must be a real box, not the
    /// inverted one a model with no vertical extent used to produce.
    #[test]
    fn a_modelled_plant_gets_a_hitbox_smaller_than_its_cell() {
        let content = GameContent::load();
        let id = content.blocks.find("cornflower").expect("shipped block");
        let baked = content.baked_models[id.0 as usize]
            .as_ref()
            .expect("cornflower is modelled");

        assert!(baked.random_yaw, "flowers vary their angle");
        let hitbox = baked.hitbox.expect("a plant does not fill its cell");
        let size = hitbox.max - hitbox.min;
        assert!(size.x > 0.0 && size.y > 0.0 && size.z > 0.0, "{size}");
        assert!(size.y <= 1.0, "taller than its cell: {size}");

        // A full cube needs no box of its own — the raycast marches through it.
        let stone = content.blocks.find("stone").expect("shipped block");
        let cube = content.baked_models[stone.0 as usize]
            .as_ref()
            .expect("stone is modelled");
        assert!(
            cube.hitbox.is_none(),
            "a full cube should stay a plain cell"
        );
    }

    /// Dropped stacks and inventory icons still draw six-sided atlas cubes, so a
    /// Blockbench block needs a small stand-in tile per face derived from its own
    /// textures. Without it they would all show the magenta marker.
    #[test]
    fn a_blockbench_block_has_derived_atlas_tiles_for_its_faces() {
        let content = GameContent::load();
        let grass = content.blocks.find("grass").expect("shipped block");

        // `Block::textures` is deliberately left empty: a derived tile index
        // must never reach `content_hash`.
        assert_eq!(content.blocks.get(grass).textures.0, [0; 6]);

        let faces = content.face_textures(grass);
        for dir in Direction::ALL {
            assert_ne!(
                faces.tile(dir),
                0,
                "{dir:?} fell back to the missing marker"
            );
        }
        // The top and the bottom come from different art (grass vs dirt).
        assert_ne!(faces.tile(Direction::PosY), faces.tile(Direction::NegY));
    }

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

        let mut tiles = crate::art::tile_registry();
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
            tiles: &mut crate::art::tile_registry(),
            visuals: BlockVisuals::default(),
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
