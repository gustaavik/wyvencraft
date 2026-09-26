//! What a world's players have achieved, and the rules for updating it.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::domain::core::BlockPos;

/// The progress of one world, shared by everyone playing it.
///
/// Keyed by content ids (structure ids, entity kind names) rather than
/// indices, so a save survives a content update that reorders the tables, and
/// held in `BTree` collections so both its serialized form and its iteration
/// order are deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldProgression {
    /// Wayrunes that have been read, by block position.
    pub read_shrines: BTreeSet<BlockPos>,
    /// Revealed structures: structure id → anchors of the instances revealed.
    pub revealed: BTreeMap<String, BTreeSet<BlockPos>>,
    /// Boss kinds (entity names) defeated at least once in this world.
    pub defeated: BTreeSet<String>,
}

impl WorldProgression {
    /// Record that the wayrune at `rune` was read and that it pointed to the
    /// `structure` anchored at `anchor`. Returns whether anything was new — a
    /// second read of the same shrine changes nothing and announces nothing.
    pub fn read_shrine(&mut self, rune: BlockPos, structure: &str, anchor: BlockPos) -> bool {
        let first_read = self.read_shrines.insert(rune);
        let newly_revealed = self.reveal(structure, anchor);
        first_read || newly_revealed
    }

    /// Reveal one structure instance. Returns whether it was hidden before.
    pub fn reveal(&mut self, structure: &str, anchor: BlockPos) -> bool {
        self.revealed
            .entry(structure.to_string())
            .or_default()
            .insert(anchor)
    }

    pub fn is_revealed(&self, structure: &str, anchor: BlockPos) -> bool {
        self.revealed
            .get(structure)
            .is_some_and(|anchors| anchors.contains(&anchor))
    }

    /// Every revealed instance, as `(structure id, anchor)`.
    pub fn revealed(&self) -> impl Iterator<Item = (&str, BlockPos)> {
        self.revealed
            .iter()
            .flat_map(|(id, anchors)| anchors.iter().map(move |&a| (id.as_str(), a)))
    }

    /// Record a boss kill. Returns whether this was the first.
    pub fn defeat(&mut self, boss: &str) -> bool {
        self.defeated.insert(boss.to_string())
    }

    pub fn is_defeated(&self, boss: &str) -> bool {
        self.defeated.contains(boss)
    }

    /// Forget everything — the `/progress reset` escape hatch for testing.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: i32) -> BlockPos {
        BlockPos::new(x, 90, -x)
    }

    #[test]
    fn reading_a_shrine_reveals_its_target_once() {
        let mut p = WorldProgression::default();
        assert!(p.read_shrine(at(1), "meadows_altar", at(300)));
        assert!(p.is_revealed("meadows_altar", at(300)));
        assert!(
            !p.read_shrine(at(1), "meadows_altar", at(300)),
            "nothing new"
        );
    }

    /// A second shrine pointing at the same altar is still news — the player
    /// read a shrine they had not — but reveals nothing twice.
    #[test]
    fn a_second_shrine_to_the_same_altar_is_recorded_without_duplicating() {
        let mut p = WorldProgression::default();
        p.read_shrine(at(1), "meadows_altar", at(300));
        assert!(p.read_shrine(at(2), "meadows_altar", at(300)));
        assert_eq!(p.revealed().count(), 1);
        assert_eq!(p.read_shrines.len(), 2);
    }

    #[test]
    fn defeating_a_boss_is_idempotent() {
        let mut p = WorldProgression::default();
        assert!(!p.is_defeated("elder stag"));
        assert!(p.defeat("elder stag"));
        assert!(!p.defeat("elder stag"));
        assert!(p.is_defeated("elder stag"));
    }

    #[test]
    fn progression_survives_a_bincode_round_trip() {
        let mut p = WorldProgression::default();
        p.read_shrine(at(1), "meadows_altar", at(300));
        p.defeat("elder stag");
        let config = bincode::config::standard();
        let bytes = bincode::serde::encode_to_vec(&p, config).unwrap();
        let (back, _): (WorldProgression, _) =
            bincode::serde::decode_from_slice(&bytes, config).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn reset_forgets_everything() {
        let mut p = WorldProgression::default();
        p.defeat("elder stag");
        p.reset();
        assert_eq!(p, WorldProgression::default());
    }
}
