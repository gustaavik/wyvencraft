//! Surface features scattered on top of the base terrain: boulders, trees, and
//! the ground cover that fills in between them.
//!
//! Features are anchored on a world-space jittered grid: each grid cell hashes
//! `(seed, cell)` to decide whether it holds a feature and where. Every chunk
//! that a feature's blocks could reach recomputes it from that hash alone, so
//! neighbouring chunks reproduce the same tree across their border with no
//! cross-chunk communication — generation stays deterministic in `(seed, pos)`.

use super::biome::Biome;
use super::config::{TreeDef, TreeShape, WorldGenConfig};
use super::noise::TerrainNoise;
use crate::core::{BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, LocalPos};
use crate::world::chunk::Chunk;

/// One potential tree per grid cell of this many blocks on each axis.
const TREE_CELL: i32 = 8;
/// Widest horizontal reach of a canopy from its trunk.
const TREE_REACH: i32 = 2;
/// One potential boulder per grid cell of this many blocks on each axis.
const BOULDER_CELL: i32 = 24;
/// Largest horizontal reach of a boulder from its centre.
const BOULDER_REACH: i32 = 3;
/// One potential ground-cover plant per grid cell of this many blocks. Much
/// denser than trees, since a plant is a single block.
const PLANT_CELL: i32 = 2;
/// A plant occupies one cell, so it never reaches into a neighbouring chunk.
const PLANT_REACH: i32 = 0;

const TREE_SALT: u64 = 0x5452_4545; // "TREE"
const BOULDER_SALT: u64 = 0x524F_434B; // "ROCK"
const PLANT_SALT: u64 = 0x504C_414E; // "PLAN"

/// How a feature block interacts with whatever already occupies the cell.
#[derive(Clone, Copy)]
enum Overwrite {
    /// Only fill empty cells (leaves must never eat terrain or trunks).
    AirOnly,
    /// Fill air plus the given block (trunks push through their own canopy
    /// leaves).
    AirAnd(BlockId),
    /// Replace anything (boulders embed into the ground).
    Anything,
}

/// Scatter all surface features into `chunk`. Deterministic in `(seed, pos)`
/// and independent of neighbouring chunks.
pub fn populate(chunk: &mut Chunk, noise: &TerrainNoise, seed: u64, config: &WorldGenConfig) {
    let origin = chunk.pos.origin();
    for_each_anchor(
        origin,
        BOULDER_CELL,
        BOULDER_REACH,
        seed,
        BOULDER_SALT,
        |x, z, h| {
            try_boulder(chunk, noise, origin, x, z, h, config);
        },
    );
    for_each_anchor(origin, TREE_CELL, TREE_REACH, seed, TREE_SALT, |x, z, h| {
        try_tree(chunk, noise, origin, x, z, h, config);
    });
    // Ground cover goes last, and only into air: trunks push through their own
    // leaves but not through anything else, so a plant placed first would punch
    // a hole in the tree that grew over it.
    for_each_anchor(
        origin,
        PLANT_CELL,
        PLANT_REACH,
        seed,
        PLANT_SALT,
        |x, z, h| {
            try_plant(chunk, noise, origin, x, z, h, config);
        },
    );
}

/// Visit the anchor of every grid cell whose feature could reach this chunk.
/// The anchor is jittered inside its cell by the cell hash, which is also
/// passed on as the feature's source of randomness.
fn for_each_anchor(
    origin: BlockPos,
    cell: i32,
    reach: i32,
    seed: u64,
    salt: u64,
    mut place: impl FnMut(i32, i32, u64),
) {
    let min_x = (origin.x - reach).div_euclid(cell);
    let max_x = (origin.x + CHUNK_SIZE - 1 + reach).div_euclid(cell);
    let min_z = (origin.z - reach).div_euclid(cell);
    let max_z = (origin.z + CHUNK_SIZE - 1 + reach).div_euclid(cell);
    for cx in min_x..=max_x {
        for cz in min_z..=max_z {
            let h = feature_hash(seed, cx, cz, salt);
            let x = cx * cell + ((h >> 8) as i32).rem_euclid(cell);
            let z = cz * cell + ((h >> 20) as i32).rem_euclid(cell);
            place(x, z, h);
        }
    }
}

/// SplitMix64-style mix of a grid cell; the sole source of feature randomness,
/// so a feature depends only on `(seed, cell, salt)`.
fn feature_hash(seed: u64, cx: i32, cz: i32, salt: u64) -> u64 {
    let mut h = seed
        ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (cx as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (cz as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Grow a tree at `(x, z)` if the climate and terrain allow one there. Which
/// tree (if any) and its chance come from the biome's worldgen config.
fn try_tree(
    chunk: &mut Chunk,
    noise: &TerrainNoise,
    origin: BlockPos,
    x: i32,
    z: i32,
    h: u64,
    config: &WorldGenConfig,
) {
    let ground = noise.surface_height(x, z).clamp(1, CHUNK_HEIGHT - 1);
    if ground <= config.sea_level {
        return;
    }
    let biome = Biome::from_temperature(noise.temperature(x, z));
    // Per-mille chance a candidate cell grows a tree, before vegetation scaling.
    let Some((tree_index, base_chance)) = config.biome(biome).tree else {
        return;
    };
    // Vegetation noise clumps trees into groves separated by clearings.
    let richness = (noise.vegetation(x, z) + 0.55).clamp(0.0, 1.3);
    if (h % 1000) as f32 >= base_chance * richness {
        return;
    }
    let tree = &config.trees[tree_index];
    match tree.shape {
        TreeShape::Oak => place_oak(chunk, origin, x, ground, z, h, tree),
        TreeShape::Spruce => place_spruce(chunk, origin, x, ground, z, h, tree),
    }
}

/// Drop one ground-cover block on the surface at `(x, z)`, if the biome grows
/// any there. The species is drawn from the biome's list by the feature hash,
/// so which plant lands where depends only on `(seed, cell)`.
fn try_plant(
    chunk: &mut Chunk,
    noise: &TerrainNoise,
    origin: BlockPos,
    x: i32,
    z: i32,
    h: u64,
    config: &WorldGenConfig,
) {
    let ground = noise.surface_height(x, z).clamp(1, CHUNK_HEIGHT - 1);
    if ground <= config.sea_level {
        return;
    }
    let biome = config.biome(Biome::from_temperature(noise.temperature(x, z)));
    if biome.plants.is_empty() {
        return;
    }
    // The same clumping trees use, so meadows follow the groves.
    let richness = (noise.vegetation(x, z) + 0.55).clamp(0.0, 1.3);
    if (h % 1000) as f32 >= biome.plant_chance_per_mille * richness {
        return;
    }
    // High bits: the low ones are already spent on the chance roll, and bits
    // 8..32 on this cell's jitter.
    let block = biome.plants[((h >> 44) % biome.plants.len() as u64) as usize];
    set_block(chunk, origin, x, ground + 1, z, block, Overwrite::AirOnly);
}

/// Trunk height drawn from the tree's inclusive range by the feature hash.
fn trunk_height(tree: &TreeDef, h: u64) -> i32 {
    let (min, max) = tree.trunk_height;
    min + ((h >> 32) % (max - min + 1) as u64) as i32
}

/// Classic oak: a short trunk wrapped in two wide leaf layers, a narrow layer,
/// and a plus-shaped cap.
fn place_oak(
    chunk: &mut Chunk,
    origin: BlockPos,
    x: i32,
    ground: i32,
    z: i32,
    h: u64,
    tree: &TreeDef,
) {
    let trunk_h = trunk_height(tree, h);
    let layers: [(i32, i32); 3] = [(trunk_h - 2, 2), (trunk_h - 1, 2), (trunk_h, 1)];
    for (layer, (dy, radius)) in layers.into_iter().enumerate() {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if dx.abs() == radius && dz.abs() == radius && corner_trimmed(h, layer, dx, dz) {
                    continue;
                }
                set_block(
                    chunk,
                    origin,
                    x + dx,
                    ground + dy,
                    z + dz,
                    tree.leaves,
                    Overwrite::AirOnly,
                );
            }
        }
    }
    for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        let (cx, cy, cz) = (x + dx, ground + trunk_h + 1, z + dz);
        set_block(chunk, origin, cx, cy, cz, tree.leaves, Overwrite::AirOnly);
    }
    for dy in 1..=trunk_h {
        set_block(
            chunk,
            origin,
            x,
            ground + dy,
            z,
            tree.trunk,
            Overwrite::AirAnd(tree.leaves),
        );
    }
}

/// Spruce: a taller trunk with a conical canopy narrowing to a tip.
fn place_spruce(
    chunk: &mut Chunk,
    origin: BlockPos,
    x: i32,
    ground: i32,
    z: i32,
    h: u64,
    tree: &TreeDef,
) {
    let trunk_h = trunk_height(tree, h);
    for dy in 3..=trunk_h {
        let radius = (1 + (trunk_h - dy) / 2).min(2);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                if radius == 2 && dx.abs() == 2 && dz.abs() == 2 {
                    continue;
                }
                set_block(
                    chunk,
                    origin,
                    x + dx,
                    ground + dy,
                    z + dz,
                    tree.leaves,
                    Overwrite::AirOnly,
                );
            }
        }
    }
    let tip = ground + trunk_h + 1;
    set_block(chunk, origin, x, tip, z, tree.leaves, Overwrite::AirOnly);
    for dy in 1..=trunk_h {
        set_block(
            chunk,
            origin,
            x,
            ground + dy,
            z,
            tree.trunk,
            Overwrite::AirAnd(tree.leaves),
        );
    }
}

/// Deterministically drop roughly half of the canopy corner blocks so oak
/// crowns read as rounded rather than boxy.
fn corner_trimmed(h: u64, layer: usize, dx: i32, dz: i32) -> bool {
    let corner = ((dx > 0) as u64) | (((dz > 0) as u64) << 1) | ((layer as u64 & 0x3) << 2);
    (h >> (40 + corner)) & 1 == 0
}

/// Drop a half-buried boulder at `(x, z)` if it sits on dry land.
fn try_boulder(
    chunk: &mut Chunk,
    noise: &TerrainNoise,
    origin: BlockPos,
    x: i32,
    z: i32,
    h: u64,
    config: &WorldGenConfig,
) {
    if h % 1000 >= config.boulder.chance_per_mille {
        return;
    }
    let ground = noise.surface_height(x, z).clamp(1, CHUNK_HEIGHT - 1);
    if ground <= config.sea_level {
        return;
    }
    let radius = 1.4 + ((h >> 32) % 7) as f32 * 0.25; // 1.4..=2.9
    let r = radius.ceil() as i32;
    debug_assert!(r <= BOULDER_REACH);
    for dx in -r..=r {
        for dy in -r..=r {
            for dz in -r..=r {
                if ((dx * dx + dy * dy + dz * dz) as f32) <= radius * radius {
                    let (bx, by, bz) = (x + dx, ground + dy, z + dz);
                    set_block(
                        chunk,
                        origin,
                        bx,
                        by,
                        bz,
                        config.boulder.block,
                        Overwrite::Anything,
                    );
                }
            }
        }
    }
}

/// Write one feature block at world coordinates, if it falls inside this chunk
/// and the overwrite rule allows replacing what is already there. The rules are
/// chosen so overlapping features resolve the same way regardless of placement
/// order (stone beats wood beats leaves beats nothing).
fn set_block(
    chunk: &mut Chunk,
    origin: BlockPos,
    x: i32,
    y: i32,
    z: i32,
    block: BlockId,
    rule: Overwrite,
) {
    let lx = x - origin.x;
    let lz = z - origin.z;
    if !(0..CHUNK_SIZE).contains(&lx)
        || !(0..CHUNK_SIZE).contains(&lz)
        || !(1..CHUNK_HEIGHT).contains(&y)
    {
        return;
    }
    let local = LocalPos {
        x: lx as u8,
        y: y as u16,
        z: lz as u8,
    };
    let existing = chunk.get(local);
    let replace = match rule {
        Overwrite::AirOnly => existing.is_air(),
        Overwrite::AirAnd(also) => existing.is_air() || existing == also,
        Overwrite::Anything => true,
    };
    if replace {
        chunk.set_generated(local, block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ChunkPos;
    use crate::world::block::{BlockRegistry, blocks};
    use crate::world::generation::{NoiseGenerator, WorldGenerator};

    /// Every distinct ground-cover block the shipped worldgen can place. The
    /// biome lists overlap (the mushrooms grow in both plains and snowy), so
    /// this deduplicates rather than concatenating.
    fn plant_ids(config: &WorldGenConfig) -> Vec<BlockId> {
        let mut ids = Vec::new();
        for biome in [Biome::Plains, Biome::Snowy, Biome::Desert] {
            for &id in &config.biome(biome).plants {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    fn sample_area() -> impl Iterator<Item = ChunkPos> {
        (-6..6).flat_map(|x| (-6..6).map(move |z| ChunkPos::new(x, z)))
    }

    #[test]
    fn ground_cover_fills_the_meadows() {
        let registry = BlockRegistry::with_builtins();
        let config = WorldGenConfig::builtin(&registry);
        let plants = plant_ids(&config);
        assert!(!plants.is_empty(), "the shipped biomes declare plants");

        let generator = NoiseGenerator::new(42);
        let mut found = vec![0usize; plants.len()];
        for pos in sample_area() {
            let chunk = generator.generate(pos);
            for lx in 0..CHUNK_SIZE {
                for lz in 0..CHUNK_SIZE {
                    for y in 1..CHUNK_HEIGHT {
                        let at = chunk.get(LocalPos {
                            x: lx as u8,
                            y: y as u16,
                            z: lz as u8,
                        });
                        if let Some(i) = plants.iter().position(|&p| p == at) {
                            found[i] += 1;
                        }
                    }
                }
            }
        }
        for (i, &count) in found.iter().enumerate() {
            assert!(count > 0, "plant {i} never generated in the sample area");
        }
    }

    /// A plant is decoration, not terrain: it stands on solid ground, never
    /// floating, never on water, never stacked on another plant.
    ///
    /// Deliberately says nothing about the block *above*: ground cover
    /// legitimately generates under a neighbouring tree's canopy overhang, and
    /// a short oak's lowest leaf layer can sit only two blocks up.
    #[test]
    fn every_plant_stands_on_solid_ground() {
        let registry = BlockRegistry::with_builtins();
        let config = WorldGenConfig::builtin(&registry);
        let plants = plant_ids(&config);
        let generator = NoiseGenerator::new(0x00C0_FFEE);

        for pos in sample_area() {
            let chunk = generator.generate(pos);
            let at = |x: usize, y: i32, z: usize| {
                chunk.get(LocalPos {
                    x: x as u8,
                    y: y as u16,
                    z: z as u8,
                })
            };
            for lx in 0..CHUNK_SIZE as usize {
                for lz in 0..CHUNK_SIZE as usize {
                    for y in 1..CHUNK_HEIGHT {
                        if !plants.contains(&at(lx, y, lz)) {
                            continue;
                        }
                        let below = at(lx, y - 1, lz);
                        assert!(!below.is_air(), "floating plant at {pos:?} ({lx},{y},{lz})");
                        assert!(
                            !plants.contains(&below),
                            "stacked plants at {pos:?} ({lx},{y},{lz})"
                        );
                        assert_ne!(
                            below,
                            blocks::WATER,
                            "plant on water at {pos:?} ({lx},{y},{lz})"
                        );
                    }
                }
            }
        }
    }

    /// Ground cover is placed after trees and only into air, so it can never
    /// eat a trunk. A trunk runs unbroken from the ground up, so a plant with
    /// wood directly above it is one that swallowed a trunk block.
    #[test]
    fn ground_cover_never_replaces_a_trunk() {
        let registry = BlockRegistry::with_builtins();
        let config = WorldGenConfig::builtin(&registry);
        let plants = plant_ids(&config);
        let generator = NoiseGenerator::new(7);

        for pos in sample_area() {
            let chunk = generator.generate(pos);
            let at = |x: usize, y: i32, z: usize| {
                chunk.get(LocalPos {
                    x: x as u8,
                    y: y as u16,
                    z: z as u8,
                })
            };
            for lx in 0..CHUNK_SIZE as usize {
                for lz in 0..CHUNK_SIZE as usize {
                    for y in 1..CHUNK_HEIGHT - 1 {
                        if plants.contains(&at(lx, y, lz)) {
                            assert_ne!(
                                at(lx, y + 1, lz),
                                blocks::OAK_LOG,
                                "plant inside a trunk at {pos:?} ({lx},{y},{lz})"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Which plant lands in a cell depends only on `(seed, cell)` — a chunk
    /// regenerated from the same seed must come back identical, since worlds
    /// keep only their edits and replay terrain from the seed on every load.
    #[test]
    fn plant_placement_is_deterministic_in_seed_and_position() {
        let a = NoiseGenerator::new(42).generate(ChunkPos::new(2, -3));
        let b = NoiseGenerator::new(42).generate(ChunkPos::new(2, -3));
        for lx in 0..CHUNK_SIZE {
            for lz in 0..CHUNK_SIZE {
                for y in 1..CHUNK_HEIGHT {
                    let local = LocalPos {
                        x: lx as u8,
                        y: y as u16,
                        z: lz as u8,
                    };
                    assert_eq!(a.get(local), b.get(local));
                }
            }
        }
    }
}
