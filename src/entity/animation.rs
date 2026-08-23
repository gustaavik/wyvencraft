//! Procedural animation for the humanoid model: a locomotion-driven walk cycle, a
//! subtle idle sway, and a one-shot arm swing (mining / placing). The state is
//! advanced each frame and sampled into a [`Pose`] consumed by `HumanoidModel`.
//!
//! Pure logic — no rendering or GPU dependencies — so it is cheap to unit test and is
//! reused for both the local player and movement-derived remote players.

use std::f32::consts::PI;

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

/// Accumulated animation state for one humanoid. `Default` is the rest pose.
#[derive(Debug, Clone, Copy, Default)]
pub struct AnimationState {
    /// Walk-cycle phase (radians), advanced by distance travelled.
    walk_phase: f32,
    /// Smoothed locomotion magnitude in `[0,1]` (idle↔walk blend).
    walk_amount: f32,
    /// Idle sway phase (radians), advanced by wall-clock time.
    idle_phase: f32,
    /// Remaining time of a one-shot arm swing (seconds); `0` = not swinging.
    swing_timer: f32,
}

impl AnimationState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Advance by `dt` seconds given the entity's current horizontal speed.
    pub fn advance(&mut self, horizontal_speed: f32, dt: f32) {
        let dt = dt.max(0.0);
        let speed = horizontal_speed.max(0.0);

        // Phase tracks distance so a faster gait swings faster; keep it bounded to
        // preserve float precision over long sessions.
        self.walk_phase = (self.walk_phase + speed * dt * GAIT_FREQ).rem_euclid(TAU);
        self.idle_phase = (self.idle_phase + dt * IDLE_FREQ).rem_euclid(TAU);

        let target = (speed / REFERENCE_SPEED).clamp(0.0, 1.0);
        let blend = 1.0 - (-dt * BLEND_RATE).exp();
        self.walk_amount += (target - self.walk_amount) * blend;

        self.swing_timer = (self.swing_timer - dt).max(0.0);
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
            // Head turn is only driven by the inventory preview; gameplay leaves it 0.
            head_yaw: 0.0,
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
        anim.advance(0.0, 0.1);
        let p = anim.pose(0.0);
        assert_eq!(p.left_leg, 0.0);
        assert_eq!(p.right_leg, 0.0);
    }

    #[test]
    fn walk_phase_advances_only_when_moving() {
        let mut idle = AnimationState::new();
        idle.advance(0.0, 0.5);
        assert_eq!(idle.walk_phase, 0.0);

        let mut moving = AnimationState::new();
        moving.advance(REFERENCE_SPEED, 0.5);
        assert!(moving.walk_phase > 0.0, "walk_phase={}", moving.walk_phase);
    }

    #[test]
    fn walk_amount_rises_toward_one_then_decays() {
        let mut anim = AnimationState::new();
        for _ in 0..60 {
            anim.advance(REFERENCE_SPEED, DT);
        }
        assert!(anim.walk_amount > 0.9, "rose to {}", anim.walk_amount);
        for _ in 0..60 {
            anim.advance(0.0, DT);
        }
        assert!(anim.walk_amount < 0.1, "decayed to {}", anim.walk_amount);
    }

    #[test]
    fn walk_pose_legs_and_arms_are_anti_phase() {
        let mut anim = AnimationState::new();
        for _ in 0..20 {
            anim.advance(REFERENCE_SPEED, DT);
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
        anim.advance(0.0, SWING_DURATION / 2.0); // mid-swing → near peak reach
        let mid = anim.pose(0.0).right_arm;
        // Positive = toward the model's front (-Z), i.e. the punch swings forward.
        assert!(mid > baseline + 1.0, "mid={mid}, baseline={baseline}");

        anim.advance(0.0, SWING_DURATION); // exhaust the swing window
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
            anim.advance(0.0, dt);
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
        anim.advance(0.0, SWING_DURATION);
        assert_eq!(anim.swing_progress(), 0.0);
    }

    /// Keeping a swing alive must not restart one already under way.
    #[test]
    fn keeping_a_swing_alive_does_not_restart_it() {
        let mut anim = AnimationState::new();
        anim.trigger_swing();
        anim.advance(0.0, SWING_DURATION / 2.0);
        let midway = anim.swing_progress();
        anim.keep_swinging();
        assert_eq!(anim.swing_progress(), midway);
    }
}
