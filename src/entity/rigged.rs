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

use std::f32::consts::{PI, TAU};

use glam::{Mat4, Quat, Vec3};

use wyven_model::Model;
use wyven_model::mesh::{self as model_mesh, UvWindow};
use wyven_model::rig::{BoneId, Pose, Rig};
use wyven_render::mesh::CpuMesh;

use crate::art::skin;
use crate::entity::AnimationState;
use crate::entity::kind::MovementParams;

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
    /// Speeds the two gaits are authored for, from the entity's own movement
    /// data rather than repeated as constants here.
    walk_speed: f32,
    run_speed: f32,
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
            walk_speed: movement.walk_speed.max(0.01),
            run_speed: movement.sprint_speed.max(movement.walk_speed + 0.01),
        };
        for (name, gait) in [(WALK, bound.walk), (RUN, bound.run)] {
            let Some(gait) = gait else { continue };
            let Some(clip) = rig.clip_at(gait.clip) else {
                continue;
            };
            if clip.end() + 1e-3 < clip.length {
                log::warn!(
                    "clip {name:?} is {:.2}s long but its last keyframe is at {:.2}s; \
                     driven by stride it plays the keyframed span, so the tail is ignored",
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

    /// One gait clip sampled at `phase` strides, or `None` if the rig has no
    /// such clip.
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
            anim.advance(speed, 0.0, 1.0 / 60.0);
        }
        anim
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixture::{Player, walking};

    #[test]
    fn the_right_hand_is_chosen_by_where_it_sits_not_what_it_is_called() {
        let player = Player::load();
        let character = player.character();
        let rig = character.rig().expect("rigged");
        let bound = character.clips;

        let hand = bound.right_hand().expect("a hand to hold things in");
        assert!(
            rig.pivot(hand).x > 0.0,
            "the character's right is +X, whatever the bone is labelled"
        );
        // And on this model that is deliberately *not* the one called `hand_r`.
        assert_eq!(rig.name(hand), "hand_l");
        assert_eq!(rig.name(bound.right_arm().expect("an arm")), "arm_l");
    }

    #[test]
    fn standing_still_leaves_the_rig_at_rest() {
        let player = Player::load();
        let character = player.character();
        let rig = character.rig().expect("rigged");
        let bound = character.clips;

        let pose = bound.pose(rig, &walking(0.0), HeadLook::default());
        assert_eq!(pose, Pose::rest(rig), "no clip, no drift");
    }

    #[test]
    fn walking_drives_the_legs_and_sprinting_drives_them_differently() {
        let player = Player::load();
        let character = player.character();
        let rig = character.rig().expect("rigged");
        let bound = character.clips;
        let leg = rig.bone("leg_r").expect("leg_r");

        let walked = bound.pose(rig, &walking(4.3), HeadLook::default());
        assert!(
            walked.get(leg).rotation.x.abs() > 0.01,
            "the walk clip should move a leg"
        );

        let sprinted = bound.pose(rig, &walking(6.5), HeadLook::default());
        assert_ne!(
            walked.get(leg).rotation,
            sprinted.get(leg).rotation,
            "the run clip should not be the walk clip"
        );
    }

    /// Distance-driven, not clock-driven: standing still for a while must not
    /// advance the gait, or the feet would skate.
    #[test]
    fn the_gait_advances_with_distance_not_time() {
        let player = Player::load();
        let character = player.character();
        let rig = character.rig().expect("rigged");
        let bound = character.clips;
        let leg = rig.bone("leg_r").expect("leg_r");

        let mut anim = walking(4.3);
        let before = bound.pose(rig, &anim, HeadLook::default()).get(leg);
        // Stop dead and let a second of wall-clock pass. `walk_amount` decays,
        // so compare the phase rather than the blended pose.
        let phase = anim.walk_phase();
        for _ in 0..60 {
            anim.advance(0.0, 0.0, 1.0 / 60.0);
        }
        assert_eq!(anim.walk_phase(), phase, "a still character takes no steps");
        assert!(before.rotation.x.abs() > 0.0);
    }

    #[test]
    fn the_head_look_rides_on_top_of_the_clip() {
        let player = Player::load();
        let character = player.character();
        let rig = character.rig().expect("rigged");
        let bound = character.clips;
        let head = rig.bone("head").expect("head");

        let look = HeadLook {
            yaw: 0.4,
            pitch: -0.2,
        };
        let pose = bound.pose(rig, &walking(4.3), look);
        assert!((pose.get(head).rotation.x - -0.2).abs() < 1e-6);
        assert!(
            (pose.get(head).rotation.y - -0.4).abs() < 1e-6,
            "yaw turns the way every other yaw in this game turns"
        );
    }

    /// The whole reason `scale` is in `entities.toml`: the authored figure is
    /// 1.096875 blocks tall and the collision box is 1.8, so a model drawn at
    /// its own size would stand chest-high inside its own hitbox.
    #[test]
    fn the_baked_player_fills_its_collision_box() {
        let player = Player::load();
        let character = player.character();
        let pose = character.rest_pose().expect("a rest pose");
        let mesh = character.bake(&pose, Vec3::new(10.0, 64.0, -3.0), 0.0);

        let ys: Vec<f32> = mesh.vertices.iter().map(|v| v.position[1]).collect();
        let low = ys.iter().copied().fold(f32::MAX, f32::min);
        let high = ys.iter().copied().fold(f32::MIN, f32::max);
        assert!((low - 64.0).abs() < 1e-3, "feet on the ground, not {low}");
        assert!(
            (high - 65.8).abs() < 1e-2,
            "head at 1.8 blocks, not {}",
            high - 64.0
        );

        // And it is standing where it was asked to, not at the origin.
        let xs: Vec<f32> = mesh.vertices.iter().map(|v| v.position[0]).collect();
        let centre = (xs.iter().copied().fold(f32::MAX, f32::min)
            + xs.iter().copied().fold(f32::MIN, f32::max))
            / 2.0;
        assert!((centre - 10.0).abs() < 1e-3, "centred at {centre}");
    }

    /// The body samples the player skin's own block of the atlas and nothing
    /// else. Off by one tile and the player would be wearing gravel.
    #[test]
    fn every_vertex_samples_inside_the_skin_block() {
        let player = Player::load();
        let character = player.character();
        let pose = character.rest_pose().expect("a rest pose");
        let mesh = character.bake(&pose, Vec3::ZERO, 0.0);

        let window = sheet_window(skin::SKIN_ORIGIN);
        for vertex in &mesh.vertices {
            for axis in 0..2 {
                let lo = window.offset[axis];
                let hi = lo + window.scale[axis];
                assert!(
                    vertex.uv[axis] >= lo - 1e-6 && vertex.uv[axis] <= hi + 1e-6,
                    "uv {:?} outside the skin block {lo}..{hi}",
                    vertex.uv
                );
            }
        }
    }

    /// Walking moves the geometry, and the character stays on the ground while
    /// it does — the `root` bone's position track dips the body, and a sign
    /// error there would sink the player through the floor.
    #[test]
    fn the_walk_cycle_moves_the_body_without_lifting_it_off_the_ground() {
        let player = Player::load();
        let character = player.character();
        let rest = character.rest_pose().expect("a rest pose");
        let walking_pose = character
            .pose(&walking(4.3), HeadLook::default())
            .expect("a walk pose");

        let still = character.bake(&rest, Vec3::ZERO, 0.0);
        let moving = character.bake(&walking_pose, Vec3::ZERO, 0.0);
        assert!(
            still
                .vertices
                .iter()
                .zip(&moving.vertices)
                .any(|(a, b)| a.position != b.position),
            "the walk clip should displace the body"
        );

        let low = moving
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MAX, f32::min);
        assert!(low > -0.2, "feet {low} blocks below the floor");
    }

    #[test]
    fn an_attack_swing_adds_to_whatever_the_arm_was_doing() {
        let player = Player::load();
        let character = player.character();
        let rig = character.rig().expect("rigged");
        let bound = character.clips;
        let arm = bound.right_arm().expect("an arm");

        let mut anim = walking(4.3);
        let resting = bound.pose(rig, &anim, HeadLook::default()).get(arm);
        anim.trigger_swing();
        anim.advance(4.3, 0.0, 0.125);
        let swinging = bound.pose(rig, &anim, HeadLook::default()).get(arm);

        assert!(
            swinging.rotation.x > resting.rotation.x,
            "the swing reaches forward: {} vs {}",
            swinging.rotation.x,
            resting.rotation.x
        );
    }
}

// --- Drawing -----------------------------------------------------------------

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
    /// The bone matrix carries the character's scale, which an item must not
    /// inherit — a 1.64× player would otherwise swing a 1.64× pickaxe. Only the
    /// hand's position and turn survive.
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
                * Mat4::from_quat(turn),
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
