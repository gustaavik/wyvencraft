//! The bundles each kind of entity starts life with.

use glam::Vec3;

use super::components::{
    Animation, Body, Decision, Health, ItemDrop, Kind, MobId, Projectile, Replica, Sensed,
    Transform, Velocity,
};
use super::{Ecs, Entity};
use crate::domain::entity::kind::EntityKind;
use crate::domain::entity::{AnimationState, Launch, MobParts};

/// Put a dropped item into the world. `kind` is the "dropped item" entity
/// kind, whose physics the drop collides with.
pub fn drop_item(ecs: &mut Ecs, (drop, launch): (ItemDrop, Launch), kind: &EntityKind) -> Entity {
    ecs.spawn((
        Transform::at(launch.position),
        Velocity(launch.velocity),
        Body::centred(kind.physics),
        drop,
    ))
}

/// Launch an arrow from `position` at `velocity`.
pub fn arrow(
    ecs: &mut Ecs,
    position: Vec3,
    velocity: Vec3,
    damage: f32,
    gravity: f32,
    lifetime: f32,
) -> Entity {
    ecs.spawn((
        Transform::at(position),
        Velocity(velocity),
        Projectile::new(damage, gravity, lifetime),
    ))
}

/// Put a simulated mob of `kind` into the world, standing at `position`.
/// `None` when the kind is no mob. `seed` fixes its brains' random streams.
pub fn mob(
    ecs: &mut Ecs,
    kind: &EntityKind,
    id: MobId,
    position: Vec3,
    seed: u64,
) -> Option<Entity> {
    let MobParts { mob, health, boss } = MobParts::spawn(kind, seed)?;
    let entity = ecs.spawn((
        id,
        Kind {
            name: kind.name.clone(),
            visual: kind.visual.clone(),
        },
        Transform::at(position),
        Velocity::default(),
        Body::standing(kind.physics),
        mob,
        health,
        Animation(AnimationState::new()),
        Sensed::default(),
        Decision::default(),
    ));
    if let Some(boss) = boss {
        ecs.insert(entity, boss);
    }
    Some(entity)
}

/// A client's copy of the host's mob `id`, first seen at `position`.
pub fn replica(ecs: &mut Ecs, kind: &EntityKind, id: MobId, position: Vec3) -> Entity {
    ecs.spawn((
        id,
        Kind {
            name: kind.name.clone(),
            visual: kind.visual.clone(),
        },
        Transform::at(position),
        Body::standing(kind.physics),
        Health::full(kind.mob.as_ref().map_or(1.0, |m| m.max_health)),
        Animation(AnimationState::new()),
        Replica {
            last_position: position,
            phase: 0,
        },
    ))
}
