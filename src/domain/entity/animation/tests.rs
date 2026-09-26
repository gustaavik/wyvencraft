//! Tests for [`super`]: `animation.rs`.

use super::*;

const DT: f32 = 1.0 / 60.0;

/// Landing is instant in the physics and must not be instant in the pose,
/// so the airborne blend ramps both ways rather than switching.
#[test]
fn leaving_the_ground_blends_in_and_landing_blends_out() {
    let mut anim = AnimationState::new();
    assert_eq!(anim.air_amount(), 0.0, "starts planted");

    for _ in 0..30 {
        anim.advance(Motion::new(0.0, 6.0, true), 0.0, DT);
    }
    let airborne = anim.air_amount();
    assert!(
        airborne > 0.9,
        "half a second of air should read as air: {airborne}"
    );

    for _ in 0..30 {
        anim.advance(Motion::still(), 0.0, DT);
    }
    assert!(
        anim.air_amount() < 0.1,
        "landing should settle: {}",
        anim.air_amount()
    );
}

/// The jump pose is picked by where in the arc the body is, so the vertical
/// speed it is picked from must not be smoothed — a lagged value would put
/// the tuck somewhere other than the apex.
#[test]
fn vertical_speed_is_reported_unsmoothed() {
    let mut anim = AnimationState::new();
    anim.advance(Motion::new(0.0, 9.0, true), 0.0, DT);
    assert_eq!(anim.vertical_speed(), 9.0);
    anim.advance(Motion::new(0.0, -4.5, true), 0.0, DT);
    assert_eq!(anim.vertical_speed(), -4.5, "no easing between frames");
}

/// Landing zeroes the physics velocity in the same frame it plants the
/// feet, and zero is the *middle* of the jump clip. Reading it on touchdown
/// threw the body back through the apex tuck and played the blend-out
/// backwards — a fast little replay right as the player landed.
#[test]
fn landing_holds_the_speed_it_arrived_at_rather_than_snapping_to_zero() {
    let mut anim = AnimationState::new();
    for _ in 0..30 {
        anim.advance(Motion::new(0.0, -9.0, true), 0.0, DT);
    }
    assert_eq!(anim.vertical_speed(), -9.0, "falling");

    // Touchdown: on_ground and velocity.y = 0 arrive together.
    anim.advance(Motion::still(), 0.0, DT);
    assert_eq!(
        anim.vertical_speed(),
        -9.0,
        "the fade must start from the pose it landed in"
    );
    assert!(
        anim.air_amount() < 1.0,
        "and the blend must be on its way out"
    );

    // A fresh jump takes over immediately.
    anim.advance(Motion::new(0.0, 9.0, true), 0.0, DT);
    assert_eq!(anim.vertical_speed(), 9.0);
}

/// A peer's grounded flag never crosses the wire, so it is inferred from the
/// movement — but walking up a slope must not read as a jump.
#[test]
fn observed_motion_tells_a_jump_from_a_step_up() {
    let stepping = Motion::observed(4.3, 0.6 * DT, DT);
    assert!(!stepping.airborne, "a slope is not a jump");

    let jumping = Motion::observed(4.3, 9.0 * DT, DT);
    assert!(jumping.airborne);
    assert!((jumping.vertical_speed - 9.0).abs() < 1e-3);

    let falling = Motion::observed(0.0, -9.0 * DT, DT);
    assert!(falling.airborne && falling.vertical_speed < 0.0);
}

/// The gait keeps accruing in mid-air: a running jump has to land back into
/// the stride it left, and `walk_phase` is what remembers where that was.
#[test]
fn the_gait_keeps_running_while_airborne() {
    let mut anim = AnimationState::new();
    anim.advance(Motion::new(REFERENCE_SPEED, 6.0, true), 0.0, DT);
    let before = anim.walk_phase();
    anim.advance(Motion::new(REFERENCE_SPEED, 5.0, true), 0.0, DT);
    assert!(anim.walk_phase() > before, "the stride did not freeze");
}

#[test]
fn idle_leaves_limbs_at_rest() {
    let mut anim = AnimationState::new();
    anim.advance(Motion::still(), 0.0, 0.1);
    let p = anim.pose(0.0);
    assert_eq!(p.left_leg, 0.0);
    assert_eq!(p.right_leg, 0.0);
}

#[test]
fn walk_phase_advances_only_when_moving() {
    let mut idle = AnimationState::new();
    idle.advance(Motion::still(), 0.0, 0.5);
    assert_eq!(idle.walk_phase, 0.0);

    let mut moving = AnimationState::new();
    moving.advance(Motion::walking(REFERENCE_SPEED), 0.0, 0.5);
    assert!(moving.walk_phase > 0.0, "walk_phase={}", moving.walk_phase);
}

#[test]
fn walk_amount_rises_toward_one_then_decays() {
    let mut anim = AnimationState::new();
    for _ in 0..60 {
        anim.advance(Motion::walking(REFERENCE_SPEED), 0.0, DT);
    }
    assert!(anim.walk_amount > 0.9, "rose to {}", anim.walk_amount);
    for _ in 0..60 {
        anim.advance(Motion::still(), 0.0, DT);
    }
    assert!(anim.walk_amount < 0.1, "decayed to {}", anim.walk_amount);
}

#[test]
fn walk_pose_legs_and_arms_are_anti_phase() {
    let mut anim = AnimationState::new();
    for _ in 0..20 {
        anim.advance(Motion::walking(REFERENCE_SPEED), 0.0, DT);
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
    anim.advance(Motion::still(), 0.0, SWING_DURATION / 2.0); // mid-swing → near peak reach
    let mid = anim.pose(0.0).right_arm;
    // Positive = toward the model's front (-Z), i.e. the punch swings forward.
    assert!(mid > baseline + 1.0, "mid={mid}, baseline={baseline}");

    anim.advance(Motion::still(), 0.0, SWING_DURATION); // exhaust the swing window
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
        anim.advance(Motion::still(), 0.0, dt);
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
    anim.advance(Motion::still(), 0.0, SWING_DURATION);
    assert_eq!(anim.swing_progress(), 0.0);
}

/// Keeping a swing alive must not restart one already under way.
#[test]
fn keeping_a_swing_alive_does_not_restart_it() {
    let mut anim = AnimationState::new();
    anim.trigger_swing();
    anim.advance(Motion::still(), 0.0, SWING_DURATION / 2.0);
    let midway = anim.swing_progress();
    anim.keep_swinging();
    assert_eq!(anim.swing_progress(), midway);
}

/// Hold a look yaw for a second while standing still.
fn stand_looking(look_yaw: f32) -> AnimationState {
    let mut anim = AnimationState::new();
    // Square up first, then turn the head — otherwise the first frame adopts
    // the new yaw wholesale and there is nothing to follow.
    anim.advance(Motion::still(), 0.0, DT);
    for _ in 0..60 {
        anim.advance(Motion::still(), look_yaw, DT);
    }
    anim
}

/// A fresh state must not spin its torso up from zero — an entity that appears
/// already facing somewhere (a spawned mob, a remote player's first snapshot)
/// is standing that way, not mid-turn.
#[test]
fn the_first_frame_squares_the_body_to_the_look() {
    let mut anim = AnimationState::new();
    anim.advance(Motion::still(), 2.0, DT);
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
    anim.advance(Motion::still(), 0.0, DT);
    for look in [PI, -PI, 2.5, -2.5] {
        let mut flicked = anim;
        flicked.advance(Motion::still(), look, DT);
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
    anim.advance(Motion::still(), 0.0, DT);
    for _ in 0..60 {
        anim.advance(Motion::walking(REFERENCE_SPEED), look, DT);
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
    anim.advance(Motion::still(), PI - 0.05, DT);
    // Step just past the seam: a hair's turn, not a near-full one.
    for _ in 0..60 {
        anim.advance(Motion::walking(REFERENCE_SPEED), -PI + 0.05, DT);
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
        wound.advance(Motion::walking(REFERENCE_SPEED), 0.4 + 40.0 * TAU, DT);
    }
    assert!(
        wound.body_yaw().abs() <= PI,
        "torso yaw left (-pi, pi]: {}",
        wound.body_yaw()
    );
    assert!(wound.pose(0.0).head_yaw.abs() < 0.02);
}
