//! The first-person view model: the player's own arm, and whatever it holds.
//!
//! In third person the hand is a consequence of the body —
//! [`HumanoidModel::hand_anchor`] finds the fist at the end of a swinging arm,
//! and the held item hangs off it. First person has no body to hang off: the
//! camera *is* the head, and the arm has to be placed against it directly.
//!
//! **Hand space is Minecraft's.** The offset below is the one vanilla uses, and
//! a `display` entry's numbers are measured against exactly this frame — which
//! is what lets an author drag a model into place in Blockbench's "first person
//! right" preview and have it land here unchanged. Retuning [`HAND_OFFSET`]
//! would silently invalidate every model authored that way; retune the model.
//!
//! The arm and the item hang off **one** frame, unlike Minecraft, which drives
//! them with two independent chains and relies on the constants agreeing. Here
//! they cannot come apart: a swing moves the frame, and both ride it.
//!
//! Boundaries: pure. No GPU, no camera object, no player — a [`HandPose`] is a
//! plain value, so all of this is testable without a Vulkan device.

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Vec3};

use crate::art::skin::{self, SkinPart};
use crate::entity::model::{BoxPlacement, HumanoidModel, ModelBox, push_box_with};
use wyven_model::mesh as model_mesh;
use wyven_render::mesh::CpuMesh;

/// Where the main hand sits in camera space: right, a little below the eye, and
/// forward. Minecraft's own offset — see the module docs on why it is not a
/// tuning knob.
const HAND_OFFSET: Vec3 = Vec3::new(0.56, -0.52, -0.72);

/// The field of view the view model is drawn at, fixed rather than taken from
/// the player's world setting: a wide FOV should widen the *world*, not stretch
/// the player's own arm across the screen.
pub const HAND_FOV_DEGREES: f32 = 70.0;

/// How the arm is turned about its fist so it enters frame from the lower
/// right. `ARM_PITCH` past 90° sends the arm's length (its local +Y, fist toward
/// shoulder) down and *behind* the camera; `ARM_YAW` then swings that out to the
/// player's right; `ARM_ROLL` twists the box about its own length to choose
/// which side of the skin faces the camera.
///
/// Unlike [`HAND_OFFSET`] these three *are* the tuning knobs — they place our
/// own arm box, which Minecraft's numbers say nothing about.
const ARM_PITCH: f32 = 143.0;
const ARM_YAW: f32 = 36.0;
const ARM_ROLL: f32 = 0.0;

/// Overlay-shell inflation for the sleeve, per side. Matches the value
/// `HumanoidModel::build_mesh_sheet` uses for limbs, so the first-person sleeve
/// sits exactly where the third-person one does.
const SLEEVE_INFLATE: f32 = 0.25 / 16.0;

/// How far the hand travels on each axis over one walk cycle, in blocks.
const BOB_SIDEWAYS: f32 = 0.06;
const BOB_VERTICAL: f32 = 0.04;

/// Everything the view model is posed from, as plain numbers.
#[derive(Debug, Clone, Copy)]
pub struct HandPose {
    /// The camera's position — the interpolated eye, so the hand cannot judder
    /// against a world that is being drawn a fraction of a tick ahead.
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// [`crate::entity::AnimationState::swing_progress`].
    pub swing: f32,
    pub walk_phase: f32,
    pub walk_amount: f32,
}

impl HandPose {
    /// Camera space → world for the hand. Both the arm and the held item are
    /// built from this one matrix, so an item can never detach from the fist.
    ///
    /// `model_mesh::anchor(eye, yaw, pitch)` *is* the camera basis: it sends
    /// `-Z` to `Player::look_direction` and `+X` to the player's right, which is
    /// the same frame the world is drawn in.
    pub fn frame(&self) -> Mat4 {
        model_mesh::anchor(self.eye, self.yaw, self.pitch)
            * Mat4::from_translation(HAND_OFFSET + bob(self.walk_phase, self.walk_amount))
            * swing(self.swing)
    }
}

/// The one-shot swing, applied to the whole hand.
///
/// `sqrt(t)` front-loads it: the hand snaps out and eases back, which is what
/// makes a hit read as a hit rather than as a wave. Straight from Minecraft's
/// arm-attack curve, because players read that timing as "the blow landed".
fn swing(t: f32) -> Mat4 {
    if t <= 0.0 {
        return Mat4::IDENTITY;
    }
    let root = t.sqrt();
    let out = (root * PI).sin(); // 0 → 1 → 0, peaking early
    let around = (root * TAU).sin(); // one loop: down, then up
    let thrust = (t * PI).sin();
    let late = (t * t * PI).sin(); // lags `out`, so the twist trails the reach
    Mat4::from_translation(Vec3::new(-0.30 * out, 0.40 * around, -0.40 * thrust))
        * Mat4::from_rotation_y(70f32.to_radians() * out)
        * Mat4::from_rotation_z(-20f32.to_radians() * late)
        * Mat4::from_rotation_x(-80f32.to_radians() * late)
}

/// Walk bob, in camera space.
///
/// Deliberately moves the *hand* and not the camera: bobbing the camera moves
/// the frustum and drags the crosshair off the thing you are aiming at.
fn bob(phase: f32, amount: f32) -> Vec3 {
    Vec3::new(
        -phase.sin() * BOB_SIDEWAYS * amount,
        -(phase * 2.0).cos().abs() * BOB_VERTICAL * amount,
        0.0,
    )
}

/// The arm's own placement within hand space: turned to point back out of
/// frame, then shifted so its **fist** — not its centre — sits at the origin,
/// which is where the held item is too.
fn arm_orientation() -> Mat4 {
    Mat4::from_rotation_y(ARM_YAW.to_radians())
        * Mat4::from_rotation_x(ARM_PITCH.to_radians())
        * Mat4::from_rotation_y(ARM_ROLL.to_radians())
}

/// The fist in the arm box's own model space: the far end of the arm, a little
/// inside the cuff. The same point [`HumanoidModel::hand_anchor`] uses, so the
/// first- and third-person hands hold an item in the same place.
fn fist(arm: ModelBox) -> Vec3 {
    arm.center - Vec3::new(0.0, arm.size.y * 0.5 - 1.0 / 16.0, 0.0)
}

/// Where the held item goes, given the hand `frame`: at the fist, oriented by
/// hand space, with the model's own placement applied on top by the caller.
pub fn item_anchor(frame: Mat4) -> Mat4 {
    frame
}

/// Build the player's right arm — the base box plus its sleeve overlay — in
/// world space, hanging off `frame`.
///
/// Lit as though it were fixed in front of the eye rather than turning with it:
/// see [`BoxPlacement`]. Without that the hand would pulse between bright and
/// dim as the player spun on the spot, which reads as a rendering fault.
pub fn arm_mesh(model: &HumanoidModel, frame: Mat4) -> CpuMesh {
    let arm = model.right_arm;
    let orientation = arm_orientation();
    let local = orientation * Mat4::from_translation(-fist(arm));
    let placement = BoxPlacement::new(frame * local)
        .lit_as(orientation)
        .shaded_as(orientation);

    let mut mesh = CpuMesh::new();
    push_box_with(
        &mut mesh,
        arm,
        skin::RIGHT_ARM,
        skin::SKIN_ORIGIN,
        placement,
    );
    // The sleeve: the same box grown on every side, sharing the arm's placement
    // so it stays locked to it. Its transparent texels are alpha-tested away.
    let sleeve = ModelBox {
        center: arm.center,
        size: arm.size + Vec3::splat(2.0 * SLEEVE_INFLATE),
    };
    push_box_with(
        &mut mesh,
        sleeve,
        skin::RIGHT_SLEEVE,
        skin::SKIN_ORIGIN,
        placement,
    );
    mesh
}

/// The skin parts a first-person arm is drawn from, for callers that need to
/// know the sheet is the player's own.
pub const ARM_PARTS: [SkinPart; 2] = [skin::RIGHT_ARM, skin::RIGHT_SLEEVE];

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(yaw: f32, pitch: f32) -> HandPose {
        HandPose {
            eye: Vec3::new(10.0, 70.0, -4.0),
            yaw,
            pitch,
            swing: 0.0,
            walk_phase: 0.0,
            walk_amount: 0.0,
        }
    }

    /// Two boxes — the arm and its sleeve — six faces each, four corners a face.
    #[test]
    fn the_arm_is_two_boxes() {
        let mesh = arm_mesh(&HumanoidModel::player(), pose(0.0, 0.0).frame());
        assert_eq!(mesh.vertices.len(), 2 * 6 * 4);
    }

    /// The check that cannot be made by looking at the screen without running
    /// the game: the arm is in front of the eye and to the player's right, for
    /// every direction they might be facing.
    #[test]
    fn the_arm_sits_in_front_of_the_eye_and_to_the_right() {
        let model = HumanoidModel::player();
        for &(yaw, pitch) in &[(0.0, 0.0), (1.7, 0.0), (-2.4, 0.6), (3.0, -0.9), (5.5, 0.2)] {
            let pose = pose(yaw, pitch);
            // `Player::look_direction` for this yaw/pitch, spelled out so the
            // test does not need a whole player to check a direction.
            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            let look = Vec3::new(cp * sy, sp, -cp * cy).normalize();
            let right = Vec3::new(yaw.cos(), 0.0, yaw.sin());
            let mesh = arm_mesh(&model, pose.frame());
            for vertex in &mesh.vertices {
                let offset = Vec3::from(vertex.position) - pose.eye;
                assert!(
                    offset.dot(look) > 0.0,
                    "vertex behind the eye at yaw {yaw} pitch {pitch}: {offset}"
                );
                assert!(
                    offset.dot(right) > 0.0,
                    "vertex on the wrong side at yaw {yaw} pitch {pitch}: {offset}"
                );
            }
        }
    }

    /// The arm's fist and the held item's origin are the same point, at rest and
    /// mid-swing alike. This is what stops an item drifting out of the hand.
    #[test]
    fn the_arm_and_the_item_share_a_fist() {
        let model = HumanoidModel::player();
        let arm = model.right_arm;
        for swing in [0.0, 0.25, 0.5, 0.9] {
            let pose = HandPose {
                swing,
                ..pose(0.8, -0.2)
            };
            let frame = pose.frame();
            // Where the arm mesh puts its fist...
            let local = arm_orientation() * Mat4::from_translation(-fist(arm));
            let arm_fist = (frame * local).transform_point3(fist(arm));
            // ...and where a held model is anchored.
            let held = item_anchor(frame).transform_point3(Vec3::ZERO);
            assert!(
                arm_fist.abs_diff_eq(held, 1e-5),
                "swing {swing}: fist {arm_fist} vs item {held}"
            );
        }
    }

    /// The hand rides the camera, so turning must not change its shading.
    #[test]
    fn turning_does_not_change_the_arms_lighting() {
        let model = HumanoidModel::player();
        let a = arm_mesh(&model, pose(0.0, 0.0).frame());
        let b = arm_mesh(&model, pose(2.9, 0.4).frame());
        for (x, y) in a.vertices.iter().zip(&b.vertices) {
            assert_eq!(x.normal, y.normal);
            assert_eq!(x.ao, y.ao);
        }
    }

    #[test]
    fn a_rested_hand_neither_swings_nor_bobs() {
        assert_eq!(swing(0.0), Mat4::IDENTITY);
        assert_eq!(bob(1.2, 0.0), Vec3::ZERO);
    }

    /// A swing must return the hand to where it started, or the view model
    /// would creep away over a session of mining.
    #[test]
    fn a_finished_swing_returns_the_hand() {
        let rest = pose(0.0, 0.0).frame();
        let ended = HandPose {
            swing: 1.0,
            ..pose(0.0, 0.0)
        }
        .frame();
        assert!(rest.abs_diff_eq(ended, 1e-5));
    }
}
