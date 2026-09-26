//! A free-floating item entity: what a broken block or a tossed stack becomes
//! until a player walks over it and picks it back up.
//!
//! Dropped items are simulated locally and are *not* synchronised over the
//! network; each peer only sees the drops produced by its own actions.
//!
//! All tuning comes from the "dropped item" entity kind in
//! `assets/entities.toml`, copied in at spawn.
//!
//! [`ItemDrop`] is the drop *apart from where it is*: its stack, its clocks and
//! the rules they drive. Position and velocity are shared with every other
//! moving body, so they are separate ECS components
//! (`application::ecs::components::{Transform, Velocity}`) and arrive here as
//! arguments.

use glam::Vec3;

use crate::domain::core::{Aabb, BlockPos};
use crate::domain::entity::kind::{
    EntityKind, ItemCubeParams, ItemEntityParams, PhysicsParams, VisualSpec,
};
use crate::domain::entity::physics;
use crate::domain::inventory::ItemStack;

/// Where a new drop starts, and how fast it is going.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Launch {
    /// Centre of the item cube.
    pub position: Vec3,
    pub velocity: Vec3,
}

/// An item stack lying in the world, subject to gravity and pickup.
#[derive(Debug, Clone)]
pub struct ItemDrop {
    pub stack: ItemStack,
    /// Seconds since the drop spawned; drives despawn and the spin/bob visuals.
    age: f32,
    /// Remaining grace period before pickup is allowed.
    pickup_delay: f32,
    // Static tuning, copied from the "dropped item" entity kind at spawn.
    params: ItemEntityParams,
    visual: ItemCubeParams,
}

impl ItemDrop {
    fn new(stack: ItemStack, delay: f32, kind: &EntityKind) -> Self {
        Self {
            stack,
            age: 0.0,
            pickup_delay: delay,
            params: kind.item.expect("dropped-item kind has item params"),
            visual: match &kind.visual {
                VisualSpec::ItemCube(cube) => *cube,
                _ => ItemCubeParams::default(),
            },
        }
    }

    /// A drop popping out of a broken block at `pos`. `angle` (radians) picks the
    /// horizontal pop direction so successive drops scatter instead of stacking.
    /// `kind` is the "dropped item" entity kind from the registry.
    pub fn block_drop(
        stack: ItemStack,
        pos: BlockPos,
        angle: f32,
        kind: &EntityKind,
    ) -> (Self, Launch) {
        let item = kind.item.expect("dropped-item kind has item params");
        let (sin, cos) = angle.sin_cos();
        let launch = Launch {
            position: Vec3::new(pos.x as f32 + 0.5, pos.y as f32 + 0.5, pos.z as f32 + 0.5),
            velocity: Vec3::new(
                cos * item.pop_horizontal,
                item.pop_vertical,
                sin * item.pop_horizontal,
            ),
        };
        (Self::new(stack, item.block_drop_delay, kind), launch)
    }

    /// A drop that only exists to be looked at: still, unspinning, and never
    /// collectable.
    ///
    /// The item placement editor uses it to show what a `ground` placement looks
    /// like without the developer having to toss the item, chase it and pick it
    /// up again. Age stays at zero, which also stops the spin and the bob — a
    /// value is far easier to judge on something holding still.
    pub fn preview(stack: ItemStack, kind: &EntityKind) -> Self {
        Self::new(stack, f32::INFINITY, kind)
    }

    /// An item tossed by the player: launched from just below the eye along the
    /// look direction.
    pub fn thrown(stack: ItemStack, eye: Vec3, look: Vec3, kind: &EntityKind) -> (Self, Launch) {
        let item = kind.item.expect("dropped-item kind has item params");
        let launch = Launch {
            position: eye + look * 0.3 - Vec3::new(0.0, 0.2, 0.0),
            velocity: look * item.throw_speed + Vec3::Y * item.throw_lift,
        };
        (Self::new(stack, item.thrown_delay, kind), launch)
    }

    /// Advance the drop's clocks: age (spin, bob, despawn) and pickup grace.
    pub fn tick(&mut self, dt: f32) {
        self.age += dt;
        self.pickup_delay = (self.pickup_delay - dt).max(0.0);
    }

    /// Edge length the drop is *drawn* at — its collision box scaled by the
    /// visual's `scale`. Bigger than its collision box on purpose: see
    /// [`ItemCubeParams::scale`].
    pub fn render_size(&self, body: &PhysicsParams) -> f32 {
        body.width * self.visual.scale
    }

    /// Distance beyond a player's box within which they collect this drop.
    pub fn pickup_range(&self) -> f32 {
        self.params.pickup_range
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

    /// Centre to render at: the physics position, lifted so the drawn box rests
    /// on the same ground its collision box does however much larger it is, plus
    /// a gentle bob.
    pub fn render_center(&self, position: Vec3, body: &PhysicsParams) -> Vec3 {
        let lift = (self.render_size(body) - body.height) * 0.5;
        position
            + Vec3::new(
                0.0,
                lift + (self.age * self.visual.bob_rate).sin() * self.visual.bob_amplitude,
                0.0,
            )
    }
}

/// Collision box of a body whose position is its centre, as a drop's is.
pub fn centred_aabb(position: Vec3, body: &PhysicsParams) -> Aabb {
    Aabb::from_center_size(position, Vec3::new(body.width, body.height, body.width))
}

/// Advance a centred, unsteered body one step: gravity, swept-AABB collision,
/// ground friction. What a drop does between spawning and being picked up.
pub fn fall_step(
    position: &mut Vec3,
    velocity: &mut Vec3,
    body: &PhysicsParams,
    dt: f32,
    is_solid: impl Fn(BlockPos) -> bool,
) {
    velocity.y = (velocity.y - body.gravity * dt).max(body.terminal_velocity);
    let result = physics::move_and_collide(centred_aabb(*position, body), *velocity * dt, is_solid);
    *position += result.delta;
    if result.on_ground {
        if velocity.y < 0.0 {
            velocity.y = 0.0;
        }
        let damp = (1.0 - body.ground_friction * dt).max(0.0);
        velocity.x *= damp;
        velocity.z *= damp;
    }
    // A drop popped into a ceiling must fall back down, not hover there
    // while its upward velocity bleeds off.
    if result.hit_ceiling {
        velocity.y = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entity::kind::EntityRegistry;
    use crate::domain::inventory::ItemId;

    fn stack() -> ItemStack {
        ItemStack::single(ItemId(0))
    }

    /// A drop plus the two components it moves with, stepped the way the
    /// drop systems step it.
    struct Sim {
        drop: ItemDrop,
        position: Vec3,
        velocity: Vec3,
        body: PhysicsParams,
    }

    impl Sim {
        fn new((drop, launch): (ItemDrop, Launch), kind: &EntityKind) -> Self {
            Self {
                drop,
                position: launch.position,
                velocity: launch.velocity,
                body: kind.physics,
            }
        }

        fn step(&mut self, dt: f32, solid: impl Fn(BlockPos) -> bool) {
            fall_step(
                &mut self.position,
                &mut self.velocity,
                &self.body,
                dt,
                solid,
            );
            self.drop.tick(dt);
        }

        fn aabb(&self) -> Aabb {
            centred_aabb(self.position, &self.body)
        }
    }

    #[test]
    fn block_drop_settles_on_the_ground() {
        let kinds = EntityRegistry::builtin();
        let kind = kinds.dropped_item();
        // Solid ground fills y < 65; the broken block sat at y = 65.
        let solid = |p: BlockPos| p.y < 65;
        let mut item = Sim::new(
            ItemDrop::block_drop(stack(), BlockPos::new(0, 65, 0), 1.0, kind),
            kind,
        );
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            item.step(dt, solid);
        }
        let bottom = item.aabb().min.y;
        assert!(
            (65.0..65.05).contains(&bottom),
            "drop should rest on the ground; bottom at {bottom}"
        );
        assert!(item.velocity.length() < 0.05, "drop should come to rest");
        assert!(item.drop.can_pickup(), "pickup delay should have elapsed");
        assert!(!item.drop.expired());
    }

    /// Drops are drawn larger than they collide, but still sit *on* the ground:
    /// the extra size goes upward, not into the floor.
    #[test]
    fn a_scaled_drop_is_drawn_bigger_but_still_rests_on_the_ground() {
        let kinds = EntityRegistry::builtin();
        let kind = kinds.dropped_item();
        let solid = |p: BlockPos| p.y < 65;
        let mut item = Sim::new(
            ItemDrop::block_drop(stack(), BlockPos::new(0, 65, 0), 1.0, kind),
            kind,
        );
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            item.step(dt, solid);
        }
        let collision = item.aabb();
        let size = item.drop.render_size(&item.body);
        assert!(
            size > collision.max.x - collision.min.x,
            "the drawn box should be larger than the collision box"
        );
        // Bob is the only thing that should ever put the drawn bottom off the
        // floor, and it is small; without the lift it would be half a drop deep.
        let drawn_bottom = item.drop.render_center(item.position, &item.body).y - size * 0.5;
        assert!(
            (drawn_bottom - collision.min.y).abs() < 0.05,
            "drawn bottom {drawn_bottom} left the ground at {}",
            collision.min.y
        );
    }

    #[test]
    fn thrown_item_flies_along_the_look_direction() {
        let kinds = EntityRegistry::builtin();
        let kind = kinds.dropped_item();
        let solid = |_: BlockPos| false;
        let mut item = Sim::new(
            ItemDrop::thrown(stack(), Vec3::new(0.0, 80.0, 0.0), Vec3::Z, kind),
            kind,
        );
        assert!(
            !item.drop.can_pickup(),
            "tossed items start with a pickup delay"
        );
        item.step(0.1, solid);
        assert!(item.position.z > 0.3, "item should move forward");
    }

    #[test]
    fn a_preview_never_becomes_collectable_and_holds_still() {
        let kinds = EntityRegistry::builtin();
        let mut preview = ItemDrop::preview(stack(), kinds.dropped_item());
        preview.tick(1_000.0);
        assert!(!preview.can_pickup());
        // A preview is never ticked by the game; age is what spins it.
        let still = ItemDrop::preview(stack(), kinds.dropped_item());
        assert_eq!(still.spin_yaw(), 0.0);
    }
}
