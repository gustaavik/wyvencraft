//! World-generation configuration, loaded from `assets/worldgen.toml`.
//!
//! The *numbers and block choices* of generation are data; the noise fields
//! and feature shapes stay code (see the sibling modules). Resolution is
//! strict: an unknown block, biome or layer name rejects the whole file,
//! because worldgen quietly placing the wrong block would corrupt every world
//! built with it. Generation changes are pinned by `worldgen_golden_hashes`.

use super::biome::BiomeId;
use super::noise::TerrainShape;
use crate::domain::core::ident::is_valid_id;
use crate::domain::core::{BlockId, CHUNK_HEIGHT};
use crate::domain::world::block::BlockRegistry;

/// Embedded copy of the shipped worldgen configuration.
pub const BUILTIN_WORLDGEN: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/worldgen.toml"));

/// An ore vein: the block it places, where it may appear, and how rare it is
/// (higher threshold = rarer). The vein's index in the table picks its noise
/// field, so table order is part of a world's identity.
#[derive(Debug)]
pub struct OreVein {
    pub block: BlockId,
    pub threshold: f32,
    /// Biomes whose ground holds it. Empty means every biome.
    pub biomes: Vec<BiomeId>,
    /// Underground layers (indices into [`WorldGenConfig::layers`]) it lies
    /// in. Empty means every layer.
    pub layers: Vec<usize>,
}

impl OreVein {
    /// Whether this vein may appear in `biome`'s ground at `layer`.
    pub fn allowed(&self, biome: BiomeId, layer: usize) -> bool {
        (self.biomes.is_empty() || self.biomes.contains(&biome))
            && (self.layers.is_empty() || self.layers.contains(&layer))
    }
}

/// How hollow one underground layer is. Thresholds are on the cave noise
/// fields: a larger `blob` means fewer, smaller caverns; a larger `tunnel`
/// means wider, more frequent tunnels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaveParams {
    pub blob: f32,
    pub tunnel: f32,
}

/// One band of the underground, Terraria style: what it is made of, what is
/// marbled through it, and how many caves it holds.
#[derive(Debug)]
pub struct Layer {
    pub id: String,
    /// Lowest y of the band. The band's top is the previous layer's bottom
    /// (the surface, for the first).
    pub bottom: i32,
    pub fill: BlockId,
    /// A second block swirled through the fill where the strata noise exceeds
    /// `above` — dirt pockets in rock, gravel seams in the caverns.
    pub mix: Option<(BlockId, f32)>,
    pub caves: CaveParams,
    /// Carved air below `y` fills with this block instead: flooded depths.
    pub pool: Option<(BlockId, i32)>,
}

/// Ocean-floor covering choices (see `[seabed]` in the file).
#[derive(Debug)]
pub struct SeabedConfig {
    pub shallow: BlockId,
    pub default_block: BlockId,
    pub gravel: BlockId,
    pub gravel_above: f32,
    pub clay: BlockId,
    pub clay_below: f32,
}

/// A canopy-building strategy implemented in `features.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TreeShape {
    Oak,
    Spruce,
}

/// One tree type: a code shape parameterized by blocks and trunk height.
#[derive(Debug)]
pub struct TreeDef {
    pub shape: TreeShape,
    pub trunk: BlockId,
    pub leaves: BlockId,
    /// Inclusive `(min, max)` trunk height in blocks.
    pub trunk_height: (i32, i32),
}

#[derive(Debug)]
pub struct BoulderConfig {
    pub block: BlockId,
    pub chance_per_mille: u64,
}

/// One biome: its place in the progression, the ring it occupies, the shape
/// of its land, and what it places on and under the surface.
#[derive(Debug)]
pub struct BiomeGen {
    /// The name other files use for it (`structures.toml`, `spawning.toml`).
    pub id: String,
    /// Progression tier, 1 at spawn. Shown to the player; bosses and gear are
    /// balanced against it.
    pub tier: u8,
    /// Inner radius of this biome's ring, in (warped) blocks from the origin.
    pub start: f32,
    /// Base height, relief and mesa strength — interpolated across borders.
    pub shape: TerrainShape,
    pub surface: BlockId,
    pub subsurface: BlockId,
    /// Replaces the first underground layer's `mix` block under this biome:
    /// mud under the Mire, packed snow under the Frostpeaks.
    pub strata: Option<BlockId>,
    /// Index into [`WorldGenConfig::trees`] + per-mille candidate chance.
    pub tree: Option<(usize, f32)>,
    /// Ground cover scattered one block above the surface, picked uniformly.
    /// Empty means a bare biome; the chance is per candidate cell, before the
    /// same vegetation clumping that groups trees into groves.
    pub plants: Vec<BlockId>,
    pub plant_chance_per_mille: f32,
    /// The colour multiplied into faces a block model marked `tintindex = 0` —
    /// grass tops, grass side overlays and the like. Defaults to white, which
    /// is the identity, so a biome that says nothing tints nothing.
    pub tint: [u8; 4],
    /// The same for `tintindex = 1`: leaves and other hanging foliage, which
    /// Minecraft colours a shade apart from the ground. Defaults to `tint`.
    pub foliage_tint: [u8; 4],
    /// The same for `tintindex = 2`: water. Defaults to
    /// [`DEFAULT_WATER_TINT`] rather than to white — the water art is
    /// greyscale, so an untinted sea would read as grey rather than as water.
    pub water_tint: [u8; 4],
}

/// The water colour of a biome that does not name one. Unlike grass and
/// foliage, white is not a sensible default here: nothing else supplies the
/// blue.
pub const DEFAULT_WATER_TINT: [u8; 4] = [63, 118, 228, 255];

/// The identity tint: white, which leaves a texture as authored. The renderer
/// spells the same value `wyven_render::NO_TINT`; it is restated here so the
/// biome rules need no renderer, and `presentation::render` pins the two equal.
pub const NO_TINT: [u8; 4] = [255; 4];

impl BiomeGen {
    /// The colour for one of the tint sources a model's `tintindex` names.
    /// Out-of-range indices take grass, which is the safe wrong answer: a
    /// greyscale texture stays visible rather than turning white.
    pub fn tint(&self, index: u8) -> [u8; 4] {
        match index {
            1 => self.foliage_tint,
            2 => self.water_tint,
            _ => self.tint,
        }
    }
}

#[derive(Debug)]
pub struct WorldGenConfig {
    pub bedrock: BlockId,
    pub water: BlockId,
    pub sea_level: i32,
    /// How far (blocks) the low-frequency warp may push a ring border.
    pub ring_warp: f32,
    /// Half-width (blocks) of the band a border's terrain shape blends over.
    pub ring_blend: f32,
    pub seabed: SeabedConfig,
    /// Top to bottom; the last reaches down to y = 1, just above bedrock.
    pub layers: Vec<Layer>,
    pub ores: Vec<OreVein>,
    pub trees: Vec<TreeDef>,
    pub boulder: BoulderConfig,
    /// In ring order, innermost first; indexed by [`BiomeId`].
    biomes: Vec<BiomeGen>,
    /// `biomes[i].start`, gathered for [`super::biome::pick_ring`].
    starts: Vec<f32>,
}

impl WorldGenConfig {
    /// Build from the embedded copy of `assets/worldgen.toml`. Infallible
    /// against the builtin block set (pinned by tests).
    pub fn builtin(blocks: &BlockRegistry) -> Self {
        Self::from_toml(BUILTIN_WORLDGEN, blocks).expect("embedded worldgen.toml must parse")
    }

    /// Parse a worldgen file against the loaded blocks. Any error — bad TOML
    /// or an unresolvable name — fails the whole file.
    pub fn from_toml(text: &str, blocks: &BlockRegistry) -> Result<Self, String> {
        let file: WorldGenFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let resolve = |name: &str| {
            blocks
                .find(name)
                .ok_or_else(|| format!("unknown block {name:?}"))
        };

        let trees: Vec<TreeDef> = file
            .tree
            .iter()
            .map(|t| {
                if t.trunk_height[0] < 1 || t.trunk_height[1] < t.trunk_height[0] {
                    return Err(format!("tree {:?}: bad trunk_height", t.name));
                }
                Ok(TreeDef {
                    shape: t.shape,
                    trunk: resolve(&t.trunk)?,
                    leaves: resolve(&t.leaves)?,
                    trunk_height: (t.trunk_height[0], t.trunk_height[1]),
                })
            })
            .collect::<Result<_, String>>()?;

        let biomes = file
            .biome
            .iter()
            .map(|def| resolve_biome(def, &file, &resolve))
            .collect::<Result<Vec<_>, String>>()?;
        validate_rings(&biomes)?;
        let layers = file
            .layer
            .iter()
            .map(|def| resolve_layer(def, &resolve))
            .collect::<Result<Vec<_>, String>>()?;
        validate_layers(&layers)?;

        let biome_index = |name: &str| {
            biomes
                .iter()
                .position(|b| b.id == name)
                .map(|i| BiomeId(i as u8))
                .ok_or_else(|| format!("unknown biome {name:?}"))
        };
        let layer_index = |name: &str| {
            layers
                .iter()
                .position(|l| l.id == name)
                .ok_or_else(|| format!("unknown layer {name:?}"))
        };
        let ores = file
            .ore
            .iter()
            .map(|o| {
                Ok(OreVein {
                    block: resolve(&o.block)?,
                    threshold: o.threshold,
                    biomes: o
                        .biomes
                        .iter()
                        .map(|b| biome_index(b))
                        .collect::<Result<_, String>>()?,
                    layers: o
                        .layers
                        .iter()
                        .map(|l| layer_index(l))
                        .collect::<Result<_, String>>()?,
                })
            })
            .collect::<Result<_, String>>()?;

        let starts = biomes.iter().map(|b| b.start).collect();
        Ok(Self {
            bedrock: resolve(&file.terrain.bedrock)?,
            water: resolve(&file.terrain.water)?,
            sea_level: file.terrain.sea_level,
            ring_warp: file.terrain.ring_warp,
            ring_blend: file.terrain.ring_blend,
            seabed: SeabedConfig {
                shallow: resolve(&file.seabed.shallow)?,
                default_block: resolve(&file.seabed.default)?,
                gravel: resolve(&file.seabed.gravel.block)?,
                gravel_above: file
                    .seabed
                    .gravel
                    .above
                    .ok_or("seabed.gravel needs `above`")?,
                clay: resolve(&file.seabed.clay.block)?,
                clay_below: file.seabed.clay.below.ok_or("seabed.clay needs `below`")?,
            },
            layers,
            ores,
            trees,
            boulder: BoulderConfig {
                block: resolve(&file.boulder.block)?,
                chance_per_mille: file.boulder.chance_per_mille,
            },
            biomes,
            starts,
        })
        .and_then(|config| {
            if !(1..CHUNK_HEIGHT).contains(&config.sea_level) {
                return Err(format!(
                    "sea_level {} is outside the world",
                    config.sea_level
                ));
            }
            Ok(config)
        })
    }

    /// The generation choices for `biome`.
    pub fn biome(&self, biome: BiomeId) -> &BiomeGen {
        &self.biomes[biome.index()]
    }

    /// Every biome, innermost ring first.
    pub fn biomes(&self) -> &[BiomeGen] {
        &self.biomes
    }

    /// The biome another file names, if this config declares it.
    pub fn find_biome(&self, id: &str) -> Option<BiomeId> {
        self.biomes
            .iter()
            .position(|b| b.id == id)
            .map(|i| BiomeId(i as u8))
    }

    /// Each ring's inner radius, in ring order.
    pub fn ring_starts(&self) -> &[f32] {
        &self.starts
    }

    /// The underground layer holding height `y`.
    pub fn layer_at(&self, y: i32) -> usize {
        self.layers
            .iter()
            .position(|layer| y >= layer.bottom)
            .unwrap_or(self.layers.len() - 1)
    }
}

type Resolve<'a> = dyn Fn(&str) -> Result<BlockId, String> + 'a;

fn rgba(rgb: Option<[u8; 3]>, default: [u8; 4]) -> [u8; 4] {
    match rgb {
        Some([r, g, b]) => [r, g, b, 255],
        None => default,
    }
}

fn resolve_biome(
    def: &BiomeDef,
    file: &WorldGenFile,
    resolve: &Resolve<'_>,
) -> Result<BiomeGen, String> {
    if !is_valid_id(&def.id) {
        return Err(format!(
            "biome id {:?} must be lowercase letters, digits and underscores",
            def.id
        ));
    }
    let tree = match &def.tree {
        Some(name) => {
            let index = file
                .tree
                .iter()
                .position(|t| t.name == *name)
                .ok_or_else(|| format!("biome {:?}: unknown tree {name:?}", def.id))?;
            Some((index, def.tree_chance_per_mille.unwrap_or(0.0)))
        }
        None => None,
    };
    let tint = rgba(def.tint, NO_TINT);
    Ok(BiomeGen {
        id: def.id.clone(),
        tier: def.tier,
        start: def.start,
        shape: TerrainShape {
            base: (file.terrain.base_height + def.base_offset) as f64,
            amplitude: def.amplitude as f64,
            mesa: def.mesa as f64,
        },
        surface: resolve(&def.surface)?,
        subsurface: resolve(&def.subsurface)?,
        strata: def.strata.as_deref().map(resolve).transpose()?,
        tree,
        plants: def
            .plants
            .iter()
            .map(|name| resolve(name))
            .collect::<Result<_, String>>()?,
        plant_chance_per_mille: def.plant_chance_per_mille.unwrap_or(0.0),
        tint,
        foliage_tint: rgba(def.foliage_tint, tint),
        water_tint: rgba(def.water_tint, DEFAULT_WATER_TINT),
    })
}

/// Rings must start at the centre, grow outward, and be uniquely named — a
/// ring out of order would silently swallow the one before it.
fn validate_rings(biomes: &[BiomeGen]) -> Result<(), String> {
    let Some(first) = biomes.first() else {
        return Err("worldgen declares no [[biome]]".into());
    };
    if first.start != 0.0 {
        return Err(format!("the first biome ({:?}) must start at 0", first.id));
    }
    if biomes.len() > usize::from(u8::MAX) {
        return Err("too many biomes".into());
    }
    for pair in biomes.windows(2) {
        if pair[1].start <= pair[0].start {
            return Err(format!(
                "biome {:?} starts at {} but must lie outside {:?} ({})",
                pair[1].id, pair[1].start, pair[0].id, pair[0].start
            ));
        }
    }
    for (i, biome) in biomes.iter().enumerate() {
        if biomes[..i].iter().any(|b| b.id == biome.id) {
            return Err(format!("duplicate biome {:?}", biome.id));
        }
    }
    Ok(())
}

fn resolve_layer(def: &LayerDef, resolve: &Resolve<'_>) -> Result<Layer, String> {
    Ok(Layer {
        id: def.id.clone(),
        bottom: def.bottom,
        fill: resolve(&def.fill)?,
        mix: match &def.mix {
            Some(mix) => Some((resolve(&mix.block)?, mix.above)),
            None => None,
        },
        caves: CaveParams {
            blob: def.caves.blob,
            tunnel: def.caves.tunnel,
        },
        pool: match &def.pool {
            Some(pool) => Some((resolve(&pool.block)?, pool.below)),
            None => None,
        },
    })
}

/// Layers stack top to bottom with no gap, and the last one reaches the
/// bedrock floor, so every underground cell belongs to exactly one.
fn validate_layers(layers: &[Layer]) -> Result<(), String> {
    let Some(last) = layers.last() else {
        return Err("worldgen declares no [[layer]]".into());
    };
    if last.bottom != 1 {
        return Err(format!(
            "the last layer ({:?}) must reach down to y = 1, not {}",
            last.id, last.bottom
        ));
    }
    for pair in layers.windows(2) {
        if pair[1].bottom >= pair[0].bottom {
            return Err(format!(
                "layer {:?} (bottom {}) must lie below {:?} (bottom {})",
                pair[1].id, pair[1].bottom, pair[0].id, pair[0].bottom
            ));
        }
    }
    Ok(())
}

// ---- TOML schema -----------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorldGenFile {
    terrain: TerrainDef,
    seabed: SeabedDef,
    #[serde(default)]
    layer: Vec<LayerDef>,
    #[serde(default)]
    ore: Vec<OreDef>,
    #[serde(default)]
    tree: Vec<TreeFileDef>,
    boulder: BoulderDef,
    #[serde(default)]
    biome: Vec<BiomeDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainDef {
    bedrock: String,
    water: String,
    base_height: i32,
    sea_level: i32,
    ring_warp: f32,
    ring_blend: f32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SeabedDef {
    shallow: String,
    default: String,
    gravel: PatchDef,
    clay: PatchDef,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchDef {
    block: String,
    above: Option<f32>,
    below: Option<f32>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerDef {
    id: String,
    bottom: i32,
    fill: String,
    mix: Option<MixDef>,
    caves: CavesDef,
    pool: Option<PoolDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MixDef {
    block: String,
    above: f32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CavesDef {
    blob: f32,
    tunnel: f32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PoolDef {
    block: String,
    below: i32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OreDef {
    block: String,
    threshold: f32,
    #[serde(default)]
    biomes: Vec<String>,
    #[serde(default)]
    layers: Vec<String>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TreeFileDef {
    name: String,
    shape: TreeShape,
    trunk: String,
    leaves: String,
    trunk_height: [i32; 2],
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoulderDef {
    block: String,
    chance_per_mille: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BiomeDef {
    id: String,
    tier: u8,
    start: f32,
    #[serde(default)]
    base_offset: i32,
    amplitude: f32,
    #[serde(default)]
    mesa: f32,
    surface: String,
    subsurface: String,
    strata: Option<String>,
    tree: Option<String>,
    tree_chance_per_mille: Option<f32>,
    #[serde(default)]
    plants: Vec<String>,
    #[serde(default)]
    plant_chance_per_mille: Option<f32>,
    /// `tint = [124, 189, 107]`, the biome's grass colour (`tintindex = 0`).
    #[serde(default)]
    tint: Option<[u8; 3]>,
    /// `foliage_tint = [...]`, the leaf colour (`tintindex = 1`). Defaults to
    /// `tint`.
    #[serde(default)]
    foliage_tint: Option<[u8; 3]>,
    /// `water_tint = [...]`, the water colour (`tintindex = 2`). Defaults to
    /// [`DEFAULT_WATER_TINT`], not to `tint`.
    #[serde(default)]
    water_tint: Option<[u8; 3]>,
}

#[cfg(test)]
mod tests {
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
}
