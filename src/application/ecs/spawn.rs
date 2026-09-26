//! The bundles each kind of entity starts life with.

use glam::Vec3;

use super::components::{Body, ItemDrop, Projectile, Transform, Velocity};
use super::{Ecs, Entity};
use crate::domain::entity::Launch;
use crate::domain::entity::kind::EntityKind;

/// Put a dropped item into the world. `kind` is the "dropped item" entity
/// kind, whose physics the drop collides with.
pub fn drop_item(ecs: &mut Ecs, (drop, launch): (ItemDrop, Launch), kind: &EntityKind) -> Entity {
    ecs.spawn((
        Transform::at(launch.position),
        Velocity(launch.velocity),
        Body(kind.physics),
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
