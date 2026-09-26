//! Mobs: the authority's simulation, a client's replicas, and reaping.
//!
//! [`simulate`] runs the five steps of [`crate::domain::entity::mob`] as five
//! passes over every mob. Each step only ever read its own mob's state, so a
//! pass per step is exactly the per-mob call it replaces.

use glam::Vec3;

use crate::application::ecs::components::{
    Animation, Body, Boss, Decision, Health, Kind, Mob, MobId, Replica, Sensed, Sight, Transform,
    Velocity,
};
use crate::application::ecs::{CommandBuffer, Ecs, Entity};
use crate::domain::core::BlockPos;
use crate::domain::entity::boss::BossParams;
use crate::domain::entity::brain::{Perception, PlayerSighting};
use crate::domain::entity::mob::eye_position;
use crate::domain::entity::{MobAction, Motion};

/// What one mob did this frame, for the caller to resolve.
#[derive(Debug, Clone, Copy)]
pub struct MobStep {
    pub entity: Entity,
    pub id: MobId,
    pub action: MobAction,
    /// A boss entering a new phase this frame.
    pub phase_change: Option<u8>,
    /// Which of the perceive pass's targets the mob was looking at.
    pub target: Option<usize>,
    /// Where it sees and shoots from, after moving.
    pub eye: Vec3,
}

/// A mob that died and has been removed.
#[derive(Debug, Clone)]
pub struct Reaped {
    pub id: MobId,
    pub kind: String,
    pub position: Vec3,
    /// Present when it was a boss.
    pub boss: Option<BossParams>,
    /// Raw player id of whoever hit it last, for kill credit.
    pub last_attacker: Option<u64>,
}

/// Advance every simulated mob one step and report what each did.
///
/// - `is_active(feet)` — whether the mob's chunk is loaded. An inactive mob
///   sits the frame out entirely; unloaded chunks read as solid and would
///   trap it.
/// - `sense(eye)` — the nearest target the mob could attack, as an index into
///   the caller's own list and a sighting of it.
pub fn simulate(
    ecs: &mut Ecs,
    dt: f32,
    is_active: impl Fn(Vec3) -> bool,
    sense: impl Fn(Vec3) -> Option<(usize, PlayerSighting)>,
    is_solid: impl Fn(BlockPos) -> bool,
) -> Vec<MobStep> {
    perceive(ecs, is_active, sense);
    think(ecs, dt);
    steer(ecs, dt);
    integrate(ecs, dt, is_solid);
    animate(ecs, dt);
    act(ecs, dt)
}

/// Pass 1: what each active mob perceives. Drains the hurt flag.
pub fn perceive(
    ecs: &mut Ecs,
    is_active: impl Fn(Vec3) -> bool,
    sense: impl Fn(Vec3) -> Option<(usize, PlayerSighting)>,
) {
    ecs.for_each_mut::<(&mut Mob, &Transform, &Body, &mut Sensed), _>(
        |_, (mob, transform, body, sensed)| {
            if !is_active(transform.position) {
                sensed.0 = None;
                return;
            }
            let seen = sense(eye_position(transform.position, &body.physics));
            sensed.0 = Some(Sight {
                perception: Perception {
                    on_ground: mob.on_ground,
                    target: seen.map(|(_, sighting)| sighting),
                    hurt: mob.take_hurt(),
                },
                target: seen.map(|(index, _)| index),
            });
        },
    );
}

/// Pass 2: tick cooldowns and decide.
pub fn think(ecs: &mut Ecs, dt: f32) {
    ecs.for_each_mut::<(&mut Mob, &Sensed, &mut Transform, &mut Decision), _>(
        |_, (mob, sensed, transform, decision)| {
            if let Some(sight) = sensed.0 {
                decision.0 = mob.think(&sight.perception, &mut transform.yaw, dt);
            }
        },
    );
}

/// Pass 3: turn each decision into velocity.
pub fn steer(ecs: &mut Ecs, dt: f32) {
    ecs.for_each_mut::<(
        &Mob,
        &Sensed,
        &Decision,
        &Transform,
        &mut Velocity,
        &Body,
        Option<&Boss>,
    ), _>(
        |_, (mob, sensed, decision, transform, velocity, body, boss)| {
            if sensed.0.is_some() {
                mob.steer(
                    &decision.0,
                    transform.yaw,
                    boss,
                    &body.physics,
                    &mut velocity.0,
                    dt,
                );
            }
        },
    );
}

/// Pass 4: move through the world.
pub fn integrate(ecs: &mut Ecs, dt: f32, is_solid: impl Fn(BlockPos) -> bool) {
    ecs.for_each_mut::<(&mut Mob, &Sensed, &mut Transform, &mut Velocity, &Body), _>(
        |_, (mob, sensed, transform, velocity, body)| {
            if sensed.0.is_some() {
                mob.integrate(
                    &mut transform.position,
                    &mut velocity.0,
                    &body.physics,
                    dt,
                    &is_solid,
                );
            }
        },
    );
}

/// Pass 5: animate from how each mob actually moved. The look yaw drives the
/// torso, which follows it.
pub fn animate(ecs: &mut Ecs, dt: f32) {
    ecs.for_each_mut::<(&Mob, &Sensed, &Transform, &Velocity, &mut Animation), _>(
        |_, (mob, sensed, transform, velocity, anim)| {
            if sensed.0.is_none() {
                return;
            }
            let v = velocity.0;
            let horizontal = Vec3::new(v.x, 0.0, v.z).length();
            let motion = Motion::new(horizontal, v.y, !mob.on_ground);
            anim.0.advance(motion, transform.yaw, dt);
        },
    );
}

/// Pass 6: the attacks mobs commit to, and any boss phase change.
pub fn act(ecs: &mut Ecs, dt: f32) -> Vec<MobStep> {
    let mut steps = Vec::new();
    ecs.for_each_mut::<(
        &mut Mob,
        &Sensed,
        &Decision,
        &mut Animation,
        &Health,
        &MobId,
        Option<&mut Boss>,
    ), _>(
        |entity, (mob, sensed, decision, anim, health, id, mut boss)| {
            let Some(sight) = sensed.0 else {
                return;
            };
            let action = mob.act(
                &decision.0,
                &sight.perception,
                &mut anim.0,
                boss.as_deref_mut(),
                health,
                dt,
            );
            steps.push(MobStep {
                entity,
                id: *id,
                action,
                phase_change: boss.and_then(Boss::take_phase_change),
                target: sight.target,
                eye: Vec3::ZERO,
            });
        },
    );
    // The eye is read after the loop, from the body the mob now has.
    for step in &mut steps {
        if let (Some(t), Some(b)) = (
            ecs.get::<Transform>(step.entity),
            ecs.get::<Body>(step.entity),
        ) {
            step.eye = eye_position(t.position, &b.physics);
        }
    }
    steps
}

/// Animate every replica from the movement its snapshots show, clamped to
/// `max_speed` so a teleport cannot drive an absurd cadence.
pub fn animate_replicas(ecs: &mut Ecs, dt: f32, max_speed: f32) {
    ecs.for_each_mut::<(&Transform, &mut Animation, &mut Replica), _>(
        |_, (transform, anim, replica)| {
            let (now, then) = (transform.position, replica.last_position);
            let speed = if dt > 0.0 {
                (Vec3::new(now.x - then.x, 0.0, now.z - then.z).length() / dt).min(max_speed)
            } else {
                0.0
            };
            // A snapshot carries no grounded flag, so the vertical half is
            // read off the movement itself, as the horizontal half is.
            let motion = Motion::observed(speed, now.y - then.y, dt);
            anim.0.advance(motion, transform.yaw, dt);
            replica.last_position = now;
        },
    );
}

/// Remove every simulated mob whose health ran out, and report each.
pub fn reap(ecs: &mut Ecs) -> Vec<Reaped> {
    let mut reaped = Vec::new();
    let mut commands = CommandBuffer::new();
    ecs.for_each_mut::<(&Mob, &Health, &MobId, &Kind, &Transform, Option<&Boss>), _>(
        |entity, (mob, health, id, kind, transform, boss)| {
            if !health.is_dead() {
                return;
            }
            reaped.push(Reaped {
                id: *id,
                kind: kind.name.clone(),
                position: transform.position,
                boss: boss.map(|b| b.params.clone()),
                last_attacker: mob.last_attacker,
            });
            commands.despawn(entity);
        },
    );
    ecs.apply(&mut commands);
    reaped
}

/// The entity carrying mob id `id`, simulated or replicated. A linear scan:
/// a session holds tens of mobs, and a second index would be one more thing
/// to keep in step with every spawn and despawn.
pub fn find(ecs: &Ecs, id: MobId) -> Option<Entity> {
    ecs.query::<(&MobId,)>()
        .find(|(_, (candidate,))| **candidate == id)
        .map(|(entity, _)| entity)
}

/// Land a hit on a simulated mob: damage, knockback, kill credit. Returns the
/// health it has left, `None` if `entity` is no simulated mob.
pub fn hit(
    ecs: &mut Ecs,
    entity: Entity,
    damage: f32,
    knockback: Vec3,
    attacker: u64,
) -> Option<f32> {
    let mut left = None;
    ecs.for_each_mut::<(&mut Mob, &mut Health, &mut Velocity), _>(|e, (mob, health, velocity)| {
        if e == entity {
            mob.damage(health, &mut velocity.0, damage, knockback);
            mob.last_attacker = Some(attacker);
            left = Some(health.current);
        }
    });
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ecs::spawn;
    use crate::domain::entity::EntityRegistry;

    fn floor(p: BlockPos) -> bool {
        p.y < 64
    }

    fn spawn_kind(ecs: &mut Ecs, kind: &str, at: Vec3) -> Entity {
        let kinds = EntityRegistry::builtin();
        spawn::mob(ecs, kinds.find(kind).unwrap(), MobId(1), at, 42).expect("a mob kind")
    }

    fn nobody(_: Vec3) -> Option<(usize, PlayerSighting)> {
        None
    }

    fn position(ecs: &Ecs, e: Entity) -> Vec3 {
        ecs.get::<Transform>(e).unwrap().position
    }

    #[test]
    fn a_mob_settles_on_the_floor() {
        let mut ecs = Ecs::new();
        let cow = spawn_kind(&mut ecs, "cow", Vec3::new(0.5, 70.0, 0.5));
        for _ in 0..600 {
            simulate(&mut ecs, 1.0 / 60.0, |_| true, nobody, floor);
        }
        assert!((position(&ecs, cow).y - 64.0).abs() < 0.05);
        assert!(ecs.get::<Mob>(cow).unwrap().on_ground);
    }

    #[test]
    fn an_inactive_mob_sits_the_frame_out() {
        let mut ecs = Ecs::new();
        let cow = spawn_kind(&mut ecs, "cow", Vec3::new(0.5, 70.0, 0.5));
        let steps = simulate(&mut ecs, 0.5, |_| false, nobody, floor);
        assert!(steps.is_empty(), "no step reported for a frozen mob");
        assert_eq!(
            position(&ecs, cow),
            Vec3::new(0.5, 70.0, 0.5),
            "it did not fall"
        );
    }

    #[test]
    fn a_zombie_beside_its_target_swings_on_a_cooldown() {
        let mut ecs = Ecs::new();
        spawn_kind(&mut ecs, "zombie", Vec3::new(0.5, 64.0, 0.5));
        let close = |_: Vec3| {
            Some((
                7,
                PlayerSighting {
                    offset: Vec3::X,
                    distance: 1.0,
                    visible: true,
                },
            ))
        };
        let mut swings = 0;
        for _ in 0..60 {
            for step in simulate(&mut ecs, 0.05, |_| true, close, floor) {
                if let MobAction::Melee { damage } = step.action {
                    assert_eq!(damage, 3.0);
                    assert_eq!(step.target, Some(7), "the target index is carried through");
                    swings += 1;
                }
            }
        }
        assert!(
            (3..=4).contains(&swings),
            "about one swing a second, got {swings}"
        );
    }

    #[test]
    fn a_dead_mob_is_reaped_and_reported_once() {
        let mut ecs = Ecs::new();
        let cow = spawn_kind(&mut ecs, "cow", Vec3::new(0.5, 64.0, 0.5));
        assert_eq!(hit(&mut ecs, cow, 100.0, Vec3::ZERO, 9), Some(0.0));
        let reaped = reap(&mut ecs);
        assert_eq!(reaped.len(), 1);
        assert_eq!(reaped[0].kind, "cow");
        assert_eq!(reaped[0].last_attacker, Some(9));
        assert!(reaped[0].boss.is_none());
        assert!(!ecs.is_alive(cow));
        assert!(reap(&mut ecs).is_empty());
    }

    #[test]
    fn a_boss_is_spawned_with_its_boss_component() {
        let mut ecs = Ecs::new();
        let stag = spawn_kind(&mut ecs, "elder stag", Vec3::new(0.5, 64.0, 0.5));
        assert!(ecs.has::<Boss>(stag));
        let cow = spawn_kind(&mut ecs, "cow", Vec3::new(4.5, 64.0, 0.5));
        assert!(!ecs.has::<Boss>(cow));
    }

    #[test]
    fn mobs_are_found_by_id_whether_simulated_or_replicated() {
        let kinds = EntityRegistry::builtin();
        let mut ecs = Ecs::new();
        let local = spawn::mob(
            &mut ecs,
            kinds.find("cow").unwrap(),
            MobId(3),
            Vec3::ZERO,
            1,
        )
        .unwrap();
        let copy = spawn::replica(&mut ecs, kinds.find("pig").unwrap(), MobId(8), Vec3::ZERO);
        assert_eq!(find(&ecs, MobId(3)), Some(local));
        assert_eq!(find(&ecs, MobId(8)), Some(copy));
        assert_eq!(find(&ecs, MobId(4)), None);
        assert_eq!(
            hit(&mut ecs, copy, 1.0, Vec3::ZERO, 0),
            None,
            "a replica takes no hits"
        );
    }

    #[test]
    fn a_moving_replica_walks() {
        let kinds = EntityRegistry::builtin();
        let mut ecs = Ecs::new();
        let copy = spawn::replica(&mut ecs, kinds.find("cow").unwrap(), MobId(1), Vec3::ZERO);
        for i in 1..=30 {
            ecs.get_mut::<Transform>(copy).unwrap().position = Vec3::new(i as f32 * 0.05, 0.0, 0.0);
            animate_replicas(&mut ecs, 1.0 / 60.0, 12.0);
        }
        let anim = ecs.get::<Animation>(copy).unwrap().0;
        assert!(
            anim.speed() > 0.5,
            "observed motion drives the gait: {}",
            anim.speed()
        );
    }
}
