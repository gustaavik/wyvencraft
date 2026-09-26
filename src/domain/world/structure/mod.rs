//! Structures — shrines, boss altars — placed into the terrain as it
//! generates.
//!
//! Pure and seed-deterministic like the rest of generation: [`Structures`]
//! answers "what stands here?", "what is nearest to there?" and "which chunk
//! gets which blocks?" from `(seed, terrain, config)` alone. That is what lets
//! a host validate a player using a shrine two thousand blocks from its own
//! player, or reveal an altar nobody has seen, without generating a chunk.
//!
//! - [`template`] — the authored block grid and its quarter turns.
//! - [`config`] — `assets/structures.toml`.
//! - [`placement`] — the jittered grid and the spawn guarantee.
//! - [`stamp`] — writing one instance's cells into one chunk.

pub mod config;
pub mod placement;
pub mod stamp;
pub mod template;

use std::sync::{Arc, OnceLock};

pub use config::{BUILTIN_STRUCTURES, StructureConfig, StructureDef};
pub use placement::Instance;
pub use template::Cell;

use crate::domain::core::{BlockPos, CHUNK_SIZE, ChunkPos};
use crate::domain::world::chunk::Chunk;
use crate::domain::world::generation::Terrain;

/// Every structure of one world: the config, bound to that world's seed and
/// terrain.
pub struct Structures {
    seed: u64,
    terrain: Arc<Terrain>,
    config: Arc<StructureConfig>,
    /// Each structure's spawn-guarantee instance, worked out on first use.
    guaranteed: Vec<OnceLock<Option<Instance>>>,
}

impl Structures {
    pub fn new(seed: u64, terrain: Arc<Terrain>, config: Arc<StructureConfig>) -> Self {
        let guaranteed = config.all().iter().map(|_| OnceLock::new()).collect();
        Self {
            seed,
            terrain,
            config,
            guaranteed,
        }
    }

    pub fn config(&self) -> &StructureConfig {
        &self.config
    }

    pub fn terrain(&self) -> &Terrain {
        &self.terrain
    }

    fn def(&self, index: usize) -> &StructureDef {
        self.config.get(index)
    }

    fn guaranteed(&self, index: usize) -> Option<Instance> {
        *self.guaranteed[index]
            .get_or_init(|| placement::guaranteed(&self.terrain, self.seed, self.def(index), index))
    }

    /// Every instance of structure `index` that could put a block in the
    /// inclusive column box `min..=max`.
    fn instances_of(&self, index: usize, min: (i32, i32), max: (i32, i32)) -> Vec<Instance> {
        let def = self.def(index);
        let reach = def.reach();
        let mut found: Vec<Instance> = placement::cells_touching(def, min, max)
            .filter_map(|(cx, cz)| placement::in_cell(&self.terrain, self.seed, def, index, cx, cz))
            .filter(|i| i.touches(reach, min, max))
            .collect();
        if def.guarantee_within.is_some()
            && let Some(extra) = self.guaranteed(index)
            && extra.touches(reach, min, max)
        {
            found.push(extra);
        }
        found
    }

    /// Every instance, of any structure, that reaches into `chunk`.
    pub fn instances_touching(&self, chunk: ChunkPos) -> Vec<Instance> {
        let origin = chunk.origin();
        let min = (origin.x, origin.z);
        let max = (origin.x + CHUNK_SIZE - 1, origin.z + CHUNK_SIZE - 1);
        (0..self.config.all().len())
            .flat_map(|index| self.instances_of(index, min, max))
            .collect()
    }

    /// Stamp every structure reaching into `chunk`. Called by the generator
    /// after terrain and features, so structures cut through trees rather
    /// than being grown over.
    pub fn stamp(&self, chunk: &mut Chunk) {
        for instance in self.instances_touching(chunk.pos) {
            stamp::stamp(
                chunk,
                &self.terrain,
                self.def(instance.structure),
                &instance,
            );
        }
    }

    /// The instance of structure `index` nearest `from`, searching up to
    /// `max_rings` grid cells out. `None` if there is none that close.
    pub fn nearest(&self, index: usize, from: BlockPos, max_rings: i32) -> Option<Instance> {
        let def = self.def(index);
        let grid = def.grid;
        let (fx, fz) = (from.x.div_euclid(grid), from.z.div_euclid(grid));
        let mut best = self
            .guaranteed(index)
            .filter(|_| def.guarantee_within.is_some())
            .map(|i| (i.distance_to(from.x, from.z), i));
        for ring in 0..=max_rings {
            // Everything in ring `r` is at least `(r - 1) * grid` away, so once
            // the best found beats that, no further ring can improve on it.
            if let Some((distance, _)) = best
                && ((ring - 1) * grid) as f32 > distance
            {
                break;
            }
            for (cx, cz) in ring_cells(fx, fz, ring) {
                let Some(instance) =
                    placement::in_cell(&self.terrain, self.seed, def, index, cx, cz)
                else {
                    continue;
                };
                let distance = instance.distance_to(from.x, from.z);
                if best.is_none_or(|(d, _)| distance < d) {
                    best = Some((distance, instance));
                }
            }
        }
        best.map(|(_, instance)| instance)
    }

    /// The structure cell at `pos`, if a structure put a block there — how the
    /// host confirms a used block really is a shrine's wayrune, from the seed,
    /// whether or not the chunk is loaded.
    pub fn instance_at(&self, pos: BlockPos) -> Option<(Instance, Cell)> {
        let column = (pos.x, pos.z);
        (0..self.config.all().len())
            .flat_map(|index| self.instances_of(index, column, column))
            .find_map(|instance| {
                let template = &self.def(instance.structure).template;
                let a = instance.anchor;
                let cell = template.cell(instance.rot, pos.x - a.x, pos.y - a.y, pos.z - a.z);
                matches!(cell, Cell::Block(_)).then_some((instance, cell))
            })
    }
}

/// The grid cells on the square ring `ring` cells out from `(cx, cz)`.
fn ring_cells(cx: i32, cz: i32, ring: i32) -> Vec<(i32, i32)> {
    if ring == 0 {
        return vec![(cx, cz)];
    }
    let mut cells = Vec::with_capacity((8 * ring) as usize);
    for d in -ring..=ring {
        cells.push((cx + d, cz - ring));
        cells.push((cx + d, cz + ring));
    }
    for d in -ring + 1..ring {
        cells.push((cx - ring, cz + d));
        cells.push((cx + ring, cz + d));
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::core::{BlockId, LocalPos};
    use crate::domain::world::block::{BlockRegistry, blocks};
    use crate::domain::world::generation::WorldGenConfig;

    fn structures(seed: u64) -> Structures {
        let blocks = BlockRegistry::with_builtins();
        let worldgen = Arc::new(WorldGenConfig::builtin(&blocks));
        let config = Arc::new(StructureConfig::builtin(&blocks, &worldgen));
        let terrain = Arc::new(Terrain::new(seed, worldgen));
        Structures::new(seed, terrain, config)
    }

    fn index(s: &Structures, id: &str) -> usize {
        s.config().find(id).unwrap()
    }

    #[test]
    fn ring_cells_walk_the_square_ring() {
        assert_eq!(ring_cells(3, 4, 0), vec![(3, 4)]);
        let ring = ring_cells(0, 0, 2);
        assert_eq!(ring.len(), 16);
        assert!(ring.iter().all(|&(x, z)| x.abs().max(z.abs()) == 2));
    }

    /// Every world has a Meadows altar and shrine near spawn.
    #[test]
    fn the_spawn_guarantee_holds_across_seeds() {
        for seed in 0..24 {
            let s = structures(seed);
            for (id, within) in [("meadows_altar", 500.0), ("meadows_shrine", 250.0)] {
                let i = index(&s, id);
                let found = s
                    .nearest(i, BlockPos::new(0, 0, 0), 4)
                    .unwrap_or_else(|| panic!("seed {seed}: no {id}"));
                assert!(
                    found.distance_to(0, 0) <= within + 1.0,
                    "seed {seed}: {id} {} blocks out",
                    found.distance_to(0, 0)
                );
            }
        }
    }

    /// `nearest` agrees with a brute-force scan of the same grid cells.
    #[test]
    fn nearest_matches_a_brute_force_scan() {
        let s = structures(11);
        let i = index(&s, "meadows_shrine");
        let def = s.config().get(i);
        for from in [BlockPos::new(0, 0, 0), BlockPos::new(-300, 0, 420)] {
            let mut all: Vec<Instance> = (-4..=4)
                .flat_map(|cx| (-4..=4).map(move |cz| (cx, cz)))
                .filter_map(|(cx, cz)| placement::in_cell(&s.terrain, s.seed, def, i, cx, cz))
                .collect();
            all.extend(s.guaranteed(i));
            let brute = all
                .iter()
                .min_by(|a, b| {
                    a.distance_to(from.x, from.z)
                        .total_cmp(&b.distance_to(from.x, from.z))
                })
                .copied();
            assert_eq!(s.nearest(i, from, 3), brute, "from {from:?}");
        }
    }

    /// Stamp a structure piecemeal into the chunks it straddles, and it
    /// matches the template cell for cell: no seams, no chunk writes another's
    /// part.
    #[test]
    fn a_structure_is_whole_across_chunk_borders() {
        let s = structures(5);
        let i = index(&s, "meadows_altar");
        let instance = s.nearest(i, BlockPos::new(0, 0, 0), 4).expect("altar");
        let def = s.config().get(i);
        let reach = def.template.reach();
        let a = instance.anchor;
        let mut checked = 0;
        for dx in -reach..=reach {
            for dz in -reach..=reach {
                let (x, z) = (a.x + dx, a.z + dz);
                let chunk_pos = BlockPos::new(x, 0, z).chunk();
                let mut chunk = Chunk::new(chunk_pos);
                s.stamp(&mut chunk);
                let origin = chunk_pos.origin();
                for dy in -def.template.below()..=def.template.above() {
                    let Cell::Block(expected) = def.template.cell(instance.rot, dx, dy, dz) else {
                        continue;
                    };
                    let local = LocalPos {
                        x: (x - origin.x) as u8,
                        y: (a.y + dy) as u16,
                        z: (z - origin.z) as u8,
                    };
                    assert_eq!(chunk.get(local), expected, "at ({x},{},{z})", a.y + dy);
                    checked += 1;
                }
            }
        }
        assert!(checked > 50, "only {checked} cells checked");
    }

    /// The host's check: the wayrune of every shrine near spawn is found at
    /// its position from the seed alone.
    #[test]
    fn instance_at_finds_the_shrines_wayrune() {
        let s = structures(21);
        let i = index(&s, "meadows_shrine");
        let shrine = s.nearest(i, BlockPos::new(0, 0, 0), 4).expect("shrine");
        let def = s.config().get(i);
        let mut rune = None;
        for dx in -3..=3 {
            for dz in -3..=3 {
                if def.template.cell(shrine.rot, dx, 1, dz) == Cell::Block(blocks::WAYRUNE) {
                    rune = Some(BlockPos::new(
                        shrine.anchor.x + dx,
                        shrine.anchor.y + 1,
                        shrine.anchor.z + dz,
                    ));
                }
            }
        }
        let rune = rune.expect("template has a wayrune");
        let (found, cell) = s.instance_at(rune).expect("a structure at the rune");
        assert_eq!(found, shrine);
        assert_eq!(cell, Cell::Block(blocks::WAYRUNE));
        let above = BlockPos::new(rune.x, rune.y + 40, rune.z);
        assert!(s.instance_at(above).is_none());
    }

    #[test]
    fn stamping_is_deterministic() {
        let (a, b) = (structures(8), structures(8));
        let i = index(&a, "meadows_altar");
        let altar = a.nearest(i, BlockPos::new(0, 0, 0), 4).unwrap();
        let pos = altar.anchor.chunk();
        let (mut ca, mut cb) = (Chunk::new(pos), Chunk::new(pos));
        a.stamp(&mut ca);
        b.stamp(&mut cb);
        assert_eq!(ca.blocks(), cb.blocks());
        assert!(ca.blocks().iter().any(|&b| b != BlockId::AIR));
    }
}
