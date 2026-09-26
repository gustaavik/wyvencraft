//! The one place a world column is sampled: which biome it is in, how it
//! blends with its neighbour, and how high its ground stands.
//!
//! The generator, surface features, structure placement and the HUD all ask
//! here, so they can never disagree about where a biome begins — a shrine is
//! placed in the Meadows by exactly the rule that painted the Meadows grass.
//! Everything is a pure function of `(seed, x, z)` and the config, which is
//! what lets a host answer "where is the nearest altar?" without generating a
//! single chunk.

use std::sync::Arc;

use super::biome::{BiomeId, Blend, pick_ring};
use super::config::{BiomeGen, WorldGenConfig};
use super::noise::TerrainNoise;
use crate::domain::core::CHUNK_HEIGHT;

/// Everything generation needs to know about one column before placing blocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    /// The top solid block's y (the surface), before features.
    pub height: i32,
    /// The biome whose blocks this column is built from.
    pub biome: BiomeId,
    /// How the column's terrain *shape* mixes two rings near a border.
    pub blend: Blend,
}

pub struct Terrain {
    noise: TerrainNoise,
    config: Arc<WorldGenConfig>,
}

impl Terrain {
    pub fn new(seed: u64, config: Arc<WorldGenConfig>) -> Self {
        Self {
            noise: TerrainNoise::with_ore_fields(seed as u32, config.ores.len()),
            config,
        }
    }

    pub fn noise(&self) -> &TerrainNoise {
        &self.noise
    }

    pub fn config(&self) -> &WorldGenConfig {
        &self.config
    }

    /// Distance from the origin as the rings see it: the true distance, bent
    /// in and out by the low-frequency warp and roughened by the jitter.
    pub fn ring_distance(&self, x: i32, z: i32) -> f32 {
        let true_distance = (x as f32).hypot(z as f32);
        true_distance
            + self.noise.ring_warp(x, z) * self.config.ring_warp
            + self.noise.ring_jitter(x, z)
    }

    fn ring(&self, x: i32, z: i32) -> (BiomeId, Blend) {
        pick_ring(
            self.ring_distance(x, z),
            self.config.ring_starts(),
            self.config.ring_blend,
        )
    }

    /// The biome whose blocks cover column `(x, z)`.
    pub fn biome_at(&self, x: i32, z: i32) -> BiomeId {
        self.ring(x, z).0
    }

    /// The generation rules of the biome at `(x, z)`.
    pub fn biome_gen(&self, x: i32, z: i32) -> &BiomeGen {
        self.config.biome(self.biome_at(x, z))
    }

    /// Sample one column: its biome, border blend and surface height.
    pub fn column(&self, x: i32, z: i32) -> Column {
        let (biome, blend) = self.ring(x, z);
        let inner = self.config.biome(blend.inner).shape;
        let outer = self.config.biome(blend.outer).shape;
        let shape = inner.lerp(outer, f64::from(blend.t));
        let height = self
            .noise
            .surface_height(x, z, shape, self.config.sea_level)
            .clamp(1, CHUNK_HEIGHT - 1);
        Column {
            height,
            biome,
            blend,
        }
    }

    /// Surface height alone (see [`Terrain::column`]).
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.column(x, z).height
    }

    /// The colour for tint source `index` at `(x, z)`, blended across a border
    /// exactly as the terrain shape is, so grass fades between two biomes'
    /// greens instead of switching at a line.
    pub fn tint(&self, x: i32, z: i32, index: u8) -> [u8; 4] {
        let (_, blend) = self.ring(x, z);
        let a = self.config.biome(blend.inner).tint(index);
        if blend.inner == blend.outer {
            return a;
        }
        let b = self.config.biome(blend.outer).tint(index);
        std::array::from_fn(|i| {
            let (a, b) = (f32::from(a[i]), f32::from(b[i]));
            (a + (b - a) * blend.t).round() as u8
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::world::block::BlockRegistry;

    fn terrain(seed: u64) -> Terrain {
        let config = WorldGenConfig::builtin(&BlockRegistry::with_builtins());
        Terrain::new(seed, Arc::new(config))
    }

    /// Spawn is always in the first ring, whatever the warp does.
    #[test]
    fn the_origin_is_always_meadows() {
        for seed in 0..32 {
            assert_eq!(terrain(seed).biome_at(0, 0), BiomeId(0), "seed {seed}");
        }
    }

    /// Walking straight out from spawn passes through every ring in order.
    #[test]
    fn walking_outward_visits_every_ring_in_order() {
        let t = terrain(42);
        let mut seen: Vec<BiomeId> = Vec::new();
        for step in 0..=500 {
            let biome = t.biome_at(step * 10, 0);
            if seen.last() != Some(&biome) {
                seen.push(biome);
            }
        }
        // Jitter can flicker a border, but never skip or reverse a whole ring.
        let mut firsts: Vec<BiomeId> = Vec::new();
        for b in seen {
            if !firsts.contains(&b) {
                firsts.push(b);
            }
        }
        let expected: Vec<BiomeId> = (0..5).map(BiomeId).collect();
        assert_eq!(firsts, expected);
    }

    /// The warp moves a border by at most `ring_warp` plus the jitter.
    #[test]
    fn a_border_stays_within_the_warp_of_its_radius() {
        let t = terrain(7);
        let slack = t.config().ring_warp * 1.5 + super::super::noise::RING_JITTER;
        for angle in 0..64 {
            let a = angle as f32 / 64.0 * std::f32::consts::TAU;
            let at = |r: f32| t.biome_at((a.cos() * r) as i32, (a.sin() * r) as i32);
            assert_eq!(at(700.0 - slack), BiomeId(0));
            assert_eq!(at(700.0 + slack), BiomeId(1));
        }
    }

    /// No cliffs at a border: neighbouring columns never differ by more than
    /// ordinary terrain does.
    #[test]
    fn borders_are_blended_not_cliffs() {
        let t = terrain(3);
        let mut worst = 0;
        for x in 2400..2900 {
            let a = t.height(x, 11);
            let b = t.height(x + 1, 11);
            worst = worst.max((a - b).abs());
        }
        assert!(
            worst <= 6,
            "a {worst}-block step across the mire/frostpeaks border"
        );
    }

    #[test]
    fn tints_fade_across_a_border() {
        let t = terrain(5);
        let meadows = t.config().biome(BiomeId(0)).tint(0);
        let darkwood = t.config().biome(BiomeId(1)).tint(0);
        assert_eq!(t.tint(0, 0, 0), meadows);
        assert_eq!(t.tint(1150, 0, 0), darkwood);
        let mut between = false;
        for x in 400..1000 {
            let c = t.tint(x, 0, 0);
            if c != meadows && c != darkwood {
                between = true;
            }
        }
        assert!(between, "no blended colour anywhere along the border");
    }

    #[test]
    fn columns_are_deterministic_in_seed_and_position() {
        let (a, b) = (terrain(9), terrain(9));
        for (x, z) in [(0, 0), (-1234, 77), (3000, -4000)] {
            assert_eq!(a.column(x, z), b.column(x, z));
        }
    }
}
