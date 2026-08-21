//! Procedural world generation.
//!
//! Design: *Strategy pattern*. [`WorldGenerator`] is the swappable interface;
//! [`NoiseGenerator`] is the default implementation. Tests / debug worlds can
//! provide alternative generators (flat world, etc.) without touching callers.

pub mod biome;
pub mod config;
pub mod features;
pub mod generator;
pub mod noise;

pub use config::WorldGenConfig;
pub use generator::NoiseGenerator;

use crate::core::ChunkPos;
use crate::world::chunk::Chunk;

/// Produces chunk contents on demand. Implementations must be deterministic in
/// `(seed, pos)` so all peers generate identical terrain.
pub trait WorldGenerator: Send + Sync {
    /// The seed that defines this world.
    fn seed(&self) -> u64;
    /// Generate the full block contents for one chunk.
    fn generate(&self, pos: ChunkPos) -> Chunk;
    /// The biome colour at a world column for one of the tint sources a block
    /// model's `tintindex` names (`0` grass, `1` foliage). The mesher asks the
    /// generator rather than the chunk because biome is a function of position,
    /// not of stored data — a chunk edited by a player still has the climate its
    /// coordinates imply.
    ///
    /// White by default: a generator with no climate model tints nothing.
    fn biome_tint(&self, _x: i32, _z: i32, _index: u8) -> [u8; 4] {
        wyven_render::NO_TINT
    }
}
