//! Procedural world generation.
//!
//! Design: *Strategy pattern*. [`WorldGenerator`] is the swappable interface,
//! declared by `wyven_voxel` because the chunk loader is what calls it;
//! [`NoiseGenerator`] here is Wyvencraft's implementation of it. Tests and
//! debug worlds can supply another (a flat world, say) without touching a
//! single caller.

pub mod biome;
pub mod config;
pub mod features;
pub mod generator;
pub mod noise;

pub use config::WorldGenConfig;
pub use generator::NoiseGenerator;
// The trait itself is engine — the voxel loader is what calls it.
pub use wyven_voxel::WorldGenerator;
