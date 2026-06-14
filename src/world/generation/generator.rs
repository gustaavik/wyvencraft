//! The default noise-based [`WorldGenerator`].

use super::biome::Biome;
use super::noise::{TerrainNoise, SEA_LEVEL};
use super::WorldGenerator;
use crate::core::{BlockPos, ChunkPos, LocalPos, CHUNK_HEIGHT, CHUNK_SIZE};
use crate::world::block::blocks;
use crate::world::chunk::Chunk;

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

                let height = self
                    .noise
                    .surface_height(wx, wz)
                    .clamp(1, CHUNK_HEIGHT - 1);
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
                        // Carve caves below the surface (never breach the top layer).
                        let carve = y < height - 1
                            && self.noise.cave_density(wx, y, wz) > 0.55;
                        if carve {
                            blocks::AIR
                        } else if y == height {
                            biome.surface_block()
                        } else if y >= height - 3 {
                            biome.subsurface_block()
                        } else {
                            blocks::STONE
                        }
                    };

                    if !block.is_air() {
                        chunk.set_generated(local, block);
                    }
                    let _ = BlockPos::new(wx, y, wz); // (world pos available for future decoration)
                }
            }
        }

        chunk
    }
}
