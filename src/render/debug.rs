//! Debug-line rendering: block selection outline and wireframes.
//!
//! Builds [`LineVertex`] geometry that the debug pipeline draws with
//! `PrimitiveTopology::LineList`. Wired into the renderer in M4 (selection box).

use glam::Vec3;

use super::vertex::LineVertex;
use crate::core::BlockPos;

/// How far the outline box is inflated beyond the block, so its edges (which
/// are coplanar with the block's faces) don't z-fight with them.
const OUTLINE_INFLATE: f32 = 0.002;

/// Append the 12 edges of the (slightly inflated) unit cube at `block` to
/// `out`, in the given colour.
pub fn push_block_outline(out: &mut Vec<LineVertex>, block: BlockPos, color: [f32; 3]) {
    let o = Vec3::new(block.x as f32, block.y as f32, block.z as f32);
    let min = o - Vec3::splat(OUTLINE_INFLATE);
    let max = o + Vec3::splat(1.0 + OUTLINE_INFLATE);
    let c = |x: f32, y: f32, z: f32| LineVertex {
        position: [x, y, z],
        color,
    };
    // 8 corners.
    let corners = [
        c(min.x, min.y, min.z),
        c(max.x, min.y, min.z),
        c(max.x, min.y, max.z),
        c(min.x, min.y, max.z),
        c(min.x, max.y, min.z),
        c(max.x, max.y, min.z),
        c(max.x, max.y, max.z),
        c(min.x, max.y, max.z),
    ];
    // 12 edges as pairs of corner indices.
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0), // bottom
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4), // top
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7), // verticals
    ];
    for (a, b) in EDGES {
        out.push(corners[a]);
        out.push(corners[b]);
    }
}
