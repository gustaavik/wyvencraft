//! Binding a rigged model's bones and clips to gameplay state.
//!
//! `wyven_model` knows how to hold a skeleton and how to sample a clip. It does
//! not know that a character walks, that one of its arms swings a pickaxe, or
//! that the head follows the mouse — those are this game's rules, and they live
//! here. The engine is asked for bones and clips *by name*; deciding which
//! names matter is the whole of this module's job.
//!
//! The clip is only ever the base layer. [`AnimationState`] still owns the
//! things no keyframe can supply — the torso lagging behind the head, the head's
//! offset from the torso, and the one-shot attack swing — and those are composed
//! on top of whatever the clip produced.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

use glam::{Mat4, Quat, Vec3};

use wyven_model::Model;
use wyven_model::mesh::{self as model_mesh, UvWindow};
use wyven_model::rig::{BoneId, Pose, Rig};
use wyven_render::mesh::CpuMesh;

use crate::domain::entity::AnimationState;
use crate::domain::entity::kind::MovementParams;
use crate::presentation::art::skin;

/// Bone names a humanoid rig is expected to use.
///
/// A model that spells one differently simply loses that feature — no head
/// turn, no held item — rather than failing to load, which is the same
/// fail-soft posture the rest of the content pipeline takes.
const HEAD: &str = "head";
const ARMS: [&str; 2] = ["arm_l", "arm_r"];
const HANDS: [&str; 2] = ["hand_l", "hand_r"];

/// Clip names a humanoid rig is expected to use. `idle` is optional: a model
/// with none simply stands in its rest pose.
const IDLE: &str = "idle";
const WALK: &str = "walk";
const RUN: &str = "run";
/// Optional, like `idle`: a model with no jump clip simply keeps its gait in
/// mid-air, which is what every model here did before one existed.
const JUMP: &str = "jump";

/// Peak forward rotation of the arm during a one-shot attack swing (radians).
/// The same value [`AnimationState`] uses for the box model, so the two read
/// identically — a rigged character does not suddenly punch harder.
const SWING_REACH: f32 = 1.4;

/// Where the head is pointing, relative to the torso the body is drawn at.
///
/// Carried as a value rather than read off [`AnimationState`] because the
/// inventory preview overrides both: there the head tracks the cursor, not the
/// camera.
#[derive(Debug, Clone, Copy, Default)]
pub struct HeadLook {
    /// Offset from the torso about the vertical axis (radians).
    pub yaw: f32,
    pub pitch: f32,
}

/// A locomotion clip and the span of it that is actually keyframed.
#[derive(Debug, Clone, Copy)]
struct Gait {
    clip: usize,
    /// Seconds of clip mapped onto one stride. See [`Clip::end`].
    cycle: f32,
}

/// One rig's bones and clips, resolved once so the per-frame path is index
/// lookups rather than string comparisons.
pub struct HumanoidRig {
    head: Option<BoneId>,
    /// The character's *right* arm and hand — the ones that swing and hold.
    right_arm: Option<BoneId>,
    right_hand: Option<BoneId>,
    idle: Option<usize>,
    walk: Option<Gait>,
    run: Option<Gait>,
    /// Sampled by vertical velocity rather than by a clock — see
    /// [`HumanoidRig::airborne`].
    jump: Option<Gait>,
    /// Speeds the two gaits are authored for, from the entity's own movement
    /// data rather than repeated as constants here.
    walk_speed: f32,
    run_speed: f32,
    /// The launch speed of this entity's own jump, which is what the jump clip's
    /// two ends are calibrated against.
    jump_speed: f32,
}

impl HumanoidRig {
    pub fn bind(rig: &Rig, movement: &MovementParams) -> Self {
        let bound = Self {
            head: rig.bone(HEAD),
            right_arm: right_of(rig, &ARMS),
            right_hand: right_of(rig, &HANDS),
            idle: rig.clip_index(IDLE),
            walk: gait(rig, WALK),
            run: gait(rig, RUN),
            jump: gait(rig, JUMP),
            walk_speed: movement.walk_speed.max(0.01),
            run_speed: movement.sprint_speed.max(movement.walk_speed + 0.01),
            jump_speed: movement.jump_speed.max(0.01),
        };
        for (name, gait) in [(WALK, bound.walk), (RUN, bound.run), (JUMP, bound.jump)] {
            let Some(gait) = gait else { continue };
            let Some(clip) = rig.clip_at(gait.clip) else {
                continue;
            };
            if clip.end() + 1e-3 < clip.length {
                log::warn!(
                    "clip {name:?} is {:.2}s long but its last keyframe is at {:.2}s; \
                     driven by movement it plays the keyframed span, so the tail is ignored",
                    clip.length,
                    clip.end()
                );
            }
        }
        bound
    }

    /// The bone an item is held in, if the rig has one.
    pub fn right_hand(&self) -> Option<BoneId> {
        self.right_hand
    }

    /// The shoulder of the arm that swings — the root of the chain a
    /// first-person view draws.
    pub fn right_arm(&self) -> Option<BoneId> {
        self.right_arm
    }

    /// The pose to draw this frame: locomotion from the clips, head look and
    /// attack swing layered on top.
    pub fn pose(&self, rig: &Rig, anim: &AnimationState, look: HeadLook) -> Pose {
        let mut pose = self.locomotion(rig, anim);

        // Leaving the ground replaces the gait rather than riding on it: a jump
        // is a whole-body shape, and half a stride mixed into it reads as a
        // stumble. The gait keeps running underneath so the landing rejoins the
        // stride it left.
        let air = anim.air_amount().clamp(0.0, 1.0);
        if air > 0.0
            && let Some(jump) = self.airborne(rig, anim)
        {
            pose.blend(&jump, air);
        }

        if let Some(head) = self.head {
            // `yaw_matrix` (and so every yaw in this game) turns the opposite
            // way to a bare rotation about +Y, which is why the sign flips here
            // and nowhere else in this file.
            pose.rotate(head, Vec3::new(look.pitch, -look.yaw, 0.0));
        }

        // The attack swing is deliberately additive: it has to read the same
        // whether the character is standing still or sprinting, so it rides on
        // top of the gait rather than replacing it.
        let swing = anim.swing_progress();
        if swing > 0.0
            && let Some(arm) = self.right_arm
        {
            pose.rotate(arm, Vec3::new((swing * PI).sin() * SWING_REACH, 0.0, 0.0));
        }
        pose
    }

    /// Idle → walk → run, blended by how fast the character is actually moving.
    ///
    /// Clip time comes from [`AnimationState::walk_phase`], which advances with
    /// *distance travelled* rather than wall-clock time. That is what keeps the
    /// stride matched to the ground: a character pushed to half speed takes the
    /// same steps half as often instead of sliding.
    fn locomotion(&self, rig: &Rig, anim: &AnimationState) -> Pose {
        let mut pose = Pose::rest(rig);
        if let Some(idle) = self.idle.and_then(|i| rig.clip_at(i)) {
            idle.sample(anim.walk_phase() / TAU * idle.length, &mut pose);
        }

        let amount = anim.walk_amount().clamp(0.0, 1.0);
        if amount <= 0.0 {
            return pose;
        }

        let phase = anim.walk_phase() / TAU;
        let Some(mut moving) = self.sample(rig, self.walk, phase) else {
            return pose;
        };
        // Above the walk speed the run clip takes over; the two are blended
        // rather than switched, so breaking into a sprint does not snap.
        if let Some(running) = self.sample(rig, self.run, phase) {
            let into_run = ((anim.speed() - self.walk_speed) / (self.run_speed - self.walk_speed))
                .clamp(0.0, 1.0);
            moving.blend(&running, into_run);
        }

        pose.blend(&moving, amount);
        pose
    }

    /// The jump clip sampled at wherever in the arc the body actually is.
    ///
    /// Vertical velocity is the clock, exactly as distance is the clock for the
    /// gait: the clip runs from launch at `+jump_speed`, through the apex where
    /// the body hangs at zero, to the fall at `-jump_speed`. Driving it this way
    /// rather than off a timer means the tuck lands at the real apex however
    /// high the jump was, a jump cut short by a ceiling reverses instead of
    /// playing on, and stepping off a ledge starts partway in — already falling,
    /// which is what a fall is.
    ///
    /// The downward end is calibrated to `jump_speed` rather than to terminal
    /// velocity so that a plain jump uses the whole clip; a longer drop simply
    /// holds the falling pose.
    fn airborne(&self, rig: &Rig, anim: &AnimationState) -> Option<Pose> {
        let rise = (anim.vertical_speed() / self.jump_speed).clamp(-1.0, 1.0);
        self.sample(rig, self.jump, (1.0 - rise) * 0.5)
    }

    /// One clip sampled at `phase` of its keyframed span, or `None` if the rig
    /// has no such clip.
    fn sample(&self, rig: &Rig, gait: Option<Gait>, phase: f32) -> Option<Pose> {
        let gait = gait?;
        let clip = rig.clip_at(gait.clip)?;
        let mut pose = Pose::rest(rig);
        clip.sample(phase * gait.cycle, &mut pose);
        Some(pose)
    }
}

fn gait(rig: &Rig, name: &str) -> Option<Gait> {
    let clip = rig.clip_index(name)?;
    let end = rig.clip_at(clip)?.end();
    (end > 0.0).then_some(Gait { clip, cycle: end })
}

/// The candidate bone on the character's **right**, chosen by where it sits
/// rather than by what it is called.
///
/// The shipped player model labels its arms from the viewer's side: `arm_r`
/// and `hand_r` are authored at negative X, which — for a model that faces −Z,
/// as every model in this game does — is the character's *left*. Its legs are
/// labelled the other way round. Trusting the name would put every held item in
/// the wrong fist and swing the wrong arm, and would do it silently.
fn right_of(rig: &Rig, names: &[&str]) -> Option<BoneId> {
    names
        .iter()
        .filter_map(|name| rig.bone(name))
        .max_by(|a, b| rig.pivot(*a).x.total_cmp(&rig.pivot(*b).x))
        .filter(|bone| rig.pivot(*bone).x > 0.0)
}

/// The shipped player model, loaded once per test that needs a real rig.
///
/// Lives outside `mod tests` so the view-model tests can use it too: the
/// first-person arm is the same rig as the body, and testing it against a
/// hand-built stub would defeat the point.
#[cfg(test)]
pub(crate) mod fixture {
    use crate::domain::entity::Motion;

    use super::*;
    use wyven_assets::FsSource;
    use wyven_model::{ModelId, ModelRegistry};

    pub struct Player {
        models: ModelRegistry,
        id: ModelId,
        clips: HumanoidRig,
    }

    impl Player {
        pub fn load() -> Self {
            let mut models = ModelRegistry::new();
            let id = models
                .load(
                    "assets/models/entity/player/player.bbmodel",
                    &FsSource::rooted("."),
                )
                .expect("the shipped player model loads");
            let clips = HumanoidRig::bind(
                models
                    .get(id)
                    .and_then(|m| m.rig.as_ref())
                    .expect("it is rigged"),
                &movement(),
            );
            Self { models, id, clips }
        }

        pub fn character(&self) -> Character<'_> {
            Character {
                model: self.models.get(self.id).expect("loaded"),
                clips: &self.clips,
                scale: SCALE,
                sheet: skin::SKIN_ORIGIN,
            }
        }
    }

    /// What `assets/entities.toml` gives the player.
    pub const SCALE: f32 = 1.641_026;

    pub fn movement() -> MovementParams {
        MovementParams {
            walk_speed: 4.3,
            sprint_speed: 6.5,
            fly_speed: 12.0,
            jump_speed: 9.0,
            eye_height: 1.62,
            reach: 5.0,
            air_control: 6.0,
            min_jump_height: 1.2,
            stop_rate: 18.0,
        }
    }

    /// An animation state that has settled at `speed`.
    pub fn walking(speed: f32) -> AnimationState {
        let mut anim = AnimationState::new();
        for _ in 0..200 {
            anim.advance(Motion::walking(speed), 0.0, 1.0 / 60.0);
        }
        anim
    }

    /// An animation state fully in the air, rising (or falling) at `vertical`.
    pub fn airborne(vertical: f32) -> AnimationState {
        let mut anim = AnimationState::new();
        for _ in 0..200 {
            anim.advance(Motion::new(0.0, vertical, true), 0.0, 1.0 / 60.0);
        }
        anim
    }
}

#[cfg(test)]
mod tests;

// --- Drawing -----------------------------------------------------------------

/// How far the grip sits from the hand bone's own pivot, in [`hand_space`]'s
/// axes: right, forward, up.
///
/// Minecraft's counterpart runs from the *shoulder* pivot — it has to travel the
/// length of the arm before it reaches the fist. We anchor on the hand bone, so
/// that distance is already spent and only the residual belongs here. Unlike
/// [`hand_space`] this **is** a tuning knob, in the same sense
/// `viewmodel::ARM_PITCH` is: it places our own rig's fist, which no authored
/// `display` entry says anything about.
const GRIP: Vec3 = Vec3::ZERO;

/// Model space → the frame an authored `thirdperson_righthand` entry is measured
/// against: **+X right, +Y forward, +Z up**.
///
/// This is *not* a tuning knob. Minecraft reaches that frame by applying
/// `Rx(-90°)·Ry(180°)` after moving to the arm bone, inside an entity space its
/// renderer has already flipped with `scale(-1, -1, 1)`; our rig needs no flip,
/// because a model here is authored Y-up facing −Z, so the whole composite
/// collapses to the quarter turn below. Every `display` number under `assets/`
/// was dragged into place against this frame, so changing it silently
/// invalidates all of them — retune [`GRIP`], or the model.
///
/// Without it the frame is simply the body's own axes, which is what put a
/// sword's `[-83.4, 87.78, 95.45]` a quarter turn out and lifted it instead of
/// pushing it forward out of the fist.
fn hand_space() -> Mat4 {
    Mat4::from_rotation_x(-FRAC_PI_2) * Mat4::from_translation(GRIP)
}

/// Everything needed to draw one rigged character: the parsed model, the bones
/// and clips bound to it, and how the entity data says to size and texture it.
///
/// Bundled because these four always travel together — the third-person body,
/// the inventory preview, a remote player and the first-person arm are the same
/// four values placed four ways.
#[derive(Clone, Copy)]
pub struct Character<'a> {
    pub model: &'a Model,
    pub clips: &'a HumanoidRig,
    /// Uniform scale fitting the authored figure to its collision box.
    pub scale: f32,
    /// Atlas tile origin of the 64×64 skin sheet this character samples.
    pub sheet: [u32; 2],
}

impl<'a> Character<'a> {
    pub fn rig(&self) -> Option<&'a Rig> {
        self.model.rig.as_ref()
    }

    /// The pose to draw this frame. `None` for a model that carries no rig,
    /// which is the same "invisible rather than a panicking frame" the
    /// file-model path takes for a bad load.
    pub fn pose(&self, anim: &AnimationState, look: HeadLook) -> Option<Pose> {
        let rig = self.rig()?;
        Some(self.clips.pose(rig, anim, look))
    }

    pub fn rest_pose(&self) -> Option<Pose> {
        self.rig().map(Pose::rest)
    }

    /// The model→world transform for this character standing at `position`.
    pub fn placement(&self, position: Vec3, yaw: f32) -> Mat4 {
        model_mesh::placement(position, yaw, 0.0, self.scale, Vec3::ZERO, Vec3::ZERO)
    }

    /// Bake the whole body standing at `position`, turned to `yaw`.
    pub fn bake(&self, pose: &Pose, position: Vec3, yaw: f32) -> CpuMesh {
        self.bake_parts(pose, self.placement(position, yaw), None)
    }

    /// Bake `only` those bones (all of them for `None`) under `transform`.
    ///
    /// `normal_basis` is left equal to `transform` here; the first-person arm,
    /// which needs them to differ, composes its own call.
    pub fn bake_parts(&self, pose: &Pose, transform: Mat4, normal_basis: Option<Mat4>) -> CpuMesh {
        self.bake_selected(pose, transform, normal_basis.unwrap_or(transform), |_| true)
    }

    /// The general bake: every part the predicate keeps.
    pub fn bake_selected(
        &self,
        pose: &Pose,
        transform: Mat4,
        normal_basis: Mat4,
        keep: impl Fn(Option<BoneId>) -> bool,
    ) -> CpuMesh {
        bake_parts(self.model, self.sheet, pose, transform, normal_basis, keep)
    }

    /// Where the bones of `root`'s subtree end up, as a predicate for
    /// [`Self::bake_selected`].
    pub fn subtree_filter(&self, root: BoneId) -> impl Fn(Option<BoneId>) -> bool + use<> {
        let inside: Vec<BoneId> = self.rig().map(|rig| rig.subtree(root)).unwrap_or_default();
        move |bone| bone.is_some_and(|b| inside.contains(&b))
    }

    /// Where a bone's pivot ends up in model space under `pose`, before the
    /// character's own placement — the joint, as the geometry sees it.
    pub fn joint(&self, pose: &Pose, bone: BoneId) -> Option<Vec3> {
        let rig = self.rig()?;
        let matrices = rig.matrices(pose);
        let matrix = matrices.get(bone.0 as usize)?;
        Some(matrix.transform_point3(rig.pivot(bone)))
    }

    /// Where an item held in the right hand goes, ready for the item model's
    /// own `display` transform to be applied on top.
    ///
    /// Deliberately scale-free. `Character::placement` is used only to carry the
    /// joint out into world space; the matrix returned is built fresh, because
    /// a 1.64× player must not swing a 1.64× pickaxe. (The bone matrices
    /// themselves carry no scale at all — see `wyven_model::rig`, where a bone
    /// holds only the animation's delta from a rest pose already baked into the
    /// vertices.)
    ///
    /// The frame this hands back is [`HAND_SPACE`]'s, which is what lets an
    /// authored `thirdperson_righthand` entry mean the same thing here as it did
    /// in the editor it was dragged into place in.
    pub fn hand_anchor(&self, pose: &Pose, position: Vec3, yaw: f32) -> Option<Mat4> {
        let rig = self.rig()?;
        let hand = self.clips.right_hand()?;
        let joint = self.joint(pose, hand)?;
        let fist = self.placement(position, yaw).transform_point3(joint);
        let turn = rig
            .matrices(pose)
            .get(hand.0 as usize)
            .map(|m| Quat::from_mat4(m).normalize())
            .unwrap_or(Quat::IDENTITY);
        Some(
            Mat4::from_translation(fist)
                * model_mesh::anchor(Vec3::ZERO, yaw, 0.0)
                * Mat4::from_quat(turn)
                * hand_space(),
        )
    }
}

/// A rigged model in its rest pose, with no clips bound to it.
///
/// The path an entity that is *drawn* from a rig but not yet *animated* by one
/// takes — every rigged mob today. Posing needs bones resolved by name and
/// clips chosen by speed ([`HumanoidRig`]); standing still needs neither.
pub fn bake_rest(
    model: &Model,
    scale: f32,
    sheet: [u32; 2],
    position: Vec3,
    yaw: f32,
) -> Option<CpuMesh> {
    let pose = Pose::rest(model.rig.as_ref()?);
    let transform = model_mesh::placement(position, yaw, 0.0, scale, Vec3::ZERO, Vec3::ZERO);
    Some(bake_parts(
        model,
        sheet,
        &pose,
        transform,
        transform,
        |_| true,
    ))
}

/// The one bake every rigged path funnels through.
fn bake_parts(
    model: &Model,
    sheet: [u32; 2],
    pose: &Pose,
    transform: Mat4,
    normal_basis: Mat4,
    keep: impl Fn(Option<BoneId>) -> bool,
) -> CpuMesh {
    let Some(rig) = model.rig.as_ref() else {
        return CpuMesh::new();
    };
    let parts: Vec<_> = rig
        .parts()
        .iter()
        .filter(|part| keep(part.bone))
        .copied()
        .collect();
    model.mesh.bake_posed(
        &parts,
        &rig.matrices(pose),
        transform,
        normal_basis,
        sheet_window(sheet),
    )
}

/// Where a 64×64 skin sheet at `origin_tile` sits inside the block atlas, as
/// the UV window a model baked against it needs.
///
/// A rigged character is drawn from the *shared atlas* rather than a texture of
/// its own, so its mesh joins `SceneFrame::opaque` beside the box models and
/// costs no extra descriptor bind however many players are on screen. The model
/// authors its UVs across a whole 64×64 sheet; this is the one place that says
/// where that sheet actually lives. Derived from [`skin::sheet_uv`] rather than
/// recomputed, so the two can never disagree about the atlas layout.
pub fn sheet_window(origin_tile: [u32; 2]) -> UvWindow {
    const WHOLE: [u32; 4] = [0, 0, skin::SKIN_SIZE, skin::SKIN_SIZE];
    let offset = skin::sheet_uv(origin_tile, WHOLE, [0.0, 0.0]);
    let far = skin::sheet_uv(origin_tile, WHOLE, [1.0, 1.0]);
    UvWindow {
        offset,
        scale: [far[0] - offset[0], far[1] - offset[1]],
    }
}
