//! Turning chunk voxel data into renderable triangle meshes.
//!
//! The current strategy is *face culling*: a face is emitted only when its
//! neighbour does not hide it. Greedy merging of coplanar faces is a planned
//! optimization layered on top of this same per-face logic.

pub mod culled;

pub use culled::{mesh_block_overlay, mesh_chunk};

use crate::render::mesh::CpuMesh;

/// Result of meshing one chunk, split by render pass so transparent geometry
/// (water/glass) can be drawn separately and sorted.
#[derive(Default)]
pub struct ChunkMeshOutput {
    pub opaque: CpuMesh,
    pub transparent: CpuMesh,
}

impl ChunkMeshOutput {
    pub fn is_empty(&self) -> bool {
        self.opaque.is_empty() && self.transparent.is_empty()
    }
}
