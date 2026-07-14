//! Foundational, dependency-light building blocks shared by every subsystem:
//! voxel coordinate types, math/geometry helpers, and frame timing.

pub mod day_cycle;
pub mod gamemode;
pub mod math;
pub mod rng;
pub mod time;
pub mod types;

pub use day_cycle::{Atmosphere, DayCycle};
pub use gamemode::GameMode;
pub use math::{Aabb, Frustum, Ray};
pub use rng::Rng64;
pub use time::Clock;
pub use types::{
    BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, CHUNK_VOLUME, ChunkPos, Direction, LocalPos,
};
