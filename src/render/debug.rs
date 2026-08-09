//! Debug-line rendering: block selection outline and wireframes.
//!
//! Builds [`LineVertex`] geometry that the debug pipeline draws with
//! `PrimitiveTopology::LineList`. Wired into the renderer in M4 (selection box).

use glam::Vec3;

use super::vertex::LineVertex;
use crate::core::Aabb;

/// How far the outline box is inflated beyond the block, so its edges (which
/// are coplanar with the block's faces) don't z-fight with them.
const OUTLINE_INFLATE: f32 = 0.002;

/// Append the 12 edges of `box_`, slightly inflated, to `out` in the given
/// colour. Callers pass the block's targeting box, so the outline hugs a
/// mushroom rather than the cell it stands in.
pub fn push_block_outline(out: &mut Vec<LineVertex>, box_: Aabb, color: [f32; 3]) {
    let min = box_.min - Vec3::splat(OUTLINE_INFLATE);
    let max = box_.max + Vec3::splat(OUTLINE_INFLATE);
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
