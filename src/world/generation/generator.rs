//! The default noise-based [`WorldGenerator`].

use super::WorldGenerator;
use super::biome::Biome;
use super::features;
use super::noise::{ORE_FIELDS, SEA_LEVEL, TerrainNoise};
use crate::core::{BlockId, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, LocalPos};
use crate::world::block::blocks;
use crate::world::chunk::Chunk;

/// An ore vein: the block it places, the vertical band it appears in, and how
/// rare it is (higher threshold = rarer).
struct OreVein {
    block: BlockId,
    min_y: i32,
    max_y: i32,
    threshold: f32,
}

/// Ore table, rarest first so a rich vein wins where noise fields overlap.
/// Indices double as [`TerrainNoise::ore_density`] field ids.
const ORE_VEINS: [OreVein; ORE_FIELDS] = [
    OreVein {
        block: blocks::DIAMOND_ORE,
        min_y: 1,
        max_y: 16,
        threshold: 0.68,
    },
    OreVein {
        block: blocks::GOLD_ORE,
        min_y: 4,
        max_y: 32,
        threshold: 0.66,
    },
    OreVein {
        block: blocks::IRON_ORE,
        min_y: 8,
        max_y: 72,
        threshold: 0.60,
    },
    OreVein {
        block: blocks::COAL_ORE,
        min_y: 16,
        max_y: 108,
        threshold: 0.55,
    },
];

/// Carve where the tunnel field (see [`TerrainNoise::cave_tunnel`]) is below
/// this squared radius.
const TUNNEL_RADIUS_SQ: f32 = 0.01;

/// Generates terrain from layered noise. Fully deterministic in `(seed, pos)`,
/// which is what lets every multiplayer peer reproduce the same world.
pub struct NoiseGenerator {
    seed: u64,
    noise: TerrainNoise,
}

impl NoiseGenerator {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            noise: TerrainNoise::new(seed as u32),
        }
    }

    /// Whether caves hollow out this below-surface cell. Blob caverns open up
    /// with depth; winding tunnels connect them. A protective shell under the
    /// surface (thicker under oceans) is never carved.
    fn is_cave(&self, x: i32, y: i32, z: i32, height: i32, underwater: bool) -> bool {
        let shell = if underwater { 4 } else { 2 };
        if y > height - shell {
            return false;
        }
        let cavern_threshold = 0.52 + 0.10 * (y as f32 / 48.0).min(1.0);
        if self.noise.cave_blob(x, y, z) > cavern_threshold {
            return true;
        }
        self.noise.cave_tunnel(x, y, z) < TUNNEL_RADIUS_SQ
    }

    /// Ocean-floor covering: sandy shallows near the coast, then noise-driven
    /// patches of sand, gravel, and clay in deeper water.
    fn seabed_block(&self, x: i32, z: i32, height: i32) -> BlockId {
        if SEA_LEVEL - height <= 2 {
            return blocks::SAND;
        }
        let n = self.noise.seabed(x, z);
        if n > 0.30 {
            blocks::GRAVEL
        } else if n < -0.35 {
            blocks::CLAY
        } else {
            blocks::SAND
        }
    }

    /// Stone, upgraded to an ore where an ore vein's noise clears its threshold.
    fn ore_or_stone(&self, x: i32, y: i32, z: i32) -> BlockId {
        for (field, vein) in ORE_VEINS.iter().enumerate() {
            if (vein.min_y..=vein.max_y).contains(&y)
                && self.noise.ore_density(field, x, y, z) > vein.threshold
            {
                return vein.block;
            }
        }
        blocks::STONE
    }

    /// Pick the block for a cell at or below the surface (`y <= height`).
    fn solid_block(&self, x: i32, y: i32, z: i32, height: i32, biome: Biome) -> BlockId {
        let underwater = height < SEA_LEVEL;
        if self.is_cave(x, y, z, height, underwater) {
            return blocks::AIR;
        }
        if underwater {
            if y >= height - 2 {
                return self.seabed_block(x, z, height);
            }
        } else if y == height {
            return biome.surface_block();
        } else if y >= height - 3 {
            return biome.subsurface_block();
        }
        self.ore_or_stone(x, y, z)
    }
}

impl WorldGenerator for NoiseGenerator {
    fn seed(&self) -> u64 {
        self.seed
    }

    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::new(pos);
        let origin = pos.origin();

        for lx in 0..CHUNK_SIZE {
            for lz in 0..CHUNK_SIZE {
                let wx = origin.x + lx;
                let wz = origin.z + lz;

                let height = self.noise.surface_height(wx, wz).clamp(1, CHUNK_HEIGHT - 1);
                let biome = Biome::from_temperature(self.noise.temperature(wx, wz));

                for y in 0..CHUNK_HEIGHT {
                    let local = LocalPos {
                        x: lx as u8,
                        y: y as u16,
                        z: lz as u8,
                    };

                    let block = if y == 0 {
                        blocks::BEDROCK
                    } else if y > height {
                        // Above ground: water up to sea level, else air.
                        if y <= SEA_LEVEL {
                            blocks::WATER
                        } else {
                            blocks::AIR
                        }
                    } else {
                        self.solid_block(wx, y, wz, height, biome)
                    };

                    if !block.is_air() {
                        chunk.set_generated(local, block);
                    }
                }
            }
        }

        features::populate(&mut chunk, &self.noise, self.seed);

        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Count occurrences of `block` in the chunks at `positions`.
    fn count_blocks(generator: &NoiseGenerator, positions: &[ChunkPos], block: BlockId) -> usize {
        positions
            .iter()
            .map(|&pos| {
                let chunk = generator.generate(pos);
                chunk.blocks().iter().filter(|&&b| b == block).count()
            })
            .sum()
    }

    fn sample_area() -> Vec<ChunkPos> {
        (-2..2)
            .flat_map(|x| (-2..2).map(move |z| ChunkPos::new(x, z)))
            .collect()
    }

    #[test]
    fn every_ore_appears_but_stays_rare() {
        let generator = NoiseGenerator::new(42);
        let area = sample_area();
        let stone = count_blocks(&generator, &area, blocks::STONE);
        for vein in &ORE_VEINS {
            let ore = count_blocks(&generator, &area, vein.block);
            assert!(ore > 0, "no {:?} generated in sample area", vein.block);
            assert!(ore * 20 < stone, "{:?} too common: {ore}", vein.block);
        }
    }

    #[test]
    fn caves_are_carved_below_the_surface() {
        let generator = NoiseGenerator::new(42);
        let chunk = generator.generate(ChunkPos::new(0, 0));
        let deep_air = (0..CHUNK_SIZE)
            .flat_map(|x| (0..CHUNK_SIZE).map(move |z| (x, z)))
            .flat_map(|(x, z)| {
                (1..40).map(move |y| LocalPos {
                    x: x as u8,
                    y: y as u16,
                    z: z as u8,
                })
            })
            .filter(|&local| chunk.get(local).is_air())
            .count();
        assert!(deep_air > 0, "expected caves below y=40");
    }

    #[test]
    fn trees_populate_the_landscape() {
        let generator = NoiseGenerator::new(42);
        let area: Vec<ChunkPos> = (-6..6)
            .flat_map(|x| (-6..6).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let wood = count_blocks(&generator, &area, blocks::WOOD);
        let leaves = count_blocks(&generator, &area, blocks::LEAVES);
        assert!(wood > 0, "no tree trunks generated in sample area");
        assert!(
            leaves > wood,
            "canopies should outnumber trunk blocks: {leaves} leaves vs {wood} wood"
        );
    }

    /// Every trunk block rests on something solid — trees never float, even when
    /// a trunk stands in a different chunk than the cell that anchored it.
    #[test]
    fn tree_trunks_are_grounded() {
        let generator = NoiseGenerator::new(42);
        for pos in sample_area() {
            let chunk = generator.generate(pos);
            for lx in 0..CHUNK_SIZE {
                for lz in 0..CHUNK_SIZE {
                    for y in 2..CHUNK_HEIGHT {
                        let at = |y: i32| {
                            chunk.get(LocalPos {
                                x: lx as u8,
                                y: y as u16,
                                z: lz as u8,
                            })
                        };
                        if at(y) == blocks::WOOD {
                            assert!(
                                !at(y - 1).is_air(),
                                "floating trunk at {pos:?} ({lx},{y},{lz})"
                            );
                        }
                    }
                }
            }
        }
    }

    /// Boulders leave stone poking above the noise surface, which base terrain
    /// never does on land.
    #[test]
    fn boulders_rise_above_the_surface() {
        let generator = NoiseGenerator::new(42);
        let noise = TerrainNoise::new(42);
        let found = (-8..8)
            .flat_map(|x| (-8..8).map(move |z| ChunkPos::new(x, z)))
            .any(|pos| {
                let chunk = generator.generate(pos);
                let origin = pos.origin();
                (0..CHUNK_SIZE)
                    .flat_map(|lx| (0..CHUNK_SIZE).map(move |lz| (lx, lz)))
                    .any(|(lx, lz)| {
                        let surface = noise.surface_height(origin.x + lx, origin.z + lz);
                        (surface + 1..CHUNK_HEIGHT).any(|y| {
                            chunk.get(LocalPos {
                                x: lx as u8,
                                y: y as u16,
                                z: lz as u8,
                            }) == blocks::STONE
                        })
                    })
            });
        assert!(
            found,
            "no boulder stone found above the surface in sample area"
        );
    }

    #[test]
    fn deep_ocean_floors_use_varied_seabed_materials() {
        let generator = NoiseGenerator::new(42);
        let mut seen = [0usize; 3]; // sand, gravel, clay
        for pos in (-6..6).flat_map(|x| (-6..6).map(move |z| ChunkPos::new(x, z))) {
            let chunk = generator.generate(pos);
            for lx in 0..CHUNK_SIZE {
                for lz in 0..CHUNK_SIZE {
                    // Walk down from sea level to the first solid block.
                    for y in (1..=SEA_LEVEL).rev() {
                        let local = LocalPos {
                            x: lx as u8,
                            y: y as u16,
                            z: lz as u8,
                        };
                        let block = chunk.get(local);
                        if block == blocks::WATER {
                            continue;
                        }
                        match block {
                            b if b == blocks::SAND => seen[0] += 1,
                            b if b == blocks::GRAVEL => seen[1] += 1,
                            b if b == blocks::CLAY => seen[2] += 1,
                            _ => {}
                        }
                        break;
                    }
                }
            }
        }
        assert!(
            seen.iter().all(|&n| n > 0),
            "expected sand, gravel and clay seabeds, got {seen:?}"
        );
    }
}
