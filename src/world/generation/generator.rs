//! The default noise-based [`WorldGenerator`].

use std::sync::Arc;

use super::WorldGenerator;
use super::biome::Biome;
use super::config::WorldGenConfig;
use super::features;
use super::noise::TerrainNoise;
use crate::core::{BlockId, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, LocalPos};
use crate::world::block::BlockRegistry;
use crate::world::chunk::Chunk;

/// Carve where the tunnel field (see [`TerrainNoise::cave_tunnel`]) is below
/// this squared radius.
const TUNNEL_RADIUS_SQ: f32 = 0.01;

/// Generates terrain from layered noise, placing the blocks chosen by the
/// [`WorldGenConfig`]. Fully deterministic in `(seed, pos)` for a given
/// config, which is what lets every multiplayer peer reproduce the same world.
pub struct NoiseGenerator {
    seed: u64,
    noise: TerrainNoise,
    config: Arc<WorldGenConfig>,
}

impl NoiseGenerator {
    /// A generator using the builtin worldgen configuration (tests and
    /// fallbacks; the app passes the loaded config via `with_config`).
    pub fn new(seed: u64) -> Self {
        let blocks = BlockRegistry::with_builtins();
        Self::with_config(seed, Arc::new(WorldGenConfig::builtin(&blocks)))
    }

    pub fn with_config(seed: u64, config: Arc<WorldGenConfig>) -> Self {
        Self {
            seed,
            noise: TerrainNoise::with_ore_fields(seed as u32, config.ores.len()),
            config,
        }
    }

    pub fn config(&self) -> &WorldGenConfig {
        &self.config
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

    /// Ocean-floor covering: shallows near the coast, then noise-driven
    /// patches (gravel/clay/default per the config) in deeper water.
    fn seabed_block(&self, x: i32, z: i32, height: i32) -> BlockId {
        let seabed = &self.config.seabed;
        if self.config.sea_level - height <= 2 {
            return seabed.shallow;
        }
        let n = self.noise.seabed(x, z);
        if n > seabed.gravel_above {
            seabed.gravel
        } else if n < seabed.clay_below {
            seabed.clay
        } else {
            seabed.default_block
        }
    }

    /// Stone, upgraded to an ore where an ore vein's noise clears its threshold.
    fn ore_or_stone(&self, x: i32, y: i32, z: i32) -> BlockId {
        for (field, vein) in self.config.ores.iter().enumerate() {
            if (vein.min_y..=vein.max_y).contains(&y)
                && self.noise.ore_density(field, x, y, z) > vein.threshold
            {
                return vein.block;
            }
        }
        self.config.stone
    }

    /// Pick the block for a cell at or below the surface (`y <= height`).
    fn solid_block(&self, x: i32, y: i32, z: i32, height: i32, biome: Biome) -> BlockId {
        let underwater = height < self.config.sea_level;
        if self.is_cave(x, y, z, height, underwater) {
            return BlockId::AIR;
        }
        if underwater {
            if y >= height - 2 {
                return self.seabed_block(x, z, height);
            }
        } else if y == height {
            return self.config.biome(biome).surface;
        } else if y >= height - 3 {
            return self.config.biome(biome).subsurface;
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
                        self.config.bedrock
                    } else if y > height {
                        // Above ground: water up to sea level, else air.
                        if y <= self.config.sea_level {
                            self.config.water
                        } else {
                            BlockId::AIR
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

        features::populate(&mut chunk, &self.noise, self.seed, &self.config);

        chunk
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::blocks;

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

    /// FNV-1a over the block ids of a chunk, in storage order.
    fn chunk_hash(chunk: &Chunk) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for block in chunk.blocks() {
            for byte in block.0.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    /// Determinism tripwire: pins the exact generator output for fixed seeds.
    /// Existing worlds regenerate unedited terrain from their seed, so any
    /// change that breaks these hashes silently rewrites players' worlds. The
    /// data-driven worldgen config must keep them green.
    #[test]
    fn worldgen_golden_hashes() {
        let positions = [
            ChunkPos::new(0, 0),
            ChunkPos::new(1, 0),
            ChunkPos::new(-1, 2),
            ChunkPos::new(3, -3),
            ChunkPos::new(-4, -4),
            ChunkPos::new(8, 5),
        ];
        const EXPECTED: [u64; 12] = [
            0x867c9b358e4c1d29,
            0xa139cfea3a0a8c5b,
            0xd5b77a589724faa3,
            0xf2ae2d34a8090f67,
            0xc753c9641d33dc5e,
            0x4709e3591d8b943a,
            0xb2e8baae9bdde1fb,
            0x62b3464268435f74,
            0x7923451cf12ae3ef,
            0x023d6824779a4174,
            0x114d35d38c5e5690,
            0x7904a7cadd01db54,
        ];
        let got: Vec<u64> = [42u64, 0x00C0_FFEE]
            .iter()
            .flat_map(|&seed| {
                let generator = NoiseGenerator::new(seed);
                positions
                    .iter()
                    .map(move |&pos| chunk_hash(&generator.generate(pos)))
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(
            got, EXPECTED,
            "generator output changed; if intentional, update EXPECTED to {got:#018x?}"
        );
    }

    #[test]
    fn every_ore_appears_but_stays_rare() {
        let generator = NoiseGenerator::new(42);
        let area = sample_area();
        let stone = count_blocks(&generator, &area, blocks::STONE);
        for vein in &generator.config().ores {
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
                    for y in (1..=generator.config().sea_level).rev() {
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
