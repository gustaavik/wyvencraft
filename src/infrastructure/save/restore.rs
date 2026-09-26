//! Putting a saved world back into a running [`Simulation`]: the edits over
//! its generated terrain, the player, the mob population and progression.
//! The other direction — snapshotting — is the `from_*` constructors in
//! [`super::data`].

use glam::Vec3;

use crate::application::ecs::components::Mob;
use crate::application::simulation::Simulation;
use crate::domain::progression::WorldProgression;
use crate::infrastructure::save::{MobsData, PlayerData, WorldData};

/// Replay a save onto `sim`, which must have been generated from the save's
/// own seed. `spawn` is the world's recorded respawn point, which a saved
/// world keeps rather than the generation anchor the simulation was built at.
pub fn restore_into(
    sim: &mut Simulation,
    spawn: Vec3,
    world: Option<&WorldData>,
    player: Option<&PlayerData>,
    mobs: MobsData,
    progression: WorldProgression,
) {
    if let Some(world) = world {
        let resolved = world.resolve(&sim.rules.blocks);
        let count = resolved.len();
        for (pos, block) in resolved {
            sim.world.apply_edit(pos, block);
        }
        sim.spawn = spawn;
        log::info!("restored {count} world edits");
    }
    if let Some(player) = player {
        player.apply(&mut sim.player, &mut sim.inventory, &sim.rules.items);
    }
    // Respawn the saved mob population. Fresh ids and brains (both are
    // session-scoped); unknown kinds fail soft like unknown blocks/items.
    let saved_mobs = mobs.mobs.len();
    for data in mobs.mobs {
        let position = Vec3::from_array(data.position);
        match sim.spawn_mob(&data.kind, position) {
            Some(id) => sim.restore_mob(id, data.health, data.night_spawned),
            None => log::warn!(
                "save references unknown mob kind '{}'; dropping it",
                data.kind
            ),
        }
    }
    sim.progression = progression;
    if saved_mobs > 0 {
        log::info!(
            "restored {} of {saved_mobs} saved mobs",
            sim.ecs.count::<Mob>()
        );
    }
}
