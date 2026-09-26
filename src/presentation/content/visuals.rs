//! How the game's content *looks*: every texture, model, icon and label,
//! resolved against the rules it decorates.
//!
//! None of this feeds the content hash — that is the point of it being a
//! separate value from [`Registries`]. Two peers whose grass is drawn, tilted
//! or labelled differently still share a world.

use std::sync::Arc;

use wyven_assets::{AssetSource as ContentSource, decode_png};
use wyven_model::mesh as model_mesh;
use wyven_model::{DisplayContext, DisplayTransforms, ModelId, ModelRegistry, blockjson};
use wyven_render::TileRegistry;
use wyven_render::block_textures::{self, AnimatedLayers, BlockTextureSet, Strip};
use wyven_voxel::{BlockModel, FaceTextures, FluidTexture, model_hitbox};

use crate::application::content::RuleVisuals;
use crate::domain::content::Registries;
use crate::domain::core::Direction;
use crate::domain::core::ident::title_case;
use crate::domain::inventory::item::ItemVisuals;
use crate::domain::inventory::{ItemRegistry, Placeable};
use crate::domain::world::BlockRegistry;
use crate::domain::world::block::{BlockJsonSpec, BlockModelSpec, BlockVisuals, FluidVisual};
use crate::domain::world::blockmodel::BakedBlockModel;

/// How an item is drawn as a 2D icon in the inventory and hotbar. Computed once
/// at content load and indexed by `ItemId`, so the UI never touches block or
/// tile registries at draw time. Kept off [`crate::domain::inventory::Item`] on purpose:
/// texture assignment is visual-only and must not feed the content hash
/// ([`crate::domain::content::Registries::hash`]), which gates multiplayer joins.
#[derive(Debug, Clone, Copy)]
pub enum ItemIcon {
    /// A placeable solid block, drawn as a shaded isometric cube.
    Cube { top: u32, left: u32, right: u32 },
    /// Anything else (tools, food, armor, fluids), drawn as one flat tile.
    Flat(u32),
    /// An item with a file-loaded model, drawn from its cell of the pre-rendered
    /// icon sheet (see [`wyven_render::icons`]). The cell index is the
    /// [`ModelId`], so the sheet and the model registry share an ordering.
    Model(ModelId),
}

/// What a block with no resolvable art draws with: the missing-texture marker
/// on every face, which is tile 0 by construction.
pub const MISSING_FACES: FaceTextures = FaceTextures::uniform(0);

/// How a loose stack is shaped where it lies in the world.
///
/// A block item is a miniature of the block, each face its own texture. Anything
/// else has only a flat icon, and wrapping that around a cube reads as six
/// apples rather than one — so it is drawn as the icon itself, one texel thick.
#[derive(Debug, Clone, Copy)]
pub enum ItemShape {
    Cube(FaceTextures),
    Sprite(u32),
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

impl ItemModel {
    /// Where this item sits within the space of whatever carries it, for the
    /// context it is being drawn in.
    ///
    /// The model file's own `display` entry wins when it has one, because its
    /// author measured it against that context; otherwise the `[item.model]`
    /// numbers place it, exactly as they did before `display` existed. One
    /// function, so the fallback can never be spelled two different ways at two
    /// call sites — a `.bbmodel`, which declares nothing, must keep being placed
    /// by precisely the matrix that has always placed it.
    pub fn local(&self, models: &ModelRegistry, context: DisplayContext) -> glam::Mat4 {
        let display = models
            .get(self.id)
            .and_then(|model| model.placement_for(context));
        model_mesh::local_transform(display, self.scale, self.rotation, self.offset)
    }
}

/// The item a fired arrow borrows its art from.
const ARROW_ITEM: &str = "arrow";

/// Where every block item is placed from.
///
/// A display-only model, the way Minecraft's `block/block` is a display-only
/// parent: a block item has no geometry file of its own — it is drawn as a cube
/// built from the block's own faces — so all this holds is where that cube sits
/// in a fist. Shared by every block item, which are all the same cube. Missing
/// or malformed, it falls back to [`crate::presentation::render::viewmodel::default_block_display`].
pub const BLOCK_ITEM_MODEL: &str = "assets/models/items/block.json";

/// Everything visual about the loaded content, indexed by the ids the
/// [`Registries`] hand out.
pub struct Visuals {
    /// Texture name → atlas tile assignments plus the CPU-side atlas pixels
    /// (uploaded once by the renderer at startup).
    pub tiles: TileRegistry,
    /// The 256×256 textures Blockbench-authored blocks sample, one array layer
    /// each. Uploaded once by the renderer alongside the atlas.
    pub block_textures: BlockTextureSet,
    /// Every model file referenced by an entity visual or an item, parsed once.
    pub models: Arc<ModelRegistry>,
    /// 2D icon for each item, indexed by `ItemId` (see [`ItemIcon`]).
    pub item_icons: Vec<ItemIcon>,
    /// 3D model for each item, indexed by `ItemId`: what a held or dropped
    /// stack is drawn as.
    pub item_models: Vec<Option<ItemModel>>,
    /// 3D model for each block, indexed by `BlockId`: a block with one is
    /// meshed by baking that model into its cell instead of six atlas-textured
    /// cube faces.
    pub block_models: Vec<Option<BlockModel>>,
    /// Blockbench-authored geometry for each block, indexed by `BlockId`. The
    /// direction every block is moving in.
    pub baked_models: Vec<Option<BakedBlockModel>>,
    /// Atlas tiles derived from a Blockbench block's own textures, indexed by
    /// `BlockId` — see [`Visuals::face_textures`], which is how everything
    /// outside the chunk mesh should read them.
    pub block_face_tiles: Vec<Option<FaceTextures>>,
    /// The cube faces the arrow projectile is drawn with, resolved once from
    /// the `arrow` item. A projectile in flight is not an inventory stack, so
    /// nothing would otherwise look this up — and looking it up by id every
    /// frame would be the only string search in the render path.
    ///
    /// Still a cube, unlike a *dropped* arrow: an arrow in flight is oriented by
    /// its own yaw, so a flat sprite would turn edge-on to the way it is going.
    /// It wants a model, not a sprite.
    pub arrow_faces: FaceTextures,
    /// The animation strip each fluid block draws from, indexed by `BlockId`
    /// and covering the auto-registered flowing blocks too.
    pub fluid_textures: Vec<Option<FluidTexture>>,
    /// What the player reads for each block, indexed by `BlockId`: the
    /// `display_name` it authored, or its id title-cased.
    pub block_display_names: Vec<String>,
    /// What the player reads for each item, indexed by `ItemId` — the string
    /// the inventory tooltip, the hotbar label and `/give`'s reply all show.
    pub item_display_names: Vec<String>,
    /// Where a held block sits, from [`BLOCK_ITEM_MODEL`].
    pub block_item_display: DisplayTransforms,
}

impl Visuals {
    /// Resolve every texture, model, icon and label the rules' appearance
    /// fields name. Fail-soft throughout: a bad model or texture costs its own
    /// block's or item's appearance and nothing else.
    pub fn load(source: &dyn ContentSource, rules: &Registries, specs: RuleVisuals) -> Self {
        let Registries {
            blocks,
            items,
            entities,
            ..
        } = rules;
        let RuleVisuals {
            blocks:
                BlockVisuals {
                    textures: block_texture_names,
                    models: block_model_specs,
                    json: block_json_paths,
                    fluids: fluid_visuals,
                    display_names: block_labels,
                },
            items:
                ItemVisuals {
                    models: mut item_model_specs,
                    display_names: item_labels,
                },
        } = specs;

        // One tile registry serves every pass below — block faces, fluid
        // stand-ins, item icons — because a tile index only means anything
        // relative to the atlas it was allocated from.
        let mut tiles = crate::presentation::art::tile_registry();

        let block_display_names = resolve_block_display_names(blocks, block_labels);
        let item_display_names =
            resolve_item_display_names(items, item_labels, &block_display_names);

        // Models load last: they are named by the entity and item definitions,
        // so the registries above have to exist first. Each path is parsed once
        // however many definitions share it.
        let mut models = ModelRegistry::new();
        for kind in entities.iter() {
            if let Some(path) = kind.visual.model_path() {
                models.load(path, source);
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

        // Finally the plain `textures = ...` blocks. This is the pass that used
        // to happen inside `BlockRegistry::from_toml`, and it runs last so a
        // block that also carries a model or a fluid strip keeps the tiles
        // derived from its own art.
        for (id, names) in block_texture_names.iter().enumerate() {
            let Some(names) = names else { continue };
            if block_face_tiles.get(id).is_some_and(Option::is_some) {
                continue;
            }
            let faces = std::array::from_fn(|face| tiles.resolve(&names[face]).tile);
            if id >= block_face_tiles.len() {
                block_face_tiles.resize(id + 1, None);
            }
            block_face_tiles[id] = Some(FaceTextures(faces));
        }
        block_face_tiles.resize(blocks.len(), None);

        let item_icons =
            build_item_icons(&mut tiles, blocks, items, &item_models, &block_face_tiles);
        // An arrow in flight is a flat billboard sampling one atlas tile, so it
        // reads the *art* by name rather than the item's icon. Those used to be
        // the same thing; they stopped being when the arrow gained a generated
        // model and its icon became an `ItemIcon::Model` with no tile at all.
        let arrow_faces = match items.find(ARROW_ITEM) {
            Some(_) => FaceTextures::uniform(tiles.resolve(&format!("items/{ARROW_ITEM}")).tile),
            None => MISSING_FACES,
        };
        Self {
            tiles,
            block_textures,
            models: Arc::new(models),
            item_icons,
            item_models,
            block_models,
            baked_models,
            block_face_tiles,
            arrow_faces,
            fluid_textures,
            block_display_names,
            item_display_names,
            block_item_display: load_block_item_display(source),
        }
    }

    /// The six atlas tiles standing in for `block` outside the chunk mesh — its
    /// inventory icon, and the little cube a dropped stack is drawn as.
    ///
    /// A Blockbench-authored block has these derived from its own model
    /// textures; every other block uses the tiles `blocks.toml` named for it.
    /// Kept here rather than written back onto `Block` because `Block` feeds
    /// the content hash, and a derived tile index would then refuse a join over
    /// a difference that is purely visual.
    pub fn face_textures(&self, block: crate::domain::core::BlockId) -> FaceTextures {
        self.block_face_tiles
            .get(block.0 as usize)
            .copied()
            .flatten()
            .unwrap_or(MISSING_FACES)
    }
}

/// Resolve every block's label: the one it authored, or its id title-cased.
fn resolve_block_display_names(
    blocks: &BlockRegistry,
    authored: Vec<Option<String>>,
) -> Vec<String> {
    blocks
        .iter()
        .map(|(id, block)| {
            authored
                .get(id.0 as usize)
                .cloned()
                .flatten()
                .unwrap_or_else(|| title_case(&block.id))
        })
        .collect()
}

/// Resolve every item's label, in precedence order: the one the `[[item]]`
/// entry authored, else — for a block item — the *block's* resolved label, so
/// a block and the item that places it can never disagree, else the id
/// title-cased.
fn resolve_item_display_names(
    items: &ItemRegistry,
    authored: Vec<Option<String>>,
    block_names: &[String],
) -> Vec<String> {
    // The reverse of `block_to_item`: the block each item places, if any.
    // Built once rather than rescanned per item.
    let mut from_block: Vec<Option<usize>> = vec![None; items.len()];
    for block in 0..block_names.len() {
        if let Some(item) = items.item_for_block(crate::domain::core::BlockId(block as u16))
            && let Some(slot) = from_block.get_mut(item.0 as usize)
        {
            *slot = Some(block);
        }
    }

    items
        .iter()
        .map(|(id, item)| {
            if let Some(label) = authored.get(id.0 as usize).cloned().flatten() {
                return label;
            }
            if let Some(block) = from_block[id.0 as usize]
                && let Some(label) = block_names.get(block)
            {
                return label.clone();
            }
            title_case(&item.id)
        })
        .collect()
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
            if let Some(&Placeable { block: block_id }) = item.get::<Placeable>() {
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
                        .unwrap_or(MISSING_FACES);
                    return ItemIcon::Cube {
                        top: faces.tile(Direction::PosY),
                        left: faces.tile(Direction::NegZ),
                        right: faces.tile(Direction::PosX),
                    };
                }
            }
            // A fluid has no cube icon but does have derived tiles; preferring
            // them keeps it off the name lookup, which has no art to find.
            let derived = item.get::<Placeable>().and_then(|placeable| {
                block_face_tiles
                    .get(placeable.block.0 as usize)
                    .copied()
                    .flatten()
                    .map(|faces| faces.tile(Direction::PosY))
            });
            match derived {
                Some(tile) => ItemIcon::Flat(tile),
                None => ItemIcon::Flat(tiles.resolve(&format!("items/{}", item.id)).tile),
            }
        })
        .collect()
}

/// Read [`BLOCK_ITEM_MODEL`]'s `display` block.
///
/// Fail-soft like every other content loader: no file, or one that does not
/// parse, logs and falls back to the numbers compiled in — so a developer who
/// deletes it gets the shipped placement back rather than a block flat in their
/// face.
fn load_block_item_display(source: &dyn ContentSource) -> DisplayTransforms {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Document {
        display: DisplayTransforms,
    }

    let builtin = crate::presentation::render::viewmodel::default_block_display();
    let Ok(text) = source.read(BLOCK_ITEM_MODEL) else {
        log::info!("no {BLOCK_ITEM_MODEL}; using the builtin block-item placement");
        return builtin;
    };
    let declared = match serde_json::from_str::<Document>(&text) {
        Ok(document) => document.display,
        Err(err) => {
            log::warn!("could not parse {BLOCK_ITEM_MODEL} ({err}); using the builtin placement");
            return builtin;
        }
    };

    // Merged per context, not taken whole. A model's own `display` may leave a
    // context out because the `[item.model]` spec will place it — but a block
    // item has no spec to fall back to, so an absent entry would resolve to the
    // identity and drop the block you are holding to the origin at full size.
    // Deleting one entry from this file should cost you that one entry.
    DisplayTransforms {
        firstperson_righthand: declared
            .firstperson_righthand
            .or(builtin.firstperson_righthand),
        thirdperson_righthand: declared
            .thirdperson_righthand
            .or(builtin.thirdperson_righthand),
        ..declared
    }
}

/// A model's bounds after its `[block.model]` placement, in block-local `0..1`
/// coordinates — the same transform `world::meshing` bakes with, minus the yaw
/// and the translation to the cell (which `model_hitbox` handles by staying
/// square and centred).
fn placed_bounds(model: &wyven_model::Model, spec: &BlockModelSpec) -> (glam::Vec3, glam::Vec3) {
    let transform = wyven_model::mesh::placement(
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
