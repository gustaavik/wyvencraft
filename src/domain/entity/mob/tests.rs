//! Tests for [`super`]: `mob.rs`.

use super::*;
use crate::domain::entity::EntityRegistry;
use crate::domain::entity::brain::PlayerSighting;

use crate::domain::entity::animation::Motion;

/// One mob and the components it moves with, stepped through the five
/// steps in order — the same sequence the ECS passes run.
struct Sim {
    mob: Mob,
    health: Health,
    boss: Option<Boss>,
    position: Vec3,
    velocity: Vec3,
    yaw: f32,
    anim: AnimationState,
    body: PhysicsParams,
}

impl Sim {
    fn update(
        &mut self,
        dt: f32,
        perception: Perception,
        is_solid: impl Fn(BlockPos) -> bool,
    ) -> MobAction {
        let intent = self.mob.think(&perception, &mut self.yaw, dt);
        let body = self.body;
        self.mob.steer(
            &intent,
            self.yaw,
            self.boss.as_ref(),
            &body,
            &mut self.velocity,
            dt,
        );
        self.mob
            .integrate(&mut self.position, &mut self.velocity, &body, dt, is_solid);
        let horizontal = Vec3::new(self.velocity.x, 0.0, self.velocity.z).length();
        let motion = Motion::new(horizontal, self.velocity.y, !self.mob.on_ground);
        self.anim.advance(motion, self.yaw, dt);
        self.mob.act(
            &intent,
            &perception,
            &mut self.anim,
            self.boss.as_mut(),
            &self.health,
            dt,
        )
    }

    fn damage(&mut self, amount: f32, knockback: Vec3) {
        self.mob
            .damage(&mut self.health, &mut self.velocity, amount, knockback);
    }

    fn eye_position(&self) -> Vec3 {
        eye_position(self.position, &self.body)
    }
}

fn spawn(kind: &str, pos: Vec3) -> Sim {
    let kinds = EntityRegistry::builtin();
    let kind = kinds.find(kind).unwrap();
    let parts = MobParts::spawn(kind, 42).expect("mob kind");
    Sim {
        mob: parts.mob,
        health: parts.health,
        boss: parts.boss,
        position: pos,
        velocity: Vec3::ZERO,
        yaw: 0.0,
        anim: AnimationState::new(),
        body: kind.physics,
    }
}

fn seeing(offset: Vec3) -> Perception {
    Perception {
        on_ground: true,
        target: Some(PlayerSighting {
            offset,
            distance: offset.length(),
            visible: true,
        }),
        hurt: false,
    }
}

#[test]
fn kinds_without_a_mob_component_do_not_spawn() {
    let kinds = EntityRegistry::builtin();
    assert!(MobParts::spawn(kinds.player(), 0).is_none());
}

#[test]
fn mob_settles_on_flat_ground() {
    // Solid floor fills y < 64; the cow starts falling from above it.
    let solid = |p: BlockPos| p.y < 64;
    let mut cow = spawn("cow", Vec3::new(0.5, 70.0, 0.5));
    let calm = Perception {
        on_ground: false,
        target: None,
        hurt: false,
    };
    for _ in 0..600 {
        cow.update(1.0 / 60.0, calm, solid);
    }
    assert!(
        (cow.position.y - 64.0).abs() < 0.05,
        "feet should rest on the floor; y = {}",
        cow.position.y
    );
    assert!(cow.mob.on_ground);
}

#[test]
fn chasing_mob_jumps_a_one_block_ledge() {
    // Floor at y < 64, with a one-block step (y = 64) beyond x >= 3.
    let solid = |p: BlockPos| p.y < 64 || (p.x >= 3 && p.y == 64);
    let mut zombie = spawn("zombie", Vec3::new(0.5, 64.0, 0.5));
    let dt = 1.0 / 60.0;
    for _ in 0..600 {
        // A visible player 10 blocks along +X (eyes above the step's
        // walking surface) keeps the chase pinned.
        let offset = Vec3::new(10.5, 65.0 + 1.62, 0.5) - zombie.eye_position();
        zombie.update(dt, seeing(offset), solid);
    }
    assert!(
        zombie.position.x > 3.5,
        "zombie should clear the ledge; x = {}",
        zombie.position.x
    );
    assert!(
        (zombie.position.y - 65.0).abs() < 0.05,
        "zombie should stand on the step; y = {}",
        zombie.position.y
    );
}

#[test]
fn melee_attacks_are_cooldown_gated() {
    let solid = |p: BlockPos| p.y < 64;
    let mut zombie = spawn("zombie", Vec3::new(0.5, 64.0, 0.5));
    let close = seeing(Vec3::new(1.0, 0.0, 0.0));
    let dt = 0.05;
    let mut hits = 0;
    let steps = (3.0 / dt) as usize; // three seconds toe-to-toe
    for _ in 0..steps {
        if let MobAction::Melee { damage } = zombie.update(dt, close, solid) {
            assert_eq!(damage, 3.0);
            hits += 1;
        }
    }
    // 1 s cooldown over 3 s: the swing count is bounded, not per-frame.
    assert!((3..=4).contains(&hits), "expected ~3 swings, got {hits}");
}

#[test]
fn skeleton_fires_lofted_projectiles() {
    let solid = |p: BlockPos| p.y < 64;
    let mut skeleton = spawn("skeleton", Vec3::new(0.5, 64.0, 0.5));
    let target = seeing(Vec3::new(10.0, 0.0, 0.0));
    let mut fired = None;
    for _ in 0..100 {
        if let MobAction::Fire { velocity, damage } = skeleton.update(0.05, target, solid) {
            fired = Some((velocity, damage));
            break;
        }
    }
    let (velocity, damage) = fired.expect("skeleton should fire");
    assert_eq!(damage, 3.0);
    assert!(velocity.x > 15.0, "arrow flies at the target");
    assert!(velocity.y > 0.0, "arrow is lofted against gravity");
}

#[test]
fn damage_applies_knockback_and_marks_hurt() {
    let mut cow = spawn("cow", Vec3::new(0.5, 64.0, 0.5));
    cow.damage(4.0, Vec3::new(3.0, 2.0, 0.0));
    assert_eq!(cow.health.current, 6.0);
    // The pop goes into the velocity gravity integrates; the horizontal
    // shove is held apart so the gait can't overwrite it next tick.
    assert!(cow.velocity.y > 0.0, "vertical pop");
    assert!(cow.mob.impulse.x > 0.0, "horizontal shove");
    assert!(cow.mob.take_hurt());
    assert!(!cow.mob.take_hurt(), "hurt flag drains");
    cow.damage(100.0, Vec3::ZERO);
    assert!(cow.health.is_dead());
}

/// Knockback resistance is a dial, not a switch: the same impulse moves a
/// mob in proportion to what it does *not* resist.
#[test]
fn knockback_scales_with_resistance() {
    let push = Vec3::new(6.0, 0.0, 0.0);
    let speed_after = |resistance: f32| {
        let mut mob = spawn("cow", Vec3::new(0.5, 64.0, 0.5));
        mob.mob.params.knockback_resistance = resistance;
        mob.damage(1.0, push);
        mob.mob.impulse.x
    };
    assert_eq!(
        speed_after(0.0),
        push.x,
        "no resistance takes the full shove"
    );
    assert_eq!(speed_after(1.0), 0.0, "full resistance is immovable");
    assert_eq!(speed_after(0.5), push.x * 0.5, "and it scales in between");
    // Out-of-range values clamp rather than inverting the shove.
    assert_eq!(speed_after(2.0), 0.0);
    assert_eq!(speed_after(-1.0), push.x);
}

/// Regression guard: `update` rewrites `velocity.x/z` from the gait every
/// frame, so an impulse folded straight into `velocity` is erased on the
/// next tick and knockback silently does nothing. It has to actually carry
/// the mob somewhere.
#[test]
fn knockback_survives_the_gait_overwriting_velocity() {
    let solid = |p: BlockPos| p.y < 64;
    let mut cow = spawn("cow", Vec3::new(0.5, 64.0, 0.5));
    let start = cow.position;
    let calm = Perception {
        on_ground: true,
        target: None,
        hurt: false,
    };
    cow.damage(1.0, Vec3::new(6.0, 3.0, 0.0));
    for _ in 0..40 {
        cow.update(1.0 / 60.0, calm, solid);
    }
    assert!(
        cow.position.x - start.x > 0.5,
        "the shove should carry it: {} -> {}",
        start,
        cow.position
    );
    // ...and then bleed off against the ground. Measured on the impulse
    // rather than on position: a passive cow wanders off under its own
    // power, which is movement but not the shove still acting.
    for _ in 0..120 {
        cow.update(1.0 / 60.0, calm, solid);
    }
    assert!(
        cow.mob.impulse.length() < 0.01,
        "the shove should decay, not persist: {}",
        cow.mob.impulse
    );
}

/// The case the two axes exist for: a fixture that decides nothing but is
/// *not* bolted down still gets sent flying by a hit — and still never
/// turns to face whoever threw it.
#[test]
fn an_inert_prop_can_still_be_knocked_back() {
    let solid = |p: BlockPos| p.y < 64;
    let mut prop = spawn("vine sword", Vec3::new(0.5, 64.0, 0.5));
    prop.mob.params.knockback_resistance = 0.0;
    let start = prop.position;

    prop.damage(1.0, Vec3::new(6.0, 3.0, 0.0));
    for _ in 0..30 {
        prop.update(
            1.0 / 60.0,
            Perception {
                on_ground: true,
                target: Some(PlayerSighting {
                    offset: Vec3::new(-1.5, 0.0, 0.0),
                    distance: 1.5,
                    visible: true,
                }),
                hurt: true,
            },
            solid,
        );
    }
    assert!(
        prop.position.x - start.x > 0.5,
        "an unresisting prop should be shoved: {} -> {}",
        start,
        prop.position
    );
    assert_eq!(prop.yaw, 0.0, "but it still must not turn");
}

/// A fixture that *is* bolted down can be broken, but not shoved or turned.
/// This is the whole-body version of the brain test: over a long run with a
/// player right next to it and repeated hits, it must not have budged.
#[test]
fn a_bolted_down_prop_is_never_moved_or_turned_by_being_hit() {
    let solid = |p: BlockPos| p.y < 64;
    let mut prop = spawn("vine sword", Vec3::new(0.5, 64.0, 0.5));
    assert_eq!(prop.mob.params.knockback_resistance, 1.0, "as shipped");
    let start = prop.position;
    let dt = 1.0 / 60.0;

    for step in 0..600 {
        // A player standing right beside it, hitting it every half second.
        let mut p = Perception {
            on_ground: true,
            target: Some(PlayerSighting {
                offset: Vec3::new(1.5, 0.0, 0.5),
                distance: 1.6,
                visible: true,
            }),
            hurt: false,
        };
        if step % 30 == 0 {
            prop.damage(0.01, Vec3::new(6.0, 3.0, 0.0));
            p.hurt = true;
        }
        prop.update(dt, p, solid);
    }

    assert_eq!(prop.yaw, 0.0, "a prop must not rotate");
    assert!(
        (prop.position.x - start.x).abs() < 1e-3 && (prop.position.z - start.z).abs() < 1e-3,
        "a prop must not be pushed: {} -> {}",
        start,
        prop.position
    );
    assert!(
        prop.health.current < prop.mob.params.max_health,
        "but it can be damaged"
    );
}
