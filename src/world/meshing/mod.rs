//! Turning chunk voxel data into renderable triangle meshes.
//!
//! The current strategy is *face culling*: a face is emitted only when its
//! neighbour does not hide it. Greedy merging of coplanar faces is a planned
//! optimization layered on top of this same per-face logic.

pub mod culled;

pub use culled::{mesh_block_overlay, mesh_chunk, push_item_cube};

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::core::BlockId;
use crate::model::{Model, ModelId, ModelRegistry};
use crate::render::mesh::CpuMesh;
use crate::world::block::BlockModel;

/// The model geometry a chunk mesh may need: the parsed models plus the
/// per-block assignment (`content::GameContent::block_models`, indexed by
/// [`BlockId`]).
///
/// Mirrors `state::ingame_state::view::ModelContent` — the mesher borrows both
/// halves for one call rather than owning either.
#[derive(Clone, Copy)]
pub struct BlockModels<'a> {
    pub models: &'a ModelRegistry,
    pub blocks: &'a [Option<BlockModel>],
}

impl<'a> BlockModels<'a> {
    /// Nothing has a model — for meshing plain cubes without a loaded registry
    /// to borrow (tests, and any caller that has no model content).
    pub fn none() -> BlockModels<'static> {
        static EMPTY: OnceLock<ModelRegistry> = OnceLock::new();
        BlockModels {
            models: EMPTY.get_or_init(ModelRegistry::new),
            blocks: &[],
        }
    }

    /// The placement and geometry for `block`, or `None` when it is an ordinary
    /// cube or its model failed to load.
    pub fn of(&self, block: BlockId) -> Option<(&'a BlockModel, &'a Model)> {
        let placement = self.blocks.get(block.0 as usize)?.as_ref()?;
        Some((placement, self.models.get(placement.id)?))
    }
}

/// Result of meshing one chunk, split by render pass so transparent geometry
/// (water/glass) can be drawn separately and sorted.
#[derive(Default)]
pub struct ChunkMeshOutput {
    pub opaque: CpuMesh,
    pub transparent: CpuMesh,
    /// Geometry from model-backed blocks, grouped by the model it samples —
    /// one mesh per model however many blocks in the chunk share it. These
    /// cannot join `opaque`: each brings its own texture and needs its own
    /// draw, exactly like the dropped-item models in `SceneCache`.
    pub models: HashMap<ModelId, CpuMesh>,
}

impl ChunkMeshOutput {
    pub fn is_empty(&self) -> bool {
        self.opaque.is_empty() && self.transparent.is_empty() && self.models.is_empty()
    }
}
