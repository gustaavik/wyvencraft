//! Projectiles: the skeleton's arrow. A point-mass ballistic body — no
//! swept collision, just gravity and a per-step solid-block test, which is
//! plenty at arrow speeds and the game's clamped timestep.
//!
//! Every peer simulates arrows it knows about (they're cheap and purely
//! visual off the authority); only the authority tests player hits and
//! applies damage, mirroring how mobs work.
//!
//! [`Projectile`] is the arrow apart from where it is; position and velocity
//! are the shared ECS components and are passed in.

use glam::Vec3;

use crate::domain::core::{Aabb, BlockPos};

/// Rendered/collision cube edge for an arrow (blocks).
const ARROW_SIZE: f32 = 0.15;

/// An arrow in flight. Tuning comes from the firing kind's
/// `[entity.mob.ranged]` params, copied in at launch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Projectile {
    /// Damage on a player hit (applied by the authority only).
    pub damage: f32,
    gravity: f32,
    age: f32,
    lifetime: f32,
}

impl Projectile {
    pub fn new(damage: f32, gravity: f32, lifetime: f32) -> Self {
        Self {
            damage,
            gravity,
            age: 0.0,
            lifetime,
        }
    }

    /// Hit-test box around the arrow head at `position`.
    pub fn aabb(position: Vec3) -> Aabb {
        Aabb::from_center_size(position, Vec3::splat(ARROW_SIZE))
    }

    /// Edge length the arrow is drawn at.
    pub const SIZE: f32 = ARROW_SIZE;

    /// Advance one step. Returns `false` when the arrow is spent — it flew
    /// into a solid block or outlived its lifetime — and should despawn.
    pub fn step(
        &mut self,
        position: &mut Vec3,
        velocity: &mut Vec3,
        dt: f32,
        is_solid: impl Fn(BlockPos) -> bool,
    ) -> bool {
        self.age += dt;
        if self.age >= self.lifetime {
            return false;
        }
        velocity.y -= self.gravity * dt;
        let next = *position + *velocity * dt;
        if is_solid(BlockPos::from_world(next)) {
            return false;
        }
        *position = next;
        true
    }

    /// Facing for rendering: yaw along the horizontal flight direction.
    pub fn yaw(velocity: Vec3) -> f32 {
        velocity.x.atan2(-velocity.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arrow and the two components it flies with.
    struct Flight {
        arrow: Projectile,
        position: Vec3,
        velocity: Vec3,
    }

    impl Flight {
        fn update(&mut self, dt: f32, is_solid: impl Fn(BlockPos) -> bool) -> bool {
            self.arrow
                .step(&mut self.position, &mut self.velocity, dt, is_solid)
        }
    }

    fn arrow(velocity: Vec3) -> Flight {
        // Skeleton-like tuning: 18 blocks/s, 20 blocks/s² drop, 8 s life.
        Flight {
            arrow: Projectile::new(3.0, 20.0, 8.0),
            position: Vec3::new(0.0, 70.0, 0.0),
            velocity,
        }
    }

    #[test]
    fn arrows_arc_under_gravity() {
        let open = |_: BlockPos| false;
        let mut a = arrow(Vec3::new(18.0, 2.0, 0.0));
        let mut peak = a.position.y;
        for _ in 0..60 {
            assert!(a.update(1.0 / 60.0, open));
            peak = peak.max(a.position.y);
        }
        assert!(a.position.x > 15.0, "flies forward: x = {}", a.position.x);
        assert!(peak > 70.0, "rises first: peak = {peak}");
        assert!(a.position.y < peak, "then falls: y = {}", a.position.y);
    }

    #[test]
    fn arrows_stop_at_a_wall() {
        let wall = |p: BlockPos| p.x >= 5;
        let mut a = arrow(Vec3::new(18.0, 0.0, 0.0));
        let mut steps = 0;
        while a.update(1.0 / 60.0, wall) {
            steps += 1;
            assert!(steps < 600, "arrow should hit the wall");
        }
        assert!(a.position.x < 5.0, "stopped before the wall");
    }

    #[test]
    fn arrows_expire() {
        let open = |_: BlockPos| false;
        let mut a = arrow(Vec3::new(1.0, 0.0, 0.0));
        let mut alive = 0;
        while a.update(0.1, open) {
            alive += 1;
            assert!(alive < 100, "must expire by lifetime");
        }
        assert!((79..=81).contains(&alive), "8 s at 0.1 steps: {alive}");
    }
}
