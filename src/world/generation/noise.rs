//! Seed-deterministic noise sampling for terrain height, caves, ores, and
//! climate.
//!
//! Wraps the [`noise`] crate so the rest of generation deals in plain
//! `surface_height` / `cave_blob` / `ore_density` queries.

use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

/// Average ground level (blocks).
pub const BASE_HEIGHT: i32 = 64;
/// Peak deviation of terrain above/below [`BASE_HEIGHT`].
pub const HEIGHT_AMPLITUDE: f64 = 28.0;
/// Water fills up to this level.
pub const SEA_LEVEL: i32 = 62;

/// Number of independent ore-vein noise fields (one per ore in the generator's
/// ore table).
pub const ORE_FIELDS: usize = 4;

/// Bundle of noise functions driving world generation. Cheap to clone-free
/// share across threads (`Sync`), so meshing/gen workers can borrow it.
pub struct TerrainNoise {
    height: Fbm<Perlin>,
    floor_detail: Perlin,
    seabed: Fbm<Perlin>,
    cave_blob: Perlin,
    cave_tunnel_a: Perlin,
    cave_tunnel_b: Perlin,
    ores: [Perlin; ORE_FIELDS],
    temperature: Fbm<Perlin>,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        let height = Fbm::<Perlin>::new(seed)
            .set_octaves(5)
            .set_frequency(0.0045)
            .set_persistence(0.5)
            .set_lacunarity(2.0);
        let floor_detail = Perlin::new(seed.wrapping_add(0x2F6E));
        let seabed = Fbm::<Perlin>::new(seed.wrapping_add(0x7A21))
            .set_octaves(3)
            .set_frequency(0.02);
        let cave_blob = Perlin::new(seed.wrapping_add(0x9E37));
        let cave_tunnel_a = Perlin::new(seed.wrapping_add(0x51ED));
        let cave_tunnel_b = Perlin::new(seed.wrapping_add(0xC2B2));
        let ores =
            std::array::from_fn(|i| Perlin::new(seed.wrapping_add(0x85EB + i as u32 * 0x0101)));
        let temperature = Fbm::<Perlin>::new(seed.wrapping_add(0x517C))
            .set_octaves(3)
            .set_frequency(0.0016);
        Self {
            height,
            floor_detail,
            seabed,
            cave_blob,
            cave_tunnel_a,
            cave_tunnel_b,
            ores,
            temperature,
        }
    }

    /// Terrain surface height at world column `(x, z)`. Underwater columns get
    /// extra small-scale relief (dunes/ridges) that fades in with depth, so the
    /// coastline itself stays where the base height puts it.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let n = self.height.get([x as f64, z as f64]); // roughly [-1, 1]
        let base = BASE_HEIGHT as f64 + n * HEIGHT_AMPLITUDE;
        let submersion = ((SEA_LEVEL as f64 - base) / 8.0).clamp(0.0, 1.0);
        if submersion == 0.0 {
            return base.round() as i32;
        }
        let s = 0.04;
        let detail = self.floor_detail.get([x as f64 * s, z as f64 * s]);
        (base + detail * 5.0 * submersion).round() as i32
    }

    /// Seabed material field in roughly `[-1, 1]`; drives sand/gravel/clay
    /// patches on the ocean floor.
    pub fn seabed(&self, x: i32, z: i32) -> f32 {
        self.seabed.get([x as f64, z as f64]) as f32
    }

    /// Climate value in roughly `[-1, 1]`; higher = hotter.
    pub fn temperature(&self, x: i32, z: i32) -> f32 {
        self.temperature.get([x as f64, z as f64]) as f32
    }

    /// 3D density for blob ("cheese") caverns. Values above a threshold are
    /// hollowed out.
    pub fn cave_blob(&self, x: i32, y: i32, z: i32) -> f32 {
        let s = 0.05;
        self.cave_blob
            .get([x as f64 * s, y as f64 * s * 2.0, z as f64 * s]) as f32
    }

    /// Squared distance from the centre of the nearest winding tunnel: two
    /// independent noise fields are both near zero along "spaghetti" paths, so
    /// small values trace long, connected tunnels through the stone.
    pub fn cave_tunnel(&self, x: i32, y: i32, z: i32) -> f32 {
        let s = 0.03;
        let p = [x as f64 * s, y as f64 * s * 1.8, z as f64 * s];
        let a = self.cave_tunnel_a.get(p) as f32;
        let b = self.cave_tunnel_b.get(p) as f32;
        a * a + b * b
    }

    /// Density of ore vein field `field` (index into the generator's ore table,
    /// `< ORE_FIELDS`) at a block position; values above a per-ore threshold
    /// become ore.
    pub fn ore_density(&self, field: usize, x: i32, y: i32, z: i32) -> f32 {
        let s = 0.11;
        self.ores[field].get([x as f64 * s, y as f64 * s, z as f64 * s]) as f32
    }
}
