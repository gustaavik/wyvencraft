//! Everything below the surface blocks: the layered underground, its caves,
//! and its ore veins.
//!
//! The ground is a stack of bands ([`super::config::Layer`]) — a dirt-flecked
//! surface layer, the stone caverns, then deepstone down to bedrock — each
//! with its own fill, marbling, cave density and ores. Which ores appear also
//! depends on the biome above, which is what makes each ring's metal worth the
//! trip. All of it is a pure function of `(seed, x, y, z)`.

use super::biome::BiomeId;
use super::config::WorldGenConfig;
use super::noise::TerrainNoise;
use crate::domain::core::BlockId;

/// Depth below the surface that is never carved, so the ground does not
/// collapse into swiss cheese from above. Thicker under water, so seas do not
/// drain into the caves beneath them.
const LAND_SHELL: i32 = 2;
const SEA_SHELL: i32 = 4;
/// Caverns in the surface layer thin out toward the top: the blob threshold
/// rises by this much across the first `CRUST_FADE` blocks below the surface.
const CRUST_FADE: i32 = 24;
const CRUST_THRESHOLD_BONUS: f32 = 0.12;

/// One underground cell's context, sampled once per column.
#[derive(Debug, Clone, Copy)]
pub struct Ground {
    pub biome: BiomeId,
    /// The column's surface y.
    pub height: i32,
    pub underwater: bool,
    /// Whether a cave may open onto the surface here.
    pub entrance: bool,
}

/// What occupies an underground cell: solid rock of some kind, or a void.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    Solid(BlockId),
    /// Carved out. `Some` when the cave is flooded with that block.
    Void(Option<BlockId>),
}

/// The underground block at `(x, y, z)`, for `y` below the surface blocks.
pub fn underground(
    noise: &TerrainNoise,
    config: &WorldGenConfig,
    ground: Ground,
    x: i32,
    y: i32,
    z: i32,
) -> Fill {
    let layer_index = config.layer_at(y);
    let layer = &config.layers[layer_index];
    if is_cave(noise, config, ground, layer_index, x, y, z) {
        let flooded = layer
            .pool
            .and_then(|(block, below)| (y < below).then_some(block));
        return Fill::Void(flooded);
    }
    for (field, vein) in config.ores.iter().enumerate() {
        if vein.allowed(ground.biome, layer_index)
            && noise.ore_density(field, x, y, z) > vein.threshold
        {
            return Fill::Solid(vein.block);
        }
    }
    let mix = match layer_index {
        0 => config
            .biome(ground.biome)
            .strata
            .map(|block| (block, layer.mix.map_or(0.35, |(_, above)| above)))
            .or(layer.mix),
        _ => layer.mix,
    };
    match mix {
        Some((block, above)) if noise.strata(x, y, z) > above => Fill::Solid(block),
        _ => Fill::Solid(layer.fill),
    }
}

/// Whether caves hollow out this cell. Blob caverns open up with depth and
/// winding tunnels connect them, at the density the layer asks for. A crust
/// under the surface is never carved — except at a cave mouth on dry land.
fn is_cave(
    noise: &TerrainNoise,
    config: &WorldGenConfig,
    ground: Ground,
    layer_index: usize,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    let shell = match (ground.underwater, ground.entrance) {
        (true, _) => SEA_SHELL,
        (false, true) => 0,
        (false, false) => LAND_SHELL,
    };
    let depth = ground.height - y;
    if depth < shell {
        return false;
    }
    let caves = config.layers[layer_index].caves;
    let crust = if layer_index == 0 {
        CRUST_THRESHOLD_BONUS * (1.0 - (depth as f32 / CRUST_FADE as f32).min(1.0))
    } else {
        0.0
    };
    if noise.cave_blob(x, y, z) > caves.blob + crust {
        return true;
    }
    noise.cave_tunnel(x, y, z) < caves.tunnel
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::world::block::{BlockRegistry, blocks};
    use crate::domain::world::generation::terrain::Terrain;

    fn terrain() -> Terrain {
        let config = WorldGenConfig::builtin(&BlockRegistry::with_builtins());
        Terrain::new(42, Arc::new(config))
    }

    fn ground(biome: u8) -> Ground {
        Ground {
            biome: BiomeId(biome),
            height: 100,
            underwater: false,
            entrance: false,
        }
    }

    /// Tally the fill of a block of cells in one layer of one biome.
    fn tally(t: &Terrain, biome: u8, ys: std::ops::Range<i32>) -> Vec<Fill> {
        let mut out = Vec::new();
        for x in 0..48 {
            for z in 0..48 {
                for y in ys.clone() {
                    out.push(underground(t.noise(), t.config(), ground(biome), x, y, z));
                }
            }
        }
        out
    }

    fn count(fills: &[Fill], block: BlockId) -> usize {
        fills.iter().filter(|&&f| f == Fill::Solid(block)).count()
    }

    #[test]
    fn each_layer_is_mostly_its_own_fill() {
        let t = terrain();
        let deep = tally(&t, 0, 2..28);
        assert!(count(&deep, blocks::DEEPSTONE) * 2 > deep.len());
        assert_eq!(count(&deep, blocks::STONE), 0, "no plain stone in the deep");
        let cavern = tally(&t, 0, 32..70);
        assert!(count(&cavern, blocks::STONE) * 2 > cavern.len());
        assert_eq!(count(&cavern, blocks::DEEPSTONE), 0);
    }

    /// Marbling: the surface layer carries dirt pockets under the Meadows and
    /// mud under the Mire — the biome's `strata` wins.
    #[test]
    fn the_surface_layer_is_marbled_with_the_biomes_strata() {
        let t = terrain();
        let meadows = tally(&t, 0, 74..96);
        assert!(count(&meadows, blocks::DIRT) > 0);
        assert_eq!(count(&meadows, blocks::MUD), 0);
        let mire = tally(&t, 2, 74..96);
        assert!(count(&mire, blocks::MUD) > 0);
        assert_eq!(count(&mire, blocks::DIRT), 0);
    }

    /// A biome's ore is found under it and nowhere else.
    #[test]
    fn biome_ores_stay_under_their_biome() {
        let t = terrain();
        let darkwood = tally(&t, 1, 2..70);
        let meadows = tally(&t, 0, 2..70);
        for ore in [blocks::TIN_ORE, blocks::COPPER_ORE] {
            assert!(count(&darkwood, ore) > 0, "{ore:?} missing under darkwood");
            assert_eq!(count(&meadows, ore), 0, "{ore:?} under the meadows");
        }
        assert!(count(&meadows, blocks::COAL_ORE) > 0);
        let rare = count(&darkwood, blocks::TIN_ORE) + count(&darkwood, blocks::COPPER_ORE);
        assert!(rare * 12 < darkwood.len(), "ore should stay rare: {rare}");
    }

    #[test]
    fn the_deepest_caves_are_flooded() {
        let t = terrain();
        let deep = tally(&t, 0, 2..12);
        let flooded = deep
            .iter()
            .filter(|&&f| f == Fill::Void(Some(blocks::WATER)))
            .count();
        assert!(flooded > 0);
        assert!(
            !deep.contains(&Fill::Void(None)),
            "dry void below the pool line"
        );
    }

    #[test]
    fn the_crust_is_never_carved_except_at_a_cave_mouth() {
        let t = terrain();
        let mut mouths = 0;
        for x in 0..256 {
            for z in 0..64 {
                let sealed = ground(0);
                let top = underground(t.noise(), t.config(), sealed, x, sealed.height - 1, z);
                assert!(matches!(top, Fill::Solid(_)));
                let open = Ground {
                    entrance: true,
                    ..sealed
                };
                for y in open.height - 2..=open.height {
                    if matches!(
                        underground(t.noise(), t.config(), open, x, y, z),
                        Fill::Void(_)
                    ) {
                        mouths += 1;
                    }
                }
            }
        }
        assert!(mouths > 0, "an entrance never opened a cave to the sky");
    }
}
