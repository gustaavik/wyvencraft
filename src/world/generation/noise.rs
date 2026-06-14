//! Seed-deterministic noise sampling for terrain height, caves, and climate.
//!
//! Wraps the [`noise`] crate so the rest of generation deals in plain
//! `surface_height` / `cave_density` / `temperature` queries.

use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

/// Average ground level (blocks).
pub const BASE_HEIGHT: i32 = 64;
/// Peak deviation of terrain above/below [`BASE_HEIGHT`].
pub const HEIGHT_AMPLITUDE: f64 = 28.0;
/// Water fills up to this level.
pub const SEA_LEVEL: i32 = 62;

/// Bundle of noise functions driving world generation. Cheap to clone-free
/// share across threads (`Sync`), so meshing/gen workers can borrow it.
pub struct TerrainNoise {
    height: Fbm<Perlin>,
    cave: Perlin,
    temperature: Fbm<Perlin>,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        let height = Fbm::<Perlin>::new(seed)
            .set_octaves(5)
            .set_frequency(0.0045)
            .set_persistence(0.5)
            .set_lacunarity(2.0);
        let cave = Perlin::new(seed.wrapping_add(0x9E37));
        let temperature = Fbm::<Perlin>::new(seed.wrapping_add(0x517C))
            .set_octaves(3)
            .set_frequency(0.0016);
        Self {
            height,
            cave,
            temperature,
        }
    }

    /// Terrain surface height at world column `(x, z)`.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let n = self.height.get([x as f64, z as f64]); // roughly [-1, 1]
        (BASE_HEIGHT as f64 + n * HEIGHT_AMPLITUDE).round() as i32
    }

    /// Climate value in roughly `[-1, 1]`; higher = hotter.
    pub fn temperature(&self, x: i32, z: i32) -> f32 {
        self.temperature.get([x as f64, z as f64]) as f32
    }

    /// 3D density used to carve caves. Positive values above a threshold are
    /// hollowed out.
    pub fn cave_density(&self, x: i32, y: i32, z: i32) -> f32 {
        let s = 0.05;
        self.cave
            .get([x as f64 * s, y as f64 * s * 2.0, z as f64 * s]) as f32
    }
}
