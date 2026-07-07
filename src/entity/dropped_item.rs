//! A free-floating item entity: what a broken block or a tossed stack becomes
//! until a player walks over it and picks it back up.
//!
//! Dropped items are simulated locally and are *not* synchronised over the
//! network; each peer only sees the drops produced by its own actions.
//!
//! All tuning comes from the "dropped item" entity kind in
//! `assets/entities.toml`, copied in at spawn.

use glam::Vec3;

use crate::core::{Aabb, BlockPos};
use crate::entity::kind::{
    EntityKind, ItemCubeParams, ItemEntityParams, PhysicsParams, VisualSpec,
};
use crate::entity::physics;
use crate::inventory::ItemStack;

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
    // Static tuning, copied from the "dropped item" entity kind at spawn.
    physics: PhysicsParams,
    params: ItemEntityParams,
    visual: ItemCubeParams,
}

impl DroppedItem {
    fn spawn(
        stack: ItemStack,
        position: Vec3,
        velocity: Vec3,
        delay: f32,
        kind: &EntityKind,
    ) -> Self {
        Self {
            stack,
            position,
            velocity,
            age: 0.0,
            pickup_delay: delay,
            physics: kind.physics,
            params: kind.item.expect("dropped-item kind has item params"),
            visual: match kind.visual {
                VisualSpec::ItemCube(cube) => cube,
                VisualSpec::Humanoid => ItemCubeParams::default(),
            },
        }
    }

    /// A drop popping out of a broken block at `pos`. `angle` (radians) picks the
    /// horizontal pop direction so successive drops scatter instead of stacking.
    /// `kind` is the "dropped item" entity kind from the registry.
    pub fn block_drop(stack: ItemStack, pos: BlockPos, angle: f32, kind: &EntityKind) -> Self {
        let item = kind.item.expect("dropped-item kind has item params");
        let (sin, cos) = angle.sin_cos();
        Self::spawn(
            stack,
            Vec3::new(pos.x as f32 + 0.5, pos.y as f32 + 0.5, pos.z as f32 + 0.5),
            Vec3::new(
                cos * item.pop_horizontal,
                item.pop_vertical,
                sin * item.pop_horizontal,
            ),
            item.block_drop_delay,
            kind,
        )
    }

    /// An item tossed by the player: launched from just below the eye along the
    /// look direction.
    pub fn thrown(stack: ItemStack, eye: Vec3, look: Vec3, kind: &EntityKind) -> Self {
        let item = kind.item.expect("dropped-item kind has item params");
        Self::spawn(
            stack,
            eye + look * 0.3 - Vec3::new(0.0, 0.2, 0.0),
            look * item.throw_speed + Vec3::Y * item.throw_lift,
            item.thrown_delay,
            kind,
        )
    }

    /// Rendered/collision cube edge length.
    pub fn size(&self) -> f32 {
        self.physics.width
    }

    /// Distance beyond a player's box within which they collect this drop.
    pub fn pickup_range(&self) -> f32 {
        self.params.pickup_range
    }

    /// Collision box in world space.
    pub fn aabb(&self) -> Aabb {
        Aabb::from_center_size(
            self.position,
            Vec3::new(self.physics.width, self.physics.height, self.physics.width),
        )
    }

    /// Advance physics one step: gravity, swept-AABB collision, ground friction.
    pub fn update(&mut self, dt: f32, is_solid: impl Fn(BlockPos) -> bool) {
        self.velocity.y =
            (self.velocity.y - self.physics.gravity * dt).max(self.physics.terminal_velocity);
        let result = physics::move_and_collide(self.aabb(), self.velocity * dt, is_solid);
        self.position += result.delta;
        if result.on_ground {
            if self.velocity.y < 0.0 {
                self.velocity.y = 0.0;
            }
            let damp = (1.0 - self.physics.ground_friction * dt).max(0.0);
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
        self.age >= self.params.despawn_seconds
    }

    /// Spin angle (radians around Y) for rendering.
    pub fn spin_yaw(&self) -> f32 {
        self.age * self.visual.spin_rate
    }

    /// Cube centre to render at: the physics position plus a gentle bob.
    pub fn render_center(&self) -> Vec3 {
        self.position
            + Vec3::new(
                0.0,
                (self.age * self.visual.bob_rate).sin() * self.visual.bob_amplitude,
                0.0,
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::kind::EntityRegistry;
    use crate::inventory::ItemId;

    fn stack() -> ItemStack {
        ItemStack::single(ItemId(0))
    }

    #[test]
    fn block_drop_settles_on_the_ground() {
        let kinds = EntityRegistry::builtin();
        // Solid ground fills y < 65; the broken block sat at y = 65.
        let solid = |p: BlockPos| p.y < 65;
        let mut item =
            DroppedItem::block_drop(stack(), BlockPos::new(0, 65, 0), 1.0, kinds.dropped_item());
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            item.update(dt, solid);
        }
        let bottom = item.position.y - item.size() * 0.5;
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
        let kinds = EntityRegistry::builtin();
        let solid = |_: BlockPos| false;
        let mut item = DroppedItem::thrown(
            stack(),
            Vec3::new(0.0, 80.0, 0.0),
            Vec3::Z,
            kinds.dropped_item(),
        );
        assert!(!item.can_pickup(), "tossed items start with a pickup delay");
        item.update(0.1, solid);
        assert!(item.position.z > 0.3, "item should move forward");
    }
}
