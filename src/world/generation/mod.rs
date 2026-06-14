//! Procedural world generation.
//!
//! Design: *Strategy pattern*. [`WorldGenerator`] is the swappable interface;
//! [`NoiseGenerator`] is the default implementation. Tests / debug worlds can
//! provide alternative generators (flat world, etc.) without touching callers.

pub mod biome;
pub mod generator;
pub mod noise;

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
}
