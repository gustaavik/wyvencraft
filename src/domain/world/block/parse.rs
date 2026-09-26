//! Reading `blocks.toml`: the file's schema, and turning it into a
//! [`BlockRegistry`] plus the appearance fields reported out of band in
//! [`BlockVisuals`].
//!
//! Structural errors (bad TOML, malformed/duplicate/reserved ids, impossible
//! fluid animation) fail the whole file — the caller falls back to
//! [`BlockRegistry::with_builtins`]. Unknown texture names only degrade that
//! block, once the content loader fails to find art for them.

use super::*;

#[derive(serde::Deserialize)]
struct BlockFile {
    #[serde(default)]
    block: Vec<BlockDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockDef {
    id: String,
    /// Overrides the label derived from `id`, for the ids the rule gets wrong.
    display_name: Option<String>,
    render: RenderType,
    solid: bool,
    hardness: f32,
    /// `[block.harvest]` — which tool this block wants.
    harvest: Option<HarvestDef>,
    /// Optional for a block whose geometry brings its own texture and never
    /// samples the atlas — either a `[block.model]` or a `block_model`.
    textures: Option<TexturesDef>,
    drops: Option<DropsDef>,
    fluid: Option<FluidDef>,
    model: Option<ModelSpec>,
    /// `block_model = "assets/models/blocks/dirt.json"` — a Blockbench *Java
    /// Block/Item* export, the way all blocks are authored going forward. Unlike
    /// `[block.model]` it needs no placement: the model is already in cell
    /// coordinates, carries several textures, and declares its own cull faces.
    ///
    /// Spelled differently from `[block.model]` only because both exist during
    /// the migration; it takes that name once the `.bbmodel` path is gone.
    block_model: Option<String>,
    /// Only meaningful alongside `[block.model]`; see [`BlockModel::random_yaw`].
    #[serde(default)]
    random_yaw: bool,
    /// `[block.interact]` — what right-clicking it does.
    interact: Option<Interaction>,
    /// `station = "workbench"` — the crafting station this block is.
    station: Option<String>,
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

/// `[block.harvest]` with `tool = "pickaxe"` or `tool = ["shears", "sword"]`,
/// plus an optional `required = true`.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HarvestDef {
    tool: ToolsDef,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    tier: u8,
}

/// `tool = "pickaxe"`, or `tool = ["shears", "sword"]` when more than one shape
/// is good at it.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum ToolsDef {
    One(String),
    Many(Vec<String>),
}

impl HarvestDef {
    fn resolve(self, block: &str) -> Result<Harvest, String> {
        let tools = match self.tool {
            ToolsDef::One(kind) => vec![kind],
            ToolsDef::Many(kinds) => kinds,
        };
        // An empty list would silently mean "no tool is good at this", which is
        // what leaving the whole table out already says — and with
        // `required = true` it would mean "nothing can ever harvest this",
        // which is certainly not what anyone typed on purpose.
        if tools.is_empty() {
            return Err(format!(
                "block {block:?}: [block.harvest] names no tool; omit the table instead"
            ));
        }
        for kind in &tools {
            if !is_valid_id(kind) {
                return Err(format!(
                    "block {block:?}: tool kind {kind:?} must be lowercase letters, digits \
                     and underscores"
                ));
            }
        }
        Ok(Harvest {
            tools,
            required: self.required,
            tier: self.tier,
        })
    }
}

/// `drops = "self" | "none" | { item = "...", count = N }`.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum DropsDef {
    Keyword(String),
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
    texture: Option<FluidTextureDef>,
}

/// `[block.fluid.texture]`; see [`FluidTextureSpec`].
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct FluidTextureDef {
    path: String,
    frames: u8,
    #[serde(default = "default_fluid_fps")]
    fps: u8,
    tint: Option<u8>,
    /// `opacity = 0.85`; see [`FluidTextureSpec::opacity`].
    opacity: Option<f32>,
}

/// Slow enough to read as a swell rather than a flicker. `fps * 3600` must stay
/// a multiple of the frame count, or the shader's hourly time wrap jumps.
fn default_fluid_fps() -> u8 {
    8
}

impl TexturesDef {
    /// The six texture names in [`Direction`] order (`-X,+X,-Y,+Y,-Z,+Z`).
    ///
    /// Names, not tiles: which atlas slot each one lands in depends on the art
    /// loaded alongside it, and that is a question for `content`, not for the
    /// block table.
    fn names(&self) -> [String; 6] {
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
        names.map(str::to_string)
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
            Self::OtherItem { item, count } => Ok(Drops::Item {
                id: item.clone(),
                count: *count,
            }),
        }
    }
}

/// A declared fluid source, remembered until its flowing blocks are
/// registered after every declared block: (source id, flow levels, texture).
type FluidSource = (BlockId, u8, Option<FluidTextureSpec>);

impl BlockRegistry {
    /// Parse a blocks file. Declared order defines the numeric [`BlockId`]s:
    /// "air" is registered first (id 0, an engine invariant), then every
    /// `[[block]]` entry, then the auto-generated flowing blocks of each fluid
    /// (so declared blocks keep their ids regardless of fluids).
    pub fn from_toml(text: &str) -> Result<Self, String> {
        Self::from_toml_with_models(text, &mut BlockVisuals::default())
    }

    /// Like [`BlockRegistry::from_toml`], but also reports each block's model
    /// assignment and display name in [`BlockVisuals`], indexed by [`BlockId`].
    ///
    /// Model assignment rides out of band because it cannot be resolved yet —
    /// blocks are parsed before the model registry exists — and because it must
    /// stay off [`Block`], which feeds `content_hash`.
    pub fn from_toml_with_models(text: &str, visuals: &mut BlockVisuals) -> Result<Self, String> {
        let file: BlockFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.block.is_empty() {
            return Err("no [[block]] entries".into());
        }
        *visuals = BlockVisuals::default();
        let mut reg = Self {
            blocks: Vec::new(),
            fluid_flow: Vec::new(),
        };
        reg.register_air(visuals);
        let mut fluids: Vec<FluidSource> = Vec::new();
        for def in file.block {
            reg.register_declared(def, visuals, &mut fluids)?;
        }
        reg.register_flowing(fluids, visuals);
        // The auto-registered flowing blocks never carry a model — they do
        // carry their source's fluid texture — but every vector is indexed by
        // `BlockId` and so must cover every block.
        let len = reg.len();
        visuals.models.resize(len, None);
        visuals.json.resize(len, None);
        visuals.fluids.resize(len, None);
        visuals.display_names.resize(len, None);
        Ok(reg)
    }

    /// Air: id 0, the engine's invariant, never declared by the file.
    fn register_air(&mut self, visuals: &mut BlockVisuals) {
        visuals.textures.push(None);
        visuals.display_names.push(None);
        self.register(Block {
            id: "air".into(),
            render: RenderType::Invisible,
            solid: false,
            hardness: 0.0,
            harvest: None,
            drops: Drops::None,
            fluid: None,
            interact: None,
            station: None,
        });
        visuals.models.push(None);
        visuals.json.push(None);
        visuals.fluids.push(None);
    }

    /// Validate and register one `[[block]]`, reporting its appearance fields
    /// into `visuals` and, if it is a fluid source, remembering it in `fluids`.
    fn register_declared(
        &mut self,
        mut def: BlockDef,
        visuals: &mut BlockVisuals,
        fluids: &mut Vec<FluidSource>,
    ) -> Result<(), String> {
        self.check_ids(&def)?;
        let drops = match &def.drops {
            Some(d) => d
                .resolve()
                .map_err(|e| format!("block {:?}: {e}", def.id))?,
            None => Drops::SelfItem,
        };
        let harvest = def.harvest.take().map(|h| h.resolve(&def.id)).transpose()?;
        // The level range is checked before the animation strip, as it always
        // was, so a file wrong in both ways reports the same error.
        if let Some(f) = &def.fluid
            && !(1..=15).contains(&f.flow_levels)
        {
            return Err(format!("block {:?}: flow_levels must be 1..=15", def.id));
        }
        let fluid_texture = match def.fluid.as_ref().and_then(|f| f.texture.as_ref()) {
            Some(t) => Some(fluid_texture_spec(&def.id, t)?),
            None => None,
        };
        let fluid = def.fluid.as_ref().map(|f| FluidInfo {
            group: fluids.len() as u16,
            level: f.flow_levels + 1,
            max_level: f.flow_levels + 1,
        });
        if def.model.is_some() && def.block_model.is_some() {
            return Err(format!(
                "block {:?}: declares both `block_model` and a `[block.model]`",
                def.id
            ));
        }
        visuals
            .textures
            .push(face_textures(&def, fluid_texture.is_some())?);
        let random_yaw = def.random_yaw;
        let model = def.model.map(|spec| BlockModelSpec { spec, random_yaw });
        visuals.json.push(
            def.block_model
                .map(|path| BlockJsonSpec { path, random_yaw }),
        );
        visuals
            .fluids
            .push(fluid_texture.clone().map(|spec| FluidVisual {
                spec,
                flowing: false,
            }));
        visuals.display_names.push(def.display_name);
        let id = self.register(Block {
            id: def.id,
            render: def.render,
            solid: def.solid,
            hardness: def.hardness,
            harvest,
            drops,
            fluid,
            interact: def.interact,
            station: def.station,
        });
        visuals.models.push(model);
        if let Some(f) = &def.fluid {
            fluids.push((id, f.flow_levels, fluid_texture));
        }
        Ok(())
    }

    /// The id rules. An id is a reference key in five other files, so a
    /// malformed one rejects the whole table rather than just its own entry:
    /// skipping an entry would renumber every later `BlockId` and silently
    /// orphan the worldgen/recipe/drop references pointing past it.
    fn check_ids(&self, def: &BlockDef) -> Result<(), String> {
        if !is_valid_id(&def.id) {
            return Err(format!(
                "block {:?}: an id must be lowercase letters, digits and underscores",
                def.id
            ));
        }
        if def.id == "air" {
            return Err("\"air\" is built in and may not be declared".into());
        }
        if self.find(&def.id).is_some() {
            return Err(format!("duplicate block {:?}", def.id));
        }
        // A station name is what recipes spell, so it follows the id rule.
        if let Some(station) = &def.station
            && !is_valid_id(station)
        {
            return Err(format!(
                "block {:?}: station {station:?} must be lowercase letters, digits \
                 and underscores",
                def.id
            ));
        }
        Ok(())
    }

    /// Auto-register the flowing blocks: same look and physics as their
    /// source, one per level, with the id "<source>_flow_<level>" (these ids
    /// are the save format — see the fluid module docs).
    fn register_flowing(&mut self, fluids: Vec<FluidSource>, visuals: &mut BlockVisuals) {
        for (group, (source_id, levels, texture)) in fluids.into_iter().enumerate() {
            let source = self.get(source_id).clone();
            let source_textures = visuals
                .textures
                .get(source_id.0 as usize)
                .cloned()
                .flatten();
            // Only worth carrying when the source spelled its own label out:
            // otherwise the derived "Water Flow 1" is already what it would say.
            let source_label = visuals
                .display_names
                .get(source_id.0 as usize)
                .cloned()
                .flatten();
            let flow: Vec<BlockId> = (1..=levels)
                .map(|level| {
                    visuals.textures.push(source_textures.clone());
                    visuals.fluids.push(texture.clone().map(|spec| FluidVisual {
                        spec,
                        flowing: true,
                    }));
                    visuals
                        .display_names
                        .push(source_label.as_ref().map(|l| format!("{l} Flow {level}")));
                    self.register(Block {
                        id: format!("{}_flow_{}", source.id, level),
                        render: source.render,
                        solid: source.solid,
                        hardness: source.hardness,
                        harvest: source.harvest.clone(),
                        drops: Drops::None,
                        fluid: Some(FluidInfo {
                            group: group as u16,
                            level,
                            max_level: levels + 1,
                        }),
                        interact: None,
                        station: None,
                    })
                })
                .collect();
            self.fluid_flow.push(flow);
        }
    }
}

/// A fluid's animation strip, validated: at least two frames, a positive
/// rate, a loop that divides the shader's hourly clock, and an opacity in
/// range.
fn fluid_texture_spec(id: &str, t: &FluidTextureDef) -> Result<FluidTextureSpec, String> {
    if t.frames < 2 {
        return Err(format!(
            "block {id:?}: a fluid texture needs at least 2 frames"
        ));
    }
    if t.fps == 0 {
        return Err(format!("block {id:?}: fluid texture fps must be > 0"));
    }
    // The shader's animation clock wraps hourly; a loop that does not divide
    // it evenly jumps at the wrap.
    if !(3600 * u32::from(t.fps)).is_multiple_of(u32::from(t.frames)) {
        return Err(format!(
            "block {id:?}: {} frames at {} fps does not divide the 3600 s animation clock evenly",
            t.frames, t.fps
        ));
    }
    let opacity = match t.opacity {
        Some(o) if (0.0..=1.0).contains(&o) => Some((o * 255.0).round() as u8),
        Some(o) => {
            return Err(format!(
                "block {id:?}: fluid texture opacity must be 0..=1, got {o}"
            ));
        }
        None => None,
    };
    Ok(FluidTextureSpec {
        path: t.path.clone(),
        frames: t.frames,
        fps: t.fps,
        tint: t.tint,
        opacity,
    })
}

/// A block's six atlas face names, or `None` when its appearance comes from
/// elsewhere.
///
/// A modelled block's geometry carries its own texture, so it needs no atlas
/// tiles here; a `block_model`'s six face tiles are derived from its own art
/// later, in `content`. Anything else without `textures` would silently
/// render as the magenta marker, which is worth rejecting loudly.
fn face_textures(def: &BlockDef, animated_fluid: bool) -> Result<Option<[String; 6]>, String> {
    let modelled = def.model.is_some() || def.block_model.is_some() || animated_fluid;
    match &def.textures {
        Some(t) => Ok(Some(t.names())),
        None if modelled => Ok(None),
        None => Err(format!(
            "block {:?}: needs `textures`, a `block_model`, a `[block.model]` \
             or a `[block.fluid.texture]`",
            def.id
        )),
    }
}
