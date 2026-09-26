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

mod parse;

#[cfg(test)]
mod tests;
