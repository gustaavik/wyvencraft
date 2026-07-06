//! Block definitions and the [`BlockRegistry`] — the single source of truth for
//! block properties (collision, rendering, textures).
//!
//! Design: *Registry pattern*. Block ids ([`BlockId`]) are indices into a
//! `Vec<Block>`; gameplay/render code looks up properties through the registry
//! rather than hard-coding `match` arms everywhere.

use crate::core::{BlockId, Direction};
use crate::render::tiles;

/// How a block participates in meshing/rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderType {
    /// Not drawn at all (air).
    Invisible,
    /// Fully opaque cube; hides neighbouring faces.
    Opaque,
    /// See-through cube (glass/water); does not hide neighbours of other
    /// block types and is drawn in the transparent pass.
    Transparent,
    /// Alpha-tested cube (leaves): texture is either fully opaque or fully
    /// clear per texel, so it draws in the opaque pass with depth writes (the
    /// shader discards clear texels). Avoids the blend-order artifacts of the
    /// unsorted transparent pass. Does not hide neighbours of other types.
    Cutout,
}

/// What a block is made of — selects which tool mines it fastest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Per-face atlas tile indices, ordered by [`Direction`] (`-X,+X,-Y,+Y,-Z,+Z`).
#[derive(Debug, Clone, Copy)]
pub struct FaceTextures(pub [u32; 6]);

impl FaceTextures {
    /// Same tile on every face.
    pub const fn uniform(tile: u32) -> Self {
        Self([tile; 6])
    }

    /// Distinct top / bottom / side tiles (the common grass/log case).
    pub const fn column(top: u32, bottom: u32, side: u32) -> Self {
        // order: -X,+X,-Y,+Y,-Z,+Z  =>  side,side,bottom,top,side,side
        Self([side, side, bottom, top, side, side])
    }

    #[inline]
    pub fn tile(&self, dir: Direction) -> u32 {
        self.0[dir as usize]
    }
}

/// Static description of a block type.
#[derive(Debug, Clone)]
pub struct Block {
    pub name: &'static str,
    pub render: RenderType,
    /// Whether entities collide with this block.
    pub solid: bool,
    pub textures: FaceTextures,
    /// Relative mining time; `f32::INFINITY` means unbreakable (e.g. bedrock).
    pub hardness: f32,
    /// What the block is made of, for tool matching.
    pub material: BlockMaterial,
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
}

/// Well-known built-in block ids. Order must match [`BlockRegistry::with_builtins`].
pub mod blocks {
    use super::BlockId;
    pub const AIR: BlockId = BlockId(0);
    pub const STONE: BlockId = BlockId(1);
    pub const DIRT: BlockId = BlockId(2);
    pub const GRASS: BlockId = BlockId(3);
    pub const SAND: BlockId = BlockId(4);
    pub const WATER: BlockId = BlockId(5);
    pub const WOOD: BlockId = BlockId(6);
    pub const LEAVES: BlockId = BlockId(7);
    pub const GLASS: BlockId = BlockId(8);
    pub const BEDROCK: BlockId = BlockId(9);
    pub const SNOW: BlockId = BlockId(10);
    pub const GRAVEL: BlockId = BlockId(11);
    pub const CLAY: BlockId = BlockId(12);
    pub const COAL_ORE: BlockId = BlockId(13);
    pub const IRON_ORE: BlockId = BlockId(14);
    pub const GOLD_ORE: BlockId = BlockId(15);
    pub const DIAMOND_ORE: BlockId = BlockId(16);
}

/// Lookup table of all registered block types.
pub struct BlockRegistry {
    blocks: Vec<Block>,
}

impl BlockRegistry {
    /// Build the registry with the default set of blocks. The push order defines
    /// the numeric [`BlockId`]s (see [`blocks`]).
    pub fn with_builtins() -> Self {
        // Atlas tile indices come from `render::tiles`, which also paints the
        // matching pixel art into the atlas (see `render::texture`).
        let mut reg = Self { blocks: Vec::new() };
        reg.register(Block {
            name: "air",
            render: RenderType::Invisible,
            solid: false,
            textures: FaceTextures::uniform(0),
            hardness: 0.0,
            material: BlockMaterial::Other,
        });
        reg.register(Block {
            name: "stone",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::STONE),
            hardness: 1.5,
            material: BlockMaterial::Stone,
        });
        reg.register(Block {
            name: "dirt",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::DIRT),
            hardness: 0.5,
            material: BlockMaterial::Dirt,
        });
        reg.register(Block {
            name: "grass",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::column(tiles::GRASS_TOP, tiles::DIRT, tiles::GRASS_SIDE),
            hardness: 0.6,
            material: BlockMaterial::Dirt,
        });
        reg.register(Block {
            name: "sand",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::SAND),
            hardness: 0.5,
            material: BlockMaterial::Sand,
        });
        reg.register(Block {
            name: "water",
            render: RenderType::Transparent,
            solid: false,
            textures: FaceTextures::uniform(tiles::WATER_0),
            hardness: f32::INFINITY,
            material: BlockMaterial::Other,
        });
        reg.register(Block {
            name: "wood",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::column(tiles::WOOD_RINGS, tiles::WOOD_RINGS, tiles::WOOD_BARK),
            hardness: 2.0,
            material: BlockMaterial::Wood,
        });
        reg.register(Block {
            name: "leaves",
            render: RenderType::Cutout,
            solid: true,
            textures: FaceTextures::uniform(tiles::LEAVES),
            hardness: 0.2,
            material: BlockMaterial::Plant,
        });
        reg.register(Block {
            name: "glass",
            render: RenderType::Transparent,
            solid: true,
            textures: FaceTextures::uniform(tiles::GLASS),
            hardness: 0.3,
            material: BlockMaterial::Glass,
        });
        reg.register(Block {
            name: "bedrock",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::BEDROCK),
            hardness: f32::INFINITY,
            material: BlockMaterial::Other,
        });
        reg.register(Block {
            name: "snow",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::SNOW),
            hardness: 0.2,
            material: BlockMaterial::Dirt,
        });
        reg.register(Block {
            name: "gravel",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::GRAVEL),
            hardness: 0.6,
            material: BlockMaterial::Dirt,
        });
        reg.register(Block {
            name: "clay",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::CLAY),
            hardness: 0.6,
            material: BlockMaterial::Dirt,
        });
        reg.register(Block {
            name: "coal ore",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::COAL_ORE),
            hardness: 3.0,
            material: BlockMaterial::Stone,
        });
        reg.register(Block {
            name: "iron ore",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::IRON_ORE),
            hardness: 3.0,
            material: BlockMaterial::Stone,
        });
        reg.register(Block {
            name: "gold ore",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::GOLD_ORE),
            hardness: 3.0,
            material: BlockMaterial::Stone,
        });
        reg.register(Block {
            name: "diamond ore",
            render: RenderType::Opaque,
            solid: true,
            textures: FaceTextures::uniform(tiles::DIAMOND_ORE),
            hardness: 3.0,
            material: BlockMaterial::Stone,
        });
        reg
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
