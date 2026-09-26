//! Block definitions and the [`BlockRegistry`] — the single source of truth for
//! block properties (collision, rendering, textures).
//!
//! Design: *Registry pattern*, loaded from data. Blocks are declared in
//! `assets/blocks.toml` (with an embedded fallback copy compiled in); the file
//! order defines the numeric [`BlockId`]s and save files reference blocks by
//! **id** (see [`crate::domain::core::ident`]). Behavior is expressed as components on
//! the block ([`Drops`], [`FluidInfo`]) rather than hard-coded `match` arms on
//! identity.
//!
//! What the player *reads* is not here: display names are presentation, so like
//! textures and models they ride out of the parse in [`BlockVisuals`] and are
//! resolved by `content`, keeping them off [`Block`] and out of `content_hash`.

use crate::domain::core::BlockId;
use crate::domain::core::ident::is_valid_id;
use wyven_model::ModelSpec;
use wyven_voxel::{BlockProperties, FluidInfo, RenderType};

/// Embedded copy of the shipped block definitions, used when
/// `assets/blocks.toml` is missing or invalid (the assets dir is CWD-relative).
pub const BUILTIN_BLOCKS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/blocks.toml"));

/// Which tool a block wants, and whether it insists on one
/// (`[block.harvest]` in `assets/blocks.toml`).
///
/// The requirement lives **here**, on the block, rather than on the tools. A
/// tool declares only what shape it is (`[item.tool] kind`); the block declares
/// what shape it wants. That way adding a block is one file, not a block entry
/// plus an edit to every tool that should be good at it — and a tool never has
/// to enumerate the world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Harvest {
    /// Tool kinds that mine this block at full speed, by `[item.tool] kind`.
    /// Never empty — a `[block.harvest]` naming nothing is rejected.
    pub tools: Vec<String>,
    /// Whether breaking it with something else yields nothing at all.
    ///
    /// `false` — the default — is the rule the game is built on: a better
    /// pickaxe only means a *faster* one, and every block stays mineable by
    /// hand. Set it where the tool is the whole point (leaves and shears).
    pub required: bool,
    /// The lowest `[item.tool] tier` of an accepted tool that can break this
    /// block at all. `0` — the default — means anything can, by hand if need
    /// be. Above zero the block is *too hard* for a lesser tool, the Valheim
    /// gate: a biome's ore stays out of reach until the previous biome's boss
    /// has given up the material for a better pickaxe.
    pub tier: u8,
}

impl Harvest {
    /// Whether a tool of `kind` mines this block at full speed.
    #[inline]
    pub fn accepts(&self, kind: &str) -> bool {
        self.tools.iter().any(|tool| tool == kind)
    }
}

/// What breaking a block yields (the `drops` field in `assets/blocks.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drops {
    /// The block's own item — the default.
    SelfItem,
    /// Nothing.
    None,
    /// A different item, by id (resolved against the item registry at use).
    Item { id: String, count: u8 },
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

/// The presentation-only data a block file carries — art assignments and the
/// label the player reads — reported out of the parse rather than stored on
/// [`Block`], which feeds `content_hash`.
///
/// Every vector is indexed by [`BlockId`] and covers every block, including the
/// auto-registered flowing fluids. `models` and `json` are separate because the
/// two paths are resolved differently — one through the [`ModelId`] registry,
/// one straight into baked quads — and because this is where the migration is
/// visible: the `json` column grows as blocks are re-authored, `models` shrinks
/// to nothing.
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
    /// `display_name = "..."` — an explicit label, where title-casing the id
    /// would get it wrong. `None` means "derive it", which `content` does with
    /// [`crate::domain::core::ident::title_case`]; carrying the `Option` rather than the
    /// resolved string is what lets a block *item* tell an authored name from a
    /// derived one.
    pub display_names: Vec<Option<String>>,
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
    /// Machine-readable key: `[a-z0-9_]`, unique, and the save/wire format.
    /// The player-facing label lives on `content`, not here — see
    /// [`BlockVisuals::display_names`].
    pub id: String,
    pub render: RenderType,
    /// Whether entities collide with this block.
    pub solid: bool,
    /// Relative mining time; `f32::INFINITY` means unbreakable (e.g. bedrock).
    pub hardness: f32,
    /// Which tool mines it, and whether one is required. `None` for a block
    /// no tool is better at (glass, bedrock).
    pub harvest: Option<Harvest>,
    /// What breaking it yields.
    pub drops: Drops,
    /// Set when the block is part of a fluid (source or flowing).
    pub fluid: Option<FluidInfo>,
    /// What right-clicking it does, for the blocks that do something.
    pub interact: Option<Interaction>,
    /// The crafting station this block is (`station = "workbench"`), which
    /// recipes naming it require within reach. Gameplay, so it is hashed.
    pub station: Option<String>,
}

/// What right-clicking a block does (`[block.interact]` in
/// `assets/blocks.toml`). The *meaning* of each kind is code — one hook per
/// variant in `state::ingame_state::block_use` — and the block only says which
/// one it is, so a new shrine or altar is a content edit.
///
/// Gameplay, so it rides `Block`'s `Debug` into the content hash: peers
/// disagreeing on what an altar summons must not share a world.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Interaction {
    /// A shrine's wayrune: reading it reveals the structure its shrine points
    /// to (`reveals` in `assets/structures.toml`).
    Shrine,
    /// A boss altar: offering the boss's `[entity.boss] offering` summons it.
    Altar {
        /// The entity kind (`assets/entities.toml`) this altar summons.
        boss: String,
    },
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
/// extend the set (numeric ids are session-local; saves and cross-file
/// references resolve by the string id).
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
    pub const DEEPSTONE: BlockId = BlockId(22);
    pub const MUD: BlockId = BlockId(23);
    pub const PACKED_SNOW: BlockId = BlockId(24);
    pub const BASALT: BlockId = BlockId(25);
    pub const ASH: BlockId = BlockId(26);
    pub const MOSSY_COBBLESTONE: BlockId = BlockId(27);
    pub const TIN_ORE: BlockId = BlockId(28);
    pub const SILVER_ORE: BlockId = BlockId(29);
    pub const CINDER_ORE: BlockId = BlockId(30);
    pub const WAYRUNE: BlockId = BlockId(31);
    pub const ELDER_ALTAR: BlockId = BlockId(32);
    pub const WORKBENCH: BlockId = BlockId(33);
    pub const FORGE: BlockId = BlockId(34);
    /// Flowing water levels 1 (shallowest) through 7: auto-registered after
    /// all declared blocks; the source block [`WATER`] is level 8.
    pub const WATER_FLOW_1: BlockId = BlockId(35);
    pub const WATER_FLOW_7: BlockId = BlockId(41);
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
    id: String,
    /// Overrides the label derived from `id`, for the ids the rule gets wrong.
    display_name: Option<String>,
    render: RenderType,
    solid: bool,
    hardness: f32,
    /// `[block.harvest]` — which tool this block wants.
    harvest: Option<HarvestDef>,
    /// Optional for a block whose geometry brings its own texture and never
    /// samples the atlas — either a `[block.model]` or a `block_model`.
    textures: Option<TexturesDef>,
    drops: Option<DropsDef>,
    fluid: Option<FluidDef>,
    model: Option<ModelSpec>,
    /// `block_model = "assets/models/blocks/dirt.json"` — a Blockbench *Java
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
    /// `[block.interact]` — what right-clicking it does.
    interact: Option<Interaction>,
    /// `station = "workbench"` — the crafting station this block is.
    station: Option<String>,
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

/// `[block.harvest]` with `tool = "pickaxe"` or `tool = ["shears", "sword"]`,
/// plus an optional `required = true`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HarvestDef {
    tool: ToolsDef,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    tier: u8,
}

/// `tool = "pickaxe"`, or `tool = ["shears", "sword"]` when more than one shape
/// is good at it.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ToolsDef {
    One(String),
    Many(Vec<String>),
}

impl HarvestDef {
    fn resolve(self, block: &str) -> Result<Harvest, String> {
        let tools = match self.tool {
            ToolsDef::One(kind) => vec![kind],
            ToolsDef::Many(kinds) => kinds,
        };
        // An empty list would silently mean "no tool is good at this", which is
        // what leaving the whole table out already says — and with
        // `required = true` it would mean "nothing can ever harvest this",
        // which is certainly not what anyone typed on purpose.
        if tools.is_empty() {
            return Err(format!(
                "block {block:?}: [block.harvest] names no tool; omit the table instead"
            ));
        }
        for kind in &tools {
            if !is_valid_id(kind) {
                return Err(format!(
                    "block {block:?}: tool kind {kind:?} must be lowercase letters, digits \
                     and underscores"
                ));
            }
        }
        Ok(Harvest {
            tools,
            required: self.required,
            tier: self.tier,
        })
    }
}

/// `drops = "self" | "none" | { item = "...", count = N }`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum DropsDef {
    Keyword(String),
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
            Self::OtherItem { item, count } => Ok(Drops::Item {
                id: item.clone(),
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
    /// one via [`crate::presentation::content::GameContent`]). Infallible: the shipped file
    /// is validated by the golden tests.
    pub fn with_builtins() -> Self {
        Self::from_toml(BUILTIN_BLOCKS).expect("embedded blocks.toml must parse")
    }

    /// Parse a blocks file. Declared order defines the numeric [`BlockId`]s:
    /// "air" is registered first (id 0, an engine invariant), then every
    /// `[[block]]` entry, then the auto-generated flowing blocks of each fluid
    /// (so declared blocks keep their ids regardless of fluids).
    ///
    /// Structural errors (bad TOML, malformed/duplicate/reserved ids) fail the
    /// whole file — the caller falls back to [`BlockRegistry::with_builtins`].
    /// Unknown texture names only degrade that block, once `content` fails to
    /// find art for them.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        Self::from_toml_with_models(text, &mut BlockVisuals::default())
    }

    /// Like [`BlockRegistry::from_toml`], but also reports each block's model
    /// assignment and display name in [`BlockVisuals`], indexed by [`BlockId`].
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
            display_names,
        } = visuals;
        texture_names.clear();
        models.clear();
        json.clear();
        fluid_visuals.clear();
        display_names.clear();
        let mut reg = Self {
            blocks: Vec::new(),
            fluid_flow: Vec::new(),
        };
        texture_names.push(None);
        display_names.push(None);
        reg.register(Block {
            id: "air".into(),
            render: RenderType::Invisible,
            solid: false,
            hardness: 0.0,
            harvest: None,
            drops: Drops::None,
            fluid: None,
            interact: None,
            station: None,
        });
        models.push(None); // air
        json.push(None);
        fluid_visuals.push(None);

        // Fluid sources, in declaration order: (source id, flow_levels).
        let mut fluids: Vec<(BlockId, u8, Option<FluidTextureSpec>)> = Vec::new();
        for def in file.block {
            // An id is a reference key in five other files, so a malformed one
            // rejects the whole table rather than just its own entry: skipping
            // an entry would renumber every later `BlockId` and silently orphan
            // the worldgen/recipe/drop references pointing past it.
            if !is_valid_id(&def.id) {
                return Err(format!(
                    "block {:?}: an id must be lowercase letters, digits and underscores",
                    def.id
                ));
            }
            if def.id == "air" {
                return Err("\"air\" is built in and may not be declared".into());
            }
            if reg.find(&def.id).is_some() {
                return Err(format!("duplicate block {:?}", def.id));
            }
            // A station name is what recipes spell, so it follows the id rule.
            if let Some(station) = &def.station
                && !is_valid_id(station)
            {
                return Err(format!(
                    "block {:?}: station {station:?} must be lowercase letters, digits \
                     and underscores",
                    def.id
                ));
            }
            let drops = match &def.drops {
                Some(d) => d
                    .resolve()
                    .map_err(|e| format!("block {:?}: {e}", def.id))?,
                None => Drops::SelfItem,
            };
            let harvest = def.harvest.map(|h| h.resolve(&def.id)).transpose()?;
            let mut fluid_texture = None;
            let fluid = match &def.fluid {
                Some(f) => {
                    if !(1..=15).contains(&f.flow_levels) {
                        return Err(format!("block {:?}: flow_levels must be 1..=15", def.id));
                    }
                    if let Some(t) = &f.texture {
                        if t.frames < 2 {
                            return Err(format!(
                                "block {:?}: a fluid texture needs at least 2 frames",
                                def.id
                            ));
                        }
                        if t.fps == 0 {
                            return Err(format!(
                                "block {:?}: fluid texture fps must be > 0",
                                def.id
                            ));
                        }
                        // The shader's animation clock wraps hourly; a loop
                        // that does not divide it evenly jumps at the wrap.
                        if !(3600 * u32::from(t.fps)).is_multiple_of(u32::from(t.frames)) {
                            return Err(format!(
                                "block {:?}: {} frames at {} fps does not divide the \
                                 3600 s animation clock evenly",
                                def.id, t.frames, t.fps
                            ));
                        }
                        let opacity = match t.opacity {
                            Some(o) if (0.0..=1.0).contains(&o) => Some((o * 255.0).round() as u8),
                            Some(o) => {
                                return Err(format!(
                                    "block {:?}: fluid texture opacity must be 0..=1, got {o}",
                                    def.id
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
                    def.id
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
                        def.id
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
            display_names.push(def.display_name);
            let id = reg.register(Block {
                id: def.id,
                render: def.render,
                solid: def.solid,
                hardness: def.hardness,
                harvest,
                drops,
                fluid,
                interact: def.interact,
                station: def.station,
            });
            models.push(model);
            if let Some(f) = &def.fluid {
                fluids.push((id, f.flow_levels, fluid_texture));
            }
        }

        // Auto-register the flowing blocks: same look and physics as their
        // source, one per level, with the id "<source>_flow_<level>" (these ids
        // are the save format — see the fluid module docs).
        for (group, (source_id, levels, texture)) in fluids.into_iter().enumerate() {
            let source = reg.get(source_id).clone();
            let source_textures = texture_names.get(source_id.0 as usize).cloned().flatten();
            // Only worth carrying when the source spelled its own label out:
            // otherwise the derived "Water Flow 1" is already what it would say.
            let source_label = display_names.get(source_id.0 as usize).cloned().flatten();
            let flow: Vec<BlockId> = (1..=levels)
                .map(|level| {
                    texture_names.push(source_textures.clone());
                    fluid_visuals.push(texture.clone().map(|spec| FluidVisual {
                        spec,
                        flowing: true,
                    }));
                    display_names.push(source_label.as_ref().map(|l| format!("{l} Flow {level}")));
                    reg.register(Block {
                        id: format!("{}_flow_{}", source.id, level),
                        render: source.render,
                        solid: source.solid,
                        hardness: source.hardness,
                        harvest: source.harvest.clone(),
                        drops: Drops::None,
                        fluid: Some(FluidInfo {
                            group: group as u16,
                            level,
                            max_level: levels + 1,
                        }),
                        interact: None,
                        station: None,
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
        display_names.resize(reg.len(), None);
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

    /// Look up a block by its id (save files reference blocks by id because
    /// numeric ids shift when the registry changes across builds).
    pub fn find(&self, id: &str) -> Option<BlockId> {
        self.blocks
            .iter()
            .position(|b| b.id == id)
            .map(|i| BlockId(i as u16))
    }
}

impl Default for BlockRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests;
