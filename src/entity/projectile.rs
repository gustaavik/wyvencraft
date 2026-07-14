//! Projectiles: the skeleton's arrow. A point-mass ballistic body — no
//! swept collision, just gravity and a per-step solid-block test, which is
//! plenty at arrow speeds and the game's clamped timestep.
//!
//! Every peer simulates arrows it knows about (they're cheap and purely
//! visual off the authority); only the authority tests player hits and
//! applies damage, mirroring how mobs work.

use glam::Vec3;

use crate::core::{Aabb, BlockPos};

/// Rendered/collision cube edge for an arrow (blocks).
const ARROW_SIZE: f32 = 0.15;

/// An arrow in flight. Tuning comes from the firing kind's
/// `[entity.mob.ranged]` params, copied in at launch.
pub struct Arrow {
    pub position: Vec3,
    pub velocity: Vec3,
    /// Damage on a player hit (applied by the authority only).
    pub damage: f32,
    gravity: f32,
    age: f32,
    lifetime: f32,
}

impl Arrow {
    pub fn new(position: Vec3, velocity: Vec3, damage: f32, gravity: f32, lifetime: f32) -> Self {
        Self {
            position,
            velocity,
            damage,
            gravity,
            age: 0.0,
            lifetime,
        }
    }

    /// Hit-test box around the arrow head.
    pub fn aabb(&self) -> Aabb {
        Aabb::from_center_size(self.position, Vec3::splat(ARROW_SIZE))
    }

    /// Advance one step. Returns `false` when the arrow is spent — it flew
    /// into a solid block or outlived its lifetime — and should despawn.
    pub fn update(&mut self, dt: f32, is_solid: impl Fn(BlockPos) -> bool) -> bool {
        self.age += dt;
        if self.age >= self.lifetime {
            return false;
        }
        self.velocity.y -= self.gravity * dt;
        let next = self.position + self.velocity * dt;
        if is_solid(BlockPos::from_world(next)) {
            return false;
        }
        self.position = next;
        true
    }

    /// Facing for rendering: yaw along the horizontal flight direction.
    pub fn yaw(&self) -> f32 {
        self.velocity.x.atan2(-self.velocity.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrow(velocity: Vec3) -> Arrow {
        // Skeleton-like tuning: 18 blocks/s, 20 blocks/s² drop, 8 s life.
        Arrow::new(Vec3::new(0.0, 70.0, 0.0), velocity, 3.0, 20.0, 8.0)
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
