//! Foundational, dependency-light building blocks shared by every subsystem:
//! voxel coordinate types, math/geometry helpers, and frame timing.

pub mod math;
pub mod time;
pub mod types;

pub use math::{Aabb, Frustum, Ray};
pub use time::Clock;
pub use types::{
    BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, CHUNK_VOLUME, ChunkPos, Direction, LocalPos,
};
