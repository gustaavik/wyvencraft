//! Foundational value types shared across the whole engine.
//!
//! These are deliberately small, `Copy`, and dependency-free so that every other
//! subsystem (world, render, net, ...) can speak the same vocabulary.

use glam::{IVec3, Vec3};

/// Width/depth of a chunk in blocks (X and Z axes).
pub const CHUNK_SIZE: i32 = 16;
/// Height of a chunk in blocks (Y axis). The world is a single layer of tall
/// column-chunks, Minecraft-style.
pub const CHUNK_HEIGHT: i32 = 256;
/// Number of blocks in one chunk.
pub const CHUNK_VOLUME: usize = (CHUNK_SIZE * CHUNK_SIZE * CHUNK_HEIGHT) as usize;

/// Identifier of a block *type* (e.g. air, stone, grass). Index into the
/// game's block registry — the engine assigns no meaning to the number.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct BlockId(pub u16);

impl BlockId {
    /// Air is always id 0 by convention; it is the empty/non-solid block.
    pub const AIR: BlockId = BlockId(0);

    #[inline]
    pub fn is_air(self) -> bool {
        self == Self::AIR
    }
}

/// Position of a chunk in the chunk grid (one unit = one chunk = [`CHUNK_SIZE`]
/// blocks on X/Z).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    /// World-space position of this chunk's minimum (0,0) corner, in blocks.
    pub fn origin(self) -> BlockPos {
        BlockPos::new(self.x * CHUNK_SIZE, 0, self.z * CHUNK_SIZE)
    }

    /// Chebyshev (chessboard) distance to another chunk, in chunks. Used for
    /// render-distance / load-radius checks.
    pub fn chebyshev_distance(self, other: ChunkPos) -> i32 {
        (self.x - other.x).abs().max((self.z - other.z).abs())
    }
}

/// Absolute position of a block in the world, in block units.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// The chunk that contains this block.
    pub fn chunk(self) -> ChunkPos {
        ChunkPos::new(self.x.div_euclid(CHUNK_SIZE), self.z.div_euclid(CHUNK_SIZE))
    }

    /// Position within the containing chunk, in `0..CHUNK_SIZE` / `0..CHUNK_HEIGHT`.
    /// Returns `None` if `y` is outside the world's vertical bounds.
    pub fn to_local(self) -> Option<LocalPos> {
        if self.y < 0 || self.y >= CHUNK_HEIGHT {
            return None;
        }
        Some(LocalPos {
            x: self.x.rem_euclid(CHUNK_SIZE) as u8,
            y: self.y as u16,
            z: self.z.rem_euclid(CHUNK_SIZE) as u8,
        })
    }

    /// Translate by a direction's unit offset.
    pub fn offset(self, dir: Direction) -> BlockPos {
        let o = dir.offset();
        BlockPos::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    /// Center of the block in world space (used by physics/raycasting).
    pub fn center(self) -> Vec3 {
        Vec3::new(
            self.x as f32 + 0.5,
            self.y as f32 + 0.5,
            self.z as f32 + 0.5,
        )
    }

    /// Convert a floating world position to the block that contains it.
    pub fn from_world(p: Vec3) -> BlockPos {
        BlockPos::new(p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32)
    }
}

/// Position inside a single chunk. Kept distinct from [`BlockPos`] so the type
/// system prevents mixing local and world coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LocalPos {
    pub x: u8,
    pub y: u16,
    pub z: u8,
}

impl LocalPos {
    /// Flatten to a linear index into a chunk's block array.
    /// Layout is Y-major then Z then X for cache-friendly column iteration.
    #[inline]
    pub fn index(self) -> usize {
        let x = self.x as usize;
        let y = self.y as usize;
        let z = self.z as usize;
        (y * CHUNK_SIZE as usize + z) * CHUNK_SIZE as usize + x
    }

    /// Inverse of [`LocalPos::index`].
    #[inline]
    pub fn from_index(i: usize) -> LocalPos {
        let s = CHUNK_SIZE as usize;
        let x = i % s;
        let z = (i / s) % s;
        let y = i / (s * s);
        LocalPos {
            x: x as u8,
            y: y as u16,
            z: z as u8,
        }
    }
}

/// The six axis-aligned faces of a cube. Iterated when culling/meshing blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Direction {
    NegX,
    PosX,
    NegY,
    PosY,
    NegZ,
    PosZ,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::NegX,
        Direction::PosX,
        Direction::NegY,
        Direction::PosY,
        Direction::NegZ,
        Direction::PosZ,
    ];

    /// Integer unit offset along this direction.
    #[inline]
    pub fn offset(self) -> IVec3 {
        match self {
            Direction::NegX => IVec3::new(-1, 0, 0),
            Direction::PosX => IVec3::new(1, 0, 0),
            Direction::NegY => IVec3::new(0, -1, 0),
            Direction::PosY => IVec3::new(0, 1, 0),
            Direction::NegZ => IVec3::new(0, 0, -1),
            Direction::PosZ => IVec3::new(0, 0, 1),
        }
    }

    /// Outward unit normal of this face.
    #[inline]
    pub fn normal(self) -> Vec3 {
        self.offset().as_vec3()
    }

    /// The face pointing the other way along the same axis. A block model's
    /// `cullface` names the neighbour direction, so hiding the face means
    /// asking that neighbour about *its* opposite face.
    #[inline]
    pub fn opposite(self) -> Direction {
        match self {
            Direction::NegX => Direction::PosX,
            Direction::PosX => Direction::NegX,
            Direction::NegY => Direction::PosY,
            Direction::PosY => Direction::NegY,
            Direction::NegZ => Direction::PosZ,
            Direction::PosZ => Direction::NegZ,
        }
    }

    /// The face on `axis` (0 = X, 1 = Y, 2 = Z) pointing the positive way when
    /// `positive`. Used to name the box face a ray crossed from its slab index.
    #[inline]
    pub fn facing(axis: usize, positive: bool) -> Direction {
        match (axis, positive) {
            (0, false) => Direction::NegX,
            (0, true) => Direction::PosX,
            (1, false) => Direction::NegY,
            (1, true) => Direction::PosY,
            (2, false) => Direction::NegZ,
            _ => Direction::PosZ,
        }
    }
}
