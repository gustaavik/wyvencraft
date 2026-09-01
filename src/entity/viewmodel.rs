//! The first-person view model: the player's own arm, and whatever it holds.
//!
//! In third person the hand is a consequence of the body — the rig puts the
//! fist at the end of a swinging arm, and the held item hangs off that bone.
//! First person has no body to hang off: the camera *is* the head, and the arm
//! has to be placed against it directly.
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

use crate::entity::rigged::Character;
use wyven_model::display::{DisplayContext, ItemTransform};
use wyven_model::mesh as model_mesh;
use wyven_model::rig::Pose;
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

/// Where the held item goes, given the hand `frame`: at the fist, oriented by
/// hand space, with the model's own placement applied on top by the caller.
pub fn item_anchor(frame: Mat4) -> Mat4 {
    frame
}

/// Where a held **cube** sits — Minecraft's `block/block` display table.
///
/// An item with a model file carries its own `display` block and places itself.
/// A block item has no model file at all: its item is synthesised from the block
/// (`Item::block`), so nothing anywhere says how big a held cube should be or
/// which way it should face. These are the numbers vanilla uses, and they are
/// read for the two hand contexts only — `Gui` and `Ground` stay owned by the
/// icon sheet and `DroppedItem::render_size`, which already look right.
///
/// The cube this places is built in `0..1` model space, the same space a
/// Blockbench export occupies, so [`ItemTransform::matrix`] positions it by
/// exactly the path an authored model takes — including its `-0.5` recentring.
pub fn block_placement(context: DisplayContext) -> ItemTransform {
    match context {
        // Turned a corner toward the camera so three faces are visible, rather
        // than one flat square filling the fist.
        DisplayContext::FirstPersonRightHand => ItemTransform {
            rotation: [0.0, 45.0, 0.0],
            translation: [0.0, 0.0, 0.0],
            scale: [0.40; 3],
        },
        // Smaller and lifted, because in third person it hangs off a fist that
        // is itself already out at the end of an arm.
        DisplayContext::ThirdPersonRightHand => ItemTransform {
            rotation: [0.0, 0.0, 0.0],
            translation: [0.0, 2.5, 0.0],
            scale: [0.375; 3],
        },
        // Nothing else asks: the icon sheet and the ground both have their own
        // sizing already. The identity keeps this total rather than panicking.
        _ => ItemTransform::default(),
    }
}

/// Build the player's right arm — the shoulder, elbow and hand of the rigged
/// model — in world space, hanging off `frame`.
///
/// Only that limb is baked, out of the same rig and the same pose the whole body
/// is drawn from in third person, so the two views can never disagree about
/// where an elbow is. The arm is shifted so its **fist** — not its shoulder —
/// sits at the frame's origin, which is where the held item is too: that is what
/// keeps an item in the hand while the arm swings.
///
/// Lit as though it were fixed in front of the eye rather than turning with it:
/// without that the hand would pulse between bright and dim as the player spun
/// on the spot, which reads as a rendering fault.
pub fn arm_mesh(character: &Character<'_>, pose: &Pose, frame: Mat4) -> CpuMesh {
    let Some(arm) = character.clips.right_arm() else {
        return CpuMesh::new();
    };
    let Some(hand) = character.clips.right_hand() else {
        return CpuMesh::new();
    };
    let Some(fist) = character.joint(pose, hand) else {
        return CpuMesh::new();
    };

    let orientation = arm_orientation();
    let scale = Mat4::from_scale(Vec3::splat(character.scale));
    // The bake applies `transform * bone`, and the bone matrices are in the
    // model's own units — so the fist has to be scaled the same way before it
    // can be subtracted off.
    let transform = frame * orientation * Mat4::from_translation(-fist * character.scale) * scale;
    character.bake_selected(pose, transform, orientation, character.subtree_filter(arm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::rigged::fixture::{Player, walking};

    /// The arm as it is drawn standing still.
    fn rested(player: &Player) -> (Character<'_>, Pose) {
        let character = player.character();
        let pose = character
            .pose(&walking(0.0), crate::entity::HeadLook::default())
            .expect("a rest pose");
        (character, pose)
    }

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

    /// The whole arm chain and nothing else: shoulder, forearm and hand, six
    /// faces each, four unwelded corners a face. If the subtree filter let the
    /// torso through, this is what would catch it.
    #[test]
    fn the_arm_is_the_three_bones_of_one_limb() {
        let player = Player::load();
        let (character, arm_pose) = rested(&player);
        let mesh = arm_mesh(&character, &arm_pose, pose(0.0, 0.0).frame());
        assert_eq!(mesh.vertices.len(), 3 * 6 * 4);
    }

    /// The check that cannot be made by looking at the screen without running
    /// the game: the arm is in front of the eye and to the player's right, for
    /// every direction they might be facing.
    #[test]
    fn the_arm_sits_in_front_of_the_eye_and_to_the_right() {
        let player = Player::load();
        let (character, arm_pose) = rested(&player);
        for &(yaw, pitch) in &[(0.0, 0.0), (1.7, 0.0), (-2.4, 0.6), (3.0, -0.9), (5.5, 0.2)] {
            let pose = pose(yaw, pitch);
            // `Player::look_direction` for this yaw/pitch, spelled out so the
            // test does not need a whole player to check a direction.
            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            let look = Vec3::new(cp * sy, sp, -cp * cy).normalize();
            let right = Vec3::new(yaw.cos(), 0.0, yaw.sin());
            let mesh = arm_mesh(&character, &arm_pose, pose.frame());
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

    /// The hand and the held item's origin are the same point, at rest and
    /// mid-swing alike. This is what stops an item drifting out of the fist —
    /// and with a three-joint arm there are two more places it could drift.
    ///
    /// Measured off the baked hand geometry rather than off the placement maths
    /// that produced it, so a sign error in either has somewhere to show.
    #[test]
    fn the_arm_and_the_item_share_a_fist() {
        let player = Player::load();
        let character = player.character();
        let hand = character.clips.right_hand().expect("a hand bone");
        for swing in [0.0, 0.25, 0.5, 0.9] {
            let mut anim = walking(0.0);
            anim.trigger_swing();
            anim.advance(0.0, 0.0, swing * 0.25);
            let arm_pose = character
                .pose(&anim, crate::entity::HeadLook::default())
                .expect("a pose");

            let hand_pose = HandPose {
                swing: anim.swing_progress(),
                ..pose(0.8, -0.2)
            };
            let frame = hand_pose.frame();

            // Where the hand's own geometry ended up...
            let mesh = arm_mesh(&character, &arm_pose, frame);
            let all = arm_mesh(&character, &arm_pose, frame).vertices.len();
            let hand_only = {
                let orientation = arm_orientation();
                let fist = character.joint(&arm_pose, hand).expect("a joint");
                let transform = frame
                    * orientation
                    * Mat4::from_translation(-fist * character.scale)
                    * Mat4::from_scale(Vec3::splat(character.scale));
                character.bake_selected(
                    &arm_pose,
                    transform,
                    orientation,
                    character.subtree_filter(hand),
                )
            };
            assert!(
                hand_only.vertices.len() < all,
                "the hand is part of the arm"
            );
            let centre: Vec3 = hand_only
                .vertices
                .iter()
                .map(|v| Vec3::from(v.position))
                .sum::<Vec3>()
                / hand_only.vertices.len() as f32;

            // ...and where a held model is anchored.
            let held = item_anchor(frame).transform_point3(Vec3::ZERO);
            assert!(
                (centre - held).length() < 0.1,
                "swing {swing}: hand centre {centre} vs item {held}"
            );
            assert!(!mesh.vertices.is_empty());
        }
    }

    /// A held cube built through the model-less fallback must land in front of
    /// the eye, for every direction the player might face. A block that renders
    /// behind the camera is invisible in exactly the way this fallback exists to
    /// fix, and no screenshot would tell you which of the two it was.
    #[test]
    fn a_held_cube_sits_in_front_of_the_eye() {
        for &(yaw, pitch) in &[(0.0, 0.0), (1.7, 0.0), (-2.4, 0.6), (3.0, -0.9), (5.5, 0.2)] {
            let pose = pose(yaw, pitch);
            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            let look = Vec3::new(cp * sy, sp, -cp * cy).normalize();

            for vertex in &held_cube(pose.frame()).vertices {
                let offset = Vec3::from(vertex.position) - pose.eye;
                assert!(
                    offset.dot(look) > 0.0,
                    "cube vertex behind the eye at yaw {yaw} pitch {pitch}: {offset}"
                );
            }
        }
    }

    /// The same invariant the arm has, for the same reason: a held block rides
    /// the camera, so spinning on the spot must not change how it is lit. Taking
    /// the normals from the full placement — which contains the camera's yaw —
    /// is the mistake this guards.
    #[test]
    fn turning_does_not_change_a_held_cubes_lighting() {
        let a = held_cube(pose(0.0, 0.0).frame());
        let b = held_cube(pose(2.9, 0.4).frame());
        for (x, y) in a.vertices.iter().zip(&b.vertices) {
            assert_eq!(x.normal, y.normal);
            assert_eq!(x.ao, y.ao);
        }
    }

    /// A held cube placed the way `SceneCache::shaped_item_mesh` places one.
    fn held_cube(frame: Mat4) -> CpuMesh {
        let local = block_placement(DisplayContext::FirstPersonRightHand).matrix();
        let mut cube = CpuMesh::new();
        crate::world::meshing::push_item_cube(
            &mut cube,
            Vec3::splat(0.5),
            1.0,
            0.0,
            &wyven_voxel::FaceTextures::uniform(1),
        );
        cube.transformed(item_anchor(frame) * local, local)
    }

    /// The hand rides the camera, so turning must not change its shading.
    #[test]
    fn turning_does_not_change_the_arms_lighting() {
        let player = Player::load();
        let (character, arm_pose) = rested(&player);
        let a = arm_mesh(&character, &arm_pose, pose(0.0, 0.0).frame());
        let b = arm_mesh(&character, &arm_pose, pose(2.9, 0.4).frame());
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
