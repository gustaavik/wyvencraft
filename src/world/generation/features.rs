//! Surface features scattered on top of the base terrain: trees and boulders.
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

const TREE_SALT: u64 = 0x5452_4545; // "TREE"
const BOULDER_SALT: u64 = 0x524F_434B; // "ROCK"

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
