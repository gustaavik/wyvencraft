//! The voxel world: block definitions, chunk storage, procedural generation,
//! meshing, streaming, and ray-targeting.

pub mod block;
pub mod blockmodel;
pub mod chunk;
pub mod fluid;
pub mod generation;
pub mod loader;
pub mod meshing;
pub mod raycast;
#[allow(clippy::module_inception)]
pub mod world;

pub use block::{Block, BlockMaterial, BlockRegistry, RenderType};
pub use chunk::Chunk;
pub use fluid::FluidSim;
pub use generation::{NoiseGenerator, WorldGenerator};
pub use loader::ChunkLoader;
pub use raycast::{RaycastHit, Target, raycast};
pub use world::World;
