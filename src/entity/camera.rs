//! Where the third-person camera actually sits.
//!
//! The desired distance is a constant, but the world gets in the way: back into
//! a wall or step under an overhang and a camera placed blindly behind the eye
//! ends up *inside* solid terrain, showing the player the underside of the
//! world. So the placement is a query, not arithmetic — trace toward the desired
//! position and stop short of the first thing in the way.
//!
//! **Eight rays, not one.** The near plane has width and height, so a block the
//! centre ray slides past can still cross a frustum corner and be clipped
//! through. Tracing from the corners of a cube around the eye and keeping the
//! nearest hit — Minecraft's own approach — costs eight short marches and
//! removes the whole class of corner cases a single ray leaves behind.
//!
//! **It snaps.** The distance is recomputed from scratch every call and holds no
//! state, which is what lets two callers in the same frame (the world camera and
//! the nameplate camera) arrive at the same answer without sharing anything.
//! Easing back out would need a smoothed field advanced exactly once per frame,
//! and the moment those two disagreed nameplates would drift off their players.
//!
//! Boundaries: pure. The world arrives as an `is_solid` closure, so none of this
//! needs a `World`, a GPU, or a session to test.

use glam::Vec3;

use crate::core::BlockPos;
use wyven_voxel::{Target, raycast};

/// The eight corners of a unit cube centred on the origin, scaled by the
/// clearance to bracket the near plane. A cube rather than the near rectangle
/// itself: it strictly contains it whichever way the camera faces, and
/// over-pulling by a few centimetres is invisible where under-pulling is the bug
/// being fixed.
const CORNERS: [Vec3; 8] = [
    Vec3::new(-1.0, -1.0, -1.0),
    Vec3::new(1.0, -1.0, -1.0),
    Vec3::new(-1.0, 1.0, -1.0),
    Vec3::new(1.0, 1.0, -1.0),
    Vec3::new(-1.0, -1.0, 1.0),
    Vec3::new(1.0, -1.0, 1.0),
    Vec3::new(-1.0, 1.0, 1.0),
    Vec3::new(1.0, 1.0, 1.0),
];

/// How far from `eye` the camera may sit along `dir`: `desired`, cut back so
/// nothing solid ends up between the camera and the player.
///
/// `dir` points from the eye *toward* the camera and need not be normalized.
/// `clearance` is how much room the camera needs in front of a surface —
/// `wyven_render::Camera::near_radius`, so the berth grows with the field of
/// view instead of being tuned for one.
///
/// The standoff falls out of the corner offsets rather than being subtracted
/// afterwards: a corner ray starts a clearance ahead of the eye, so the distance
/// at which *it* reaches a wall is already the distance at which the camera's
/// own corner would — subtracting again would pull in twice as far.
///
/// The result is never negative: an eye already inside a block collapses the
/// camera onto the eye rather than turning it inside out.
pub fn clear_distance(
    eye: Vec3,
    dir: Vec3,
    desired: f32,
    clearance: f32,
    is_solid: impl Fn(BlockPos) -> bool,
) -> f32 {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return desired;
    }

    let blocked = |p: BlockPos| is_solid(p).then_some(Target::Cell);
    let mut distance = desired;
    for corner in CORNERS {
        let origin = eye + corner * clearance;
        if let Some(hit) = raycast(origin, dir, desired, blocked) {
            distance = distance.min(hit.distance);
        }
    }
    distance.clamp(0.0, desired)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 70°/16:9 near plane, which is what the game ships with.
    const CLEARANCE: f32 = 0.174;
    const DESIRED: f32 = 4.0;

    /// Nothing is solid anywhere.
    fn empty(_: BlockPos) -> bool {
        false
    }

    /// Only the listed cells are solid.
    fn solid(cells: &[BlockPos]) -> impl Fn(BlockPos) -> bool + '_ {
        move |p| cells.contains(&p)
    }

    #[test]
    fn an_open_view_keeps_the_full_distance() {
        let d = clear_distance(Vec3::new(0.5, 0.5, 0.5), Vec3::X, DESIRED, CLEARANCE, empty);
        assert_eq!(d, DESIRED);
    }

    #[test]
    fn a_wall_pulls_the_camera_in_short_of_it() {
        let wall = [BlockPos::new(3, 0, 0)];
        let d = clear_distance(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::X,
            DESIRED,
            CLEARANCE,
            solid(&wall),
        );

        // The wall's face is 2.5 away, and the camera must stop a clearance
        // short of it so its near plane stays out of the block.
        let expected = 2.5 - CLEARANCE;
        assert!(
            (d - expected).abs() < 1.0e-4,
            "expected about {expected}, got {d}"
        );
    }

    /// The reason for eight rays. This block sits beside the line the eye
    /// travels along — a single centre ray misses it entirely — but it is well
    /// inside the near plane's corner, so it would clip through.
    #[test]
    fn a_corner_the_centre_ray_misses_still_pulls_the_camera_in() {
        // Eye at the very edge of its cell, so a +Z offset of one clearance
        // crosses into the neighbouring column while the centre ray does not.
        let eye = Vec3::new(0.5, 0.5, 0.95);
        let beside = [BlockPos::new(3, 0, 1)];

        let centre_only = raycast(eye, Vec3::X, DESIRED, |p| {
            solid(&beside)(p).then_some(Target::Cell)
        });
        assert!(
            centre_only.is_none(),
            "the centre ray must miss for this test"
        );

        let d = clear_distance(eye, Vec3::X, DESIRED, CLEARANCE, solid(&beside));
        assert!(
            d < DESIRED,
            "a corner ray should have caught the block, got {d}"
        );
    }

    /// Standing inside a block (spawned into terrain, or shoved by a mob) must
    /// not send the camera out the far side.
    #[test]
    fn an_eye_inside_a_block_collapses_onto_the_eye() {
        let here = [BlockPos::new(0, 0, 0)];
        let d = clear_distance(
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::X,
            DESIRED,
            CLEARANCE,
            solid(&here),
        );
        assert_eq!(d, 0.0);
    }

    /// A degenerate direction has nothing to trace along; it must not come back
    /// as NaN and poison the view matrix.
    #[test]
    fn a_zero_direction_is_left_alone() {
        let d = clear_distance(Vec3::ZERO, Vec3::ZERO, DESIRED, CLEARANCE, empty);
        assert_eq!(d, DESIRED);
    }
}
