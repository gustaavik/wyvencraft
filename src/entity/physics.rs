//! Swept-AABB collision against the voxel grid.
//!
//! Movement is resolved one axis at a time (Y, then X, then Z) so an entity
//! slides along walls and lands cleanly on the ground without tunnelling.

use glam::Vec3;

use crate::core::{Aabb, BlockPos};

/// Small gap kept between the entity box and blocks to avoid floating-point
/// sticking on surfaces.
const SKIN: f32 = 1.0e-3;

pub struct CollisionResult {
    /// The actual (clamped) movement applied.
    pub delta: Vec3,
    /// True if the entity is resting on solid ground after the move.
    pub on_ground: bool,
}

/// Move `aabb` by `velocity`, clamping against solid blocks reported by
/// `is_solid`. Returns the clamped delta and ground contact.
pub fn move_and_collide(
    aabb: Aabb,
    velocity: Vec3,
    is_solid: impl Fn(BlockPos) -> bool,
) -> CollisionResult {
    let mut current = aabb;

    let dy = sweep_axis(current, velocity.y, 1, &is_solid);
    current = current.translate(Vec3::new(0.0, dy, 0.0));

    let dx = sweep_axis(current, velocity.x, 0, &is_solid);
    current = current.translate(Vec3::new(dx, 0.0, 0.0));

    let dz = sweep_axis(current, velocity.z, 2, &is_solid);

    // We were moving down but got stopped short -> standing on ground.
    let on_ground = velocity.y < 0.0 && dy > velocity.y + SKIN;

    CollisionResult {
        delta: Vec3::new(dx, dy, dz),
        on_ground,
    }
}

/// Clamp a single-axis movement so the box doesn't enter a solid block.
fn sweep_axis(
    aabb: Aabb,
    amount: f32,
    axis: usize,
    is_solid: &impl Fn(BlockPos) -> bool,
) -> f32 {
    if amount == 0.0 {
        return 0.0;
    }

    let min = aabb.min.to_array();
    let max = aabb.max.to_array();

    // Range of blocks to test on the two non-moving axes.
    let other = [(axis + 1) % 3, (axis + 2) % 3];
    let mut o_min = [0i32; 2];
    let mut o_max = [0i32; 2];
    for (i, &a) in other.iter().enumerate() {
        o_min[i] = (min[a] + SKIN).floor() as i32;
        o_max[i] = (max[a] - SKIN).ceil() as i32 - 1;
    }

    // Block range along the moving axis, covering the swept volume.
    let (lo, hi) = if amount > 0.0 {
        ((max[axis]).floor() as i32, (max[axis] + amount).floor() as i32)
    } else {
        ((min[axis] + amount).floor() as i32, (min[axis]).floor() as i32)
    };

    let mut result = amount;
    for a in lo..=hi {
        for u in o_min[0]..=o_max[0] {
            for v in o_min[1]..=o_max[1] {
                let mut coord = [0i32; 3];
                coord[axis] = a;
                coord[other[0]] = u;
                coord[other[1]] = v;
                let pos = BlockPos::new(coord[0], coord[1], coord[2]);
                if !is_solid(pos) {
                    continue;
                }
                if amount > 0.0 {
                    let contact = a as f32 - max[axis] - SKIN;
                    if contact >= 0.0 && contact < result {
                        result = contact;
                    }
                } else {
                    let contact = (a as f32 + 1.0) - min[axis] + SKIN;
                    if contact <= 0.0 && contact > result {
                        result = contact;
                    }
                }
            }
        }
    }
    result
}
