//! A free-floating item entity: what a broken block or a tossed stack becomes
//! until a player walks over it and picks it back up.
//!
//! Dropped items are simulated locally and are *not* synchronised over the
//! network; each peer only sees the drops produced by its own actions.

use glam::Vec3;

use crate::core::{Aabb, BlockPos};
use crate::entity::physics;
use crate::inventory::ItemStack;

/// Edge length of the item cube (collision and rendering).
pub const DROP_SIZE: f32 = 0.25;
/// Dropped items vanish after this many seconds.
const DESPAWN_SECONDS: f32 = 300.0;
/// Gravity/terminal velocity mirror the player's physics constants.
const GRAVITY: f32 = 28.0;
const TERMINAL_VELOCITY: f32 = -60.0;
/// Exponential horizontal damping per second while resting on the ground.
const GROUND_FRICTION: f32 = 10.0;
/// Pop speed for drops spawned by breaking a block.
const POP_HORIZONTAL: f32 = 1.4;
const POP_VERTICAL: f32 = 3.2;
/// Launch speed for items tossed with the drop key.
const THROW_SPEED: f32 = 6.0;
const THROW_LIFT: f32 = 2.0;
/// Grace period before a fresh drop can be picked up. Tossed items get a long
/// one so they aren't re-collected before they leave the player's reach.
const BLOCK_DROP_DELAY: f32 = 0.3;
const THROWN_DELAY: f32 = 1.5;
/// Visual spin rate (rad/s) and idle bob.
const SPIN_RATE: f32 = 1.8;
const BOB_AMPLITUDE: f32 = 0.03;
const BOB_RATE: f32 = 2.4;

/// An item stack lying in the world, subject to gravity and pickup.
pub struct DroppedItem {
    pub stack: ItemStack,
    /// Centre of the item cube.
    pub position: Vec3,
    pub velocity: Vec3,
    /// Seconds since the drop spawned; drives despawn and the spin/bob visuals.
    age: f32,
    /// Remaining grace period before pickup is allowed.
    pickup_delay: f32,
}

impl DroppedItem {
    /// A drop popping out of a broken block at `pos`. `angle` (radians) picks the
    /// horizontal pop direction so successive drops scatter instead of stacking.
    pub fn block_drop(stack: ItemStack, pos: BlockPos, angle: f32) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self {
            stack,
            position: Vec3::new(pos.x as f32 + 0.5, pos.y as f32 + 0.5, pos.z as f32 + 0.5),
            velocity: Vec3::new(cos * POP_HORIZONTAL, POP_VERTICAL, sin * POP_HORIZONTAL),
            age: 0.0,
            pickup_delay: BLOCK_DROP_DELAY,
        }
    }

    /// An item tossed by the player: launched from just below the eye along the
    /// look direction.
    pub fn thrown(stack: ItemStack, eye: Vec3, look: Vec3) -> Self {
        Self {
            stack,
            position: eye + look * 0.3 - Vec3::new(0.0, 0.2, 0.0),
            velocity: look * THROW_SPEED + Vec3::Y * THROW_LIFT,
            age: 0.0,
            pickup_delay: THROWN_DELAY,
        }
    }

    /// Collision box in world space.
    pub fn aabb(&self) -> Aabb {
        Aabb::from_center_size(self.position, Vec3::splat(DROP_SIZE))
    }

    /// Advance physics one step: gravity, swept-AABB collision, ground friction.
    pub fn update(&mut self, dt: f32, is_solid: impl Fn(BlockPos) -> bool) {
        self.velocity.y = (self.velocity.y - GRAVITY * dt).max(TERMINAL_VELOCITY);
        let result = physics::move_and_collide(self.aabb(), self.velocity * dt, is_solid);
        self.position += result.delta;
        if result.on_ground {
            if self.velocity.y < 0.0 {
                self.velocity.y = 0.0;
            }
            let damp = (1.0 - GROUND_FRICTION * dt).max(0.0);
            self.velocity.x *= damp;
            self.velocity.z *= damp;
        }
        self.age += dt;
        self.pickup_delay = (self.pickup_delay - dt).max(0.0);
    }

    pub fn can_pickup(&self) -> bool {
        self.pickup_delay <= 0.0
    }

    pub fn expired(&self) -> bool {
        self.age >= DESPAWN_SECONDS
    }

    /// Spin angle (radians around Y) for rendering.
    pub fn spin_yaw(&self) -> f32 {
        self.age * SPIN_RATE
    }

    /// Cube centre to render at: the physics position plus a gentle bob.
    pub fn render_center(&self) -> Vec3 {
        self.position + Vec3::new(0.0, (self.age * BOB_RATE).sin() * BOB_AMPLITUDE, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::ItemId;

    fn stack() -> ItemStack {
        ItemStack::single(ItemId(0))
    }

    #[test]
    fn block_drop_settles_on_the_ground() {
        // Solid ground fills y < 65; the broken block sat at y = 65.
        let solid = |p: BlockPos| p.y < 65;
        let mut item = DroppedItem::block_drop(stack(), BlockPos::new(0, 65, 0), 1.0);
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            item.update(dt, solid);
        }
        let bottom = item.position.y - DROP_SIZE * 0.5;
        assert!(
            (65.0..65.05).contains(&bottom),
            "drop should rest on the ground; bottom at {bottom}"
        );
        assert!(item.velocity.length() < 0.05, "drop should come to rest");
        assert!(item.can_pickup(), "pickup delay should have elapsed");
        assert!(!item.expired());
    }

    #[test]
    fn thrown_item_flies_along_the_look_direction() {
        let solid = |_: BlockPos| false;
        let mut item = DroppedItem::thrown(stack(), Vec3::new(0.0, 80.0, 0.0), Vec3::Z);
        assert!(!item.can_pickup(), "tossed items start with a pickup delay");
        item.update(0.1, solid);
        assert!(item.position.z > 0.3, "item should move forward");
    }
}
