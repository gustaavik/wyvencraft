//! Voxel ray traversal (Amanatides & Woo DDA) for block targeting — what the
//! player is looking at, for breaking/placing.

use glam::Vec3;

use crate::core::{BlockPos, Direction};

/// Result of a successful voxel raycast.
#[derive(Debug, Clone, Copy)]
pub struct RaycastHit {
    /// The solid block that was hit.
    pub block: BlockPos,
    /// The face of `block` that the ray entered through (points toward the ray
    /// origin). Adding this offset to `block` gives the placement position.
    pub face: Direction,
}

impl RaycastHit {
    /// Where a new block should be placed (adjacent to the hit face).
    pub fn place_position(&self) -> BlockPos {
        self.block.offset(self.face)
    }
}

/// March a ray through the voxel grid until `is_solid` reports a hit or
/// `max_distance` (in blocks) is exceeded.
pub fn raycast(
    origin: Vec3,
    dir: Vec3,
    max_distance: f32,
    is_solid: impl Fn(BlockPos) -> bool,
) -> Option<RaycastHit> {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }

    let mut block = BlockPos::from_world(origin);

    // Step direction per axis.
    let step = [
        if dir.x > 0.0 { 1 } else { -1 },
        if dir.y > 0.0 { 1 } else { -1 },
        if dir.z > 0.0 { 1 } else { -1 },
    ];

    // Distance (in t) to cross one full voxel per axis.
    let t_delta = Vec3::new(
        (1.0 / dir.x).abs(),
        (1.0 / dir.y).abs(),
        (1.0 / dir.z).abs(),
    );

    // Distance (in t) to the first voxel boundary per axis.
    let dist_to_boundary = |o: f32, d: f32, b: i32| -> f32 {
        if d > 0.0 {
            (b as f32 + 1.0 - o) / d
        } else {
            (o - b as f32) / -d
        }
    };
    let mut t_max = Vec3::new(
        if dir.x != 0.0 {
            dist_to_boundary(origin.x, dir.x, block.x)
        } else {
            f32::INFINITY
        },
        if dir.y != 0.0 {
            dist_to_boundary(origin.y, dir.y, block.y)
        } else {
            f32::INFINITY
        },
        if dir.z != 0.0 {
            dist_to_boundary(origin.z, dir.z, block.z)
        } else {
            f32::INFINITY
        },
    );

    let mut face = Direction::PosY;
    let mut traveled = 0.0;

    while traveled <= max_distance {
        if is_solid(block) {
            return Some(RaycastHit { block, face });
        }

        // Advance along the axis with the nearest boundary.
        if t_max.x < t_max.y && t_max.x < t_max.z {
            block.x += step[0];
            traveled = t_max.x;
            t_max.x += t_delta.x;
            face = if step[0] > 0 { Direction::NegX } else { Direction::PosX };
        } else if t_max.y < t_max.z {
            block.y += step[1];
            traveled = t_max.y;
            t_max.y += t_delta.y;
            face = if step[1] > 0 { Direction::NegY } else { Direction::PosY };
        } else {
            block.z += step[2];
            traveled = t_max.z;
            t_max.z += t_delta.z;
            face = if step[2] > 0 { Direction::NegZ } else { Direction::PosZ };
        }
    }

    None
}
