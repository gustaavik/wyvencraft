//! Tests for [`super`]: `config.rs`.

use super::*;
use crate::domain::world::block::blocks;

fn builtin() -> WorldGenConfig {
    WorldGenConfig::builtin(&BlockRegistry::with_builtins())
}

fn with(text: String) -> Result<WorldGenConfig, String> {
    WorldGenConfig::from_toml(&text, &BlockRegistry::with_builtins())
}

/// The shipped progression: five rings, innermost first, tiers climbing.
#[test]
fn the_builtin_rings_climb_in_tier_outward() {
    let config = builtin();
    let ids: Vec<&str> = config.biomes().iter().map(|b| b.id.as_str()).collect();
    assert_eq!(
        ids,
        ["meadows", "darkwood", "mire", "frostpeaks", "ashlands"]
    );
    let tiers: Vec<u8> = config.biomes().iter().map(|b| b.tier).collect();
    assert_eq!(tiers, [1, 2, 3, 4, 5]);
    assert_eq!(config.find_biome("mire"), Some(BiomeId(2)));
    assert_eq!(config.find_biome("nowhere"), None);
    let meadows = config.biome(BiomeId(0));
    assert_eq!(
        (meadows.surface, meadows.subsurface),
        (blocks::GRASS, blocks::DIRT)
    );
}

#[test]
fn layers_cover_the_underground_top_to_bottom() {
    let config = builtin();
    let ids: Vec<&str> = config.layers.iter().map(|l| l.id.as_str()).collect();
    assert_eq!(ids, ["surface", "cavern", "deep"]);
    assert_eq!(config.layer_at(200), 0);
    assert_eq!(config.layer_at(config.layers[0].bottom), 0);
    assert_eq!(config.layer_at(config.layers[0].bottom - 1), 1);
    assert_eq!(config.layer_at(1), 2);
    assert_eq!(config.layers[2].fill, blocks::DEEPSTONE);
}

/// Each biome's ore lies only in that biome — that is what makes the next
/// ring worth travelling to.
#[test]
fn biome_ores_are_confined_to_their_biome() {
    let config = builtin();
    let tin = config
        .ores
        .iter()
        .find(|o| o.block == blocks::TIN_ORE)
        .expect("tin ore");
    let darkwood = config.find_biome("darkwood").unwrap();
    assert!(tin.allowed(darkwood, 1));
    assert!(!tin.allowed(BiomeId(0), 1));
    let coal = config
        .ores
        .iter()
        .find(|o| o.block == blocks::COAL_ORE)
        .unwrap();
    assert!(coal.allowed(BiomeId(0), 0), "coal is everywhere near spawn");
}

/// The three tint sources a `tintindex` can name, each routed to its own
/// biome colour.
#[test]
fn every_tint_source_resolves_to_its_own_colour() {
    let config = builtin();
    let meadows = config.biome(BiomeId(0));
    assert_eq!(meadows.tint(0), [124, 189, 107, 255]);
    assert_eq!(meadows.tint(1), [93, 165, 74, 255]);
    assert_eq!(meadows.tint(2), [63, 118, 228, 255]);
    assert_ne!(config.biome(BiomeId(2)).tint(2), meadows.tint(2));
}

/// Grass and foliage default to white, which multiplies to no change.
/// Water cannot: its art is greyscale and nothing else supplies the blue.
#[test]
fn an_unstated_water_colour_takes_the_default_blue() {
    let text = BUILTIN_WORLDGEN.replace("water_tint = [63, 118, 228]", "");
    let config = with(text).expect("still valid");
    assert_eq!(config.biome(BiomeId(0)).tint(2), DEFAULT_WATER_TINT);
    assert_ne!(DEFAULT_WATER_TINT, NO_TINT);
}

/// Unknown block names must reject the whole file.
#[test]
fn unknown_blocks_are_rejected() {
    let text = BUILTIN_WORLDGEN.replace("bedrock = \"bedrock\"", "bedrock = \"nope\"");
    assert!(with(text).is_err());
}

#[test]
fn an_ore_naming_an_unknown_biome_or_layer_is_rejected() {
    let biome = BUILTIN_WORLDGEN.replacen("biomes = [\"darkwood\"]", "biomes = [\"nope\"]", 1);
    assert!(with(biome).unwrap_err().contains("unknown biome"));
    let layer = BUILTIN_WORLDGEN.replacen("\"cavern\"]", "\"nope\"]", 1);
    assert!(with(layer).unwrap_err().contains("unknown layer"));
}

#[test]
fn rings_out_of_order_are_rejected() {
    let text = BUILTIN_WORLDGEN.replace("start = 1600", "start = 500");
    assert!(with(text).unwrap_err().contains("must lie outside"));
}

#[test]
fn a_first_ring_off_centre_is_rejected() {
    let text = BUILTIN_WORLDGEN.replacen("start = 0", "start = 10", 1);
    assert!(with(text).unwrap_err().contains("must start at 0"));
}

#[test]
fn layers_that_stop_short_of_bedrock_are_rejected() {
    let text = BUILTIN_WORLDGEN.replace("bottom = 1\n", "bottom = 5\n");
    assert!(with(text).unwrap_err().contains("y = 1"));
}
