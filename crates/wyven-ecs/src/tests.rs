//! Behaviour of the store as a whole: spawning, despawning, queries and
//! deferred commands.

use crate::{CommandBuffer, Ecs, Entity, With, Without};

#[derive(Debug, Clone, Copy, PartialEq)]
struct Pos(f32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Vel(f32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Health(u32);
#[derive(Debug, Clone, Copy, PartialEq)]
struct Frozen;

fn sorted(mut v: Vec<Entity>) -> Vec<Entity> {
    v.sort();
    v
}

#[test]
fn spawn_gives_each_entity_its_bundle() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn((Pos(1.0), Vel(2.0)));
    let b = ecs.spawn((Pos(3.0),));

    assert_eq!(ecs.get::<Pos>(a), Some(&Pos(1.0)));
    assert_eq!(ecs.get::<Vel>(a), Some(&Vel(2.0)));
    assert_eq!(ecs.get::<Vel>(b), None);
    assert_eq!(ecs.len(), 2);
    assert_eq!(ecs.count::<Pos>(), 2);
    assert_eq!(ecs.count::<Vel>(), 1);
}

#[test]
fn despawn_removes_every_component_and_is_idempotent() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn((Pos(1.0), Vel(2.0), Health(3)));
    assert!(ecs.despawn(a));
    assert!(!ecs.despawn(a));
    assert!(!ecs.is_alive(a));
    assert_eq!(ecs.count::<Pos>(), 0);
    assert_eq!(ecs.count::<Vel>(), 0);
    assert_eq!(ecs.count::<Health>(), 0);
}

#[test]
fn a_stale_handle_sees_nothing_of_the_entity_reusing_its_slot() {
    let mut ecs = Ecs::new();
    let old = ecs.spawn((Pos(1.0),));
    ecs.despawn(old);
    let new = ecs.spawn((Pos(2.0),));

    assert_eq!(old.index(), new.index());
    assert_eq!(ecs.get::<Pos>(old), None);
    assert!(
        !ecs.insert(old, Vel(9.0)),
        "inserting on a dead entity is refused"
    );
    assert_eq!(ecs.get::<Vel>(new), None);
    assert!(!ecs.despawn(old));
    assert!(ecs.is_alive(new));
}

#[test]
fn insert_replaces_and_remove_takes() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn(());
    assert!(ecs.insert(a, Health(1)));
    assert!(ecs.insert(a, Health(2)));
    assert_eq!(ecs.get::<Health>(a), Some(&Health(2)));
    assert_eq!(ecs.remove::<Health>(a), Some(Health(2)));
    assert_eq!(ecs.remove::<Health>(a), None);
    assert!(!ecs.has::<Health>(a));
    assert!(ecs.is_alive(a), "removing a component leaves the entity");
}

#[test]
fn query_joins_required_terms() {
    let mut ecs = Ecs::new();
    let moving = ecs.spawn((Pos(0.0), Vel(1.0)));
    let _still = ecs.spawn((Pos(0.0),));
    let _ghost = ecs.spawn((Vel(1.0),));

    let found: Vec<Entity> = ecs.query::<(&Pos, &Vel)>().map(|(e, _)| e).collect();
    assert_eq!(found, vec![moving]);
}

#[test]
fn query_with_optional_and_filters() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn((Pos(1.0), Health(5)));
    let b = ecs.spawn((Pos(2.0),));
    let c = ecs.spawn((Pos(3.0), Frozen));

    let rows: Vec<(Entity, Option<u32>)> = ecs
        .query::<(&Pos, Option<&Health>, Without<Frozen>)>()
        .map(|(e, (_, h, ()))| (e, h.map(|h| h.0)))
        .collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.contains(&(a, Some(5))));
    assert!(rows.contains(&(b, None)));

    let frozen: Vec<Entity> = ecs.query::<(With<Frozen>,)>().map(|(e, _)| e).collect();
    assert_eq!(frozen, vec![c]);
}

#[test]
fn a_query_with_nothing_required_walks_every_entity() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn(());
    let b = ecs.spawn((Frozen,));
    let c = ecs.spawn((Pos(0.0),));

    let all: Vec<Entity> = ecs.query::<(Option<&Pos>,)>().map(|(e, _)| e).collect();
    assert_eq!(sorted(all), vec![a, b, c]);
    let thawed: Vec<Entity> = ecs.query::<(Without<Frozen>,)>().map(|(e, _)| e).collect();
    assert_eq!(sorted(thawed), vec![a, c]);
}

#[test]
fn a_query_on_a_never_seen_type_is_empty() {
    let mut ecs = Ecs::new();
    ecs.spawn((Pos(0.0),));
    assert_eq!(ecs.query::<(&Pos, &Vel)>().count(), 0);
    let mut calls = 0;
    ecs.for_each_mut::<(&mut Pos, &Vel), _>(|_, _| calls += 1);
    assert_eq!(calls, 0);
    assert_eq!(ecs.count::<Pos>(), 1, "the taken storage was put back");
}

#[test]
fn for_each_mut_writes_several_storages_at_once() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn((Pos(0.0), Vel(2.0), Health(10)));
    let b = ecs.spawn((Pos(5.0), Vel(-1.0)));
    let frozen = ecs.spawn((Pos(7.0), Vel(3.0), Frozen));

    ecs.for_each_mut::<(&mut Pos, &mut Vel, Option<&mut Health>, Without<Frozen>), _>(
        |_, (pos, vel, health, ())| {
            pos.0 += vel.0;
            vel.0 *= 0.5;
            if let Some(h) = health {
                h.0 -= 1;
            }
        },
    );

    assert_eq!(ecs.get::<Pos>(a), Some(&Pos(2.0)));
    assert_eq!(ecs.get::<Vel>(a), Some(&Vel(1.0)));
    assert_eq!(ecs.get::<Health>(a), Some(&Health(9)));
    assert_eq!(ecs.get::<Pos>(b), Some(&Pos(4.0)));
    assert_eq!(
        ecs.get::<Pos>(frozen),
        Some(&Pos(7.0)),
        "Without excluded it"
    );
}

#[test]
#[should_panic(expected = "names")]
fn naming_a_type_twice_in_a_mutable_query_panics() {
    let mut ecs = Ecs::new();
    ecs.spawn((Pos(0.0),));
    ecs.for_each_mut::<(&mut Pos, &Pos), _>(|_, _| {});
}

#[test]
fn a_panicking_closure_leaves_the_storages_in_place() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn((Pos(0.0), Vel(1.0)));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ecs.for_each_mut::<(&mut Pos, &Vel), _>(|_, _| panic!("boom"));
    }));
    assert!(outcome.is_err());
    assert_eq!(ecs.get::<Pos>(a), Some(&Pos(0.0)));
    assert_eq!(ecs.get::<Vel>(a), Some(&Vel(1.0)));
}

#[test]
fn commands_apply_in_order_after_iteration() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn((Health(0),));
    let b = ecs.spawn((Health(3),));

    let mut commands = CommandBuffer::new();
    ecs.for_each_mut::<(&Health,), _>(|e, (h,)| {
        if h.0 == 0 {
            commands.despawn(e);
            commands.spawn((Pos(1.0),));
        }
    });
    commands.insert(b, Frozen);
    commands.remove::<Frozen>(b);
    commands.insert(b, Vel(4.0));
    assert_eq!(commands.len(), 5);

    ecs.apply(&mut commands);
    assert!(commands.is_empty());
    assert!(!ecs.is_alive(a));
    assert!(!ecs.has::<Frozen>(b), "remove ran after insert");
    assert_eq!(ecs.get::<Vel>(b), Some(&Vel(4.0)));
    assert_eq!(ecs.count::<Pos>(), 1);
}

#[test]
fn clear_forgets_everything() {
    let mut ecs = Ecs::new();
    let a = ecs.spawn((Pos(0.0),));
    ecs.clear();
    assert!(ecs.is_empty());
    assert!(!ecs.is_alive(a));
    assert_eq!(ecs.count::<Pos>(), 0);
}

#[test]
fn ten_thousand_entities_iterate_and_despawn_consistently() {
    let mut ecs = Ecs::new();
    let entities: Vec<Entity> = (0..10_000)
        .map(|i| {
            if i % 3 == 0 {
                ecs.spawn((Pos(i as f32), Vel(1.0)))
            } else {
                ecs.spawn((Pos(i as f32),))
            }
        })
        .collect();

    ecs.for_each_mut::<(&mut Pos, &Vel), _>(|_, (p, v)| p.0 += v.0);
    for (i, &e) in entities.iter().enumerate() {
        let expected = if i % 3 == 0 { i as f32 + 1.0 } else { i as f32 };
        assert_eq!(ecs.get::<Pos>(e), Some(&Pos(expected)));
    }

    for &e in entities.iter().step_by(2) {
        ecs.despawn(e);
    }
    assert_eq!(ecs.len(), 5_000);
    assert_eq!(ecs.query::<(&Pos,)>().count(), 5_000);
    for (i, &e) in entities.iter().enumerate() {
        assert_eq!(ecs.is_alive(e), i % 2 == 1);
    }
}
