//! Structure definitions, loaded from `assets/structures.toml`.
//!
//! Strict like worldgen: an unknown block, biome or structure name rejects the
//! whole file, because a structure is part of the terrain every peer
//! regenerates from the seed — silently dropping one would put a shrine in one
//! player's world and not another's.

use std::collections::HashMap;

use super::template::Template;
use crate::domain::core::ident::is_valid_id;
use crate::domain::world::block::BlockRegistry;
use crate::domain::world::generation::{BiomeId, WorldGenConfig};

/// Embedded copy of the shipped structures file.
pub const BUILTIN_STRUCTURES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/structures.toml"
));

/// A cleared, levelled disc around a structure — room to fight a boss in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arena {
    pub radius: i32,
    /// Top block of the levelled ground.
    pub floor: crate::domain::core::BlockId,
    /// What raises low ground up to the floor.
    pub fill: crate::domain::core::BlockId,
    /// How far above the floor trees and hills are cleared away.
    pub clearance: i32,
}

#[derive(Debug)]
pub struct StructureDef {
    pub id: String,
    pub biome: BiomeId,
    /// One candidate per `grid × grid` cell of the world.
    pub grid: i32,
    pub chance_per_mille: u64,
    /// Largest height difference across the footprint a site may have.
    pub max_slope: i32,
    /// Refuse sites whose ground is at or below sea level.
    pub above_sea: bool,
    /// The structure this one points players to (a shrine names its altar).
    pub reveals: Option<usize>,
    /// Make sure one exists within this many blocks of spawn.
    pub guarantee_within: Option<f32>,
    pub arena: Option<Arena>,
    pub template: Template,
    /// Seeds this structure's placement hash, from its id — so adding a
    /// structure never moves another.
    pub salt: u64,
}

impl StructureDef {
    /// How far any block of an instance reaches from its anchor horizontally.
    pub fn reach(&self) -> i32 {
        self.template
            .reach()
            .max(self.arena.map_or(0, |arena| arena.radius))
    }
}

#[derive(Debug, Default)]
pub struct StructureConfig {
    structures: Vec<StructureDef>,
}

impl StructureConfig {
    /// Build from the embedded copy of `assets/structures.toml`.
    pub fn builtin(blocks: &BlockRegistry, worldgen: &WorldGenConfig) -> Self {
        Self::from_toml(BUILTIN_STRUCTURES, blocks, worldgen)
            .expect("embedded structures.toml must parse")
    }

    pub fn from_toml(
        text: &str,
        blocks: &BlockRegistry,
        worldgen: &WorldGenConfig,
    ) -> Result<Self, String> {
        let file: StructureFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let resolve = |name: &str| {
            blocks
                .find(name)
                .ok_or_else(|| format!("unknown block {name:?}"))
        };
        let structures = file
            .structure
            .iter()
            .map(|def| resolve_structure(def, &file.structure, worldgen, &resolve))
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { structures })
    }

    pub fn all(&self) -> &[StructureDef] {
        &self.structures
    }

    pub fn get(&self, index: usize) -> &StructureDef {
        &self.structures[index]
    }

    pub fn find(&self, id: &str) -> Option<usize> {
        self.structures.iter().position(|s| s.id == id)
    }
}

/// FNV-1a of a structure id: a stable per-structure salt.
fn id_salt(id: &str) -> u64 {
    id.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

// ---- TOML schema -----------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StructureFile {
    #[serde(default)]
    structure: Vec<StructureFileDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StructureFileDef {
    id: String,
    biome: String,
    placement: PlacementDef,
    #[serde(default = "default_slope")]
    max_slope: i32,
    #[serde(default = "yes")]
    above_sea: bool,
    reveals: Option<String>,
    guarantee: Option<GuaranteeDef>,
    arena: Option<ArenaDef>,
    template: TemplateDef,
}

fn default_slope() -> i32 {
    4
}

fn yes() -> bool {
    true
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementDef {
    grid: i32,
    chance_per_mille: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GuaranteeDef {
    within: f32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ArenaDef {
    radius: i32,
    floor: String,
    fill: String,
    #[serde(default = "default_clearance")]
    clearance: i32,
}

fn default_clearance() -> i32 {
    16
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateDef {
    palette: HashMap<String, String>,
    #[serde(default)]
    floor: i32,
    foundation: Option<String>,
    layers: Vec<Vec<String>>,
}

/// Resolve one `[[structure]]` against the blocks, the biomes and its sibling
/// structures (`all`, which `reveals` names). Every error names the structure.
fn resolve_structure(
    def: &StructureFileDef,
    all: &[StructureFileDef],
    worldgen: &WorldGenConfig,
    resolve: &dyn Fn(&str) -> Result<crate::domain::core::BlockId, String>,
) -> Result<StructureDef, String> {
    let fail = |e: String| format!("structure {:?}: {e}", def.id);
    if !is_valid_id(&def.id) {
        return Err(fail(
            "id must be lowercase letters, digits and underscores".into(),
        ));
    }
    if all.iter().filter(|d| d.id == def.id).count() > 1 {
        return Err(fail("declared twice".into()));
    }
    let biome = worldgen
        .find_biome(&def.biome)
        .ok_or_else(|| fail(format!("unknown biome {:?}", def.biome)))?;
    let reveals = match &def.reveals {
        Some(target) => Some(
            all.iter()
                .position(|d| d.id == *target)
                .ok_or_else(|| fail(format!("reveals unknown structure {target:?}")))?,
        ),
        None => None,
    };
    let palette = resolve_palette(&def.template.palette, resolve).map_err(&fail)?;
    let foundation = def
        .template
        .foundation
        .as_deref()
        .map(resolve)
        .transpose()
        .map_err(&fail)?;
    let template = Template::parse(
        &def.template.layers,
        &palette,
        def.template.floor,
        foundation,
    )
    .map_err(&fail)?;
    let arena = match &def.arena {
        Some(a) => Some(Arena {
            radius: a.radius,
            floor: resolve(&a.floor).map_err(&fail)?,
            fill: resolve(&a.fill).map_err(&fail)?,
            clearance: a.clearance,
        }),
        None => None,
    };
    let structure = StructureDef {
        id: def.id.clone(),
        biome,
        grid: def.placement.grid,
        chance_per_mille: def.placement.chance_per_mille,
        max_slope: def.max_slope,
        above_sea: def.above_sea,
        reveals,
        guarantee_within: def.guarantee.as_ref().map(|g| g.within),
        arena,
        template,
        salt: id_salt(&def.id),
    };
    // An instance is kept inside its own grid cell, so the cell must leave
    // room for it on every side.
    if structure.grid < 2 * structure.reach() + 8 {
        return Err(fail(format!(
            "grid {} is too small for a structure reaching {} blocks",
            structure.grid,
            structure.reach()
        )));
    }
    Ok(structure)
}

/// A template's palette: one character per block, a space being reserved for
/// "keep whatever is there".
fn resolve_palette(
    palette: &HashMap<String, String>,
    resolve: &dyn Fn(&str) -> Result<crate::domain::core::BlockId, String>,
) -> Result<HashMap<char, crate::domain::core::BlockId>, String> {
    palette
        .iter()
        .map(|(key, name)| {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(' '), None) => Err("a space always means \"keep\"".to_string()),
                (Some(ch), None) => Ok((ch, resolve(name)?)),
                _ => Err(format!("palette key {key:?} must be one character")),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(text: &str) -> Result<StructureConfig, String> {
        let blocks = BlockRegistry::with_builtins();
        let worldgen = WorldGenConfig::builtin(&blocks);
        StructureConfig::from_toml(text, &blocks, &worldgen)
    }

    #[test]
    fn the_builtin_file_declares_the_meadows_shrine_and_altar() {
        let config = load(BUILTIN_STRUCTURES).expect("builtin parses");
        let shrine = config.find("meadows_shrine").expect("shrine");
        let altar = config.find("meadows_altar").expect("altar");
        assert_eq!(config.get(shrine).reveals, Some(altar));
        assert!(config.get(altar).arena.is_some());
        assert!(config.get(altar).guarantee_within.is_some());
    }

    #[test]
    fn unknown_names_reject_the_file() {
        let biome = BUILTIN_STRUCTURES.replacen("biome = \"meadows\"", "biome = \"nope\"", 1);
        assert!(load(&biome).unwrap_err().contains("unknown biome"));
        let reveals = BUILTIN_STRUCTURES.replace("reveals = \"meadows_altar\"", "reveals = \"x\"");
        assert!(load(&reveals).unwrap_err().contains("unknown structure"));
        let block = BUILTIN_STRUCTURES.replacen("\"wayrune\"", "\"nope\"", 1);
        assert!(load(&block).unwrap_err().contains("unknown block"));
    }

    #[test]
    fn a_grid_too_small_for_its_structure_is_rejected() {
        let text = BUILTIN_STRUCTURES.replacen("grid = 512", "grid = 20", 1);
        assert!(load(&text).unwrap_err().contains("too small"));
    }

    #[test]
    fn salts_differ_per_structure() {
        assert_ne!(id_salt("meadows_shrine"), id_salt("meadows_altar"));
    }
}
