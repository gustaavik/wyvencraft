//! What a block *looks like*, as far as meshing is concerned.
//!
//! This is the vocabulary the mesher speaks and a [`BlockCatalog`] answers in.
//! Deliberately separate from whatever a game's own block table holds: a name, a
//! hardness, what it drops and whether a pickaxe helps are rules, and the mesher
//! has no use for any of them.
//!
//! [`BlockCatalog`]: crate::BlockCatalog

use glam::Vec3;

use wyven_core::{Aabb, Direction};
use wyven_model::ModelId;

/// How a block participates in meshing/rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderType {
    /// Not drawn at all (air).
    Invisible,
    /// Fully opaque cube; hides neighbouring faces.
    Opaque,
    /// See-through cube (glass/water); does not hide neighbours of other
    /// block types and is drawn in the transparent pass.
    Transparent,
    /// Alpha-tested cube (leaves): texture is either fully opaque or fully
    /// clear per texel, so it draws in the opaque pass with depth writes (the
    /// shader discards clear texels). Avoids the blend-order artifacts of the
    /// unsorted transparent pass. Does not hide neighbours of other types.
    Cutout,
}

/// Per-face atlas tile indices, ordered by [`Direction`] (`-X,+X,-Y,+Y,-Z,+Z`).
#[derive(Debug, Clone, Copy)]
pub struct FaceTextures(pub [u32; 6]);

impl FaceTextures {
    /// Same tile on every face.
    pub const fn uniform(tile: u32) -> Self {
        Self([tile; 6])
    }

    /// Distinct top / bottom / side tiles (the common grass/log case).
    pub const fn column(top: u32, bottom: u32, side: u32) -> Self {
        // order: -X,+X,-Y,+Y,-Z,+Z  =>  side,side,bottom,top,side,side
        Self([side, side, bottom, top, side, side])
    }

    #[inline]
    pub fn tile(&self, dir: Direction) -> u32 {
        self.0[dir as usize]
    }
}

/// Fluid behavior component: marks a block as part of a level-based fluid.
///
/// A fluid is declared on its source block (`[block.fluid]` with
/// `flow_levels = N`); the loader then auto-registers one flowing block per
/// level `1..=N`, and the source carries level `N + 1`. Spreading and
/// receding between those blocks is the game's simulation; all the mesher
/// needs is the level, which sets the rendered surface height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidInfo {
    /// Which fluid this block belongs to (ordinal among fluid sources).
    pub group: u16,
    /// This block's level: `max_level` for the source, decaying to 1.
    pub level: u8,
    /// The source's level (`flow_levels + 1`); the scale for surface heights.
    pub max_level: u8,
}

impl FluidInfo {
    /// Sources are permanent until replaced; flowing blocks re-evaluate.
    #[inline]
    pub fn is_source(&self) -> bool {
        self.level == self.max_level
    }
}

/// Geometry loaded from a model file instead of the six atlas-textured cube
/// faces. A block carrying one is meshed by baking the model into its cell and
/// emits no cube faces at all.
///
/// Kept out of the game's own block table on purpose: two players whose
/// flowers are drawn slightly differently have no reason to be refused a
/// shared world, so nothing derived from a model may feed a content hash.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockModel {
    pub id: ModelId,
    pub scale: f32,
    pub rotation: Vec3,
    pub offset: Vec3,
    /// Turn each instance by a hash of its position, so a field of plants does
    /// not read as a grid of clones.
    pub random_yaw: bool,
    /// What the crosshair actually hits, in block-local `0..1` coordinates —
    /// measured from the model rather than authored, so it always matches what
    /// is drawn. A mushroom filling a fifth of its cell should not be
    /// targetable from the far corner of that cell.
    pub hitbox: Aabb,
}

/// The tightest sensible targeting box for a model occupying `bounds` (in
/// block-local `0..1` space, the model already placed).
///
/// Square in plan and centred, because `random_yaw` turns each instance and a
/// box that hugged one orientation would be wrong for every other. Clamped into
/// the cell, and never smaller than [`MIN_HITBOX`] so a flat or degenerate model
/// stays clickable.
pub fn model_hitbox(bounds: (Vec3, Vec3)) -> Aabb {
    /// Floor on either dimension of a derived hitbox, in blocks.
    const MIN_HITBOX: f32 = 0.1;

    let (lo, hi) = bounds;
    let radius = [lo.x - 0.5, hi.x - 0.5, lo.z - 0.5, hi.z - 0.5]
        .into_iter()
        .fold(0.0f32, |r, d| r.max(d.abs()))
        .clamp(MIN_HITBOX * 0.5, 0.5);
    // Leave room for `MIN_HITBOX` above the floor, so a model with no vertical
    // extent at all — a single horizontal face — still gets a targetable box
    // instead of an inverted one.
    let bottom = lo.y.clamp(0.0, 1.0 - MIN_HITBOX);
    let top = hi.y.clamp(bottom + MIN_HITBOX, 1.0);
    Aabb::new(
        Vec3::new(0.5 - radius, bottom, 0.5 - radius),
        Vec3::new(0.5 + radius, top, 0.5 + radius),
    )
}
