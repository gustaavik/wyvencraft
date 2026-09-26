//! Items in the world: dropping, throwing, falling and being collected.

use super::Simulation;
use crate::application::ecs::components::{ItemDrop, Transform};
use crate::application::ecs::{spawn, systems};
use crate::domain::entity::Launch;
use crate::domain::inventory::ItemStack;

impl Simulation {
    /// Throw `stack` out in front of the player.
    ///
    /// The one place that spells this, so every route an item takes out of the
    /// inventory — the drop key, and a held stack that no longer fits when the
    /// panel closes — lands the same way.
    pub fn throw(&mut self, stack: ItemStack) {
        let drop = ItemDrop::thrown(
            stack,
            self.player.eye_position(),
            self.player.look_direction(),
            self.rules.entities.dropped_item(),
        );
        self.spawn_drop(drop);
    }

    /// Put a dropped item into the world, colliding as the "dropped item" kind.
    pub fn spawn_drop(&mut self, drop: (ItemDrop, Launch)) {
        spawn::drop_item(&mut self.ecs, drop, self.rules.entities.dropped_item());
    }

    /// Advance drop physics, collect drops the player walks over, cull expired
    /// ones.
    pub fn update_drops(&mut self, dt: f32) {
        let world = &self.world;
        systems::drops::fall(&mut self.ecs, dt, |p| world.is_solid_for_collision(p));
        // A dead player walks over drops without collecting them.
        let collector = (!self.dead).then(|| self.player.aabb());
        let (inventory, items) = (&mut self.inventory, &self.rules.items);
        systems::drops::pick_up(&mut self.ecs, collector, |stack| {
            inventory.add(stack, items)
        });
    }

    /// Every drop lying in the world, with where it is.
    pub fn drops(&self) -> impl Iterator<Item = (&ItemDrop, glam::Vec3)> {
        self.ecs
            .query::<(&ItemDrop, &Transform)>()
            .map(|(_, (drop, transform))| (drop, transform.position))
    }
}
