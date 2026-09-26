//! Where the player is and where they are headed: the compass markers for
//! revealed structures and the biome readout on the debug overlay.

use glam::Vec3;

use super::InGameState;
use crate::core::BlockPos;
use crate::core::ident::title_case;
use crate::progression::bearing;
use crate::ui::compass::Waypoint;

/// How far south of a structure `WYVEN_DEBUG_GOTO` stands the player.
const DEBUG_GOTO_STANDOFF: f32 = 11.0;
/// How many grid cells out `/locate` and `WYVEN_DEBUG_GOTO` look.
const LOCATE_RINGS: i32 = 12;

impl InGameState {
    /// The anchor of the nearest `structure` to `from`, if there is one within
    /// reach of the search.
    pub(super) fn locate_structure(&self, structure: &str, from: [f32; 3]) -> Option<BlockPos> {
        let index = self.structures.config().find(structure)?;
        let from = BlockPos::from_world(Vec3::from_array(from));
        self.structures
            .nearest(index, from, LOCATE_RINGS)
            .map(|found| found.anchor)
    }

    /// A compass marker for every structure this world has revealed.
    pub(super) fn waypoints(&self) -> Vec<Waypoint> {
        let here = self.player.position;
        self.progression
            .revealed()
            .map(|(structure, anchor)| {
                let at = Vec3::new(
                    anchor.x as f32 + 0.5,
                    anchor.y as f32,
                    anchor.z as f32 + 0.5,
                );
                let b = bearing(here, self.player.yaw, at);
                Waypoint {
                    label: title_case(structure),
                    angle: b.angle,
                    distance: b.distance,
                }
            })
            .collect()
    }

    /// `WYVEN_DEBUG_GOTO=<structure>` — start beside the nearest one, facing
    /// it, the way `WYVEN_DEBUG_SPAWN` starts beside a mob. Paired with
    /// `WYVEN_SCREENSHOT_AT` this is what makes a structure checkable from an
    /// automated run, since walking to one needs a human at the keyboard.
    pub(super) fn debug_goto_from_env(&mut self) {
        let Ok(structure) = std::env::var("WYVEN_DEBUG_GOTO") else {
            return;
        };
        let structure = structure.trim();
        let Some(anchor) = self.locate_structure(structure, self.player.position.to_array()) else {
            log::warn!("WYVEN_DEBUG_GOTO: no structure {structure:?} near spawn");
            return;
        };
        let standoff = Vec3::new(0.5, 0.0, DEBUG_GOTO_STANDOFF + 0.5);
        let target = Vec3::new(anchor.x as f32, anchor.y as f32 + 1.0, anchor.z as f32);
        self.player.teleport(target + standoff);
        // Face it: looking along -z is yaw 0, and the standoff is due south.
        self.player.yaw = 0.0;
        self.player.pitch = -0.25;
        log::info!("WYVEN_DEBUG_GOTO: at {structure} ({anchor:?})");
    }

    /// "biome: darkwood (tier 2)" for the debug overlay.
    pub(super) fn biome_line(&self) -> String {
        let p = self.player.position;
        let terrain = self.structures.terrain();
        let biome = terrain.biome_gen(p.x.floor() as i32, p.z.floor() as i32);
        let ring = terrain.ring_distance(p.x.floor() as i32, p.z.floor() as i32);
        format!(
            "biome: {} (tier {}, ring {:.0})",
            biome.id, biome.tier, ring
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::GameContent;
    use crate::core::GameMode;

    #[test]
    fn every_revealed_structure_gets_a_marker() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        assert!(state.waypoints().is_empty());
        let spot = BlockPos::new(
            state.player.position.x as i32,
            90,
            state.player.position.z as i32 - 300,
        );
        state.progression.reveal("meadows_altar", spot);
        let marks = state.waypoints();
        assert_eq!(marks.len(), 1);
        assert_eq!(marks[0].label, "Meadows Altar");
        assert!((marks[0].distance - 300.0).abs() < 2.0);
    }

    #[test]
    fn spawn_reads_as_the_first_ring() {
        let state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        assert!(state.biome_line().starts_with("biome: meadows (tier 1"));
    }
}
