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

    /// Start a one-shot arm swing (e.g. breaking or placing a block).
    pub fn trigger_swing(&mut self) {
        self.swing_timer = SWING_DURATION;
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
}
