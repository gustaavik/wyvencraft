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

use glam::{Vec2, Vec3};

use crate::core::BlockPos;
use wyven_core::wrap_angle;
use wyven_render::Camera;
use wyven_voxel::{Target, raycast};

/// Where a camera sits relative to its subject, in the subject's own frame.
///
/// Polar rather than Cartesian, because the only interesting thing to do with
/// two of these is blend between them — and a straight lerp between "behind the
/// player" and "in front of the player" passes through their head at the
/// halfway point, while an azimuth lerp orbits around them.
///
/// Every gameplay perspective *and* the inventory's framing shot are values of
/// this one type, so the placement and the clearance trace that limits it are
/// still derived exactly once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    /// Radians about +Y from the subject's facing: 0 is in front of them
    /// (looking back at their face), PI is behind them.
    pub azimuth: f32,
    /// Radians above the horizon. Positive puts the camera up, looking down.
    pub elevation: f32,
    /// Desired distance from the eye, before the world is consulted.
    pub distance: f32,
    /// Height of the aim point relative to the eye. `0.0` looks straight back
    /// at the eye, which is what every gameplay perspective wants.
    pub aim: f32,
    /// Horizontal lens shift, in NDC — see [`Camera::projection_offset`].
    pub shift: f32,
}

/// The vertical extent, in blocks, the inspect shot frames around the player.
const INSPECT_FRAME_HEIGHT: f32 = 2.4;
/// The fraction of the screen's height that extent fills.
const INSPECT_FILL: f32 = 0.66;
/// Never closer than this, however wide the field of view.
const INSPECT_MIN_DISTANCE: f32 = 2.0;
/// Radians above the eye's horizon, so the shot looks very slightly down.
const INSPECT_ELEVATION: f32 = 0.10;
/// Aim point relative to the eye: the chest, so the feet stay in frame.
const INSPECT_AIM: f32 = -0.55;

impl Shot {
    /// The horizontal forward and right vectors for a subject facing `yaw`.
    fn basis(yaw: f32) -> (Vec3, Vec3) {
        let (sy, cy) = yaw.sin_cos();
        (Vec3::new(sy, 0.0, -cy), Vec3::new(cy, 0.0, sy))
    }

    /// Unit offset from the eye *toward* the camera, for a subject facing `yaw`.
    pub fn offset(self, yaw: f32) -> Vec3 {
        let (forward, right) = Self::basis(yaw);
        let (sa, ca) = self.azimuth.sin_cos();
        let (se, ce) = self.elevation.sin_cos();
        ce * (ca * forward + sa * right) + se * Vec3::Y
    }

    /// Shortest-arc blend toward `other`. `t` outside `0..=1` is clamped.
    pub fn blend(self, other: Shot, t: f32) -> Shot {
        let t = t.clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        Shot {
            // Around the short way, so a swing from behind to in front orbits
            // rather than sweeping through whichever side the numbers happen
            // to name.
            azimuth: self.azimuth + wrap_angle(other.azimuth - self.azimuth) * t,
            elevation: lerp(self.elevation, other.elevation),
            distance: lerp(self.distance, other.distance),
            aim: lerp(self.aim, other.aim),
            shift: lerp(self.shift, other.shift),
        }
    }

    /// Resolve to a camera. `distance` is what the world actually allowed —
    /// [`clear_distance`] along [`Shot::offset`] with `self.distance` desired.
    pub fn camera(
        self,
        eye: Vec3,
        yaw: f32,
        distance: f32,
        fov_degrees: f32,
        aspect: f32,
    ) -> Camera {
        let offset = self.offset(yaw);
        let to_camera = offset * distance;
        let mut camera = Camera::new(fov_degrees, aspect);
        camera.position = eye + to_camera;
        // Look back at the aim point. With `aim == 0` this is exactly
        // `-offset` for any positive distance; the fallback covers first
        // person, where the distance is zero and there is no vector to
        // normalize, and a clamp that collapsed the camera onto the eye.
        camera.forward = (Vec3::Y * self.aim - to_camera)
            .try_normalize()
            .unwrap_or(-offset);
        camera.projection_offset = Vec2::new(self.shift, 0.0);
        camera
    }

    /// The inventory's framing shot: dead-on the subject's front, slightly
    /// raised, aimed at the chest, with the image slid so they sit at
    /// `stage_center_x` (a fraction across the screen).
    ///
    /// Deliberately takes no pitch. [`crate::entity::Perspective::ThirdFront`]
    /// inherits the player's, which would send this camera underground the
    /// moment the inventory was opened while looking up.
    ///
    /// The distance is derived from the field of view rather than fixed,
    /// because the field of view is a player setting: a constant tuned for 70°
    /// renders a postage stamp at 110°. Fixing the *framing* instead is the
    /// same reasoning [`Camera::near_radius`] uses.
    pub fn inspect(fov_y: f32, stage_center_x: f32) -> Shot {
        let distance = ((INSPECT_FRAME_HEIGHT / (2.0 * INSPECT_FILL)) / (fov_y * 0.5).tan())
            .max(INSPECT_MIN_DISTANCE);
        Shot {
            azimuth: 0.0,
            elevation: INSPECT_ELEVATION,
            distance,
            aim: INSPECT_AIM,
            // NDC spans -1..1 across the screen, so a fraction of the width
            // converts straight over — and stays correct at any aspect, which
            // a tuned pixel offset would not.
            shift: 2.0 * stage_center_x - 1.0,
        }
    }
}

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

    // --- Shot -------------------------------------------------------------

    use crate::entity::Perspective;

    /// The look direction the old `camera_placement` was written against.
    fn look(yaw: f32, pitch: f32) -> Vec3 {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        Vec3::new(cp * sy, sp, -cp * cy).normalize()
    }

    /// The refactor's load-bearing claim: the polar form reproduces what
    /// `Perspective::camera_placement` used to return, exactly, everywhere.
    /// `ThirdBack` was `-look` and `ThirdFront` was `+look`; the reconstruction
    /// is trigonometric identity, not approximation, so this is tight.
    #[test]
    fn the_polar_shot_reproduces_every_perspective() {
        for yaw_step in -6..=6 {
            for pitch_step in -5..=5 {
                let yaw = yaw_step as f32 * 0.5;
                let pitch = pitch_step as f32 * 0.3;
                let look = look(yaw, pitch);

                let back = Perspective::ThirdBack.shot(pitch, DESIRED).offset(yaw);
                assert!(
                    (back + look).length() < 1e-5,
                    "third-back at yaw {yaw} pitch {pitch}: {back:?} != {:?}",
                    -look
                );

                let front = Perspective::ThirdFront.shot(pitch, DESIRED).offset(yaw);
                assert!(
                    (front - look).length() < 1e-5,
                    "third-front at yaw {yaw} pitch {pitch}: {front:?} != {look:?}"
                );
            }
        }
    }

    /// First person is `ThirdBack` at zero distance, which has to land the
    /// camera exactly on the eye looking along the look direction — what the
    /// old `None` + `unwrap_or((Vec3::ZERO, look))` pair did explicitly.
    #[test]
    fn first_person_puts_the_camera_on_the_eye() {
        let eye = Vec3::new(3.0, 70.0, -2.0);
        for pitch_step in -4..=4 {
            let pitch = pitch_step as f32 * 0.35;
            let yaw = 1.1;
            let shot = Perspective::First.shot(pitch, DESIRED);
            assert_eq!(shot.distance, 0.0);

            let camera = shot.camera(eye, yaw, 0.0, 70.0, 16.0 / 9.0);
            assert!((camera.position - eye).length() < 1e-5);
            assert!(
                (camera.forward - look(yaw, pitch)).length() < 1e-5,
                "first person looks along the look direction"
            );
        }
    }

    /// The inspect shot stands in front of the player, looking back at them.
    #[test]
    fn the_inspect_shot_looks_at_the_players_face() {
        let eye = Vec3::new(0.0, 1.62, 0.0);
        for yaw_step in -4..=4 {
            let yaw = yaw_step as f32 * 0.7;
            let facing = look(yaw, 0.0);
            let shot = Shot::inspect(70f32.to_radians(), 0.17);
            let camera = shot.camera(eye, yaw, shot.distance, 70.0, 16.0 / 9.0);

            assert!(
                (camera.position - eye).dot(facing) > 0.0,
                "the camera must stand where the player is facing"
            );
            assert!(
                camera.forward.dot(facing) < 0.0,
                "and look back at them"
            );
        }
    }

    /// It must ignore the player's pitch. `ThirdFront` inherits it, so opening
    /// the inventory while looking up would otherwise bury the camera.
    #[test]
    fn the_inspect_shot_ignores_where_the_player_is_looking() {
        let level = Shot::inspect(70f32.to_radians(), 0.17);
        // Nothing in `inspect`'s signature can carry a pitch, which is the
        // point — assert the resulting elevation is the authored one whatever
        // the player is doing.
        assert_eq!(level.elevation, INSPECT_ELEVATION);
        assert!(level.elevation.abs() < 0.5, "and is close to level");
    }

    /// The framing, not the distance, is what is held constant: a player who
    /// widens their field of view must still see a model of the same size.
    #[test]
    fn the_inspect_distance_frames_the_model_the_same_at_any_field_of_view() {
        let subtended = |fov_degrees: f32| {
            let fov = fov_degrees.to_radians();
            let shot = Shot::inspect(fov, 0.17);
            // Half-height of the view at the subject, as a fraction of the
            // framed extent.
            shot.distance * (fov * 0.5).tan()
        };
        let at_60 = subtended(60.0);
        let at_70 = subtended(70.0);
        assert!(
            (at_60 - at_70).abs() < 1e-3,
            "framing drifted: {at_60} vs {at_70}"
        );
        // A wide field of view would ask the camera closer than is comfortable,
        // so the floor takes over — a deliberate trade of framing for distance,
        // and the reason this stops being a pure inverse-tangent above ~80°.
        for wide in [90.0, 110.0] {
            assert_eq!(
                Shot::inspect(f32::to_radians(wide), 0.17).distance,
                INSPECT_MIN_DISTANCE,
                "at {wide}° the minimum distance should bind"
            );
        }
    }

    /// Why the blend is polar. A Cartesian lerp from "behind the player" to
    /// "in front of the player" passes through the player at the halfway
    /// point; an azimuth lerp orbits around them at a steady radius.
    #[test]
    fn blending_from_behind_orbits_rather_than_passing_through_the_player() {
        let yaw = 0.6;
        let back = Perspective::ThirdBack.shot(0.0, DESIRED);
        let inspect = Shot::inspect(70f32.to_radians(), 0.17);
        let floor = back.distance.min(inspect.distance) * 0.99;

        for step in 0..=50 {
            let t = step as f32 / 50.0;
            let shot = back.blend(inspect, t);
            let radius = (shot.offset(yaw) * shot.distance).length();
            assert!(
                radius >= floor,
                "at t={t} the camera closed to {radius}, inside the {floor} floor"
            );
        }
    }

    /// The lens shift is what puts the model on the left, and it is derived
    /// from a screen fraction so it holds at any aspect ratio.
    #[test]
    fn the_inspect_shot_slides_the_subject_off_centre() {
        let eye = Vec3::new(0.0, 1.62, 0.0);
        let chest = eye + Vec3::Y * INSPECT_AIM;
        for aspect in [16.0 / 9.0, 4.0 / 3.0, 21.0 / 9.0] {
            let shot = Shot::inspect(70f32.to_radians(), 0.17);
            let camera = shot.camera(eye, 0.0, shot.distance, 70.0, aspect);
            let projected = camera.project(chest).expect("the chest is in front");
            assert!(
                (0.10..0.28).contains(&projected.x),
                "at aspect {aspect} the model landed at {}, not in the left third",
                projected.x
            );
        }
    }

    /// A wall in front of the player pulls the inspect camera in, exactly as it
    /// does the third-person one — the shift is in the projection, so the
    /// camera never leaves the ray this traces along.
    #[test]
    fn a_wall_pulls_the_inspect_camera_in() {
        let eye = Vec3::new(0.5, 1.5, 0.5);
        let shot = Shot::inspect(70f32.to_radians(), 0.17);
        // Facing -Z, so the camera swings out to -Z; put a wall two cells that way.
        let wall = BlockPos::new(0, 1, -2);
        let d = clear_distance(
            eye,
            shot.offset(0.0),
            shot.distance,
            CLEARANCE,
            solid(&[wall]),
        );
        assert!(
            d < shot.distance,
            "the wall should have pulled the camera in from {}",
            shot.distance
        );
        assert!(d > 0.0, "but not all the way onto the eye");
    }
}
