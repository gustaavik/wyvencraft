//! Dropped items: falling, ageing, and being picked up.

use crate::application::ecs::components::{Body, ItemDrop, Transform, Velocity};
use crate::application::ecs::{CommandBuffer, Ecs};
use crate::domain::core::{Aabb, BlockPos};
use crate::domain::entity::dropped_item::fall_step;
use crate::domain::inventory::ItemStack;

/// Advance every drop's physics and clocks one step.
pub fn fall(ecs: &mut Ecs, dt: f32, is_solid: impl Fn(BlockPos) -> bool) {
    ecs.for_each_mut::<(&mut Transform, &mut Velocity, &Body, &mut ItemDrop), _>(
        |_, (transform, velocity, body, drop)| {
            fall_step(
                &mut transform.position,
                &mut velocity.0,
                &body.0,
                dt,
                &is_solid,
            );
            drop.tick(dt);
        },
    );
}

/// Cull expired drops, and hand every collectable one within reach of
/// `collector` to `take`, which returns how many of the stack did *not* fit.
/// What does not fit stays on the ground. `None` collects nothing — a dead
/// player walks over drops without picking them up.
pub fn pick_up(ecs: &mut Ecs, collector: Option<Aabb>, mut take: impl FnMut(ItemStack) -> u8) {
    let mut commands = CommandBuffer::new();
    ecs.for_each_mut::<(&Transform, &Body, &mut ItemDrop), _>(|entity, (transform, body, drop)| {
        if drop.expired() {
            commands.despawn(entity);
            return;
        }
        let Some(collector) = collector else {
            return;
        };
        let reach = collector.expand(glam::Vec3::splat(drop.pickup_range()));
        if !drop.can_pickup() || !reach.intersects(body.aabb(transform.position)) {
            return;
        }
        match take(drop.stack) {
            0 => commands.despawn(entity),
            leftover => drop.stack.count = leftover,
        }
    });
    ecs.apply(&mut commands);
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::application::ecs::spawn;
    use crate::domain::entity::kind::EntityRegistry;
    use crate::domain::inventory::ItemId;

    fn ground(p: BlockPos) -> bool {
        p.y < 65
    }

    fn settled_drop(ecs: &mut Ecs, count: u8) -> wyven_ecs::Entity {
        let kinds = EntityRegistry::builtin();
        let kind = kinds.dropped_item();
        let stack = ItemStack {
            count,
            ..ItemStack::single(ItemId(1))
        };
        let e = spawn::drop_item(
            ecs,
            ItemDrop::block_drop(stack, BlockPos::new(0, 65, 0), 0.0, kind),
            kind,
        );
        for _ in 0..120 {
            fall(ecs, 1.0 / 60.0, ground);
        }
        e
    }

    fn player_at_drop() -> Aabb {
        Aabb::from_center_size(Vec3::new(0.5, 65.9, 0.5), Vec3::new(0.6, 1.8, 0.6))
    }

    #[test]
    fn a_drop_within_reach_is_collected_whole() {
        let mut ecs = Ecs::new();
        let e = settled_drop(&mut ecs, 3);
        let mut got = Vec::new();
        pick_up(&mut ecs, Some(player_at_drop()), |stack| {
            got.push(stack.count);
            0
        });
        assert_eq!(got, [3]);
        assert!(!ecs.is_alive(e));
    }

    #[test]
    fn what_does_not_fit_stays_on_the_ground() {
        let mut ecs = Ecs::new();
        let e = settled_drop(&mut ecs, 5);
        pick_up(&mut ecs, Some(player_at_drop()), |_| 2);
        assert_eq!(ecs.get::<ItemDrop>(e).map(|d| d.stack.count), Some(2));
    }

    #[test]
    fn nobody_collecting_leaves_drops_alone() {
        let mut ecs = Ecs::new();
        let e = settled_drop(&mut ecs, 1);
        pick_up(&mut ecs, None, |_| panic!("nothing may be taken"));
        assert!(ecs.is_alive(e));
    }

    #[test]
    fn a_drop_out_of_reach_is_left() {
        let mut ecs = Ecs::new();
        let e = settled_drop(&mut ecs, 1);
        let far = Aabb::from_center_size(Vec3::new(40.0, 66.0, 0.0), Vec3::splat(1.0));
        pick_up(&mut ecs, Some(far), |_| panic!("out of reach"));
        assert!(ecs.is_alive(e));
    }

    #[test]
    fn a_fresh_drop_waits_out_its_pickup_delay() {
        let kinds = EntityRegistry::builtin();
        let kind = kinds.dropped_item();
        let mut ecs = Ecs::new();
        let e = spawn::drop_item(
            &mut ecs,
            ItemDrop::block_drop(
                ItemStack::single(ItemId(1)),
                BlockPos::new(0, 65, 0),
                0.0,
                kind,
            ),
            kind,
        );
        pick_up(&mut ecs, Some(player_at_drop()), |_| {
            panic!("still in its grace period")
        });
        assert!(ecs.is_alive(e));
    }

    #[test]
    fn an_expired_drop_despawns_even_with_nobody_near() {
        let mut ecs = Ecs::new();
        let e = settled_drop(&mut ecs, 1);
        for _ in 0..10 {
            fall(&mut ecs, 60.0, ground);
        }
        pick_up(&mut ecs, None, |_| 0);
        assert!(!ecs.is_alive(e));
    }
}
