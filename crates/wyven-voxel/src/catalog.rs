//! What the voxel layer needs to know about a block, and nothing more.
//!
//! A block id is an opaque `u16` to this crate. Two traits give it meaning, and
//! a game implements them over whatever block table it actually has:
//!
//! - [`BlockCatalog`] — appearance, for the mesher. Six atlas tiles, or baked
//!   geometry, or a placed model, or a fluid surface.
//! - [`BlockProperties`] — the handful of predicates [`World`](crate::World)
//!   answers about a *position*: can you walk through it, can you click it, does
//!   placing into it replace what is there.
//!
//! Neither carries a name, a hardness, a drop table or a tool match. Those are
//! rules; the voxel layer has no use for them, and keeping them out is what lets
//! the mesher be tested with no game loaded at all.

use wyven_core::{BlockId, Direction};
use wyven_model::Model;

use crate::appearance::{BlockModel, FaceTextures, FluidInfo, RenderType};
use crate::blockmodel::BakedBlockModel;
use crate::fluid_texture::FluidTexture;

/// How each block id is drawn.
///
/// Implemented over static tables, so every method is a bounds-checked index —
/// which is why the mesher takes `&impl BlockCatalog` and not `&dyn`: this is
/// the per-voxel hot path, and monomorphising it back to a direct load is free.
pub trait BlockCatalog {
    /// Which pass the block draws in, and whether it draws at all.
    fn render_type(&self, id: BlockId) -> RenderType;

    /// The six atlas tiles a plain cube draws with. Ignored for a block that
    /// answers [`baked`](Self::baked) or [`placed_model`](Self::placed_model).
    fn face_textures(&self, id: BlockId) -> FaceTextures;

    /// The fluid cell this block is, if it is one.
    fn fluid(&self, id: BlockId) -> Option<FluidInfo>;

    /// Geometry authored as a Blockbench block model and baked into the cell.
    /// Supersedes the cube faces for any block that has it.
    fn baked(&self, id: BlockId) -> Option<&BakedBlockModel>;

    /// A model file placed in the cell (ground cover, props). The block then
    /// emits no cube faces at all.
    fn placed_model(&self, id: BlockId) -> Option<(&BlockModel, &Model)>;

    /// The animation strip a fluid block draws from.
    fn fluid_texture(&self, id: BlockId) -> Option<&FluidTexture>;

    #[inline]
    fn is_opaque(&self, id: BlockId) -> bool {
        matches!(self.render_type(id), RenderType::Opaque)
    }

    #[inline]
    fn is_visible(&self, id: BlockId) -> bool {
        !matches!(self.render_type(id), RenderType::Invisible)
    }

    #[inline]
    fn is_transparent(&self, id: BlockId) -> bool {
        matches!(self.render_type(id), RenderType::Transparent)
    }

    #[inline]
    fn is_cutout(&self, id: BlockId) -> bool {
        matches!(self.render_type(id), RenderType::Cutout)
    }

    #[inline]
    fn is_fluid(&self, id: BlockId) -> bool {
        self.fluid(id).is_some()
    }

    /// Whether `id` fills the cell face pointing `dir` with opaque texels — the
    /// modelled counterpart of [`is_opaque`](Self::is_opaque), and what lets a
    /// baked block hide the face of the cube beside it.
    #[inline]
    fn occludes(&self, id: BlockId, dir: Direction) -> bool {
        self.baked(id).is_some_and(|m| m.occludes[dir as usize])
    }
}

/// The predicates [`World`](crate::World) answers about a block at a position.
///
/// Shared behind an `Arc` and read from the physics and streaming threads, hence
/// `Send + Sync`.
pub trait BlockProperties: Send + Sync {
    /// Whether entities collide with it.
    fn is_solid(&self, id: BlockId) -> bool;

    /// Whether the crosshair can select it.
    ///
    /// Deliberately wider than [`is_solid`](Self::is_solid): decoration you can
    /// walk through must still be breakable.
    fn is_targetable(&self, id: BlockId) -> bool;

    /// Whether placing a block into this cell swallows what is there rather
    /// than stacking on its face.
    fn is_replaceable(&self, id: BlockId) -> bool;
}
