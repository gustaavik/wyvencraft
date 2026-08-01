//! Swept-AABB collision against the voxel grid.
//!
//! Movement is resolved one axis at a time (Y, then X, then Z) so an entity
//! slides along walls and lands cleanly on the ground without tunnelling.

use glam::{BVec3, Vec3};

use crate::core::{Aabb, BlockPos};

/// Small gap kept between the entity box and blocks to avoid floating-point
/// sticking on surfaces.
const SKIN: f32 = 1.0e-3;

pub struct CollisionResult {
    /// The actual (clamped) movement applied.
    pub delta: Vec3,
    /// True if the entity is resting on solid ground after the move.
    pub on_ground: bool,
    /// True if upward motion was stopped by a block overhead. Callers must zero
    /// their vertical velocity on this, or it keeps pushing into the ceiling.
    pub hit_ceiling: bool,
    /// Per-axis: motion along that axis was clamped short by a block.
    pub blocked: BVec3,
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

    // `sweep_axis` returns the requested amount untouched unless it hit
    // something, so "moved less far than asked" is an exact contact test.
    let blocked = BVec3::new(
        dx.abs() < velocity.x.abs() - SKIN,
        dy.abs() < velocity.y.abs() - SKIN,
        dz.abs() < velocity.z.abs() - SKIN,
    );
    // We were moving down but got stopped short -> standing on ground; moving
    // up but stopped short -> head against a block.
    let on_ground = velocity.y < 0.0 && blocked.y;
    let hit_ceiling = velocity.y > 0.0 && blocked.y;

    CollisionResult {
        delta: Vec3::new(dx, dy, dz),
        on_ground,
        hit_ceiling,
        blocked,
    }
}

/// Clamp a single-axis movement so the box doesn't enter a solid block.
fn sweep_axis(aabb: Aabb, amount: f32, axis: usize, is_solid: &impl Fn(BlockPos) -> bool) -> f32 {
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
        (
            (max[axis]).floor() as i32,
            (max[axis] + amount).floor() as i32,
        )
    } else {
        (
            (min[axis] + amount).floor() as i32,
            (min[axis]).floor() as i32,
        )
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
                    if contact >= -SKIN && contact < result {
                        result = contact;
                    }
                } else {
                    let contact = (a as f32 + 1.0) - min[axis] + SKIN;
                    if contact <= SKIN && contact > result {
                        result = contact;
                    }
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Aabb;
    use glam::Vec3;

    #[test]
    fn standing_flush_on_block_does_not_sink() {
        // Solid ground fills y < 65; player feet flush at the integer top y = 65.0.
        let solid = |p: BlockPos| p.y < 65;
        let aabb = Aabb::new(Vec3::new(0.2, 65.0, 0.2), Vec3::new(0.8, 66.8, 0.8));
        let r = move_and_collide(aabb, Vec3::new(0.0, -0.07, 0.0), solid);
        assert!(
            r.delta.y > -SKIN,
            "player sank into the block: {}",
            r.delta.y
        );
        assert!(
            r.on_ground,
            "player should be grounded when flush on a block"
        );
        assert!(!r.hit_ceiling, "falling is not a ceiling hit");
    }

    #[test]
    fn rising_into_a_block_reports_a_ceiling_hit() {
        // Open air below y = 67, solid ceiling from y = 67 up. The 1.8-tall box
        // starts with 0.2 of headroom and is asked to rise a full block.
        let solid = |p: BlockPos| p.y >= 67;
        let aabb = Aabb::new(Vec3::new(0.2, 65.0, 0.2), Vec3::new(0.8, 66.8, 0.8));
        let r = move_and_collide(aabb, Vec3::new(0.0, 1.0, 0.0), solid);
        assert!(r.hit_ceiling, "upward motion was clamped by the ceiling");
        assert!(!r.on_ground, "rising into a ceiling is not landing");
        assert!(r.blocked.y, "the Y axis was blocked");
        assert!(
            r.delta.y < 0.21,
            "cannot rise past the ceiling: {}",
            r.delta.y
        );
    }

    #[test]
    fn sliding_along_a_wall_reports_the_blocked_axis() {
        // Wall at x >= 1: horizontal motion into it is clamped, Z is free.
        let solid = |p: BlockPos| p.x >= 1;
        let aabb = Aabb::new(Vec3::new(0.2, 65.0, 0.2), Vec3::new(0.8, 66.8, 0.8));
        let r = move_and_collide(aabb, Vec3::new(0.5, 0.0, 0.5), solid);
        assert!(r.blocked.x, "X should be stopped by the wall");
        assert!(!r.blocked.z, "Z should slide freely");
        assert!(!r.hit_ceiling && !r.on_ground);
        assert!((r.delta.z - 0.5).abs() < 1e-6, "Z slid: {}", r.delta.z);
    }
}
