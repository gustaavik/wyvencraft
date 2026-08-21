//! Block definitions and the [`BlockRegistry`] — the single source of truth for
//! block properties (collision, rendering, textures).
//!
//! Design: *Registry pattern*, loaded from data. Blocks are declared in
//! `assets/blocks.toml` (with an embedded fallback copy compiled in); the file
//! order defines the numeric [`BlockId`]s and save files reference blocks by
//! name. Behavior is expressed as components on the block ([`Drops`],
//! [`FluidInfo`]) rather than hard-coded `match` arms on identity.

use crate::core::BlockId;
use wyven_model::ModelSpec;
use wyven_voxel::{BlockProperties, FluidInfo, RenderType};

/// Embedded copy of the shipped block definitions, used when
/// `assets/blocks.toml` is missing or invalid (the assets dir is CWD-relative).
pub const BUILTIN_BLOCKS: &str = include_str!("../../assets/blocks.toml");

/// What a block is made of — selects which tool mines it fastest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockMaterial {
    Stone,
    Dirt,
    Sand,
    Wood,
    Plant,
    Glass,
    /// No preferred tool (bedrock, misc).
    Other,
}

/// What breaking a block yields (the `drops` field in `assets/blocks.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drops {
    /// The block's own item — the default.
    SelfItem,
    /// Nothing.
    None,
    /// The block's own item, but only when the held tool's kind matches
    /// (e.g. leaves require shears).
    SelfWithTool { kind: String },
    /// A different item, by name (resolved against the item registry at use).
    Item { name: String, count: u8 },
}

/// A block's `[block.model]`, still unresolved: the model registry does not
/// exist yet when blocks are parsed, so the spec is reported out of the loader
/// and turned into a [`BlockModel`] afterwards (exactly how `[item.model]`
/// reaches `content::ItemModel`).
#[derive(Debug, Clone, PartialEq)]
pub struct BlockModelSpec {
    pub spec: ModelSpec,
    pub random_yaw: bool,
}

/// The visual-only model assignments a block file carries, reported out of the
/// parse rather than stored on [`Block`].
///
/// Both vectors are indexed by [`BlockId`] and cover every block, including the
/// auto-registered flowing fluids. They are separate because the two paths are
/// resolved differently — one through the [`ModelId`] registry, one straight
/// into baked quads — and because this is where the migration is visible: the
/// `json` column grows as blocks are re-authored, `models` shrinks to nothing.
#[derive(Debug, Default)]
pub struct BlockVisuals {
    /// `textures = ...` — the six texture *names*, in [`Direction`] order,
    /// still unresolved. Resolving them to atlas slots is `content`'s job: a
    /// tile index is derived from art, and anything derived from art must stay
    /// off [`Block`], which feeds `content_hash`.
    pub textures: Vec<Option<[String; 6]>>,
    /// `[block.model]` — a `.bbmodel`/`.gltf` file plus its placement.
    pub models: Vec<Option<BlockModelSpec>>,
    /// `block_model` — a Blockbench Java Block/Item `.json` and its placement.
    pub json: Vec<Option<BlockJsonSpec>>,
    /// `[block.fluid.texture]` — the animation strip a fluid draws from, copied
    /// onto each of its auto-registered flowing blocks with `flowing` set.
    pub fluids: Vec<Option<FluidVisual>>,
}

/// A fluid's animation strip, still unresolved, plus which of its two columns
/// this particular block reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidVisual {
    pub spec: FluidTextureSpec,
    /// Set on the auto-registered `<source> flow N` blocks. Their side faces
    /// take the flowing column; a source reads the still one everywhere.
    pub flowing: bool,
}

/// `[block.fluid.texture]`: an animation strip of `frames` square frames
/// stacked top to bottom, in as many columns as the image's width allows.
///
/// **Column 0 is flowing, column 1 is still** (a one-column strip serves both).
/// Only the frame count is authored — the frame size, and with it the column
/// count, follow from the image.
///
/// Kept *off* [`Block`] like every other visual assignment: `Block` feeds
/// `content_hash`, which gates multiplayer joins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidTextureSpec {
    pub path: String,
    pub frames: u8,
    pub fps: u8,
    /// Which biome colour multiplies the (greyscale) art, in Minecraft's
    /// `tintindex` numbering — `2` is water. `None` leaves it as authored.
    pub tint: Option<u8>,
    /// How opaque the surface is, `0..=255`. A body of water is a *single*
    /// blended sheet — the faces inside it are culled — so depth adds no
    /// opacity and this alone decides how much of the riverbed shows through.
    /// `None` keeps the art's own alpha.
    pub opacity: Option<u8>,
}

/// A block's `block_model`, still unresolved. The path is read after the block
/// registry exists, exactly like [`BlockModelSpec`]; `random_yaw` rides along
/// because it is authored on the `[[block]]` table, not in the model file.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockJsonSpec {
    pub path: String,
    pub random_yaw: bool,
}

/// Static description of a block type.
#[derive(Debug, Clone)]
pub struct Block {
    pub name: String,
    pub render: RenderType,
    /// Whether entities collide with this block.
    pub solid: bool,
    /// Relative mining time; `f32::INFINITY` means unbreakable (e.g. bedrock).
    pub hardness: f32,
    /// What the block is made of, for tool matching.
    pub material: BlockMaterial,
    /// What breaking it yields.
    pub drops: Drops,
    /// Set when the block is part of a fluid (source or flowing).
    pub fluid: Option<FluidInfo>,
}

impl Block {
    #[inline]
    pub fn is_opaque(&self) -> bool {
        matches!(self.render, RenderType::Opaque)
    }

    #[inline]
    pub fn is_visible(&self) -> bool {
        !matches!(self.render, RenderType::Invisible)
    }

    #[inline]
    pub fn is_transparent(&self) -> bool {
        matches!(self.render, RenderType::Transparent)
    }

    #[inline]
    pub fn is_cutout(&self) -> bool {
        matches!(self.render, RenderType::Cutout)
    }

    /// Whether the block can ever be mined (finite hardness).
    #[inline]
    pub fn is_breakable(&self) -> bool {
        self.hardness.is_finite()
    }

    /// Whether placing a block *at* this one replaces it rather than stacking
    /// on its face. True for breakable decoration you can walk through (a
    /// flower); never for fluids, which the crosshair sees straight through.
    #[inline]
    pub fn is_replaceable(&self) -> bool {
        !self.solid && self.is_breakable() && self.fluid.is_none() && self.is_visible()
    }
}

/// How the engine reads a `BlockId` through this table.
///
/// These three are the *only* things `wyven_voxel::World` needs to know about a
/// block, and each is derived from a rule the block table already holds. The
/// engine never sees the name, hardness, material or drops that surround them.
impl BlockProperties for BlockRegistry {
    #[inline]
    fn is_solid(&self, id: BlockId) -> bool {
        self.get(id).solid
    }

    /// Deliberately wider than [`is_solid`](BlockProperties::is_solid):
    /// decoration you can walk through (a flower) must still be breakable.
    /// Fluids stay out — the crosshair reaches through water — and so does air,
    /// which is invisible.
    #[inline]
    fn is_targetable(&self, id: BlockId) -> bool {
        let block = self.get(id);
        block.solid || (block.is_visible() && block.fluid.is_none())
    }

    #[inline]
    fn is_replaceable(&self, id: BlockId) -> bool {
        self.get(id).is_replaceable()
    }
}

/// Ids of the *builtin* block set, in its declared order — a convenience for
/// tests. Gameplay code must never use these: content files may reorder or
/// extend the set (ids are session-local; saves and cross-file references
/// resolve by name).
pub mod blocks {
    use super::BlockId;
    pub const AIR: BlockId = BlockId(0);
    pub const STONE: BlockId = BlockId(1);
    pub const DIRT: BlockId = BlockId(2);
    pub const GRASS: BlockId = BlockId(3);
    pub const SAND: BlockId = BlockId(4);
    pub const WATER: BlockId = BlockId(5);
    pub const OAK_LOG: BlockId = BlockId(6);
    pub const OAK_LEAVES: BlockId = BlockId(7);
    pub const GLASS: BlockId = BlockId(8);
    pub const BEDROCK: BlockId = BlockId(9);
    pub const SNOW: BlockId = BlockId(10);
    pub const GRAVEL: BlockId = BlockId(11);
    pub const CLAY: BlockId = BlockId(12);
    pub const COAL_ORE: BlockId = BlockId(13);
    pub const IRON_ORE: BlockId = BlockId(14);
    pub const COPPER_ORE: BlockId = BlockId(15);
    pub const COBBLESTONE: BlockId = BlockId(16);
    pub const BLUE_BELLS: BlockId = BlockId(17);
    pub const RED_FLOWER: BlockId = BlockId(18);
    pub const RED_MUSHROOM: BlockId = BlockId(19);
    pub const BROWN_MUSHROOM: BlockId = BlockId(20);
    pub const CORNFLOWER: BlockId = BlockId(21);
    /// Flowing water levels 1 (shallowest) through 7: auto-registered after
    /// all declared blocks; the source block [`WATER`] is level 8.
    pub const WATER_FLOW_1: BlockId = BlockId(22);
    pub const WATER_FLOW_7: BlockId = BlockId(28);
}

// ---- TOML schema -----------------------------------------------------------

#[derive(serde::Deserialize)]
struct BlockFile {
    #[serde(default)]
    block: Vec<BlockDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockDef {
    name: String,
    render: RenderType,
    solid: bool,
    hardness: f32,
    material: BlockMaterial,
    /// Optional for a block whose geometry brings its own texture and never
    /// samples the atlas — either a `[block.model]` or a `block_model`.
    textures: Option<TexturesDef>,
    drops: Option<DropsDef>,
    fluid: Option<FluidDef>,
    model: Option<ModelSpec>,
    /// `block_model = "blocks/dirt_block.json"` — a Blockbench *Java
    /// Block/Item* export, the way all blocks are authored going forward. Unlike
    /// `[block.model]` it needs no placement: the model is already in cell
    /// coordinates, carries several textures, and declares its own cull faces.
    ///
    /// Spelled differently from `[block.model]` only because both exist during
    /// the migration; it takes that name once the `.bbmodel` path is gone.
    block_model: Option<String>,
    /// Only meaningful alongside `[block.model]`; see [`BlockModel::random_yaw`].
    #[serde(default)]
    random_yaw: bool,
}

/// `textures = "stone"`, the top/bottom/side shorthand, or all six faces.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum TexturesDef {
    Uniform(String),
    Column {
        top: String,
        bottom: String,
        side: String,
    },
    Faces {
        neg_x: String,
        pos_x: String,
        neg_y: String,
        pos_y: String,
        neg_z: String,
        pos_z: String,
    },
}

/// `drops = "self" | "none" | { requires_tool = "..." } | { item = "...", count = N }`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum DropsDef {
    Keyword(String),
    RequiresTool {
        requires_tool: String,
    },
    OtherItem {
        item: String,
        #[serde(default = "default_drop_count")]
        count: u8,
    },
}

fn default_drop_count() -> u8 {
    1
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FluidDef {
    flow_levels: u8,
    texture: Option<FluidTextureDef>,
}

/// `[block.fluid.texture]`; see [`FluidTextureSpec`].
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FluidTextureDef {
    path: String,
    frames: u8,
    #[serde(default = "default_fluid_fps")]
    fps: u8,
    tint: Option<u8>,
    /// `opacity = 0.85`; see [`FluidTextureSpec::opacity`].
    opacity: Option<f32>,
}

/// Slow enough to read as a swell rather than a flicker. `fps * 3600` must stay
/// a multiple of the frame count, or the shader's hourly time wrap jumps.
fn default_fluid_fps() -> u8 {
    8
}

impl TexturesDef {
    /// The six texture names in [`Direction`] order (`-X,+X,-Y,+Y,-Z,+Z`).
    ///
    /// Names, not tiles: which atlas slot each one lands in depends on the art
    /// loaded alongside it, and that is a question for `content`, not for the
    /// block table.
    fn names(&self) -> [String; 6] {
        let names: [&str; 6] = match self {
            Self::Uniform(name) => [name; 6],
            // order: -X,+X,-Y,+Y,-Z,+Z  =>  side,side,bottom,top,side,side
            Self::Column { top, bottom, side } => [side, side, bottom, top, side, side],
            Self::Faces {
                neg_x,
                pos_x,
                neg_y,
                pos_y,
                neg_z,
                pos_z,
            } => [neg_x, pos_x, neg_y, pos_y, neg_z, pos_z],
        };
        names.map(str::to_string)
    }
}

impl DropsDef {
    fn resolve(&self) -> Result<Drops, String> {
        match self {
            Self::Keyword(word) => match word.as_str() {
                "self" => Ok(Drops::SelfItem),
                "none" => Ok(Drops::None),
                other => Err(format!("unknown drops keyword {other:?}")),
            },
            Self::RequiresTool { requires_tool } => Ok(Drops::SelfWithTool {
                kind: requires_tool.clone(),
            }),
            Self::OtherItem { item, count } => Ok(Drops::Item {
                name: item.clone(),
                count: *count,
            }),
        }
    }
}

/// Lookup table of all registered block types.
#[derive(Debug)]
pub struct BlockRegistry {
    blocks: Vec<Block>,
    /// Flowing blocks per fluid group, indexed `[group][level - 1]`.
    fluid_flow: Vec<Vec<BlockId>>,
}

impl BlockRegistry {
    /// Build the registry from the embedded copy of `assets/blocks.toml`,
    /// with a private tile registry (tests and fallbacks; the app path shares
    /// one via [`crate::content::GameContent`]). Infallible: the shipped file
    /// is validated by the golden tests.
    pub fn with_builtins() -> Self {
        Self::from_toml(BUILTIN_BLOCKS).expect("embedded blocks.toml must parse")
    }

    /// Parse a blocks file. Declared order defines the numeric [`BlockId`]s:
    /// "air" is registered first (id 0, an engine invariant), then every
    /// `[[block]]` entry, then the auto-generated flowing blocks of each fluid
    /// (so declared blocks keep their ids regardless of fluids).
    ///
    /// Structural errors (bad TOML, duplicate/reserved names) fail the whole
    /// file — the caller falls back to [`BlockRegistry::with_builtins`].
    /// Unknown texture names only degrade that block, once `content` fails to
    /// find art for them.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        Self::from_toml_with_models(text, &mut BlockVisuals::default())
    }

    /// Like [`BlockRegistry::from_toml`], but also reports each block's model
    /// assignment in [`BlockVisuals`], indexed by [`BlockId`].
    ///
    /// Model assignment rides out of band because it cannot be resolved yet —
    /// blocks are parsed before the model registry exists — and because it must
    /// stay off [`Block`], which feeds `content_hash`. See [`BlockModel`].
    pub fn from_toml_with_models(text: &str, visuals: &mut BlockVisuals) -> Result<Self, String> {
        let file: BlockFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.block.is_empty() {
            return Err("no [[block]] entries".into());
        }

        let BlockVisuals {
            textures: texture_names,
            models,
            json,
            fluids: fluid_visuals,
        } = visuals;
        texture_names.clear();
        models.clear();
        json.clear();
        fluid_visuals.clear();
        let mut reg = Self {
            blocks: Vec::new(),
            fluid_flow: Vec::new(),
        };
        texture_names.push(None);
        reg.register(Block {
            name: "air".into(),
            render: RenderType::Invisible,
            solid: false,
            hardness: 0.0,
            material: BlockMaterial::Other,
            drops: Drops::None,
            fluid: None,
        });
        models.push(None); // air
        json.push(None);
        fluid_visuals.push(None);

        // Fluid sources, in declaration order: (source id, flow_levels).
        let mut fluids: Vec<(BlockId, u8, Option<FluidTextureSpec>)> = Vec::new();
        for def in file.block {
            if def.name == "air" {
                return Err("\"air\" is built in and may not be declared".into());
            }
            if reg.find(&def.name).is_some() {
                return Err(format!("duplicate block {:?}", def.name));
            }
            let drops = match &def.drops {
                Some(d) => d
                    .resolve()
                    .map_err(|e| format!("block {:?}: {e}", def.name))?,
                None => Drops::SelfItem,
            };
            let mut fluid_texture = None;
            let fluid = match &def.fluid {
                Some(f) => {
                    if !(1..=15).contains(&f.flow_levels) {
                        return Err(format!("block {:?}: flow_levels must be 1..=15", def.name));
                    }
                    if let Some(t) = &f.texture {
                        if t.frames < 2 {
                            return Err(format!(
                                "block {:?}: a fluid texture needs at least 2 frames",
                                def.name
                            ));
                        }
                        if t.fps == 0 {
                            return Err(format!(
                                "block {:?}: fluid texture fps must be > 0",
                                def.name
                            ));
                        }
                        // The shader's animation clock wraps hourly; a loop
                        // that does not divide it evenly jumps at the wrap.
                        if !(3600 * u32::from(t.fps)).is_multiple_of(u32::from(t.frames)) {
                            return Err(format!(
                                "block {:?}: {} frames at {} fps does not divide the \
                                 3600 s animation clock evenly",
                                def.name, t.frames, t.fps
                            ));
                        }
                        let opacity = match t.opacity {
                            Some(o) if (0.0..=1.0).contains(&o) => Some((o * 255.0).round() as u8),
                            Some(o) => {
                                return Err(format!(
                                    "block {:?}: fluid texture opacity must be 0..=1, got {o}",
                                    def.name
                                ));
                            }
                            None => None,
                        };
                        fluid_texture = Some(FluidTextureSpec {
                            path: t.path.clone(),
                            frames: t.frames,
                            fps: t.fps,
                            tint: t.tint,
                            opacity,
                        });
                    }
                    Some(FluidInfo {
                        group: fluids.len() as u16,
                        level: f.flow_levels + 1,
                        max_level: f.flow_levels + 1,
                    })
                }
                None => None,
            };
            if def.model.is_some() && def.block_model.is_some() {
                return Err(format!(
                    "block {:?}: declares both `block_model` and a `[block.model]`",
                    def.name
                ));
            }
            // A modelled block's geometry carries its own texture, so it needs
            // no atlas tiles here; a `block_model`'s six face tiles are derived
            // from its own art later, in `content`. Anything else without
            // `textures` would silently render as the magenta marker, which is
            // worth rejecting loudly.
            let modelled =
                def.model.is_some() || def.block_model.is_some() || fluid_texture.is_some();
            let textures = match &def.textures {
                Some(t) => Some(t.names()),
                None if modelled => None,
                None => {
                    return Err(format!(
                        "block {:?}: needs `textures`, a `block_model`, a `[block.model]` \
                         or a `[block.fluid.texture]`",
                        def.name
                    ));
                }
            };
            texture_names.push(textures);
            let random_yaw = def.random_yaw;
            let model = def.model.map(|spec| BlockModelSpec { spec, random_yaw });
            json.push(
                def.block_model
                    .map(|path| BlockJsonSpec { path, random_yaw }),
            );
            fluid_visuals.push(fluid_texture.clone().map(|spec| FluidVisual {
                spec,
                flowing: false,
            }));
            let id = reg.register(Block {
                name: def.name,
                render: def.render,
                solid: def.solid,
                hardness: def.hardness,
                material: def.material,
                drops,
                fluid,
            });
            models.push(model);
            if let Some(f) = &def.fluid {
                fluids.push((id, f.flow_levels, fluid_texture));
            }
        }

        // Auto-register the flowing blocks: same look and physics as their
        // source, one per level, named "<source> flow <level>" (these names
        // are the save format — see the fluid module docs).
        for (group, (source_id, levels, texture)) in fluids.into_iter().enumerate() {
            let source = reg.get(source_id).clone();
            let source_textures = texture_names.get(source_id.0 as usize).cloned().flatten();
            let flow: Vec<BlockId> = (1..=levels)
                .map(|level| {
                    texture_names.push(source_textures.clone());
                    fluid_visuals.push(texture.clone().map(|spec| FluidVisual {
                        spec,
                        flowing: true,
                    }));
                    reg.register(Block {
                        name: format!("{} flow {}", source.name, level),
                        render: source.render,
                        solid: source.solid,
                        hardness: source.hardness,
                        material: source.material,
                        drops: Drops::None,
                        fluid: Some(FluidInfo {
                            group: group as u16,
                            level,
                            max_level: levels + 1,
                        }),
                    })
                })
                .collect();
            reg.fluid_flow.push(flow);
        }
        // The auto-registered flowing blocks never carry a model — they do
        // carry their source's fluid texture — but every vector is indexed by
        // `BlockId` and so must cover every block.
        models.resize(reg.len(), None);
        json.resize(reg.len(), None);
        fluid_visuals.resize(reg.len(), None);
        Ok(reg)
    }

    /// The fluid component of `id`, if any.
    #[inline]
    pub fn fluid(&self, id: BlockId) -> Option<FluidInfo> {
        self.get(id).fluid
    }

    /// Whether `id` is part of any fluid (source or flowing).
    #[inline]
    pub fn is_fluid(&self, id: BlockId) -> bool {
        self.fluid(id).is_some()
    }

    /// Flowing (non-source) fluid — simulation state with no item form.
    #[inline]
    pub fn is_flowing_fluid(&self, id: BlockId) -> bool {
        self.fluid(id).is_some_and(|f| !f.is_source())
    }

    /// The flowing block of fluid `group` at `level` (clamped to the group's
    /// valid range).
    #[inline]
    pub fn flowing(&self, group: u16, level: u8) -> BlockId {
        let flow = &self.fluid_flow[group as usize];
        flow[usize::from(level.clamp(1, flow.len() as u8)) - 1]
    }

    /// Append a new block type, returning its assigned id.
    pub fn register(&mut self, block: Block) -> BlockId {
        let id = BlockId(self.blocks.len() as u16);
        self.blocks.push(block);
        id
    }

    #[inline]
    pub fn get(&self, id: BlockId) -> &Block {
        // Unknown ids fall back to air to stay panic-free on bad network data.
        self.blocks.get(id.0 as usize).unwrap_or(&self.blocks[0])
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (BlockId, &Block)> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (BlockId(i as u16), b))
    }

    /// Look up a block by its registered name (save files reference blocks by
    /// name because numeric ids shift when the registry changes across builds).
    pub fn find(&self, name: &str) -> Option<BlockId> {
        self.blocks
            .iter()
            .position(|b| b.name == name)
            .map(|i| BlockId(i as u16))
    }
}

impl Default for BlockRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader's animation clock wraps every 3600 s. A loop that does not
    /// divide that evenly jumps mid-swell once an hour — subtle enough to ship
    /// by accident, so the loader refuses it outright.
    #[test]
    fn a_fluid_loop_that_does_not_divide_the_animation_clock_is_rejected() {
        // 64 frames at 6 fps: 6 * 3600 leaves 32 frames over.
        let bad = BUILTIN_BLOCKS.replace("fps = 8", "fps = 6");
        let err = BlockRegistry::from_toml(&bad).expect_err("must not parse");
        assert!(err.contains("3600"), "{err}");

        // ...while the shipped pairing, and every other multiple of 4, is fine.
        for fps in ["4", "8", "12", "20"] {
            let text = BUILTIN_BLOCKS.replace("fps = 8", &format!("fps = {fps}"));
            assert!(
                BlockRegistry::from_toml(&text).is_ok(),
                "{fps} fps over 64 frames should be accepted"
            );
        }
    }

    /// Golden snapshot of the shipped block set. The data-driven loader must
    /// reproduce this exactly: names are the save format (matched verbatim by
    /// [`BlockRegistry::find`]) and registration order defines the numeric ids
    /// used in chunk storage and on the wire.
    #[test]
    fn builtin_blocks_golden() {
        use BlockMaterial as M;
        use RenderType as R;
        const INF: f32 = f32::INFINITY;
        let expected: [(&str, R, bool, f32, M); 29] = [
            ("air", R::Invisible, false, 0.0, M::Other),
            ("stone", R::Opaque, true, 1.5, M::Stone),
            ("dirt", R::Opaque, true, 0.5, M::Dirt),
            ("grass", R::Opaque, true, 0.6, M::Dirt),
            ("sand", R::Opaque, true, 0.5, M::Sand),
            ("water", R::Transparent, false, INF, M::Other),
            ("oak log", R::Opaque, true, 2.0, M::Wood),
            ("oak leaves", R::Cutout, true, 0.2, M::Plant),
            ("glass", R::Transparent, true, 0.3, M::Glass),
            ("bedrock", R::Opaque, true, INF, M::Other),
            ("snow", R::Opaque, true, 0.2, M::Dirt),
            ("gravel", R::Opaque, true, 0.6, M::Dirt),
            ("clay", R::Opaque, true, 0.6, M::Dirt),
            ("coal ore", R::Opaque, true, 3.0, M::Stone),
            ("iron ore", R::Opaque, true, 3.0, M::Stone),
            ("copper ore", R::Opaque, true, 3.0, M::Stone),
            ("cobblestone", R::Opaque, true, 2.0, M::Stone),
            ("blue bells", R::Cutout, false, 0.0, M::Plant),
            ("red flower", R::Cutout, false, 0.0, M::Plant),
            ("red mushroom", R::Cutout, false, 0.0, M::Plant),
            ("brown mushroom", R::Cutout, false, 0.0, M::Plant),
            ("cornflower", R::Cutout, false, 0.0, M::Plant),
            ("water flow 1", R::Transparent, false, INF, M::Other),
            ("water flow 2", R::Transparent, false, INF, M::Other),
            ("water flow 3", R::Transparent, false, INF, M::Other),
            ("water flow 4", R::Transparent, false, INF, M::Other),
            ("water flow 5", R::Transparent, false, INF, M::Other),
            ("water flow 6", R::Transparent, false, INF, M::Other),
            ("water flow 7", R::Transparent, false, INF, M::Other),
        ];
        let reg = BlockRegistry::with_builtins();
        assert_eq!(reg.len(), expected.len(), "block count changed");
        for (i, &(name, render, solid, hardness, material)) in expected.iter().enumerate() {
            let block = reg.get(BlockId(i as u16));
            assert_eq!(block.name, name, "block {i}: name");
            assert_eq!(block.render, render, "{name}: render");
            assert_eq!(block.solid, solid, "{name}: solid");
            assert_eq!(block.hardness, hardness, "{name}: hardness");
            assert_eq!(block.material, material, "{name}: material");
        }
    }

    #[test]
    fn water_levels_match_registry_order() {
        let reg = BlockRegistry::with_builtins();
        assert_eq!(reg.find("water"), Some(blocks::WATER));
        assert_eq!(reg.find("water flow 1"), Some(blocks::WATER_FLOW_1));
        assert_eq!(reg.find("water flow 7"), Some(blocks::WATER_FLOW_7));

        let source = reg.fluid(blocks::WATER).expect("water is a fluid");
        assert!(source.is_source());
        assert_eq!((source.group, source.level, source.max_level), (0, 8, 8));
        for level in 1..=7u8 {
            let id = reg.flowing(0, level);
            let f = reg.fluid(id).expect("flow block is a fluid");
            assert_eq!((f.group, f.level, f.max_level), (0, level, 8));
            assert!(reg.is_fluid(id) && reg.is_flowing_fluid(id));
        }
        assert!(reg.is_fluid(blocks::WATER) && !reg.is_flowing_fluid(blocks::WATER));
        assert_eq!(reg.fluid(blocks::STONE), None);
        assert_eq!(reg.fluid(BlockId::AIR), None);
    }

    /// The behavior components declared in blocks.toml parse into the typed
    /// component fields.
    #[test]
    fn blocks_toml_components_parse() {
        let reg = BlockRegistry::with_builtins();
        assert_eq!(
            reg.get(blocks::OAK_LEAVES).drops,
            Drops::SelfWithTool {
                kind: "shears".into()
            }
        );
        // Mining stone yields cobblestone, exactly as the recipes assume.
        assert_eq!(
            reg.get(blocks::STONE).drops,
            Drops::Item {
                name: "cobblestone".into(),
                count: 1
            }
        );
        assert_eq!(reg.get(blocks::WATER_FLOW_1).drops, Drops::None);
        // Flow blocks inherit the source's look and physics.
        let (water, flow) = (reg.get(blocks::WATER), reg.get(blocks::WATER_FLOW_1));
        assert_eq!(flow.render, water.render);
        assert_eq!(flow.solid, water.solid);
        assert_eq!(flow.hardness, water.hardness);
    }

    /// Parse a blocks file with a throwaway tile registry.
    fn parse(text: &str) -> Result<BlockRegistry, String> {
        BlockRegistry::from_toml(text)
    }

    /// Structural errors reject the whole file (the loader then falls back to
    /// the builtin copy); reordering trips the well-known-id validation.
    #[test]
    fn invalid_block_files_are_rejected() {
        assert!(parse("not toml [").is_err());
        assert!(parse("").is_err());
        let air = r#"
            [[block]]
            name = "air"
            render = "opaque"
            solid = true
            hardness = 1.0
            material = "other"
            textures = "stone"
        "#;
        assert!(parse(air).is_err());
        let dup = r#"
            [[block]]
            name = "stone"
            render = "opaque"
            solid = true
            hardness = 1.0
            material = "stone"
            textures = "stone"

            [[block]]
            name = "stone"
            render = "opaque"
            solid = true
            hardness = 1.0
            material = "stone"
            textures = "stone"
        "#;
        assert!(parse(dup).is_err());
        // A minimal but valid file parses fine — ids are session-local, so
        // files are free to define any block set.
        let minimal = parse(
            r#"
            [[block]]
            name = "dirt"
            render = "opaque"
            solid = true
            hardness = 0.5
            material = "dirt"
            textures = "dirt"
        "#,
        )
        .expect("valid file");
        assert_eq!(minimal.len(), 2, "air + dirt");
    }
}
