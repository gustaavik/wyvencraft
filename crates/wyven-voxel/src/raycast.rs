//! Voxel ray traversal (Amanatides & Woo DDA) for block targeting — what the
//! player is looking at, for breaking/placing.

use glam::Vec3;

use wyven_core::{Aabb, BlockPos, Direction};

/// What a cell offers the ray.
///
/// Most blocks fill their cell, and for those the DDA's own cell crossing *is*
/// the answer. Ground cover doesn't: a mushroom occupies a fraction of its
/// block, and a crosshair that grabbed the whole cell would target it from a
/// stride away. Those cells hand back a smaller box, which the ray can miss —
/// in which case the march simply continues past it.
#[derive(Debug, Clone, Copy)]
pub enum Target {
    /// The whole cell.
    Cell,
    /// A smaller box, in world space, somewhere inside the cell.
    Box(Aabb),
}

/// Result of a successful voxel raycast.
#[derive(Debug, Clone, Copy)]
pub struct RaycastHit {
    /// The solid block that was hit.
    pub block: BlockPos,
    /// The face of `block` that the ray entered through (points toward the ray
    /// origin). Adding this offset to `block` gives the placement position.
    pub face: Direction,
    /// Distance along `dir` at which the ray entered the target. `0.0` when the
    /// origin already sits inside it, since it crossed nothing to get there.
    pub distance: f32,
}

impl RaycastHit {
    /// Where a new block should be placed (adjacent to the hit face).
    pub fn place_position(&self) -> BlockPos {
        self.block.offset(self.face)
    }
}

/// March a ray through the voxel grid until `target` reports a cell the ray
/// actually hits, or `max_distance` (in blocks) is exceeded.
///
/// `target` answers `None` for a cell the ray passes straight through. A
/// [`Target::Box`] cell may still be missed, in which case the march continues
/// — which is what lets a crosshair slide past a mushroom to the ground behind.
pub fn raycast(
    origin: Vec3,
    dir: Vec3,
    max_distance: f32,
    target: impl Fn(BlockPos) -> Option<Target>,
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
        match target(block) {
            // A full cell is settled by the crossing the DDA already made.
            Some(Target::Cell) => {
                return Some(RaycastHit {
                    block,
                    face,
                    // The DDA stopped at this cell's own boundary, so the
                    // crossing it already made *is* the entry distance.
                    distance: traveled,
                });
            }
            Some(Target::Box(aabb)) => {
                if let Some((t, entered)) = aabb.ray_enter(origin, dir, max_distance) {
                    return Some(RaycastHit {
                        block,
                        // Starting inside the box crosses no face; keep the one
                        // the ray used to enter the cell.
                        face: entered.unwrap_or(face),
                        // The box sits somewhere inside the cell, so its own
                        // entry is further along than the cell's.
                        distance: t,
                    });
                }
                // Missed the box — keep marching past it.
            }
            None => {}
        }

        // Advance along the axis with the nearest boundary.
        if t_max.x < t_max.y && t_max.x < t_max.z {
            block.x += step[0];
            traveled = t_max.x;
            t_max.x += t_delta.x;
            face = if step[0] > 0 {
                Direction::NegX
            } else {
                Direction::PosX
            };
        } else if t_max.y < t_max.z {
            block.y += step[1];
            traveled = t_max.y;
            t_max.y += t_delta.y;
            face = if step[1] > 0 {
                Direction::NegY
            } else {
                Direction::PosY
            };
        } else {
            block.z += step[2];
            traveled = t_max.z;
            t_max.z += t_delta.z;
            face = if step[2] > 0 {
                Direction::NegZ
            } else {
                Direction::PosZ
            };
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell-filling block at `pos`, everything else empty.
    fn only(pos: BlockPos) -> impl Fn(BlockPos) -> Option<Target> {
        move |p| (p == pos).then_some(Target::Cell)
    }

    /// The distance is what lets a caller stop *short* of what it hit — the
    /// third-person camera pulls itself in by it. Without it the only way back
    /// to a length is to re-derive the entry plane from `block` and `face`.
    #[test]
    fn a_hit_reports_how_far_the_ray_travelled() {
        let target = BlockPos::new(4, 0, 0);
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 8.0, only(target)).expect("hit");
        assert!(
            (hit.distance - 3.5).abs() < 1.0e-5,
            "entered the cell at x = 4 from x = 0.5, so 3.5; got {}",
            hit.distance
        );
    }

    /// A ray that starts inside its target crossed nothing to get there.
    #[test]
    fn a_hit_in_the_origins_own_cell_is_at_zero_distance() {
        let here = BlockPos::new(0, 0, 0);
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 8.0, only(here)).expect("hit");
        assert_eq!(hit.distance, 0.0);
    }

    #[test]
    fn a_full_cell_is_hit_on_the_face_the_ray_crossed() {
        let target = BlockPos::new(4, 0, 0);
        let hit = raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 8.0, only(target)).expect("hit");
        assert_eq!(hit.block, target);
        assert_eq!(hit.face, Direction::NegX, "entered through the -X face");
        assert_eq!(hit.place_position(), BlockPos::new(3, 0, 0));
    }

    #[test]
    fn nothing_in_range_is_a_miss() {
        let far = only(BlockPos::new(40, 0, 0));
        assert!(raycast(Vec3::new(0.5, 0.5, 0.5), Vec3::X, 5.0, far).is_none());
    }

    /// The point of `Target::Box`: a ray aimed over a short plant carries on to
    /// whatever stands behind it instead of stopping at its cell.
    #[test]
    fn a_ray_that_misses_a_small_box_keeps_marching() {
        let plant = BlockPos::new(2, 0, 0);
        let wall = BlockPos::new(6, 0, 0);
        // A stubby box hugging the bottom of the plant's cell.
        let boxes = move |p: BlockPos| {
            if p == plant {
                Some(Target::Box(Aabb::new(
                    Vec3::new(2.4, 0.0, -0.1),
                    Vec3::new(2.6, 0.3, 0.1),
                )))
            } else if p == wall {
                Some(Target::Cell)
            } else {
                None
            }
        };
        // Eye level 0.5: over the plant's 0.3-tall box, so it reaches the wall.
        let over = raycast(Vec3::new(0.0, 0.5, 0.0), Vec3::X, 10.0, boxes).expect("hit");
        assert_eq!(over.block, wall, "slid over the plant");

        // Aimed low, the same ray stops at the plant.
        let into = raycast(Vec3::new(0.0, 0.15, 0.0), Vec3::X, 10.0, boxes).expect("hit");
        assert_eq!(into.block, plant);
        assert_eq!(into.face, Direction::NegX, "entered the box's -X face");
        assert!(
            (into.distance - 2.4).abs() < 1.0e-5,
            "the box's own front face at 2.4, not the cell boundary at 2.0; got {}",
            into.distance
        );
    }

    /// A box hit reports the face of the *box*, which is what a placed block
    /// gets stacked against — and for a plant standing on the ground, looking
    /// down at it must not place into the ground.
    #[test]
    fn a_box_hit_reports_the_box_face_not_the_cell_face() {
        let plant = BlockPos::new(0, 4, 0);
        let boxes = move |p: BlockPos| {
            (p == plant).then_some(Target::Box(Aabb::new(
                Vec3::new(0.4, 4.0, 0.4),
                Vec3::new(0.6, 4.5, 0.6),
            )))
        };
        // Straight down from above: enters through the box's top.
        let hit = raycast(Vec3::new(0.5, 8.0, 0.5), Vec3::NEG_Y, 10.0, boxes).expect("hit");
        assert_eq!(hit.block, plant);
        assert_eq!(hit.face, Direction::PosY);
        assert_eq!(hit.place_position(), BlockPos::new(0, 5, 0));
    }
}
