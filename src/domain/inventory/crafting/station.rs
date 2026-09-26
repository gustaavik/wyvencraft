//! Crafting stations: which ones exist, and which stand within reach.
//!
//! A station is any block declaring `station = "<name>"` in
//! `assets/blocks.toml`; a recipe naming that station can only be made within
//! [`STATION_RADIUS`] of one. The scan takes the world as a closure rather than
//! a `World`, so it is tested with no chunks at all.

use crate::domain::core::{BlockId, BlockPos};
use crate::domain::world::BlockRegistry;

/// How far a station reaches, in blocks — a room, not a village. Measured as a
/// sphere from the block the player's eye is in, so a bench on the floor beside
/// you and one on a shelf above both count.
pub const STATION_RADIUS: i32 = 5;

/// The stations within reach, as a small sorted set of names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StationSet(Vec<String>);

impl StationSet {
    pub fn contains(&self, station: &str) -> bool {
        self.0.binary_search_by(|s| s.as_str().cmp(station)).is_ok()
    }

    pub fn insert(&mut self, station: &str) {
        if let Err(at) = self.0.binary_search_by(|s| s.as_str().cmp(station)) {
            self.0.insert(at, station.to_string());
        }
    }

    /// Whether a recipe needing `station` (`None` = by hand) can be made here.
    pub fn satisfies(&self, station: Option<&str>) -> bool {
        station.is_none_or(|s| self.contains(s))
    }
}

impl<'a> FromIterator<&'a str> for StationSet {
    fn from_iter<I: IntoIterator<Item = &'a str>>(iter: I) -> Self {
        let mut set = Self::default();
        for station in iter {
            set.insert(station);
        }
        set
    }
}

/// Every station name the loaded blocks declare, in block order, once each.
/// Block order is also the order the crafting panel lists them in, so the file
/// decides that the workbench comes before the forge.
pub fn station_ids(blocks: &BlockRegistry) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (_, block) in blocks.iter() {
        if let Some(station) = &block.station
            && !ids.contains(station)
        {
            ids.push(station.clone());
        }
    }
    ids
}

/// The stations within `radius` blocks of `center`.
pub fn stations_near(
    center: BlockPos,
    radius: i32,
    blocks: &BlockRegistry,
    block_at: impl Fn(BlockPos) -> BlockId,
) -> StationSet {
    let mut found = StationSet::default();
    let r2 = radius * radius;
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                if dx * dx + dy * dy + dz * dz > r2 {
                    continue;
                }
                let pos = BlockPos::new(center.x + dx, center.y + dy, center.z + dz);
                if let Some(station) = &blocks.get(block_at(pos)).station {
                    found.insert(station);
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::world::block::blocks::{AIR, FORGE, WORKBENCH};

    #[test]
    fn the_builtin_blocks_declare_the_workbench_then_the_forge() {
        let blocks = BlockRegistry::with_builtins();
        assert_eq!(station_ids(&blocks), ["workbench", "forge"]);
        assert_eq!(blocks.get(WORKBENCH).station.as_deref(), Some("workbench"));
        assert_eq!(blocks.get(FORGE).station.as_deref(), Some("forge"));
    }

    /// A station exactly at the radius counts; one block further does not.
    #[test]
    fn a_station_at_the_edge_counts_and_one_past_it_does_not() {
        let blocks = BlockRegistry::with_builtins();
        let origin = BlockPos::new(10, 64, -3);
        let at = |pos: BlockPos| move |p: BlockPos| if p == pos { WORKBENCH } else { AIR };

        let edge = BlockPos::new(origin.x + STATION_RADIUS, origin.y, origin.z);
        assert!(stations_near(origin, STATION_RADIUS, &blocks, at(edge)).contains("workbench"));

        let past = BlockPos::new(origin.x + STATION_RADIUS + 1, origin.y, origin.z);
        assert!(!stations_near(origin, STATION_RADIUS, &blocks, at(past)).contains("workbench"));

        // The reach is a sphere: the cube's corner is out of it.
        let corner = BlockPos::new(
            origin.x + STATION_RADIUS,
            origin.y + STATION_RADIUS,
            origin.z + STATION_RADIUS,
        );
        assert!(!stations_near(origin, STATION_RADIUS, &blocks, at(corner)).contains("workbench"));
    }

    #[test]
    fn both_stations_are_found_together() {
        let blocks = BlockRegistry::with_builtins();
        let origin = BlockPos::new(0, 0, 0);
        let world = |p: BlockPos| match (p.x, p.y, p.z) {
            (1, 0, 0) => WORKBENCH,
            (0, -1, 2) => FORGE,
            _ => AIR,
        };
        let near = stations_near(origin, STATION_RADIUS, &blocks, world);
        assert!(near.contains("workbench") && near.contains("forge"));
        assert!(near.satisfies(None), "hand recipes need nothing");
        assert!(near.satisfies(Some("forge")));
        assert!(!near.satisfies(Some("anvil")));
    }

    #[test]
    fn the_set_stays_sorted_and_deduplicated() {
        let set: StationSet = ["forge", "workbench", "forge"].into_iter().collect();
        assert_eq!(set, StationSet(vec!["forge".into(), "workbench".into()]));
    }
}
