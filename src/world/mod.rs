//! Wyvencraft's voxel world: what its blocks *are*, how its terrain is
//! generated, and how its fluids spread.
//!
//! The substrate underneath — chunk storage, the background loader, the
//! face-culling mesher, raycasting, [`World`] itself — is [`wyven_voxel`], and
//! is re-exported here so `crate::world::Chunk` still resolves. What stays on
//! this side is everything the engine deliberately has no opinion about:
//!
//! - [`block`] — the block table: names, hardness, materials, drops, fluid
//!   components. [`BlockRegistry`] implements [`wyven_voxel::BlockProperties`],
//!   which is how a `BlockId` acquires meaning inside the engine.
//! - [`generation`] — [`NoiseGenerator`], this game's implementation of
//!   [`wyven_voxel::WorldGenerator`], plus its climate, biome and feature model.
//! - [`fluid`] — the spreading rules for `[block.fluid]` components. Levels,
//!   auto-registered flowing variants and decay are Wyvencraft's policy, not a
//!   substrate anything else would reuse.

pub mod block;
pub mod fluid;
pub mod generation;
#[cfg(test)]
mod meshing_tests;

pub use block::{Block, BlockMaterial, BlockRegistry};
pub use fluid::FluidSim;
pub use generation::{NoiseGenerator, WorldGenerator};

pub use wyven_voxel::{
    BakedBlockModel, BlockCatalog, BlockModel, BlockProperties, Chunk, ChunkLoader,
    ChunkMeshOutput, FaceTextures, FluidInfo, FluidTexture, RaycastHit, RenderType, Target, World,
    blockmodel, chunk, loader, meshing, raycast, world,
};
