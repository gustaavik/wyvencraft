//! Which biome a column belongs to: Valheim-style distance rings.
//!
//! Biomes are concentric bands around the world origin, ordered by
//! progression tier — the further from spawn, the harder the land. The
//! *distance* fed in here is already warped by noise (see
//! [`super::terrain::Terrain::ring_distance`]), so the rings come out as
//! ragged, wandering borders rather than circles. This module is only the pure
//! step from that distance to a biome and a blend between two of them.
//!
//! What each biome *places* is data — [`super::config::BiomeGen`].

/// Index of a biome in [`super::config::WorldGenConfig::biomes`], which is in
/// ring order: `BiomeId(0)` is the innermost ring, around spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BiomeId(pub u8);

impl BiomeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// How a column sits between two neighbouring rings: `t = 0` is wholly the
/// `inner` biome, `t = 1` wholly the `outer`. Away from any border `inner ==
/// outer` and `t == 0`.
///
/// Terrain *shape* (height, relief, mesas) is interpolated by `t`, so a border
/// never becomes a cliff; the blocks placed are chosen discretely, by which
/// side of the border the column is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blend {
    pub inner: BiomeId,
    pub outer: BiomeId,
    pub t: f32,
}

impl Blend {
    fn solid(biome: BiomeId) -> Self {
        Self {
            inner: biome,
            outer: biome,
            t: 0.0,
        }
    }
}

/// Pick the ring a (warped) distance falls in, and how it blends with its
/// neighbour.
///
/// `starts[i]` is the inner radius of ring `i`: ascending, with `starts[0]`
/// the centre. The last ring has no outer edge. A distance below the first
/// start (the warp can push one negative) belongs to ring 0. `blend` is the
/// half-width of the band each border is smoothed across.
pub fn pick_ring(distance: f32, starts: &[f32], blend: f32) -> (BiomeId, Blend) {
    debug_assert!(!starts.is_empty(), "a world needs at least one biome");
    let index = starts
        .iter()
        .rposition(|&start| distance >= start)
        .unwrap_or(0);
    let biome = BiomeId(index as u8);

    // The border nearest this distance: either this ring's own inner edge or
    // the next ring's.
    let nearest = [index, index + 1]
        .into_iter()
        .filter(|&i| i >= 1 && i < starts.len())
        .min_by(|&a, &b| {
            (distance - starts[a])
                .abs()
                .total_cmp(&(distance - starts[b]).abs())
        });
    let Some(border) = nearest else {
        return (biome, Blend::solid(biome));
    };
    let edge = starts[border];
    if blend <= 0.0 || (distance - edge).abs() >= blend {
        return (biome, Blend::solid(biome));
    }
    let t = smoothstep(edge - blend, edge + blend, distance);
    let blend = Blend {
        inner: BiomeId((border - 1) as u8),
        outer: BiomeId(border as u8),
        t,
    };
    (biome, blend)
}

/// Hermite interpolation from 0 at `edge0` to 1 at `edge1`, clamped outside.
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STARTS: [f32; 4] = [0.0, 700.0, 1600.0, 2600.0];

    #[test]
    fn a_distance_falls_in_the_ring_whose_band_contains_it() {
        let ring = |d| pick_ring(d, &STARTS, 0.0).0;
        assert_eq!(ring(0.0), BiomeId(0));
        assert_eq!(ring(699.0), BiomeId(0));
        assert_eq!(ring(700.0), BiomeId(1));
        assert_eq!(ring(1599.0), BiomeId(1));
        assert_eq!(ring(2000.0), BiomeId(2));
    }

    #[test]
    fn the_last_ring_has_no_outer_edge() {
        assert_eq!(pick_ring(1.0e7, &STARTS, 48.0).0, BiomeId(3));
    }

    #[test]
    fn a_warped_negative_distance_is_still_the_centre() {
        assert_eq!(pick_ring(-150.0, &STARTS, 48.0).0, BiomeId(0));
    }

    #[test]
    fn away_from_borders_there_is_no_blend() {
        let (biome, blend) = pick_ring(300.0, &STARTS, 48.0);
        assert_eq!(blend, Blend::solid(biome));
    }

    /// Across a border the shape blend runs 0 → 1 monotonically, and is
    /// exactly half-way on the border itself.
    #[test]
    fn the_blend_rises_monotonically_across_a_border() {
        let mut last = -1.0;
        for step in 1..96 {
            let d = 700.0 - 48.0 + step as f32;
            let (_, blend) = pick_ring(d, &STARTS, 48.0);
            assert_eq!((blend.inner, blend.outer), (BiomeId(0), BiomeId(1)));
            assert!(blend.t >= last, "t fell at {d}");
            last = blend.t;
        }
        let (_, on_border) = pick_ring(700.0, &STARTS, 48.0);
        assert!((on_border.t - 0.5).abs() < 1e-6);
    }

    #[test]
    fn a_single_biome_world_never_blends() {
        let (biome, blend) = pick_ring(5000.0, &[0.0], 48.0);
        assert_eq!(biome, BiomeId(0));
        assert_eq!(blend.t, 0.0);
    }
}
