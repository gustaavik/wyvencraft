//! The voxel world: block definitions, chunk storage, procedural generation,
//! meshing, streaming, and ray-targeting.

pub mod block;
pub mod chunk;
pub mod generation;
pub mod loader;
pub mod meshing;
pub mod raycast;
#[allow(clippy::module_inception)]
pub mod world;

pub use block::{Block, BlockRegistry, RenderType};
pub use chunk::Chunk;
pub use generation::{NoiseGenerator, WorldGenerator};
pub use raycast::{raycast, RaycastHit};
pub use world::World;
