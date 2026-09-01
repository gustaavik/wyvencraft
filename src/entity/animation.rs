//! Procedural animation for the humanoid model: a locomotion-driven walk cycle, a
//! subtle idle sway, and a one-shot arm swing (mining / placing). The state is
//! advanced each frame and sampled into a [`Pose`] consumed by `HumanoidModel`.
//!
//! Pure logic — no rendering or GPU dependencies — so it is cheap to unit test and is
//! reused for both the local player and movement-derived remote players.

use std::f32::consts::PI;

use crate::core::wrap_angle;
use crate::entity::model::Pose;

const TAU: f32 = 2.0 * PI;

/// Horizontal speed (blocks/s) treated as a "full" walk for idle↔walk blending.
const REFERENCE_SPEED: f32 = 4.3;
/// Radians of walk phase accrued per block travelled (stride cadence).
const GAIT_FREQ: f32 = 2.0;
/// Peak limb swing at full walk amount (radians, ~51°).
const SWING_MAX: f32 = 0.9;
/// Idle arm sway amplitude (radians).
const IDLE_AMP: f32 = 0.06;
/// Idle sway cadence (radians/s).
const IDLE_FREQ: f32 = 1.6;
/// Exponential blend rate for `walk_amount` (per second); frame-rate independent.
const BLEND_RATE: f32 = 10.0;
/// Duration of a one-shot arm swing (seconds).
const SWING_DURATION: f32 = 0.25;
/// Peak forward rotation of the arm during a one-shot swing (radians).
const SWING_REACH: f32 = 1.4;

/// How far the head may turn off the torso before the torso starts to follow
/// (radians, 45°). Inside this cone the head turns alone — this is the "neck".
const FREE_HEAD_TURN: f32 = 45.0 * PI / 180.0;
/// Hard limit on the neck (radians, 50°). Past it the torso is dragged bodily so
/// the offset can never grow, whatever the head did in one frame — a mouse flick,
/// a snapped remote yaw, or a mob's brain slamming its facing round.
const MAX_HEAD_TURN: f32 = 50.0 * PI / 180.0;
/// Exponential rate the torso eases toward the head once the head leaves the free
/// cone (per second). Minecraft's 0.3-per-tick works out at about this.
const BODY_FOLLOW_RATE: f32 = 8.0;
/// Rate the torso squares up under the look direction at a full walk (per second),
/// scaled by `walk_amount`. Faster than [`BODY_FOLLOW_RATE`] because a walking
/// body that stayed twisted would read as sliding sideways.
const BODY_WALK_RATE: f32 = 12.0;

/// Accumulated animation state for one humanoid. `Default` is the rest pose.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnimationState {
    /// Walk-cycle phase (radians), advanced by distance travelled.
    walk_phase: f32,
    /// Smoothed locomotion magnitude in `[0,1]` (idle↔walk blend).
    walk_amount: f32,
    /// Smoothed horizontal speed (blocks/s), on the same blend as
    /// `walk_amount`. Kept separately because `walk_amount` saturates at
    /// [`REFERENCE_SPEED`] and so cannot tell a walk from a sprint — which is
    /// exactly the choice a model with both a `walk` and a `run` clip has to
    /// make.
    speed: f32,
    /// Idle sway phase (radians), advanced by wall-clock time.
    idle_phase: f32,
    /// Remaining time of a one-shot arm swing (seconds); `0` = not swinging.
    swing_timer: f32,
    /// Torso facing (radians about Y). `None` until the first [`Self::advance`],
    /// which squares it under the head rather than spinning up from zero.
    body_yaw: Option<f32>,
    /// The look yaw last handed to [`Self::advance`], so [`Self::pose`] can give
    /// the head its offset from the torso.
    look_yaw: f32,
}

impl AnimationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance by `dt` seconds given the entity's current horizontal speed and the
    /// direction it is *looking* (its head yaw — the camera for a player, the
    /// brain's chosen facing for a mob).
    ///
    /// The look yaw is what drives [`Self::body_yaw`]: the torso is a follower, so
    /// callers keep steering with their own yaw and only *draw* with this one.
    pub fn advance(&mut self, horizontal_speed: f32, look_yaw: f32, dt: f32) {
        let dt = dt.max(0.0);
        let speed = horizontal_speed.max(0.0);

        // Phase tracks distance so a faster gait swings faster; keep it bounded to
        // preserve float precision over long sessions.
        self.walk_phase = (self.walk_phase + speed * dt * GAIT_FREQ).rem_euclid(TAU);
        self.idle_phase = (self.idle_phase + dt * IDLE_FREQ).rem_euclid(TAU);

        let target = (speed / REFERENCE_SPEED).clamp(0.0, 1.0);
        let blend = 1.0 - (-dt * BLEND_RATE).exp();
        self.walk_amount += (target - self.walk_amount) * blend;
        self.speed += (speed - self.speed) * blend;

        self.turn_body(look_yaw, dt);

        self.swing_timer = (self.swing_timer - dt).max(0.0);
    }

    /// Drag the torso after the head: free inside [`FREE_HEAD_TURN`], easing once
    /// the head leaves that cone, squaring up under the look direction while
    /// walking, and never twisted past [`MAX_HEAD_TURN`].
    ///
    /// Reads `walk_amount`, so it must run *after* the blend above.
    fn turn_body(&mut self, look_yaw: f32, dt: f32) {
        let mut body = *self.body_yaw.get_or_insert(look_yaw);
        let mut offset = wrap_angle(look_yaw - body);

        // Walking always squares up; standing still, only a head outside the cone
        // pulls — which is what lets the head turn alone while you stand.
        let rate = BODY_WALK_RATE * self.walk_amount
            + if offset.abs() > FREE_HEAD_TURN {
                BODY_FOLLOW_RATE
            } else {
                0.0
            };
        if rate > 0.0 {
            // Same frame-rate-independent blend as `walk_amount`.
            body += offset * (1.0 - (-dt * rate).exp());
            offset = wrap_angle(look_yaw - body);
        }

        // Whatever the easing did, the neck never over-twists: a head that jumped
        // takes the torso with it rather than wrapping round.
        if offset.abs() > MAX_HEAD_TURN {
            body = look_yaw - offset.clamp(-MAX_HEAD_TURN, MAX_HEAD_TURN);
        }

        // Wrapping the stored torso yaw is what keeps precision bounded: a look
        // yaw winds up all session, and the torso would follow it out of range.
        self.body_yaw = Some(wrap_angle(body));
        self.look_yaw = look_yaw;
    }

    /// Where the torso faces — the yaw a humanoid mesh is *built* at, as opposed
    /// to the look yaw that drives the camera and the movement basis.
    pub fn body_yaw(&self) -> f32 {
        self.body_yaw.unwrap_or(self.look_yaw)
    }

    /// Start a one-shot arm swing (e.g. placing a block, or a single click).
    pub fn trigger_swing(&mut self) {
        self.swing_timer = SWING_DURATION;
    }

    /// Keep a swing running: start one if none is in flight, and leave one
    /// already under way alone.
    ///
    /// This is what a *held* action wants. Mining is a sequence of blows rather
    /// than one long reach, so the arm should loop for as long as the button is
    /// down — but calling [`Self::trigger_swing`] every frame would reset the
    /// timer every frame and freeze the arm at the start of its arc instead.
    pub fn keep_swinging(&mut self) {
        if self.swing_timer <= 0.0 {
            self.trigger_swing();
        }
    }

    /// Progress through the one-shot swing: `0` as it starts, rising to `1` as
    /// it ends, and `0` whenever there is no swing running.
    ///
    /// The third-person arm reads the swing through [`Pose::right_arm`], which
    /// is an angle. A first-person view model needs the raw phase instead,
    /// because the hand traces a different curve than the shoulder does.
    pub fn swing_progress(&self) -> f32 {
        if self.swing_timer <= 0.0 {
            0.0
        } else {
            1.0 - self.swing_timer / SWING_DURATION
        }
    }

    /// Walk-cycle phase, for animation that rides the gait without being a limb.
    pub fn walk_phase(&self) -> f32 {
        self.walk_phase
    }

    /// Smoothed idle↔walk blend in `[0,1]`, the amplitude of that gait.
    pub fn walk_amount(&self) -> f32 {
        self.walk_amount
    }

    /// Smoothed horizontal speed in blocks/s — which *gait* is being used, as
    /// opposed to [`Self::walk_amount`]'s how much of one.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    /// How far the head is turned off the torso (radians).
    ///
    /// The mesh is drawn at [`Self::body_yaw`], which lags the look direction;
    /// this is what puts the face back where the entity is actually looking.
    /// Exposed as well as folded into [`Self::pose`] because a rigged model
    /// applies it to a *bone* rather than reading it off a box pose.
    pub fn head_offset(&self) -> f32 {
        wrap_angle(self.look_yaw - self.body_yaw())
    }

    /// Sample the current articulation into a [`Pose`]. `head_pitch` orients the head.
    pub fn pose(&self, head_pitch: f32) -> Pose {
        let swing = self.walk_phase.sin() * SWING_MAX * self.walk_amount;
        // Subtle idle sway, fading out as the walk takes over.
        let idle = self.idle_phase.sin() * IDLE_AMP * (1.0 - self.walk_amount);

        // One-shot swing on the main (right) arm: a smooth out-and-back over its window.
        let extra = if self.swing_timer > 0.0 {
            let progress = 1.0 - (self.swing_timer / SWING_DURATION);
            // Positive arm angle swings toward the model's front (-Z); see `rot_x`.
            (progress * PI).sin() * SWING_REACH
        } else {
            0.0
        };

        Pose {
            head_pitch,
            // The inventory preview overrides this to track the cursor instead.
            head_yaw: self.head_offset(),
            // Arms swing opposite the legs (diagonal gait); idle sway mirrors between
            // the arms; the one-shot swing rides on top of the right arm.
            left_arm: swing + idle,
            right_arm: -swing - idle + extra,
            left_leg: -swing,
            right_leg: swing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn idle_leaves_limbs_at_rest() {
        let mut anim = AnimationState::new();
        anim.advance(0.0, 0.0, 0.1);
        let p = anim.pose(0.0);
        assert_eq!(p.left_leg, 0.0);
        assert_eq!(p.right_leg, 0.0);
    }

    #[test]
    fn walk_phase_advances_only_when_moving() {
        let mut idle = AnimationState::new();
        idle.advance(0.0, 0.0, 0.5);
        assert_eq!(idle.walk_phase, 0.0);

        let mut moving = AnimationState::new();
        moving.advance(REFERENCE_SPEED, 0.0, 0.5);
        assert!(moving.walk_phase > 0.0, "walk_phase={}", moving.walk_phase);
    }

    #[test]
    fn walk_amount_rises_toward_one_then_decays() {
        let mut anim = AnimationState::new();
        for _ in 0..60 {
            anim.advance(REFERENCE_SPEED, 0.0, DT);
        }
        assert!(anim.walk_amount > 0.9, "rose to {}", anim.walk_amount);
        for _ in 0..60 {
            anim.advance(0.0, 0.0, DT);
        }
        assert!(anim.walk_amount < 0.1, "decayed to {}", anim.walk_amount);
    }

    #[test]
    fn walk_pose_legs_and_arms_are_anti_phase() {
        let mut anim = AnimationState::new();
        for _ in 0..20 {
            anim.advance(REFERENCE_SPEED, 0.0, DT);
        }
        let p = anim.pose(0.0);
        // Limbs are actually swinging.
        assert!(p.left_leg.abs() > 0.01, "left_leg={}", p.left_leg);
        // Legs oppose each other, and (with no active one-shot swing) so do the arms.
        assert!((p.left_leg + p.right_leg).abs() < 1e-6);
        assert!((p.left_arm + p.right_arm).abs() < 1e-6);
    }

    #[test]
    fn one_shot_swing_peaks_then_returns() {
        let mut anim = AnimationState::new();
        let baseline = anim.pose(0.0).right_arm;

        anim.trigger_swing();
        anim.advance(0.0, 0.0, SWING_DURATION / 2.0); // mid-swing → near peak reach
        let mid = anim.pose(0.0).right_arm;
        // Positive = toward the model's front (-Z), i.e. the punch swings forward.
        assert!(mid > baseline + 1.0, "mid={mid}, baseline={baseline}");

        anim.advance(0.0, 0.0, SWING_DURATION); // exhaust the swing window
        let after = anim.pose(0.0).right_arm;
        assert!(
            (after - baseline).abs() < 0.2,
            "after={after}, baseline={baseline}"
        );
    }

    #[test]
    fn head_pitch_passes_through() {
        let anim = AnimationState::new();
        assert_eq!(anim.pose(0.42).head_pitch, 0.42);
    }

    /// Holding the dig button must land repeated blows, each running its whole
    /// arc — where re-triggering every frame would pin the arm at the start of
    /// one arc forever.
    ///
    /// A swing ends exactly where it began (both curves are zero at the end of
    /// their arc, which `viewmodel::a_finished_swing_returns_the_hand` pins), so
    /// the single frame between one blow and the next is at the rest pose and
    /// invisible. What matters is that the next blow starts.
    #[test]
    fn a_held_swing_lands_repeated_blows() {
        let mut anim = AnimationState::new();
        let (frames, dt) = (12, SWING_DURATION / 3.0);
        let mut blows = 0;
        let mut deepest: f32 = 0.0;
        for _ in 0..frames {
            if anim.swing_progress() == 0.0 {
                blows += 1;
            }
            anim.keep_swinging();
            anim.advance(0.0, 0.0, dt);
            deepest = deepest.max(anim.swing_progress());
        }
        // Three frames to a swing, so twelve frames of holding is four blows.
        assert_eq!(
            blows, 4,
            "held mining swung {blows} time(s) in {frames} frames"
        );
        assert!(deepest > 0.5, "no swing got past the start of its arc");
    }

    /// ...and once the button is released, the swing finishes and stops.
    #[test]
    fn a_held_swing_ends_when_the_button_does() {
        let mut anim = AnimationState::new();
        anim.keep_swinging();
        anim.advance(0.0, 0.0, SWING_DURATION);
        assert_eq!(anim.swing_progress(), 0.0);
    }

    /// Keeping a swing alive must not restart one already under way.
    #[test]
    fn keeping_a_swing_alive_does_not_restart_it() {
        let mut anim = AnimationState::new();
        anim.trigger_swing();
        anim.advance(0.0, 0.0, SWING_DURATION / 2.0);
        let midway = anim.swing_progress();
        anim.keep_swinging();
        assert_eq!(anim.swing_progress(), midway);
    }

    /// Hold a look yaw for a second while standing still.
    fn stand_looking(look_yaw: f32) -> AnimationState {
        let mut anim = AnimationState::new();
        // Square up first, then turn the head — otherwise the first frame adopts
        // the new yaw wholesale and there is nothing to follow.
        anim.advance(0.0, 0.0, DT);
        for _ in 0..60 {
            anim.advance(0.0, look_yaw, DT);
        }
        anim
    }

    /// A fresh state must not spin its torso up from zero — an entity that appears
    /// already facing somewhere (a spawned mob, a remote player's first snapshot)
    /// is standing that way, not mid-turn.
    #[test]
    fn the_first_frame_squares_the_body_to_the_look() {
        let mut anim = AnimationState::new();
        anim.advance(0.0, 2.0, DT);
        assert!(
            (anim.body_yaw() - 2.0).abs() < 1e-4,
            "body at {}",
            anim.body_yaw()
        );
        assert!(anim.pose(0.0).head_yaw.abs() < 1e-4);
    }

    /// The point of the whole thing: standing still, a modest head turn moves the
    /// head and leaves the torso where it was.
    #[test]
    fn the_head_turns_freely_before_the_body_follows() {
        let look = 30.0 * PI / 180.0;
        let anim = stand_looking(look);
        assert!(
            anim.body_yaw().abs() < 1e-4,
            "torso should not have moved, but sits at {}",
            anim.body_yaw()
        );
        assert!(
            (anim.pose(0.0).head_yaw - look).abs() < 1e-4,
            "head_yaw={}, expected {look}",
            anim.pose(0.0).head_yaw
        );
    }

    /// ...but keep turning and the torso comes with you, settling just inside the
    /// free cone rather than at the hard cap.
    #[test]
    fn a_far_head_turn_drags_the_body_along() {
        let look = 90.0 * PI / 180.0;
        let anim = stand_looking(look);
        let offset = anim.pose(0.0).head_yaw;
        assert!(
            offset > 0.0,
            "the head should still lead, but offset={offset}"
        );
        assert!(
            offset <= FREE_HEAD_TURN + 1e-3,
            "torso stopped following at {offset} rad, past the free cone"
        );
        assert!(
            anim.body_yaw() > 0.5,
            "torso barely moved: {}",
            anim.body_yaw()
        );
    }

    /// One frame can jump the look yaw arbitrarily — a mouse flick, a snapped
    /// remote yaw, a mob's brain slamming its facing round. The neck must not wrap.
    #[test]
    fn the_neck_never_twists_past_the_cap() {
        let mut anim = AnimationState::new();
        anim.advance(0.0, 0.0, DT);
        for look in [PI, -PI, 2.5, -2.5] {
            let mut flicked = anim;
            flicked.advance(0.0, look, DT);
            let offset = flicked.pose(0.0).head_yaw;
            assert!(
                offset.abs() <= MAX_HEAD_TURN + 1e-4,
                "look {look} twisted the neck to {offset}"
            );
        }
    }

    /// Walking squares the torso up under the look direction, so nobody runs with
    /// a permanently twisted back.
    #[test]
    fn walking_squares_the_body_up_under_the_look() {
        let look = 40.0 * PI / 180.0;
        let mut anim = AnimationState::new();
        anim.advance(0.0, 0.0, DT);
        for _ in 0..60 {
            anim.advance(REFERENCE_SPEED, look, DT);
        }
        let offset = anim.pose(0.0).head_yaw;
        assert!(
            offset.abs() < 0.02,
            "a walking torso should be square under the head, but offset={offset}"
        );
    }

    /// The seam at ±π is nothing special: a look yaw that crosses it turns the
    /// short way, and the stored torso yaw stays bounded however far the head has
    /// wound round over a session.
    #[test]
    fn body_yaw_follows_across_the_wrap_seam() {
        let mut anim = AnimationState::new();
        anim.advance(0.0, PI - 0.05, DT);
        // Step just past the seam: a hair's turn, not a near-full one.
        for _ in 0..60 {
            anim.advance(REFERENCE_SPEED, -PI + 0.05, DT);
        }
        assert!(
            anim.body_yaw().abs() > PI - 0.2,
            "torso wandered to {}",
            anim.body_yaw()
        );
        assert!(anim.pose(0.0).head_yaw.abs() < 0.02);

        // A look yaw wound many turns round must not carry the torso out of range.
        let mut wound = AnimationState::new();
        for _ in 0..120 {
            wound.advance(REFERENCE_SPEED, 0.4 + 40.0 * TAU, DT);
        }
        assert!(
            wound.body_yaw().abs() <= PI,
            "torso yaw left (-pi, pi]: {}",
            wound.body_yaw()
        );
        assert!(wound.pose(0.0).head_yaw.abs() < 0.02);
    }
}
