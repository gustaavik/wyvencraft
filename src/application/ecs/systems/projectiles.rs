//! Arrows: flight, and what they hit.

use crate::application::ecs::components::{Projectile, Transform, Velocity};
use crate::application::ecs::{CommandBuffer, Ecs};
use crate::domain::core::{Aabb, BlockPos};

/// Fly every projectile one step. A spent one — into a wall, or past its
/// lifetime — despawns; one entering a target's box despawns and is reported
/// as `(target, damage)`. Targets are tried in the order given, so a caller
/// listing the local player first gives it precedence where boxes overlap.
pub fn fly<T: Copy>(
    ecs: &mut Ecs,
    dt: f32,
    is_solid: impl Fn(BlockPos) -> bool,
    targets: &[(T, Aabb)],
) -> Vec<(T, f32)> {
    let mut hits = Vec::new();
    let mut commands = CommandBuffer::new();
    ecs.for_each_mut::<(&mut Transform, &mut Velocity, &mut Projectile), _>(
        |entity, (transform, velocity, arrow)| {
            if !arrow.step(&mut transform.position, &mut velocity.0, dt, &is_solid) {
                commands.despawn(entity);
                return;
            }
            let hitbox = Projectile::aabb(transform.position);
            if let Some((target, _)) = targets.iter().find(|(_, b)| b.intersects(hitbox)) {
                hits.push((*target, arrow.damage));
                commands.despawn(entity);
            }
        },
    );
    ecs.apply(&mut commands);
    hits
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::application::ecs::spawn;

    fn open(_: BlockPos) -> bool {
        false
    }

    #[test]
    fn an_arrow_hits_the_first_target_it_enters() {
        let mut ecs = Ecs::new();
        let e = spawn::arrow(
            &mut ecs,
            Vec3::new(0.0, 70.0, 0.0),
            Vec3::X * 18.0,
            3.0,
            0.0,
            8.0,
        );
        let target = Aabb::from_center_size(Vec3::new(2.0, 70.0, 0.0), Vec3::splat(1.0));
        let mut hits = Vec::new();
        for _ in 0..30 {
            hits.extend(fly(
                &mut ecs,
                1.0 / 60.0,
                open,
                &[("a", target), ("b", target)],
            ));
        }
        assert_eq!(hits, [("a", 3.0)]);
        assert!(!ecs.is_alive(e));
    }

    #[test]
    fn an_arrow_into_a_wall_is_spent_without_a_hit() {
        let mut ecs = Ecs::new();
        let e = spawn::arrow(
            &mut ecs,
            Vec3::new(0.0, 70.0, 0.0),
            Vec3::X * 18.0,
            3.0,
            0.0,
            8.0,
        );
        let mut hits: Vec<((), f32)> = Vec::new();
        for _ in 0..60 {
            hits.extend(fly(&mut ecs, 1.0 / 60.0, |p: BlockPos| p.x >= 3, &[]));
        }
        assert!(hits.is_empty());
        assert!(!ecs.is_alive(e));
    }

    #[test]
    fn an_arrow_in_open_air_keeps_flying() {
        let mut ecs = Ecs::new();
        let e = spawn::arrow(
            &mut ecs,
            Vec3::new(0.0, 70.0, 0.0),
            Vec3::X * 18.0,
            3.0,
            20.0,
            8.0,
        );
        let _ = fly::<()>(&mut ecs, 0.5, open, &[]);
        assert!(ecs.is_alive(e));
        assert!(ecs.get::<Transform>(e).unwrap().position.x > 8.0);
    }
}
