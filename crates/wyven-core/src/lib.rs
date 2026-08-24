//! Foundational, dependency-light building blocks every other crate speaks:
//! voxel coordinate types, math/geometry helpers, deterministic RNG, and frame
//! timing.
//!
//! Nothing here knows what a block *means*. [`BlockId`] is an opaque index the
//! game gives significance to; [`Aabb`], [`Ray`] and [`Frustum`] are geometry.
//! That is what lets the engine crates share a vocabulary without sharing a
//! rulebook.

pub mod math;
pub mod rng;
pub mod time;
pub mod types;

pub use math::{Aabb, Frustum, Ray, rotate_y, wrap_angle};
pub use rng::Rng64;
pub use time::{Clock, FIXED_DT};
pub use types::{
    BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, CHUNK_VOLUME, ChunkPos, Direction, LocalPos,
};
