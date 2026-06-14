//! Debug-line rendering: block selection outline and wireframes.
//!
//! Builds [`LineVertex`] geometry that the debug pipeline draws with
//! `PrimitiveTopology::LineList`. Wired into the renderer in M4 (selection box).

use glam::Vec3;

use super::vertex::LineVertex;
use crate::core::BlockPos;

/// Append the 12 edges of the unit cube at `block` to `out`, in the given colour.
pub fn push_block_outline(out: &mut Vec<LineVertex>, block: BlockPos, color: [f32; 3]) {
    let o = Vec3::new(block.x as f32, block.y as f32, block.z as f32);
    let c = |dx: f32, dy: f32, dz: f32| LineVertex {
        position: [o.x + dx, o.y + dy, o.z + dz],
        color,
    };
    // 8 corners.
    let corners = [
        c(0.0, 0.0, 0.0),
        c(1.0, 0.0, 0.0),
        c(1.0, 0.0, 1.0),
        c(0.0, 0.0, 1.0),
        c(0.0, 1.0, 0.0),
        c(1.0, 1.0, 0.0),
        c(1.0, 1.0, 1.0),
        c(0.0, 1.0, 1.0),
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
