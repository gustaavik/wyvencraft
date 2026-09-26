//! Seed-deterministic noise sampling for terrain height, caves, ores, and
//! climate.
//!
//! Wraps the [`noise`] crate so the rest of generation deals in plain
//! `surface_height` / `cave_blob` / `ore_density` queries.

use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

/// Terraced mesa steps as `(plateau-field threshold, tread height above the
/// biome's base)`. Where the plateau field clears each successive threshold
/// the terrain climbs to that step's flat tread, so mesas rise as layered
/// "wedding cake" buttes rather than one sheer tower.
const MESA_LEVELS: [(f64, f64); 3] = [(0.40, 12.0), (0.48, 21.0), (0.56, 30.0)];
/// Height of the highest mesa tabletop above its biome's base.
pub const MESA_TOP: f64 = MESA_LEVELS[MESA_LEVELS.len() - 1].1;
/// Width of the plateau-field band over which each step climbs to its tread —
/// narrow, so the risers read as cliffs.
const MESA_EDGE: f64 = 0.03;
/// Largest distance (blocks) the fine ring jitter moves a biome border, on top
/// of the configured low-frequency warp — what makes a border ragged rather
/// than a smooth curve.
pub const RING_JITTER: f32 = 12.0;

/// Default number of independent ore-vein noise fields (the loaded config
/// sizes the real set via [`TerrainNoise::with_ore_fields`]).
pub const ORE_FIELDS: usize = 6;

/// The shape of the land in one biome: where the ground sits, how far it
/// rolls, and how strongly mesas rise. Interpolated across ring borders, which
/// is why it is plain numbers rather than a biome reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainShape {
    /// Average ground level (blocks).
    pub base: f64,
    /// Peak deviation above/below `base`.
    pub amplitude: f64,
    /// `0..=1` strength of the terraced mesas.
    pub mesa: f64,
}

impl TerrainShape {
    /// The shape `t` of the way from `self` to `other`.
    pub fn lerp(self, other: TerrainShape, t: f64) -> TerrainShape {
        let mix = |a: f64, b: f64| a + (b - a) * t;
        TerrainShape {
            base: mix(self.base, other.base),
            amplitude: mix(self.amplitude, other.amplitude),
            mesa: mix(self.mesa, other.mesa),
        }
    }
}

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
    /// One field per ore-table entry, seeded by index — the table's order is
    /// part of a world's identity.
    ores: Vec<Perlin>,
    vegetation: Perlin,
    ring_warp: Fbm<Perlin>,
    ring_jitter: Perlin,
    strata: Perlin,
    entrance: Perlin,
}

impl TerrainNoise {
    pub fn new(seed: u32) -> Self {
        Self::with_ore_fields(seed, ORE_FIELDS)
    }

    pub fn with_ore_fields(seed: u32, ore_fields: usize) -> Self {
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
        let ores = (0..ore_fields)
            .map(|i| Perlin::new(seed.wrapping_add(0x85EB + i as u32 * 0x0101)))
            .collect();
        let vegetation = Perlin::new(seed.wrapping_add(0x1B87));
        let ring_warp = Fbm::<Perlin>::new(seed.wrapping_add(0x5249))
            .set_octaves(3)
            .set_frequency(0.0012);
        let ring_jitter = Perlin::new(seed.wrapping_add(0x4A17));
        let strata = Perlin::new(seed.wrapping_add(0x57A7));
        let entrance = Perlin::new(seed.wrapping_add(0xE472));
        Self {
            height,
            plateau,
            floor_detail,
            seabed,
            cave_blob,
            cave_tunnel_a,
            cave_tunnel_b,
            ores,
            vegetation,
            ring_warp,
            ring_jitter,
            strata,
            entrance,
        }
    }

    /// Terrain surface height at world column `(x, z)` for a land `shape`.
    /// Where `shape.mesa` is non-zero the plateau field can raise terraced
    /// mesas (see [`MESA_LEVELS`]). Underwater columns get extra small-scale
    /// relief (dunes/ridges) that fades in with depth, so the coastline itself
    /// stays where the base height puts it.
    pub fn surface_height(&self, x: i32, z: i32, shape: TerrainShape, sea_level: i32) -> i32 {
        let p = [x as f64, z as f64];
        let n = self.height.get(p); // roughly [-1, 1]
        let ground = shape.base + n * shape.amplitude;
        let h = ground + self.mesa_lift(p, n, shape.base, ground) * shape.mesa;

        let submersion = ((sea_level as f64 - h) / 8.0).clamp(0.0, 1.0);
        if submersion == 0.0 {
            return h.round() as i32;
        }
        let s = 0.04;
        let detail = self.floor_detail.get([x as f64 * s, z as f64 * s]);
        (h + detail * 5.0 * submersion).round() as i32
    }

    /// Extra height contributed by terraced mesas at `p`, given the height-field
    /// sample `n`, the biome's `base` and the unlifted `ground`. Each
    /// [`MESA_LEVELS`] step blends toward its own flat tread through a narrow
    /// band, stacking into stepped buttes.
    fn mesa_lift(&self, p: [f64; 2], n: f64, base: f64, ground: f64) -> f64 {
        let m = self.plateau.get(p);
        if m <= MESA_LEVELS[0].0 {
            return 0.0;
        }
        let mut lifted = ground;
        for (threshold, level) in MESA_LEVELS {
            let t = smoothstep(threshold, threshold + MESA_EDGE, m);
            if t <= 0.0 {
                break;
            }
            // A slight roll from the height field keeps treads from being glass-flat.
            let tread = base + level + n * 3.0;
            lifted += (tread - lifted).max(0.0) * t;
        }
        lifted - ground
    }

    /// Low-frequency ring warp in roughly `[-1, 1]`: scaled by the config's
    /// `ring_warp`, it bends every biome border in and out.
    pub fn ring_warp(&self, x: i32, z: i32) -> f32 {
        self.ring_warp.get([x as f64, z as f64]) as f32
    }

    /// Fine jitter in blocks (±[`RING_JITTER`]) that roughens a border's edge.
    pub fn ring_jitter(&self, x: i32, z: i32) -> f32 {
        let s = 0.035;
        self.ring_jitter.get([x as f64 * s, z as f64 * s]) as f32 * RING_JITTER
    }

    /// 3D strata field in roughly `[-1, 1]`: where it clears a layer's `mix`
    /// threshold, the layer's second block marbles through its fill.
    pub fn strata(&self, x: i32, y: i32, z: i32) -> f32 {
        let s = 0.07;
        self.strata
            .get([x as f64 * s, y as f64 * s * 1.6, z as f64 * s]) as f32
    }

    /// Whether a cave may break through the ground at this column — a cave
    /// mouth. Sparse patches, so most of the land keeps its protective crust.
    pub fn cave_entrance(&self, x: i32, z: i32) -> bool {
        let s = 0.012;
        self.entrance.get([x as f64 * s, z as f64 * s]) > 0.55
    }

    /// Seabed material field in roughly `[-1, 1]`; drives sand/gravel/clay
    /// patches on the ocean floor.
    pub fn seabed(&self, x: i32, z: i32) -> f32 {
        self.seabed.get([x as f64, z as f64]) as f32
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
    use super::*;

    const ROLLING: TerrainShape = TerrainShape {
        base: 96.0,
        amplitude: 18.0,
        mesa: 0.0,
    };
    const BADLANDS: TerrainShape = TerrainShape {
        mesa: 1.0,
        ..ROLLING
    };

    /// Mesas rise only where a biome asks for them, are capped at
    /// [`MESA_TOP`] above its base, and climb in terraced steps: every tread
    /// level of [`MESA_LEVELS`] shows up as flat ground somewhere.
    #[test]
    fn mesas_are_terraced_tabletops_where_a_biome_wants_them() {
        let noise = TerrainNoise::new(42);
        let mut mesa_columns = 0usize;
        let mut tread_columns = [0usize; MESA_LEVELS.len()];
        for x in (-1024..1024).step_by(8) {
            for z in (-1024..1024).step_by(8) {
                let flat = noise.surface_height(x, z, ROLLING, 0);
                let lifted = noise.surface_height(x, z, BADLANDS, 0);
                assert!(lifted >= flat, "a mesa never digs");
                let top = (BADLANDS.base + MESA_TOP) as i32 + 4;
                assert!(
                    lifted <= top.max(flat),
                    "column ({x},{z}) too tall: {lifted}"
                );

                let p = [x as f64, z as f64];
                let n = noise.height.get(p);
                let ground = BADLANDS.base + n * BADLANDS.amplitude;
                let lift = noise.mesa_lift(p, n, BADLANDS.base, ground);
                if lift < 0.5 {
                    continue;
                }
                mesa_columns += 1;
                for (i, (_, level)) in MESA_LEVELS.iter().enumerate() {
                    if (ground + lift - (BADLANDS.base + level + n * 3.0)).abs() <= 1.0 {
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

    #[test]
    fn shapes_interpolate_linearly() {
        let low = ROLLING;
        let high = TerrainShape {
            base: 120.0,
            amplitude: 60.0,
            mesa: 1.0,
        };
        assert_eq!(low.lerp(high, 0.0), low);
        assert_eq!(low.lerp(high, 1.0), high);
        let mid = low.lerp(high, 0.5);
        assert_eq!((mid.base, mid.amplitude, mid.mesa), (108.0, 39.0, 0.5));
    }

    #[test]
    fn ring_jitter_stays_within_its_bound() {
        let noise = TerrainNoise::new(7);
        for x in (-2000..2000).step_by(37) {
            for z in (-2000..2000).step_by(41) {
                assert!(noise.ring_jitter(x, z).abs() <= RING_JITTER);
                assert!(noise.ring_warp(x, z).abs() <= 1.5);
            }
        }
    }
}
