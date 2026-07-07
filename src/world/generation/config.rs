//! World-generation configuration, loaded from `assets/worldgen.toml`.
//!
//! The *numbers and block choices* of generation are data; the noise fields,
//! climate model, and feature shapes stay code (see the sibling modules).
//! Resolution is strict: an unknown block name rejects the whole file, because
//! worldgen quietly placing the wrong block would corrupt every world built
//! with it. Determinism note: the shipped file reproduces the hardcoded
//! generator byte-for-byte (pinned by the `worldgen_golden_hashes` test).

use super::biome::Biome;
use crate::core::BlockId;
use crate::world::block::BlockRegistry;

/// Embedded copy of the shipped worldgen configuration.
pub const BUILTIN_WORLDGEN: &str = include_str!("../../../assets/worldgen.toml");

/// An ore vein: the block it places, the vertical band it appears in, and how
/// rare it is (higher threshold = rarer). The vein's index in the table picks
/// its noise field, so table order is part of a world's identity.
#[derive(Debug)]
pub struct OreVein {
    pub block: BlockId,
    pub min_y: i32,
    pub max_y: i32,
    pub threshold: f32,
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

/// What one biome places on and under the surface.
#[derive(Debug)]
pub struct BiomeGen {
    pub surface: BlockId,
    pub subsurface: BlockId,
    /// Index into [`WorldGenConfig::trees`] + per-mille candidate chance.
    pub tree: Option<(usize, f32)>,
}

#[derive(Debug)]
pub struct WorldGenConfig {
    pub bedrock: BlockId,
    pub stone: BlockId,
    pub water: BlockId,
    pub sea_level: i32,
    pub seabed: SeabedConfig,
    pub ores: Vec<OreVein>,
    pub trees: Vec<TreeDef>,
    pub boulder: BoulderConfig,
    /// Indexed via [`WorldGenConfig::biome`].
    biomes: [BiomeGen; 3],
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

        let biome = |def: &BiomeDef| -> Result<BiomeGen, String> {
            let tree = match &def.tree {
                Some(name) => {
                    let index = file
                        .tree
                        .iter()
                        .position(|t| t.name == *name)
                        .ok_or_else(|| format!("unknown tree {name:?}"))?;
                    let chance = def.tree_chance_per_mille.unwrap_or(0.0);
                    Some((index, chance))
                }
                None => None,
            };
            Ok(BiomeGen {
                surface: resolve(&def.surface)?,
                subsurface: resolve(&def.subsurface)?,
                tree,
            })
        };

        Ok(Self {
            bedrock: resolve(&file.terrain.bedrock)?,
            stone: resolve(&file.terrain.stone)?,
            water: resolve(&file.terrain.water)?,
            sea_level: file.terrain.sea_level,
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
            ores: file
                .ore
                .iter()
                .map(|o| {
                    Ok(OreVein {
                        block: resolve(&o.block)?,
                        min_y: o.min_y,
                        max_y: o.max_y,
                        threshold: o.threshold,
                    })
                })
                .collect::<Result<_, String>>()?,
            trees,
            boulder: BoulderConfig {
                block: resolve(&file.boulder.block)?,
                chance_per_mille: file.boulder.chance_per_mille,
            },
            biomes: [
                biome(&file.biome.snowy)?,
                biome(&file.biome.plains)?,
                biome(&file.biome.desert)?,
            ],
        })
    }

    /// The generation choices for `biome`.
    pub fn biome(&self, biome: Biome) -> &BiomeGen {
        let index = match biome {
            Biome::Snowy => 0,
            Biome::Plains => 1,
            Biome::Desert => 2,
        };
        &self.biomes[index]
    }
}

// ---- TOML schema -----------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WorldGenFile {
    terrain: TerrainDef,
    seabed: SeabedDef,
    #[serde(default)]
    ore: Vec<OreDef>,
    #[serde(default)]
    tree: Vec<TreeFileDef>,
    boulder: BoulderDef,
    biome: BiomesDef,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainDef {
    bedrock: String,
    stone: String,
    water: String,
    sea_level: i32,
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
struct OreDef {
    block: String,
    min_y: i32,
    max_y: i32,
    threshold: f32,
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
struct BiomesDef {
    plains: BiomeDef,
    snowy: BiomeDef,
    desert: BiomeDef,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BiomeDef {
    surface: String,
    subsurface: String,
    tree: Option<String>,
    tree_chance_per_mille: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::blocks;

    /// Golden snapshot: the shipped worldgen.toml carries exactly the values
    /// the hardcoded generator used.
    #[test]
    fn builtin_worldgen_golden() {
        let registry = BlockRegistry::with_builtins();
        let config = WorldGenConfig::builtin(&registry);
        assert_eq!(config.bedrock, blocks::BEDROCK);
        assert_eq!(config.stone, blocks::STONE);
        assert_eq!(config.water, blocks::WATER);
        assert_eq!(config.sea_level, 62);

        assert_eq!(config.seabed.shallow, blocks::SAND);
        assert_eq!(config.seabed.default_block, blocks::SAND);
        assert_eq!(
            (config.seabed.gravel, config.seabed.gravel_above),
            (blocks::GRAVEL, 0.30)
        );
        assert_eq!(
            (config.seabed.clay, config.seabed.clay_below),
            (blocks::CLAY, -0.35)
        );

        let ores: Vec<_> = config
            .ores
            .iter()
            .map(|o| (o.block, o.min_y, o.max_y, o.threshold))
            .collect();
        assert_eq!(
            ores,
            vec![
                (blocks::DIAMOND_ORE, 1, 16, 0.68),
                (blocks::GOLD_ORE, 4, 32, 0.66),
                (blocks::IRON_ORE, 8, 72, 0.60),
                (blocks::COAL_ORE, 16, 108, 0.55),
            ]
        );

        assert_eq!(config.trees.len(), 2);
        let oak = &config.trees[0];
        assert_eq!(oak.shape, TreeShape::Oak);
        assert_eq!((oak.trunk, oak.leaves), (blocks::WOOD, blocks::LEAVES));
        assert_eq!(oak.trunk_height, (4, 6));
        let spruce = &config.trees[1];
        assert_eq!(spruce.shape, TreeShape::Spruce);
        assert_eq!(spruce.trunk_height, (6, 8));

        assert_eq!(config.boulder.block, blocks::STONE);
        assert_eq!(config.boulder.chance_per_mille, 300);

        let plains = config.biome(Biome::Plains);
        assert_eq!(
            (plains.surface, plains.subsurface),
            (blocks::GRASS, blocks::DIRT)
        );
        assert_eq!(plains.tree, Some((0, 550.0)));
        let snowy = config.biome(Biome::Snowy);
        assert_eq!(
            (snowy.surface, snowy.subsurface),
            (blocks::SNOW, blocks::DIRT)
        );
        assert_eq!(snowy.tree, Some((1, 400.0)));
        let desert = config.biome(Biome::Desert);
        assert_eq!(
            (desert.surface, desert.subsurface),
            (blocks::SAND, blocks::SAND)
        );
        assert_eq!(desert.tree, None);
    }

    /// Unknown block names must reject the whole file.
    #[test]
    fn unknown_blocks_are_rejected() {
        let registry = BlockRegistry::with_builtins();
        let text = BUILTIN_WORLDGEN.replace("bedrock = \"bedrock\"", "bedrock = \"nope\"");
        assert!(WorldGenConfig::from_toml(&text, &registry).is_err());
    }
}
