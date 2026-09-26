//! The default noise-based [`WorldGenerator`].

use std::sync::Arc;

use super::WorldGenerator;
use super::config::WorldGenConfig;
use super::features;
use super::terrain::{Column, Terrain};
use super::underground::{Fill, Ground, underground};
use crate::core::{BlockId, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, LocalPos};
use crate::world::block::BlockRegistry;
use crate::world::chunk::Chunk;
use crate::world::structure::{StructureConfig, Structures};

/// Blocks of `subsurface` under a biome's surface block, above the layers.
const SUBSURFACE_DEPTH: i32 = 3;

/// Generates terrain from layered noise, placing the blocks chosen by the
/// [`WorldGenConfig`]. Fully deterministic in `(seed, pos)` for a given
/// config, which is what lets every multiplayer peer reproduce the same world.
pub struct NoiseGenerator {
    seed: u64,
    terrain: Arc<Terrain>,
    structures: Arc<Structures>,
}

impl NoiseGenerator {
    /// A generator using the builtin worldgen configuration (tests and
    /// fallbacks; the app passes the loaded config via `with_config`).
    pub fn new(seed: u64) -> Self {
        let blocks = BlockRegistry::with_builtins();
        let worldgen = Arc::new(WorldGenConfig::builtin(&blocks));
        let structures = Arc::new(StructureConfig::builtin(&blocks, &worldgen));
        Self::with_config(seed, worldgen, structures)
    }

    pub fn with_config(
        seed: u64,
        config: Arc<WorldGenConfig>,
        structures: Arc<StructureConfig>,
    ) -> Self {
        let terrain = Arc::new(Terrain::new(seed, config));
        Self {
            seed,
            structures: Arc::new(Structures::new(seed, terrain.clone(), structures)),
            terrain,
        }
    }

    pub fn config(&self) -> &WorldGenConfig {
        self.terrain.config()
    }

    /// The column sampler this generator builds from — shared with anything
    /// that must agree with it about biomes and heights (the HUD, structure
    /// lookups).
    pub fn terrain(&self) -> &Arc<Terrain> {
        &self.terrain
    }

    /// Where this world's shrines and altars stand — the same instances the
    /// generator stamps, for the game to locate, reveal and validate.
    pub fn structures(&self) -> &Arc<Structures> {
        &self.structures
    }

    /// Ocean-floor covering: shallows near the coast, then noise-driven
    /// patches (gravel/clay/default per the config) in deeper water.
    fn seabed_block(&self, x: i32, z: i32, height: i32) -> BlockId {
        let config = self.config();
        let seabed = &config.seabed;
        if config.sea_level - height <= 2 {
            return seabed.shallow;
        }
        let n = self.terrain.noise().seabed(x, z);
        if n > seabed.gravel_above {
            seabed.gravel
        } else if n < seabed.clay_below {
            seabed.clay
        } else {
            seabed.default_block
        }
    }

    /// The block for a cell at or below the surface (`y <= column.height`).
    fn solid_block(&self, x: i32, y: i32, z: i32, column: &Column, entrance: bool) -> BlockId {
        let config = self.config();
        let underwater = column.height < config.sea_level;
        let ground = Ground {
            biome: column.biome,
            height: column.height,
            underwater,
            entrance,
        };
        let depth = column.height - y;
        let fill = underground(self.terrain.noise(), config, ground, x, y, z);
        if let Fill::Void(flooded) = fill {
            return flooded.unwrap_or(BlockId::AIR);
        }
        let biome = config.biome(column.biome);
        if underwater {
            if depth <= 2 {
                return self.seabed_block(x, z, column.height);
            }
        } else if depth == 0 {
            return biome.surface;
        } else if depth <= SUBSURFACE_DEPTH {
            return biome.subsurface;
        }
        match fill {
            Fill::Solid(block) => block,
            Fill::Void(_) => unreachable!("handled above"),
        }
    }
}

impl WorldGenerator for NoiseGenerator {
    fn seed(&self) -> u64 {
        self.seed
    }

    fn biome_tint(&self, x: i32, z: i32, index: u8) -> [u8; 4] {
        // The same ring sample `generate` uses to pick surface blocks, so a
        // grass block's tint always agrees with the biome that placed it.
        self.terrain.tint(x, z, index)
    }

    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::new(pos);
        let origin = pos.origin();
        let config = self.config();

        for lx in 0..CHUNK_SIZE {
            for lz in 0..CHUNK_SIZE {
                let wx = origin.x + lx;
                let wz = origin.z + lz;
                let column = self.terrain.column(wx, wz);
                let entrance = self.terrain.noise().cave_entrance(wx, wz);

                for y in 0..CHUNK_HEIGHT {
                    let local = LocalPos {
                        x: lx as u8,
                        y: y as u16,
                        z: lz as u8,
                    };

                    let block = if y == 0 {
                        config.bedrock
                    } else if y > column.height {
                        // Above ground: water up to sea level, else air.
                        if y <= config.sea_level {
                            config.water
                        } else {
                            BlockId::AIR
                        }
                    } else {
                        self.solid_block(wx, y, wz, &column, entrance)
                    };

                    if !block.is_air() {
                        chunk.set_generated(local, block);
                    }
                }
            }
        }

        features::populate(&mut chunk, &self.terrain, self.seed);
        self.structures.stamp(&mut chunk);

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
            0x8f6948a59c07b437,
            0x88f88fc22f02e301,
            0x06fab1ced8f542b7,
            0x87d9b64b87a13ded,
            0x046cfd11c57c41bd,
            0x7fb67548762376f1,
            0xcb503fdceca740df,
            0x583639ebdda9534c,
            0xae8377d68d9c2d69,
            0x926fa0ca225ba872,
            0x9b484f1514d1a890,
            0x2d5ffcaca9207e70,
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

    /// Near spawn only the Meadows' ores generate — every biome's metal
    /// waits in its own ring (see `underground::biome_ores_stay_under_their_biome`).
    #[test]
    fn spawn_has_coal_but_no_biome_metal() {
        let generator = NoiseGenerator::new(42);
        let area = sample_area();
        let stone = count_blocks(&generator, &area, blocks::STONE);
        let coal = count_blocks(&generator, &area, blocks::COAL_ORE);
        assert!(coal > 0, "no coal near spawn");
        assert!(coal * 20 < stone, "coal too common: {coal}");
        for metal in [blocks::COPPER_ORE, blocks::TIN_ORE, blocks::IRON_ORE] {
            assert_eq!(
                count_blocks(&generator, &area, metal),
                0,
                "{metal:?} at spawn"
            );
        }
    }

    #[test]
    fn caves_are_carved_below_the_surface() {
        let generator = NoiseGenerator::new(42);
        let chunk = generator.generate(ChunkPos::new(0, 0));
        let deep_air = (0..CHUNK_SIZE)
            .flat_map(|x| (0..CHUNK_SIZE).map(move |z| (x, z)))
            .flat_map(|(x, z)| {
                (1..70).map(move |y| LocalPos {
                    x: x as u8,
                    y: y as u16,
                    z: z as u8,
                })
            })
            .filter(|&local| chunk.get(local).is_air())
            .count();
        assert!(deep_air > 0, "expected caves below y=70");
    }

    #[test]
    fn trees_populate_the_landscape() {
        let generator = NoiseGenerator::new(42);
        let area: Vec<ChunkPos> = (-6..6)
            .flat_map(|x| (-6..6).map(move |z| ChunkPos::new(x, z)))
            .collect();
        let wood = count_blocks(&generator, &area, blocks::OAK_LOG);
        let leaves = count_blocks(&generator, &area, blocks::OAK_LEAVES);
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
                        if at(y) == blocks::OAK_LOG {
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
        let terrain = generator.terrain().clone();
        let found = (-8..8)
            .flat_map(|x| (-8..8).map(move |z| ChunkPos::new(x, z)))
            .any(|pos| {
                let chunk = generator.generate(pos);
                let origin = pos.origin();
                (0..CHUNK_SIZE)
                    .flat_map(|lx| (0..CHUNK_SIZE).map(move |lz| (lx, lz)))
                    .any(|(lx, lz)| {
                        let surface = terrain.height(origin.x + lx, origin.z + lz);
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
        // Meadows seas are shallow lakes, so sample a wide, sparse area to
        // find water deep enough for the seabed noise to matter.
        let area = (-24..24)
            .step_by(3)
            .flat_map(|x| (-24..24).step_by(3).map(move |z| ChunkPos::new(x, z)));
        for pos in area {
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
