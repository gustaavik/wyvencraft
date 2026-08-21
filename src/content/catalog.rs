//! Wyvencraft's answer to [`BlockCatalog`] — how a `BlockId` acquires an
//! appearance.
//!
//! The engine's mesher asks four questions about a block: which pass it draws
//! in, what its six cube faces are textured with, whether it is baked geometry
//! or a placed model, and whether it is a fluid surface. Those answers live in
//! five different places on this side — the block table for the render type, and
//! four parallel tables indexed by `BlockId` for everything visual, kept off
//! `Block` because `Block` feeds `content_hash` and a purely visual difference
//! must never refuse a join.
//!
//! [`BlockAppearance`] is the one borrow that gathers them, so the mesher takes
//! a single argument and neither side has to know the other's layout.

use wyven_model::{Model, ModelRegistry};
use wyven_voxel::{
    BakedBlockModel, BlockCatalog, BlockModel, FaceTextures, FluidInfo, FluidTexture, RenderType,
};

use crate::core::BlockId;
use crate::world::BlockRegistry;

/// Every table the mesher reads, borrowed for the length of one call.
///
/// `Copy` because it is threaded through the per-chunk mesh loop; it owns
/// nothing and costs five pointers.
#[derive(Clone, Copy)]
pub struct BlockAppearance<'a> {
    /// Render type and fluid membership — the two visual facts that *are* on
    /// the block table, because both are declared in `blocks.toml`.
    pub blocks: &'a BlockRegistry,
    /// The atlas tiles each block's faces sample. Derived at load for a
    /// Blockbench-authored block, declared for every other, which is why it is
    /// a table here rather than a field on `Block`.
    pub face_tiles: &'a [Option<FaceTextures>],
    /// Parsed model files, referenced by index from `placed`.
    pub models: &'a ModelRegistry,
    /// `[block.model]` ground cover: a model file placed in the cell.
    pub placed: &'a [Option<BlockModel>],
    /// `block_model`: Blockbench geometry baked into the cell. Supersedes
    /// `placed` for any id in both.
    pub baked: &'a [Option<BakedBlockModel>],
    /// `[block.fluid.texture]`: the animation strip a fluid draws from.
    pub fluids: &'a [Option<FluidTexture>],
}

impl BlockCatalog for BlockAppearance<'_> {
    #[inline]
    fn render_type(&self, id: BlockId) -> RenderType {
        self.blocks.get(id).render
    }

    #[inline]
    fn face_textures(&self, id: BlockId) -> FaceTextures {
        self.face_tiles
            .get(id.0 as usize)
            .copied()
            .flatten()
            .unwrap_or(self.blocks.get(id).textures)
    }

    #[inline]
    fn fluid(&self, id: BlockId) -> Option<FluidInfo> {
        self.blocks.fluid(id)
    }

    #[inline]
    fn baked(&self, id: BlockId) -> Option<&BakedBlockModel> {
        self.baked.get(id.0 as usize)?.as_ref()
    }

    #[inline]
    fn placed_model(&self, id: BlockId) -> Option<(&BlockModel, &Model)> {
        let placement = self.placed.get(id.0 as usize)?.as_ref()?;
        Some((placement, self.models.get(placement.id)?))
    }

    #[inline]
    fn fluid_texture(&self, id: BlockId) -> Option<&FluidTexture> {
        self.fluids.get(id.0 as usize)?.as_ref()
    }
}
