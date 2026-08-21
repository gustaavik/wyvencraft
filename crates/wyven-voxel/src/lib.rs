//! The voxel substrate: chunk storage, background generation, face-culling
//! meshing, and ray targeting.
//!
//! This is engine, not game. A block id is an opaque `u16` here; what it means
//! arrives through two traits a game implements over its own block table —
//! [`BlockCatalog`] for appearance and [`BlockProperties`] for the predicates
//! [`World`] answers about a position. Filling chunks with blocks in the first
//! place is a [`WorldGenerator`].
//!
//! Nothing in this crate knows a block's name, hardness, drop table or tool
//! match, which is what lets the mesher be exercised with no game loaded.

pub mod appearance;
pub mod blockmodel;
pub mod catalog;
pub mod chunk;
pub mod fluid_texture;
pub mod generate;
pub mod loader;
pub mod meshing;
pub mod raycast;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
#[allow(clippy::module_inception)]
pub mod world;

pub use appearance::{BlockModel, FaceTextures, FluidInfo, RenderType, model_hitbox};
pub use blockmodel::BakedBlockModel;
pub use catalog::{BlockCatalog, BlockProperties};
pub use chunk::Chunk;
pub use fluid_texture::FluidTexture;
pub use generate::WorldGenerator;
pub use loader::ChunkLoader;
pub use meshing::{ChunkMeshOutput, mesh_block_overlay, mesh_chunk, push_item_cube};
pub use raycast::{RaycastHit, Target, raycast};
pub use world::World;
