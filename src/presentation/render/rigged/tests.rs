//! Tests for [`super`]: `rigged.rs`.

use super::*;
use crate::domain::entity::Motion;
use fixture::{Player, walking};

/// The frame `hand_anchor` hands an item, for a player standing still.
fn resting_anchor(player: &Player, position: Vec3, yaw: f32) -> Mat4 {
    let character = player.character();
    let pose = character
        .rest_pose()
        .expect("the shipped player model is rigged");
    character
        .hand_anchor(&pose, position, yaw)
        .expect("it has a right hand")
}

fn approx(a: Vec3, b: Vec3) -> bool {
    (a - b).abs().max_element() < 1e-4
}

/// The whole of the third-person placement bug, as one assertion.
///
/// An authored `thirdperson_righthand` entry is measured against Minecraft's
/// hand space — **+X right, +Y forward, +Z up** — and not against the body's
/// own axes, which are +X right, +Y *up*, −Z forward. Before `hand_space`
/// the two were the same matrix, so every sword came out a quarter turn
/// wrong and every `translation` pushed the item up instead of out.
#[test]
fn the_hand_frame_is_minecrafts_and_not_the_bodys() {
    let player = Player::load();
    let anchor = resting_anchor(&player, Vec3::ZERO, 0.0);

    // At yaw 0 a character faces −Z, so "forward" is −Z and "right" is +X.
    assert!(
        approx(anchor.transform_vector3(Vec3::X), Vec3::X),
        "+X is the character's right"
    );
    assert!(
        approx(anchor.transform_vector3(Vec3::Y), -Vec3::Z),
        "+Y is the direction the character faces"
    );
    assert!(
        approx(anchor.transform_vector3(Vec3::Z), Vec3::Y),
        "+Z is up"
    );
}

/// The frame turns with the body, so an item cannot swing out of the fist.
#[test]
fn the_hand_frame_follows_the_body_round() {
    let player = Player::load();
    let anchor = resting_anchor(&player, Vec3::ZERO, FRAC_PI_2);

    // A quarter turn from facing −Z is facing +X — the sign `Player::yaw`
    // and `yaw_matrix` agree on.
    assert!(
        approx(anchor.transform_vector3(Vec3::Y), Vec3::X),
        "forward turned with the body"
    );
    assert!(
        approx(anchor.transform_vector3(Vec3::Z), Vec3::Y),
        "up is still up"
    );
}

/// The item is placed at the fist, not at the player's feet.
#[test]
fn the_hand_frame_sits_on_the_hand_joint() {
    let player = Player::load();
    let character = player.character();
    let pose = character.rest_pose().expect("rigged");
    let hand = character.clips.right_hand().expect("a hand");
    let position = Vec3::new(10.0, 70.0, -4.0);

    let joint = character.joint(&pose, hand).expect("a joint");
    let fist = character.placement(position, 0.0).transform_point3(joint);
    let origin = resting_anchor(&player, position, 0.0).transform_point3(Vec3::ZERO);

    assert!(approx(origin, fist), "{origin} is not the fist at {fist}");
}

/// A 1.64× player does not swing a 1.64× pickaxe: the character's scale
/// carries the *joint* out into the world and is then left behind, so an
/// item is sized only by its own `display` entry.
#[test]
fn the_hand_frame_carries_no_scale() {
    let player = Player::load();
    let anchor = resting_anchor(&player, Vec3::ZERO, 0.7);

    for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
        let length = anchor.transform_vector3(axis).length();
        assert!((length - 1.0).abs() < 1e-4, "{axis} was scaled to {length}");
    }
}

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

/// Rising, hanging and falling have to be three different shapes, or the
/// jump reads as one held pose for its whole flight.
#[test]
fn the_jump_clip_reads_the_arc_from_vertical_speed() {
    let player = Player::load();
    let character = player.character();
    let rig = character.rig().expect("rigged");
    let bound = character.clips;
    let knee = rig.bone("knee_r").expect("knee_r");

    // `jump_speed` for this fixture is 9.0: the clip's two ends.
    let launch = bound.pose(rig, &fixture::airborne(9.0), HeadLook::default());
    let apex = bound.pose(rig, &fixture::airborne(0.0), HeadLook::default());
    let fall = bound.pose(rig, &fixture::airborne(-9.0), HeadLook::default());

    let bend = |pose: &Pose| pose.get(knee).rotation.x.to_degrees();
    // Authored: -12 pushing off, -65 tucked at the apex, -22 reaching down.
    assert!(
        bend(&apex) < bend(&launch),
        "the knee should tuck at the apex"
    );
    assert!(bend(&apex) < bend(&fall), "and come back down to land");
    assert!(
        bend(&launch) < 0.0 && bend(&fall) < 0.0,
        "a knee only bends one way"
    );
}

/// Past the ends of the arc the pose holds rather than wrapping — a long
/// fall must not loop back round to the launch.
#[test]
fn falling_faster_than_a_jump_holds_the_falling_pose() {
    let player = Player::load();
    let character = player.character();
    let rig = character.rig().expect("rigged");
    let bound = character.clips;

    let fall = bound.pose(rig, &fixture::airborne(-9.0), HeadLook::default());
    let plummet = bound.pose(rig, &fixture::airborne(-40.0), HeadLook::default());
    assert_eq!(fall, plummet, "terminal velocity is still the falling pose");
}

/// The visible symptom of not holding the landing speed: the knee snapping
/// back up into the tuck at the moment the feet touch down.
#[test]
fn touching_down_keeps_the_pose_it_landed_in() {
    let player = Player::load();
    let character = player.character();
    let rig = character.rig().expect("rigged");
    let bound = character.clips;
    let knee = rig.bone("knee_r").expect("knee_r");

    let mut anim = fixture::airborne(-9.0);
    let falling = bound.pose(rig, &anim, HeadLook::default());

    // One frame of contact: grounded, and the physics has zeroed velocity.y.
    anim.advance(Motion::still(), 0.0, 1.0 / 60.0);
    let landed = bound.pose(rig, &anim, HeadLook::default());

    let bend = |pose: &Pose| pose.get(knee).rotation.x.to_degrees();
    let apex = bound.pose(rig, &fixture::airborne(0.0), HeadLook::default());

    // The knee must relax *out* of the landing pose toward rest, not bend
    // further into the tuck. That sign is the whole bug: reading the zeroed
    // velocity on touchdown sent it the other way.
    assert!(
        bend(&landed) > bend(&falling),
        "the knee should straighten on landing, not tuck: landed {} vs falling {}",
        bend(&landed),
        bend(&falling)
    );
    assert!(
        bend(&landed) > bend(&apex) + 50.0,
        "landed {} is nowhere near the apex tuck {}",
        bend(&landed),
        bend(&apex)
    );
}

/// The jump replaces the gait rather than riding on it, but only once the
/// body is actually off the ground.
#[test]
fn a_grounded_character_shows_no_trace_of_the_jump() {
    let player = Player::load();
    let character = player.character();
    let rig = character.rig().expect("rigged");
    let bound = character.clips;

    let walked = bound.pose(rig, &walking(4.3), HeadLook::default());
    // Same gait, but airborne: the pose has to change.
    let mut anim = walking(4.3);
    for _ in 0..30 {
        anim.advance(Motion::new(4.3, 6.0, true), 0.0, 1.0 / 60.0);
    }
    let jumped = bound.pose(rig, &anim, HeadLook::default());
    assert_ne!(walked, jumped, "leaving the ground should change the pose");
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
        anim.advance(Motion::still(), 0.0, 1.0 / 60.0);
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
    anim.advance(Motion::walking(4.3), 0.0, 0.125);
    let swinging = bound.pose(rig, &anim, HeadLook::default()).get(arm);

    assert!(
        swinging.rotation.x > resting.rotation.x,
        "the swing reaches forward: {} vs {}",
        swinging.rotation.x,
        resting.rotation.x
    );
}
