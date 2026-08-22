//! Turning chunk voxel data into renderable triangle meshes.
//!
//! The current strategy is *face culling*: a face is emitted only when its
//! neighbour does not hide it. Greedy merging of coplanar faces is a planned
//! optimization layered on top of this same per-face logic.
//!
//! What a block *is* reaches the mesher only through
//! [`BlockCatalog`](crate::BlockCatalog) — six atlas tiles, baked geometry, a
//! placed model, or a fluid surface. It never sees a name or a hardness.

pub mod culled;
pub mod sprite;

pub use culled::{mesh_block_overlay, mesh_chunk, push_item_cube};
pub use sprite::{ItemSprite, push_item_sprite};

use std::collections::HashMap;

use wyven_model::ModelId;
use wyven_render::mesh::CpuMesh;

/// Result of meshing one chunk, split by the pipeline that draws it and then by
/// render pass, so transparent geometry (water/glass) can be drawn separately
/// and sorted.
#[derive(Default)]
pub struct ChunkMeshOutput {
    /// Cube faces sampling the shared 32-pixel atlas.
    pub opaque: CpuMesh,
    pub transparent: CpuMesh,
    /// Geometry from Blockbench-authored blocks, sampling the block texture
    /// array. One mesh for the whole chunk however many block types and
    /// textures it contains — the layer index is per vertex, so they all batch
    /// into a single draw. This is what `opaque`/`transparent` become once every
    /// block has been re-authored as a model.
    pub array_opaque: CpuMesh,
    pub array_transparent: CpuMesh,
    /// Geometry from `.bbmodel`-backed blocks, grouped by the model it samples —
    /// one mesh per model however many blocks in the chunk share it. These
    /// cannot join `opaque`: each brings its own texture and needs its own
    /// draw, exactly like the dropped-item models in `SceneCache`.
    pub models: HashMap<ModelId, CpuMesh>,
}

impl ChunkMeshOutput {
    pub fn is_empty(&self) -> bool {
        self.opaque.is_empty()
            && self.transparent.is_empty()
            && self.array_opaque.is_empty()
            && self.array_transparent.is_empty()
            && self.models.is_empty()
    }
}
