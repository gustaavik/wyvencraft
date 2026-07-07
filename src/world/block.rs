//! Block definitions and the [`BlockRegistry`] — the single source of truth for
//! block properties (collision, rendering, textures).
//!
//! Design: *Registry pattern*, loaded from data. Blocks are declared in
//! `assets/blocks.toml` (with an embedded fallback copy compiled in); the file
//! order defines the numeric [`BlockId`]s and save files reference blocks by
//! name. Behavior is expressed as components on the block ([`Drops`],
//! [`FluidInfo`]) rather than hard-coded `match` arms on identity.

use crate::core::{BlockId, Direction};
use crate::render::TileRegistry;

/// Embedded copy of the shipped block definitions, used when
/// `assets/blocks.toml` is missing or invalid (the assets dir is CWD-relative).
pub const BUILTIN_BLOCKS: &str = include_str!("../../assets/blocks.toml");

/// How a block participates in meshing/rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RenderType {
    /// Not drawn at all (air).
    Invisible,
    /// Fully opaque cube; hides neighbouring faces.
    Opaque,
    /// See-through cube (glass/water); does not hide neighbours of other
    /// block types and is drawn in the transparent pass.
    Transparent,
    /// Alpha-tested cube (leaves): texture is either fully opaque or fully
    /// clear per texel, so it draws in the opaque pass with depth writes (the
    /// shader discards clear texels). Avoids the blend-order artifacts of the
    /// unsorted transparent pass. Does not hide neighbours of other types.
    Cutout,
}

/// What a block is made of — selects which tool mines it fastest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockMaterial {
    Stone,
    Dirt,
    Sand,
    Wood,
    Plant,
    Glass,
    /// No preferred tool (bedrock, misc).
    Other,
}

/// Per-face atlas tile indices, ordered by [`Direction`] (`-X,+X,-Y,+Y,-Z,+Z`).
#[derive(Debug, Clone, Copy)]
pub struct FaceTextures(pub [u32; 6]);

impl FaceTextures {
    /// Same tile on every face.
    pub const fn uniform(tile: u32) -> Self {
        Self([tile; 6])
    }

    /// Distinct top / bottom / side tiles (the common grass/log case).
    pub const fn column(top: u32, bottom: u32, side: u32) -> Self {
        // order: -X,+X,-Y,+Y,-Z,+Z  =>  side,side,bottom,top,side,side
        Self([side, side, bottom, top, side, side])
    }

    #[inline]
    pub fn tile(&self, dir: Direction) -> u32 {
        self.0[dir as usize]
    }
}

/// What breaking a block yields (the `drops` field in `assets/blocks.toml`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drops {
    /// The block's own item — the default.
    SelfItem,
    /// Nothing.
    None,
    /// The block's own item, but only when the held tool's kind matches
    /// (e.g. leaves require shears).
    SelfWithTool { kind: String },
    /// A different item, by name (resolved against the item registry at use).
    Item { name: String, count: u8 },
}

/// Fluid behavior component: marks a block as part of a level-based fluid.
///
/// A fluid is declared on its source block (`[block.fluid]` with
/// `flow_levels = N`); the loader then auto-registers one flowing block per
/// level `1..=N`, and the source carries level `N + 1`. The simulation in
/// [`crate::world::fluid`] spreads and recedes between these blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidInfo {
    /// Which fluid this block belongs to (ordinal among fluid sources).
    pub group: u16,
    /// This block's level: `max_level` for the source, decaying to 1.
    pub level: u8,
    /// The source's level (`flow_levels + 1`); the scale for surface heights.
    pub max_level: u8,
}

impl FluidInfo {
    /// Sources are permanent until replaced; flowing blocks re-evaluate.
    #[inline]
    pub fn is_source(&self) -> bool {
        self.level == self.max_level
    }
}

/// Static description of a block type.
#[derive(Debug, Clone)]
pub struct Block {
    pub name: String,
    pub render: RenderType,
    /// Whether entities collide with this block.
    pub solid: bool,
    pub textures: FaceTextures,
    /// Per-face "texture is animated" bits, indexed by [`Direction`] like
    /// [`FaceTextures`]. Precomputed at load so the mesher's hot loop can set
    /// the shader's animation flag without a registry lookup.
    pub animated_faces: u8,
    /// Relative mining time; `f32::INFINITY` means unbreakable (e.g. bedrock).
    pub hardness: f32,
    /// What the block is made of, for tool matching.
    pub material: BlockMaterial,
    /// What breaking it yields.
    pub drops: Drops,
    /// Set when the block is part of a fluid (source or flowing).
    pub fluid: Option<FluidInfo>,
}

impl Block {
    /// Whether the face towards `dir` shows an animated texture.
    #[inline]
    pub fn face_animated(&self, dir: Direction) -> bool {
        self.animated_faces & (1 << dir as usize) != 0
    }
}

impl Block {
    #[inline]
    pub fn is_opaque(&self) -> bool {
        matches!(self.render, RenderType::Opaque)
    }

    #[inline]
    pub fn is_visible(&self) -> bool {
        !matches!(self.render, RenderType::Invisible)
    }

    #[inline]
    pub fn is_transparent(&self) -> bool {
        matches!(self.render, RenderType::Transparent)
    }

    #[inline]
    pub fn is_cutout(&self) -> bool {
        matches!(self.render, RenderType::Cutout)
    }

    /// Whether the block can ever be mined (finite hardness).
    #[inline]
    pub fn is_breakable(&self) -> bool {
        self.hardness.is_finite()
    }
}

/// Ids of the *builtin* block set, in its declared order — a convenience for
/// tests. Gameplay code must never use these: content files may reorder or
/// extend the set (ids are session-local; saves and cross-file references
/// resolve by name).
pub mod blocks {
    use super::BlockId;
    pub const AIR: BlockId = BlockId(0);
    pub const STONE: BlockId = BlockId(1);
    pub const DIRT: BlockId = BlockId(2);
    pub const GRASS: BlockId = BlockId(3);
    pub const SAND: BlockId = BlockId(4);
    pub const WATER: BlockId = BlockId(5);
    pub const WOOD: BlockId = BlockId(6);
    pub const LEAVES: BlockId = BlockId(7);
    pub const GLASS: BlockId = BlockId(8);
    pub const BEDROCK: BlockId = BlockId(9);
    pub const SNOW: BlockId = BlockId(10);
    pub const GRAVEL: BlockId = BlockId(11);
    pub const CLAY: BlockId = BlockId(12);
    pub const COAL_ORE: BlockId = BlockId(13);
    pub const IRON_ORE: BlockId = BlockId(14);
    pub const GOLD_ORE: BlockId = BlockId(15);
    pub const DIAMOND_ORE: BlockId = BlockId(16);
    /// Flowing water levels 1 (shallowest) through 7: auto-registered after
    /// all declared blocks; the source block [`WATER`] is level 8.
    pub const WATER_FLOW_1: BlockId = BlockId(17);
    pub const WATER_FLOW_7: BlockId = BlockId(23);
}

// ---- TOML schema -----------------------------------------------------------

#[derive(serde::Deserialize)]
struct BlockFile {
    #[serde(default)]
    block: Vec<BlockDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockDef {
    name: String,
    render: RenderType,
    solid: bool,
    hardness: f32,
    material: BlockMaterial,
    textures: TexturesDef,
    drops: Option<DropsDef>,
    fluid: Option<FluidDef>,
}

/// `textures = "stone"`, the top/bottom/side shorthand, or all six faces.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum TexturesDef {
    Uniform(String),
    Column {
        top: String,
        bottom: String,
        side: String,
    },
    Faces {
        neg_x: String,
        pos_x: String,
        neg_y: String,
        pos_y: String,
        neg_z: String,
        pos_z: String,
    },
}

/// `drops = "self" | "none" | { requires_tool = "..." } | { item = "...", count = N }`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum DropsDef {
    Keyword(String),
    RequiresTool {
        requires_tool: String,
    },
    OtherItem {
        item: String,
        #[serde(default = "default_drop_count")]
        count: u8,
    },
}

fn default_drop_count() -> u8 {
    1
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FluidDef {
    flow_levels: u8,
}

impl TexturesDef {
    /// Resolve texture names to atlas tiles via the tile registry (which
    /// warns and substitutes the missing marker for unknown names). Returns
    /// the per-face tiles plus the per-face animation bitmask.
    fn resolve(&self, tiles: &mut TileRegistry) -> (FaceTextures, u8) {
        let names: [&str; 6] = match self {
            Self::Uniform(name) => [name; 6],
            // order: -X,+X,-Y,+Y,-Z,+Z  =>  side,side,bottom,top,side,side
            Self::Column { top, bottom, side } => [side, side, bottom, top, side, side],
            Self::Faces {
                neg_x,
                pos_x,
                neg_y,
                pos_y,
                neg_z,
                pos_z,
            } => [neg_x, pos_x, neg_y, pos_y, neg_z, pos_z],
        };
        let mut faces = [0u32; 6];
        let mut animated = 0u8;
        for (i, name) in names.iter().enumerate() {
            let entry = tiles.resolve(name);
            faces[i] = entry.tile;
            if entry.is_animated() {
                animated |= 1 << i;
            }
        }
        (FaceTextures(faces), animated)
    }
}

impl DropsDef {
    fn resolve(&self) -> Result<Drops, String> {
        match self {
            Self::Keyword(word) => match word.as_str() {
                "self" => Ok(Drops::SelfItem),
                "none" => Ok(Drops::None),
                other => Err(format!("unknown drops keyword {other:?}")),
            },
            Self::RequiresTool { requires_tool } => Ok(Drops::SelfWithTool {
                kind: requires_tool.clone(),
            }),
            Self::OtherItem { item, count } => Ok(Drops::Item {
                name: item.clone(),
                count: *count,
            }),
        }
    }
}

/// Lookup table of all registered block types.
#[derive(Debug)]
pub struct BlockRegistry {
    blocks: Vec<Block>,
    /// Flowing blocks per fluid group, indexed `[group][level - 1]`.
    fluid_flow: Vec<Vec<BlockId>>,
}

impl BlockRegistry {
    /// Build the registry from the embedded copy of `assets/blocks.toml`,
    /// with a private tile registry (tests and fallbacks; the app path shares
    /// one via [`crate::content::GameContent`]). Infallible: the shipped file
    /// is validated by the golden tests.
    pub fn with_builtins() -> Self {
        let mut tiles = TileRegistry::with_engine_tiles();
        Self::from_toml(BUILTIN_BLOCKS, &mut tiles).expect("embedded blocks.toml must parse")
    }

    /// Parse a blocks file. Declared order defines the numeric [`BlockId`]s:
    /// "air" is registered first (id 0, an engine invariant), then every
    /// `[[block]]` entry, then the auto-generated flowing blocks of each fluid
    /// (so declared blocks keep their ids regardless of fluids).
    ///
    /// Structural errors (bad TOML, duplicate/reserved names) fail the whole
    /// file — the caller falls back to [`BlockRegistry::with_builtins`].
    /// Unknown texture names only degrade that block (tile 0 + a warning).
    pub fn from_toml(text: &str, tiles: &mut TileRegistry) -> Result<Self, String> {
        let file: BlockFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.block.is_empty() {
            return Err("no [[block]] entries".into());
        }

        let mut reg = Self {
            blocks: Vec::new(),
            fluid_flow: Vec::new(),
        };
        reg.register(Block {
            name: "air".into(),
            render: RenderType::Invisible,
            solid: false,
            textures: FaceTextures::uniform(0),
            animated_faces: 0,
            hardness: 0.0,
            material: BlockMaterial::Other,
            drops: Drops::None,
            fluid: None,
        });

        // Fluid sources, in declaration order: (source id, flow_levels).
        let mut fluids: Vec<(BlockId, u8)> = Vec::new();
        for def in file.block {
            if def.name == "air" {
                return Err("\"air\" is built in and may not be declared".into());
            }
            if reg.find(&def.name).is_some() {
                return Err(format!("duplicate block {:?}", def.name));
            }
            let drops = match &def.drops {
                Some(d) => d
                    .resolve()
                    .map_err(|e| format!("block {:?}: {e}", def.name))?,
                None => Drops::SelfItem,
            };
            let fluid = match &def.fluid {
                Some(f) => {
                    if !(1..=15).contains(&f.flow_levels) {
                        return Err(format!("block {:?}: flow_levels must be 1..=15", def.name));
                    }
                    Some(FluidInfo {
                        group: fluids.len() as u16,
                        level: f.flow_levels + 1,
                        max_level: f.flow_levels + 1,
                    })
                }
                None => None,
            };
            let (textures, animated_faces) = def.textures.resolve(tiles);
            let id = reg.register(Block {
                name: def.name,
                render: def.render,
                solid: def.solid,
                textures,
                animated_faces,
                hardness: def.hardness,
                material: def.material,
                drops,
                fluid,
            });
            if let Some(f) = &def.fluid {
                fluids.push((id, f.flow_levels));
            }
        }

        // Auto-register the flowing blocks: same look and physics as their
        // source, one per level, named "<source> flow <level>" (these names
        // are the save format — see the fluid module docs).
        for (group, (source_id, levels)) in fluids.into_iter().enumerate() {
            let source = reg.get(source_id).clone();
            let flow: Vec<BlockId> = (1..=levels)
                .map(|level| {
                    reg.register(Block {
                        name: format!("{} flow {}", source.name, level),
                        render: source.render,
                        solid: source.solid,
                        textures: source.textures,
                        animated_faces: source.animated_faces,
                        hardness: source.hardness,
                        material: source.material,
                        drops: Drops::None,
                        fluid: Some(FluidInfo {
                            group: group as u16,
                            level,
                            max_level: levels + 1,
                        }),
                    })
                })
                .collect();
            reg.fluid_flow.push(flow);
        }
        Ok(reg)
    }

    /// The fluid component of `id`, if any.
    #[inline]
    pub fn fluid(&self, id: BlockId) -> Option<FluidInfo> {
        self.get(id).fluid
    }

    /// Whether `id` is part of any fluid (source or flowing).
    #[inline]
    pub fn is_fluid(&self, id: BlockId) -> bool {
        self.fluid(id).is_some()
    }

    /// Flowing (non-source) fluid — simulation state with no item form.
    #[inline]
    pub fn is_flowing_fluid(&self, id: BlockId) -> bool {
        self.fluid(id).is_some_and(|f| !f.is_source())
    }

    /// The flowing block of fluid `group` at `level` (clamped to the group's
    /// valid range).
    #[inline]
    pub fn flowing(&self, group: u16, level: u8) -> BlockId {
        let flow = &self.fluid_flow[group as usize];
        flow[usize::from(level.clamp(1, flow.len() as u8)) - 1]
    }

    /// Append a new block type, returning its assigned id.
    pub fn register(&mut self, block: Block) -> BlockId {
        let id = BlockId(self.blocks.len() as u16);
        self.blocks.push(block);
        id
    }

    #[inline]
    pub fn get(&self, id: BlockId) -> &Block {
        // Unknown ids fall back to air to stay panic-free on bad network data.
        self.blocks.get(id.0 as usize).unwrap_or(&self.blocks[0])
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (BlockId, &Block)> {
        self.blocks
            .iter()
            .enumerate()
            .map(|(i, b)| (BlockId(i as u16), b))
    }

    /// Look up a block by its registered name (save files reference blocks by
    /// name because numeric ids shift when the registry changes across builds).
    pub fn find(&self, name: &str) -> Option<BlockId> {
        self.blocks
            .iter()
            .position(|b| b.name == name)
            .map(|i| BlockId(i as u16))
    }
}

impl Default for BlockRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden snapshot of the shipped block set. The data-driven loader must
    /// reproduce this exactly: names are the save format (matched verbatim by
    /// [`BlockRegistry::find`]) and registration order defines the numeric ids
    /// used in chunk storage and on the wire.
    #[test]
    fn builtin_blocks_golden() {
        use BlockMaterial as M;
        use RenderType as R;
        const INF: f32 = f32::INFINITY;
        let expected: [(&str, R, bool, f32, M); 24] = [
            ("air", R::Invisible, false, 0.0, M::Other),
            ("stone", R::Opaque, true, 1.5, M::Stone),
            ("dirt", R::Opaque, true, 0.5, M::Dirt),
            ("grass", R::Opaque, true, 0.6, M::Dirt),
            ("sand", R::Opaque, true, 0.5, M::Sand),
            ("water", R::Transparent, false, INF, M::Other),
            ("wood", R::Opaque, true, 2.0, M::Wood),
            ("leaves", R::Cutout, true, 0.2, M::Plant),
            ("glass", R::Transparent, true, 0.3, M::Glass),
            ("bedrock", R::Opaque, true, INF, M::Other),
            ("snow", R::Opaque, true, 0.2, M::Dirt),
            ("gravel", R::Opaque, true, 0.6, M::Dirt),
            ("clay", R::Opaque, true, 0.6, M::Dirt),
            ("coal ore", R::Opaque, true, 3.0, M::Stone),
            ("iron ore", R::Opaque, true, 3.0, M::Stone),
            ("gold ore", R::Opaque, true, 3.0, M::Stone),
            ("diamond ore", R::Opaque, true, 3.0, M::Stone),
            ("water flow 1", R::Transparent, false, INF, M::Other),
            ("water flow 2", R::Transparent, false, INF, M::Other),
            ("water flow 3", R::Transparent, false, INF, M::Other),
            ("water flow 4", R::Transparent, false, INF, M::Other),
            ("water flow 5", R::Transparent, false, INF, M::Other),
            ("water flow 6", R::Transparent, false, INF, M::Other),
            ("water flow 7", R::Transparent, false, INF, M::Other),
        ];
        let reg = BlockRegistry::with_builtins();
        assert_eq!(reg.len(), expected.len(), "block count changed");
        for (i, &(name, render, solid, hardness, material)) in expected.iter().enumerate() {
            let block = reg.get(BlockId(i as u16));
            assert_eq!(block.name, name, "block {i}: name");
            assert_eq!(block.render, render, "{name}: render");
            assert_eq!(block.solid, solid, "{name}: solid");
            assert_eq!(block.hardness, hardness, "{name}: hardness");
            assert_eq!(block.material, material, "{name}: material");
        }
    }

    #[test]
    fn water_levels_match_registry_order() {
        let reg = BlockRegistry::with_builtins();
        assert_eq!(reg.find("water"), Some(blocks::WATER));
        assert_eq!(reg.find("water flow 1"), Some(blocks::WATER_FLOW_1));
        assert_eq!(reg.find("water flow 7"), Some(blocks::WATER_FLOW_7));

        let source = reg.fluid(blocks::WATER).expect("water is a fluid");
        assert!(source.is_source());
        assert_eq!((source.group, source.level, source.max_level), (0, 8, 8));
        for level in 1..=7u8 {
            let id = reg.flowing(0, level);
            let f = reg.fluid(id).expect("flow block is a fluid");
            assert_eq!((f.group, f.level, f.max_level), (0, level, 8));
            assert!(reg.is_fluid(id) && reg.is_flowing_fluid(id));
        }
        assert!(reg.is_fluid(blocks::WATER) && !reg.is_flowing_fluid(blocks::WATER));
        assert_eq!(reg.fluid(blocks::STONE), None);
        assert_eq!(reg.fluid(BlockId::AIR), None);
    }

    /// The behavior components declared in blocks.toml parse into the typed
    /// component fields.
    #[test]
    fn blocks_toml_components_parse() {
        let reg = BlockRegistry::with_builtins();
        assert_eq!(
            reg.get(blocks::LEAVES).drops,
            Drops::SelfWithTool {
                kind: "shears".into()
            }
        );
        assert_eq!(reg.get(blocks::STONE).drops, Drops::SelfItem);
        assert_eq!(reg.get(blocks::WATER_FLOW_1).drops, Drops::None);
        // Flow blocks inherit the source's look and physics.
        let (water, flow) = (reg.get(blocks::WATER), reg.get(blocks::WATER_FLOW_1));
        assert_eq!(flow.render, water.render);
        assert_eq!(flow.solid, water.solid);
        assert_eq!(flow.hardness, water.hardness);
    }

    /// Parse a blocks file with a throwaway tile registry.
    fn parse(text: &str) -> Result<BlockRegistry, String> {
        let mut tiles = TileRegistry::with_engine_tiles();
        BlockRegistry::from_toml(text, &mut tiles)
    }

    /// Structural errors reject the whole file (the loader then falls back to
    /// the builtin copy); reordering trips the well-known-id validation.
    #[test]
    fn invalid_block_files_are_rejected() {
        assert!(parse("not toml [").is_err());
        assert!(parse("").is_err());
        let air = r#"
            [[block]]
            name = "air"
            render = "opaque"
            solid = true
            hardness = 1.0
            material = "other"
            textures = "stone"
        "#;
        assert!(parse(air).is_err());
        let dup = r#"
            [[block]]
            name = "stone"
            render = "opaque"
            solid = true
            hardness = 1.0
            material = "stone"
            textures = "stone"

            [[block]]
            name = "stone"
            render = "opaque"
            solid = true
            hardness = 1.0
            material = "stone"
            textures = "stone"
        "#;
        assert!(parse(dup).is_err());
        // A minimal but valid file parses fine — ids are session-local, so
        // files are free to define any block set.
        let minimal = parse(
            r#"
            [[block]]
            name = "dirt"
            render = "opaque"
            solid = true
            hardness = 0.5
            material = "dirt"
            textures = "dirt"
        "#,
        )
        .expect("valid file");
        assert_eq!(minimal.len(), 2, "air + dirt");
    }
}
