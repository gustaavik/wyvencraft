//! Seed-deterministic noise sampling for terrain height, caves, ores, and
//! climate.
//!
//! Wraps the [`noise`] crate so the rest of generation deals in plain
//! `surface_height` / `cave_blob` / `ore_density` queries.

use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

use super::biome;

/// Average ground level (blocks).
pub const BASE_HEIGHT: i32 = 64;
/// Peak deviation of terrain above/below [`BASE_HEIGHT`].
pub const HEIGHT_AMPLITUDE: f64 = 28.0;
/// Water fills up to this level.
pub const SEA_LEVEL: i32 = 62;

/// Terraced mesa steps as `(plateau-field threshold, tread height)`. Where the
/// plateau field clears each successive threshold the terrain climbs to that
/// step's flat tread, so mesas rise as layered "wedding cake" buttes rather
/// than one sheer tower.
const MESA_LEVELS: [(f64, f64); 3] = [(0.40, 76.0), (0.48, 85.0), (0.56, 94.0)];
/// Height of the highest mesa tabletop (the last entry of [`MESA_LEVELS`]).
pub const MESA_TOP: i32 = MESA_LEVELS[MESA_LEVELS.len() - 1].1 as i32;
/// Width of the plateau-field band over which each step climbs to its tread —
/// narrow, so the risers read as cliffs.
const MESA_EDGE: f64 = 0.03;
/// Temperature band over which mesas fade out toward the plains biome borders,
/// so no cliff wall forms along the biome contour itself.
const MESA_BIOME_FADE: f64 = 0.12;

/// Number of independent ore-vein noise fields (one per ore in the generator's
/// ore table).
pub const ORE_FIELDS: usize = 4;

/// Bundle of noise functions driving world generation. Cheap to clone-free
/// share across threads (`Sync`), so meshing/gen workers can borrow it.
pub struct TerrainNoise {
    height: Fbm<Perlin>,
    plateau: Fbm<Perlin>,
    floor_detail: Perlin,
    seabed: Fbm<Perlin>,
    cave_blob: Perlin,
    cave_tunnel_a: Perlin,
    cave_tunnel_b: Perlin,
    ores: [Perlin; ORE_FIELDS],
    temperature: Fbm<Perlin>,
    vegetation: Perlin,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        let height = Fbm::<Perlin>::new(seed)
            .set_octaves(5)
            .set_frequency(0.0045)
            .set_persistence(0.5)
            .set_lacunarity(2.0);
        let plateau = Fbm::<Perlin>::new(seed.wrapping_add(0x3D4A))
            .set_octaves(2)
            .set_frequency(0.002);
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
        let vegetation = Perlin::new(seed.wrapping_add(0x1B87));
        Self {
            height,
            plateau,
            floor_detail,
            seabed,
            cave_blob,
            cave_tunnel_a,
            cave_tunnel_b,
            ores,
            temperature,
            vegetation,
        }
    }

    /// Terrain surface height at world column `(x, z)`. In the plains biome the
    /// plateau field can raise terraced mesas (see [`MESA_LEVELS`]). Underwater
    /// columns get extra small-scale relief (dunes/ridges) that fades in with
    /// depth, so the coastline itself stays where the base height puts it.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let p = [x as f64, z as f64];
        let n = self.height.get(p); // roughly [-1, 1]
        let base = BASE_HEIGHT as f64 + n * HEIGHT_AMPLITUDE;
        let h = base + self.mesa_lift(p, n, base);

        let submersion = ((SEA_LEVEL as f64 - h) / 8.0).clamp(0.0, 1.0);
        if submersion == 0.0 {
            return h.round() as i32;
        }
        let s = 0.04;
        let detail = self.floor_detail.get([x as f64 * s, z as f64 * s]);
        (h + detail * 5.0 * submersion).round() as i32
    }

    /// Extra height contributed by terraced mesas at `p`, given the height-field
    /// sample `n` and the base terrain height. Each [`MESA_LEVELS`] step blends
    /// toward its own flat tread through a narrow band, stacking into stepped
    /// buttes; the whole lift fades to zero outside the plains biome.
    fn mesa_lift(&self, p: [f64; 2], n: f64, base: f64) -> f64 {
        let m = self.plateau.get(p);
        if m <= MESA_LEVELS[0].0 {
            return 0.0;
        }
        // Mesas belong to the plains: fade the lift out approaching the
        // snowy/desert temperature borders.
        let cold = biome::SNOWY_MAX_TEMP as f64;
        let hot = biome::DESERT_MIN_TEMP as f64;
        let temp = self.temperature.get(p);
        let strength = smoothstep(cold, cold + MESA_BIOME_FADE, temp)
            * (1.0 - smoothstep(hot - MESA_BIOME_FADE, hot, temp));
        if strength <= 0.0 {
            return 0.0;
        }
        let mut lifted = base;
        for (threshold, level) in MESA_LEVELS {
            let t = smoothstep(threshold, threshold + MESA_EDGE, m);
            if t <= 0.0 {
                break;
            }
            // A slight roll from the height field keeps treads from being glass-flat.
            let tread = level + n * 3.0;
            lifted += (tread - lifted).max(0.0) * t;
        }
        (lifted - base) * strength
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

    /// Vegetation richness in roughly `[-1, 1]`; scales tree density so forests
    /// clump into groves separated by clearings.
    pub fn vegetation(&self, x: i32, z: i32) -> f32 {
        let s = 0.006;
        self.vegetation.get([x as f64 * s, z as f64 * s]) as f32
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

/// Hermite interpolation from 0 at `edge0` to 1 at `edge1`, clamped outside.
fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::biome::Biome;
    use super::*;

    /// Mesas are capped at [`MESA_TOP`], rise only inside the plains biome, and
    /// climb in terraced steps: every tread level of [`MESA_LEVELS`] shows up
    /// as flat ground somewhere.
    #[test]
    fn mesas_are_terraced_plains_only_tabletops() {
        let noise = TerrainNoise::new(42);
        let mut mesa_columns = 0usize;
        let mut tread_columns = [0usize; MESA_LEVELS.len()];
        for x in (-1024..1024).step_by(8) {
            for z in (-1024..1024).step_by(8) {
                let h = noise.surface_height(x, z);
                assert!(h <= MESA_TOP + 8, "column ({x},{z}) too tall: {h}");

                let p = [x as f64, z as f64];
                let n = noise.height.get(p);
                let base = BASE_HEIGHT as f64 + n * HEIGHT_AMPLITUDE;
                let lift = noise.mesa_lift(p, n, base);
                if lift < 0.5 {
                    continue;
                }
                mesa_columns += 1;
                let temp = noise.temperature(x, z);
                assert_eq!(
                    Biome::from_temperature(temp),
                    Biome::Plains,
                    "mesa lift {lift:.1} outside plains at ({x},{z}), temp {temp:.2}"
                );
                for (i, (_, level)) in MESA_LEVELS.iter().enumerate() {
                    if (base + lift - (level + n * 3.0)).abs() <= 1.0 {
                        tread_columns[i] += 1;
                    }
                }
            }
        }
        assert!(
            mesa_columns > 50,
            "expected mesas in the sample region, found {mesa_columns} columns"
        );
        assert!(
            tread_columns.iter().all(|&c| c > 0),
            "expected every terrace level to appear as a flat tread, got {tread_columns:?}"
        );
    }
}
