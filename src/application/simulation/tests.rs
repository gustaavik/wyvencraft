//! The simulation on its own: a seed and the builtin rules, no screen,
//! socket or GPU.

use glam::Vec3;

use super::*;
use crate::domain::inventory::crafting::station_ids;

fn sim(mode: GameMode) -> Simulation {
    let rules = Registries::builtin();
    let recipes = RecipeBook::from_toml(
        crate::domain::inventory::crafting::BUILTIN_RECIPES,
        &rules.items,
        &station_ids(&rules.blocks),
    )
    .expect("builtin recipes");
    Simulation::new(
        rules,
        7,
        SimulationStart {
            spawn: None,
            day_cycle: DayCycle::default(),
            mode,
            recipes,
            authoritative: true,
        },
    )
}

#[test]
fn a_new_world_stands_the_player_on_ground() {
    let sim = sim(GameMode::Survival);
    let feet = BlockPos::from_world(sim.player.position);
    assert!(
        sim.world
            .is_solid(BlockPos::new(feet.x, feet.y - 1, feet.z))
    );
    assert!(!sim.dead);
}

#[test]
fn survival_starts_with_the_kit_and_creative_empty() {
    assert!(
        sim(GameMode::Survival)
            .inventory
            .item_in_selected()
            .is_some()
    );
    let creative = sim(GameMode::Creative);
    assert!((0..9).all(|slot| creative.inventory.slot(slot).is_none()));
}

#[test]
fn a_spawned_mob_is_announced_to_peers() {
    let mut sim = sim(GameMode::Survival);
    let at = sim.player.position + Vec3::new(3.0, 0.0, 0.0);
    let id = sim.spawn_mob("cow", at).expect("cow is a mob");
    assert!(matches!(
        sim.outbox.as_slice(),
        [crate::application::protocol::ServerMessage::MobSpawned { id: announced, .. }] if *announced == id.0
    ));
    assert!(sim.spawn_mob("not a mob", at).is_none());
}

#[test]
fn a_thrown_item_leaves_the_hand_and_lies_in_the_world() {
    let mut sim = sim(GameMode::Survival);
    let index = (0..9)
        .find(|&i| sim.inventory.slot(i).is_some_and(|s| s.count > 1))
        .expect("the kit has a stack of something");
    let one = sim.inventory.take_one(index).expect("a stack to take from");
    sim.throw(one);
    assert_eq!(sim.drops().count(), 1);
    let (drop, at) = sim.drops().next().unwrap();
    assert_eq!(drop.stack.item, one.item);
    assert!(
        at.distance(sim.player.eye_position()) < 1.0,
        "it leaves from the hand"
    );
}

#[test]
fn a_drop_at_the_players_feet_is_collected() {
    let mut sim = sim(GameMode::Survival);
    let index = (0..9)
        .find(|&i| sim.inventory.slot(i).is_some_and(|s| s.count > 1))
        .expect("the kit has a stack of something");
    let one = sim.inventory.take_one(index).expect("a stack to take from");
    let before = sim.inventory.count_of(one.item);
    let feet = BlockPos::from_world(sim.player.position);
    let drop = crate::domain::entity::ItemDrop::block_drop(
        one,
        feet,
        0.0,
        sim.rules.entities.dropped_item(),
    );
    sim.spawn_drop(drop);
    for _ in 0..120 {
        sim.update_drops(1.0 / 60.0);
    }
    assert_eq!(sim.drops().count(), 0, "the player stood on it");
    assert_eq!(sim.inventory.count_of(one.item), before + 1);
}

#[test]
fn death_freezes_the_player_until_respawn() {
    let mut sim = sim(GameMode::Survival);
    sim.damage_local_player(10_000.0);
    assert!(sim.dead);
    let frozen = sim.step_player(crate::domain::entity::MovementInput::default(), 0.1, 0.05);
    assert!(frozen.is_none(), "a corpse does not step");
    sim.respawn();
    assert!(!sim.dead);
    assert_eq!(sim.player.position, sim.spawn);
}

#[test]
fn the_clock_wraps_hourly() {
    let mut sim = sim(GameMode::Survival);
    sim.tick_clock(CLOCK_PERIOD - 1.0);
    sim.tick_clock(2.0);
    assert!((sim.clock - 1.0).abs() < 1e-3);
}
