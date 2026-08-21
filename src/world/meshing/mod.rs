//! Turning chunk voxel data into renderable triangle meshes.
//!
//! The current strategy is *face culling*: a face is emitted only when its
//! neighbour does not hide it. Greedy merging of coplanar faces is a planned
//! optimization layered on top of this same per-face logic.

pub mod culled;

pub use culled::{mesh_block_overlay, mesh_chunk, push_item_cube};

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::content::FluidTexture;
use crate::core::{BlockId, Direction};
use crate::model::{Model, ModelId, ModelRegistry};
use crate::render::mesh::CpuMesh;
use crate::world::block::BlockModel;
use crate::world::blockmodel::BakedBlockModel;

/// The model geometry a chunk mesh may need: the parsed `.bbmodel`/`.gltf`
/// models plus the per-block assignment (`content::GameContent::block_models`,
/// indexed by [`BlockId`]), and the Blockbench-authored block models
/// (`content::GameContent::baked_models`, also indexed by [`BlockId`]).
///
/// Mirrors `state::ingame_state::view::ModelContent` — the mesher borrows every
/// half for one call rather than owning any.
#[derive(Clone, Copy)]
pub struct BlockModels<'a> {
    pub models: &'a ModelRegistry,
    pub blocks: &'a [Option<BlockModel>],
    /// Blocks authored as Blockbench `.json`. These supersede `blocks` for any
    /// id present in both, and are the direction all blocks are moving in.
    pub baked: &'a [Option<BakedBlockModel>],
    /// The animation strip each fluid draws from
    /// (`content::GameContent::fluid_textures`). A fluid has no model — its
    /// surface height is per-corner — so it takes its layers from here instead.
    pub fluids: &'a [Option<FluidTexture>],
}

impl<'a> BlockModels<'a> {
    /// Nothing has a model — for meshing plain cubes without a loaded registry
    /// to borrow (tests, and any caller that has no model content).
    pub fn none() -> BlockModels<'static> {
        static EMPTY: OnceLock<ModelRegistry> = OnceLock::new();
        BlockModels {
            models: EMPTY.get_or_init(ModelRegistry::new),
            blocks: &[],
            baked: &[],
            fluids: &[],
        }
    }

    /// The placement and geometry for `block`, or `None` when it is an ordinary
    /// cube or its model failed to load.
    pub fn of(&self, block: BlockId) -> Option<(&'a BlockModel, &'a Model)> {
        let placement = self.blocks.get(block.0 as usize)?.as_ref()?;
        Some((placement, self.models.get(placement.id)?))
    }

    /// The Blockbench-authored geometry for `block`, if it has any.
    pub fn baked_of(&self, block: BlockId) -> Option<&'a BakedBlockModel> {
        self.baked.get(block.0 as usize)?.as_ref()
    }

    /// The animation strip `block` draws from, if it is a fluid with one.
    pub fn fluid_of(&self, block: BlockId) -> Option<&'a FluidTexture> {
        self.fluids.get(block.0 as usize)?.as_ref()
    }

    /// Whether `block` fills the cell face pointing `dir` with an opaque
    /// texture — the modelled counterpart of `Block::is_opaque`, and what lets
    /// a modelled block hide the face of the atlas-textured cube beside it.
    pub fn occludes(&self, block: BlockId, dir: Direction) -> bool {
        self.baked_of(block)
            .is_some_and(|m| m.occludes[dir as usize])
    }
}

/// Result of meshing one chunk, split by the pipeline that draws it and then by
/// render pass, so transparent geometry (water/glass) can be drawn separately
/// and sorted.
#[derive(Default)]
pub struct ChunkMeshOutput {
    /// Cube faces sampling the shared 16-pixel atlas.
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
