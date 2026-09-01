//! Loading 3D models from files.
//!
//! Everything the engine drew before this module was hard-coded Rust geometry:
//! the built-in box models can build boxes and nothing else. This module reads
//! a model file and hands back triangles plus the texture they sample, so a
//! shape authored in Blockbench (or anything that exports glTF) can be used as
//! an entity's body or an item's model without touching Rust.
//!
//! Three formats are supported, all JSON:
//! - `.gltf` — the interchange format every DCC tool exports. Blockbench's
//!   export is self-contained (buffer and texture as inline `data:` URIs).
//! - `.bbmodel` — Blockbench's native file, cuboids with per-face UV rects.
//! - `.json` — Blockbench's *Java Block/Item* export, the format blocks are
//!   authored in, read here by [`javamodel`] for the *item* path.
//!
//! Adding a fourth means writing one [`ModelLoader`] and listing it in
//! [`ModelRegistry::LOADERS`]; nothing else in the module changes, and nothing
//! outside it can tell which loader produced a given [`Model`].
//!
//! [`blockjson`] is the parser behind that third entry, and stays outside the
//! registry itself: it names several textures per model and carries `cullface`
//! and `tintindex`, which the chunk mesher needs and a [`Model`]'s single
//! [`Rgba8`] has nowhere to put. Blocks call it directly; [`javamodel`] is the
//! adapter that flattens what it produces into a [`Model`], and is what claims
//! `.json` in the registry.
//!
//! It also reads the one thing a `.gltf` and a `.bbmodel` cannot express: the
//! [`display`] block, which places a model separately for the hand, the ground
//! and the inventory slot instead of making one placement serve all three.
//!
//! Boundaries: this layer is pure. It reads through the [`ContentSource`] port,
//! never touches the filesystem or the GPU directly, and produces model-space
//! geometry — baking a model into the world at a position and yaw is
//! [`ModelMesh::to_cpu_mesh`], and uploading it is the caller's business.

pub mod bbmodel;
pub mod blockjson;
pub mod clip;
pub mod datauri;
pub mod display;
pub mod generated;
pub mod gltf;
pub mod javamodel;
pub mod mesh;
pub mod rig;
pub mod silhouette;

use std::collections::HashMap;

use glam::Vec3;

use wyven_assets::AssetSource as ContentSource;
use wyven_assets::Rgba8;

pub use clip::{Channel, Clip, Interpolation, Keyframe, LoopMode, Track};
pub use display::{DisplayContext, DisplayTransforms, ItemTransform};
pub use mesh::{ModelMesh, UvWindow};
pub use rig::{Bone, BoneId, BonePart, BoneTransform, Pose, Rig};

use bbmodel::BbmodelLoader;
use gltf::GltfLoader;
use javamodel::JavaModelLoader;

/// Index of a loaded model in a [`ModelRegistry`]. Cheap to copy and to store
/// on per-entity data, which matters because entity visuals are cloned per mob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(pub u32);

/// How a data file points at a model: which file, and how to place it.
///
/// Shared by `[entity.visual] kind = "model"` and `[item.model]` so both spell
/// it identically, and deliberately holds nothing but a path and two numbers —
/// entity visuals are cloned per mob, so this must stay cheap to copy.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// `assets/`-relative path, e.g. `"assets/models/items/vine_sword.bbmodel"`.
    pub path: String,
    /// Uniform scale applied to the model's own units.
    #[serde(default = "unit_scale")]
    pub scale: f32,
    /// Shift within the model's own space, applied before `scale`. Blockbench
    /// authors on a 0..1 block, so `[-0.5, 0, -0.5]` re-centres a model on the
    /// entity position it is drawn at.
    #[serde(default)]
    pub offset: [f32; 3],
    /// Rotation in degrees about the model's own axes, applied *after* `offset`
    /// has re-centred it — so a model turns about itself, not about the point it
    /// is drawn at. Exists because exports disagree on which plane a flat object
    /// lies in: the tool models are flat in XY, `vine_sword` in YZ.
    #[serde(default)]
    pub rotation: [f32; 3],
}

fn unit_scale() -> f32 {
    1.0
}

impl ModelSpec {
    pub fn offset(&self) -> Vec3 {
        Vec3::from(self.offset)
    }

    /// The authored rotation, converted from degrees to radians.
    pub fn rotation(&self) -> Vec3 {
        Vec3::from(self.rotation.map(f32::to_radians))
    }
}

/// A model file, parsed: its geometry in model space and the texture it samples.
pub struct Model {
    pub mesh: ModelMesh,
    pub texture: Rgba8,
    /// Model-space bounds, handy for sanity checks and for centring a model.
    pub bounds: (Vec3, Vec3),
    /// Where the file asks to be placed in each context it can be drawn in.
    /// Empty for every format that cannot express it — which is `.gltf` and
    /// `.bbmodel`, i.e. everything authored before [`display`] existed.
    pub display: DisplayTransforms,
    /// The bones and clips the file declared, or `None` for a flat model.
    ///
    /// An `Option` rather than an empty rig so a caller that only wants
    /// triangles never has to know rigs exist, and so the two cases cannot be
    /// confused: `mesh` alone is always the rest pose, whether or not anything
    /// could animate it.
    pub rig: Option<rig::Rig>,
}

impl Model {
    pub(crate) fn new(mesh: ModelMesh, texture: Rgba8) -> Result<Self, String> {
        let bounds = mesh.bounds().ok_or("model has no geometry")?;
        Ok(Self {
            mesh,
            texture,
            bounds,
            display: DisplayTransforms::default(),
            rig: None,
        })
    }

    /// Attach the placements the file declared. Only [`javamodel`] has any.
    pub(crate) fn with_display(mut self, display: DisplayTransforms) -> Self {
        self.display = display;
        self
    }

    /// Attach the skeleton the file declared. Only [`bbmodel`] has one.
    pub(crate) fn with_rig(mut self, rig: Option<rig::Rig>) -> Self {
        self.rig = rig;
        self
    }

    /// The pose this model is drawn in when nothing is animating it.
    pub fn rest_pose(&self) -> Option<rig::Pose> {
        self.rig.as_ref().map(rig::Pose::rest)
    }

    /// The placement this model's file asks for in `context`, or `None` when it
    /// declares none — in which case the caller's own data file decides, which
    /// is what [`mesh::local_transform`] is for.
    pub fn placement_for(&self, context: DisplayContext) -> Option<ItemTransform> {
        self.display.get(context)
    }

    pub fn triangle_count(&self) -> usize {
        self.mesh.triangle_count()
    }

    pub fn vertex_count(&self) -> usize {
        self.mesh.positions.len()
    }
}

/// One file format.
///
/// Implementations normalise to the conventions [`ModelMesh`] documents, so
/// callers can substitute any loader for any other without noticing.
pub trait ModelLoader {
    /// Lowercase extensions this loader claims, e.g. `["gltf"]`.
    fn extensions(&self) -> &'static [&'static str];

    /// Parse `bytes`. `dir` is the `assets/`-relative directory the file came
    /// from, used with `source` to resolve sidecar references (an external
    /// buffer or texture) — self-contained files never touch it.
    fn load(&self, bytes: &[u8], dir: &str, source: &dyn ContentSource) -> Result<Model, String>;
}

/// Models loaded so far, keyed by their `assets/`-relative path.
///
/// Parsing is memoised: two entity kinds pointing at the same file share one
/// [`ModelId`], one parse and (downstream) one GPU texture.
#[derive(Default)]
pub struct ModelRegistry {
    models: Vec<Model>,
    by_path: HashMap<String, Option<ModelId>>,
}

impl ModelRegistry {
    /// The registered loaders. Adding a format means adding an entry here.
    const LOADERS: &'static [&'static dyn ModelLoader] =
        &[&GltfLoader, &BbmodelLoader, &JavaModelLoader];

    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn get(&self, id: ModelId) -> Option<&Model> {
        self.models.get(id.0 as usize)
    }

    /// The id a previously loaded `path` was given, if it loaded at all.
    pub fn find(&self, path: &str) -> Option<ModelId> {
        self.by_path.get(path).copied().flatten()
    }

    /// Load `path`, or return the id it already has.
    ///
    /// Fail-soft, like every other content loader: a missing file, an unknown
    /// extension or a malformed model logs a warning and yields `None`, so one
    /// bad model costs its own entity's appearance and not the whole boot. The
    /// failure is remembered, so a broken path warns once rather than once per
    /// entity that references it.
    pub fn load(&mut self, path: &str, source: &dyn ContentSource) -> Option<ModelId> {
        if let Some(&cached) = self.by_path.get(path) {
            return cached;
        }
        let result = self.try_load(path, source);
        let id = match result {
            Ok(model) => {
                log::info!(
                    "loaded model {path} ({} verts, {} tris, {}x{} texture)",
                    model.vertex_count(),
                    model.triangle_count(),
                    model.texture.width(),
                    model.texture.height()
                );
                let id = ModelId(self.models.len() as u32);
                self.models.push(model);
                Some(id)
            }
            Err(err) => {
                log::warn!("could not load model {path}: {err}");
                None
            }
        };
        self.by_path.insert(path.to_string(), id);
        id
    }

    fn try_load(&self, path: &str, source: &dyn ContentSource) -> Result<Model, String> {
        let extension = path
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
            .ok_or_else(|| format!("{path} has no file extension"))?;
        let loader = Self::LOADERS
            .iter()
            .find(|l| l.extensions().contains(&extension.as_str()))
            .ok_or_else(|| {
                format!(
                    "unsupported model format {extension:?} (supported: {})",
                    Self::supported_extensions().join(", ")
                )
            })?;
        let bytes = source
            .read_bytes(path)
            .map_err(|e| format!("could not read the file: {e}"))?;
        loader.load(&bytes, parent_dir(path), source)
    }

    fn supported_extensions() -> Vec<&'static str> {
        Self::LOADERS
            .iter()
            .flat_map(|l| l.extensions().iter().copied())
            .collect()
    }
}

/// The directory part of an `assets/`-relative path (`""` if there is none).
fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

/// Resolve a reference made *by* a model file (an external buffer or texture)
/// against the directory that file lives in.
pub(crate) fn resolve_sibling(dir: &str, path: &str) -> String {
    let joined = if dir.is_empty() {
        path.to_string()
    } else {
        format!("{dir}/{path}")
    };
    normalize_path(&joined)
}

/// Collapse `.` and `..` segments in an `assets/`-relative path.
///
/// The OS would do this for a real filesystem read, but content also comes from
/// [`wyven_assets::MapSource`], which looks paths up verbatim — so a block
/// model in `assets/models/blocks/` naming `"../../textures/blocks/dirt.png"` only resolves the
/// same way from both sources if the collapsing happens here.
///
/// A `..` that would climb above the root is kept rather than dropped, so a
/// bogus path stays visibly bogus instead of silently becoming a sibling of the
/// asset root.
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." if matches!(parts.last(), Some(&last) if last != "..") => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Mat4;
    use wyven_assets::{FsSource, MapSource};

    /// `assets/` sits at the workspace root; a test runner starts in the
    /// crate directory, so the shipped-export fixtures need it named.
    fn assets() -> FsSource {
        FsSource::rooted(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    const BBMODEL: &str = "assets/models/items/vine_sword.bbmodel";
    const JAVA: &str = "assets/models/items/wooden_sword.json";
    /// The shipped rigged player: a 15-bone skeleton and two looping clips.
    const RIGGED: &str = "assets/models/entity/player/player.bbmodel";
    /// Blockbench 4.x, which inlines its group on the outliner node.
    const OLD_FORMAT: &str = "assets/models/blocks/plant1.bbmodel";

    /// Measured from the shipped `vine_sword` export: 21 cubes, 6 faces each, 4 unwelded vertices per face.
    const EXPECTED_VERTS: usize = 504;
    const EXPECTED_TRIS: usize = 252;

    fn load(path: &str) -> Model {
        let mut registry = ModelRegistry::new();
        let id = registry
            .load(path, &assets())
            .unwrap_or_else(|| panic!("{path} should load"));
        registry.models.swap_remove(id.0 as usize)
    }

    #[test]
    fn parent_dir_splits_asset_paths() {
        assert_eq!(parent_dir("assets/models/a.gltf"), "assets/models");
        assert_eq!(parent_dir("a.gltf"), "");
    }

    #[test]
    fn resolve_sibling_joins_against_the_models_directory() {
        assert_eq!(
            resolve_sibling("assets/models/vine_sword", "vine.png"),
            "assets/models/vine_sword/vine.png"
        );
        assert_eq!(
            resolve_sibling("assets/models", "./t.png"),
            "assets/models/t.png"
        );
        assert_eq!(resolve_sibling("", "t.png"), "t.png");
    }

    /// Blockbench writes block-model texture refs relative to the exported
    /// file, so `assets/models/blocks/x.json` names `../../textures/blocks/dirt.png`.
    #[test]
    fn resolve_sibling_collapses_parent_segments() {
        assert_eq!(
            resolve_sibling("assets/blocks", "../textures/blocks/dirt.png"),
            "assets/textures/blocks/dirt.png"
        );
        assert_eq!(
            resolve_sibling("assets/blocks/nested", "../../textures/blocks/a.png"),
            "assets/textures/blocks/a.png"
        );
        // Climbing above the root is kept, so the path stays visibly wrong.
        assert_eq!(resolve_sibling("assets", "../../x.png"), "../x.png");
    }

    /// The Java Block/Item export is the only format that can say where a model
    /// belongs in each context, and the shipped sword says it for all of them.
    #[test]
    fn loads_the_java_export_with_its_display_block() {
        let model = load(JAVA);
        assert_eq!(model.texture.size, [32, 32]);
        assert!(!model.display.is_empty());

        let first = model
            .placement_for(DisplayContext::FirstPersonRightHand)
            .expect("firstperson_righthand");
        assert_eq!(first.translation, [0.0, 1.0, 1.0]);
        assert!((first.scale[0] - 0.79883).abs() < 1e-6);
        assert!((first.rotation[0] - -99.9).abs() < 1e-4);

        for context in [
            DisplayContext::ThirdPersonRightHand,
            DisplayContext::Gui,
            DisplayContext::Ground,
            DisplayContext::Fixed,
        ] {
            assert!(
                model.placement_for(context).is_some(),
                "{context:?} should be declared"
            );
        }
        // `"head": {"scale": [0, 0, 0]}` is how the file says "not here".
        assert_eq!(model.placement_for(DisplayContext::Head), None);
    }

    /// The fallback contract every model authored before `display` relies on: a
    /// `.bbmodel` declares nothing, so its `[item.model]` spec keeps placing it.
    #[test]
    fn a_bbmodel_declares_no_placement_of_its_own() {
        let model = load(BBMODEL);
        assert!(model.display.is_empty());
        assert_eq!(
            model.placement_for(DisplayContext::ThirdPersonRightHand),
            None
        );
    }

    #[test]
    fn loads_the_bbmodel_export() {
        let model = load(BBMODEL);
        assert_eq!(model.vertex_count(), EXPECTED_VERTS);
        assert_eq!(model.triangle_count(), EXPECTED_TRIS);
        assert_eq!(model.texture.size, [32, 32]);
    }

    #[test]
    fn the_sword_lands_where_blockbench_authored_it() {
        // Measured from the files: the blade runs above the block and the hilt
        // dips below it, so a model is emphatically not confined to 0..1.
        let (lo, hi) = load(BBMODEL).bounds;
        assert!(
            lo.abs_diff_eq(Vec3::new(0.467188, -0.889055, 0.113190), 1e-4),
            "lo = {lo}"
        );
        assert!(
            hi.abs_diff_eq(Vec3::new(0.532812, 1.470678, 0.886810), 1e-4),
            "hi = {hi}"
        );
    }

    #[test]
    fn an_unknown_extension_is_reported_not_guessed() {
        let mut registry = ModelRegistry::new();
        let source = MapSource::new().with("assets/models/a.obj", "v 0 0 0");
        assert!(registry.load("assets/models/a.obj", &source).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn a_missing_file_fails_soft() {
        let mut registry = ModelRegistry::new();
        assert!(
            registry
                .load("assets/models/nope.gltf", &MapSource::new())
                .is_none()
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn malformed_json_fails_soft() {
        let mut registry = ModelRegistry::new();
        let source = MapSource::new().with("assets/models/bad.gltf", "{ not json");
        assert!(registry.load("assets/models/bad.gltf", &source).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn the_same_path_parses_once_and_shares_an_id() {
        let mut registry = ModelRegistry::new();
        let first = registry.load(BBMODEL, &assets()).expect("loads");
        let second = registry.load(BBMODEL, &assets()).expect("cached");
        assert_eq!(first, second);
        assert_eq!(registry.len(), 1, "the file should be parsed once");
        assert!(registry.find(BBMODEL).is_some());
    }

    #[test]
    fn a_failed_path_is_remembered_so_it_warns_once() {
        let mut registry = ModelRegistry::new();
        let source = MapSource::new();
        assert!(registry.load("assets/models/gone.gltf", &source).is_none());
        assert!(registry.load("assets/models/gone.gltf", &source).is_none());
        assert_eq!(registry.by_path.len(), 1);
    }
    /// The whole point of storing pivots post-rest: a rigged model drawn in its
    /// rest pose must be *exactly* the flat mesh, not merely very close to it.
    ///
    /// Every `.bbmodel` with a hierarchy is now loaded through the rig-building
    /// walk, so if this drifts, so do the swords, the plants and the player at
    /// once — and it drifts silently, as a model very slightly inside out.
    #[test]
    fn the_rest_pose_bake_is_the_flat_bake() {
        for path in [
            RIGGED,
            OLD_FORMAT,
            "assets/models/items/wooden_sword.bbmodel",
        ] {
            let model = load(path);
            let rig = model
                .rig
                .as_ref()
                .unwrap_or_else(|| panic!("{path} has a hierarchy, so it has a rig"));
            let transform = Mat4::from_scale(Vec3::splat(1.7)) * Mat4::from_rotation_y(0.4);

            let flat = model.mesh.bake(transform);
            let posed = model.mesh.bake_posed(
                rig.parts(),
                &rig.matrices(&rig::Pose::rest(rig)),
                transform,
                transform,
                mesh::UvWindow::FULL,
            );

            assert_eq!(flat.vertices.len(), posed.vertices.len(), "{path}");
            assert_eq!(flat.indices, posed.indices, "{path} triangles");
            for (i, (a, b)) in flat.vertices.iter().zip(&posed.vertices).enumerate() {
                assert_eq!(a.position, b.position, "{path} vertex {i} position");
                assert_eq!(a.normal, b.normal, "{path} vertex {i} normal");
                assert_eq!(a.uv, b.uv, "{path} vertex {i} uv");
                assert_eq!(a.ao, b.ao, "{path} vertex {i} shade");
            }
        }
    }

    /// The shipped sword's outliner is 21 bare element uuids — no groups at all.
    /// It must keep taking the flat path it always did.
    #[test]
    fn a_shipped_model_with_a_flat_outliner_has_no_rig() {
        assert!(load(BBMODEL).rig.is_none());
    }

    /// Blockbench 4.x puts a group's pivot on the outliner node; 5.0 puts it in
    /// the `groups` table. Reading only the node — which is what the loader used
    /// to do — silently zeroed every 5.0 pivot.
    #[test]
    fn both_blockbench_group_layouts_are_read() {
        let old = load(OLD_FORMAT);
        let old_rig = old.rig.expect("4.x inlines its group");
        let inline = old_rig.bone("group").expect("named on the node itself");
        assert_eq!(old_rig.pivot(inline), Vec3::new(7.0, 0.0, 10.0) / 16.0);

        let new = load("assets/models/items/wooden_sword.bbmodel");
        let new_rig = new.rig.expect("5.0 tables its group");
        let tabled = new_rig.bone("group").expect("named in the groups table");
        assert_eq!(new_rig.pivot(tabled), Vec3::new(7.0, 3.0, 7.0) / 16.0);
    }

    /// Blockbench 5.0 keeps group definitions in a table of their own; read
    /// only from the outliner node, every pivot here would silently be zero.
    #[test]
    fn a_format_5_file_reads_its_bones_from_the_groups_table() {
        let model = load(RIGGED);
        let rig = model.rig.expect("the player is rigged");
        assert_eq!(rig.bone_count(), 15);

        let head = rig.bone("head").expect("a head bone");
        let pivot = rig.pivot(head);
        // Authored at [0, 13.3, 0] in sixteenths of a block.
        assert!((pivot.y - 13.3 / 16.0).abs() < 1e-6, "head pivot {pivot}");

        let torso = rig.bone("torso").expect("a torso bone");
        assert_eq!(rig.bones()[head.0 as usize].parent, Some(torso));
        assert_eq!(rig.bones()[torso.0 as usize].parent, rig.bone("root"));
    }

    /// The arm chain is three deep, which the old one-pivot box model could not
    /// express at all — an elbow is the reason this work exists.
    #[test]
    fn the_arm_is_a_chain_of_three_joints() {
        let model = load(RIGGED);
        let rig = model.rig.expect("the player is rigged");
        let shoulder = rig.bone("arm_l").expect("arm_l");
        let names: Vec<&str> = rig
            .subtree(shoulder)
            .into_iter()
            .map(|b| rig.name(b))
            .collect();
        assert_eq!(names, vec!["arm_l", "albow_l", "hand_l"]);
    }

    #[test]
    fn the_players_clips_are_read_with_their_loop_and_length() {
        let model = load(RIGGED);
        let rig = model.rig.expect("the player is rigged");
        assert_eq!(rig.clips().len(), 2);

        let walk = rig.clip("walk").expect("a walk clip");
        assert_eq!(walk.length, 1.0);
        assert_eq!(walk.loop_mode, clip::LoopMode::Loop);
        assert!(!walk.is_empty());

        assert!(rig.clip("run").is_some());
        assert!(
            rig.clip("moonwalk").is_none(),
            "clips are found by name, not guessed"
        );
    }

    /// Keyframe values arrive as JSON *strings*, in degrees. A plain `f32`
    /// deserialize would reject the file; forgetting the conversion would swing
    /// a leg by 37 radians.
    #[test]
    fn keyframe_values_are_parsed_from_strings_and_converted_to_radians() {
        let model = load(RIGGED);
        let rig = model.rig.expect("the player is rigged");
        let walk = rig.clip("walk").expect("a walk clip");
        let leg = rig.bone("leg_r").expect("leg_r");

        let track = walk
            .tracks()
            .iter()
            .find(|t| t.bone == leg && t.channel == clip::Channel::Rotation)
            .expect("leg_r is animated");
        // Authored: -35 at t=0, +37.5 at t=0.5.
        let start = track.sample(0.0).x.to_degrees();
        let mid = track.sample(0.5).x.to_degrees();
        assert!((start - -35.0).abs() < 1e-3, "start {start}");
        assert!((mid - 37.5).abs() < 1e-3, "mid {mid}");
    }

    /// A clip does move the mesh — the guard against a rig that parses
    /// beautifully and animates nothing.
    #[test]
    fn playing_a_clip_moves_the_geometry() {
        let model = load(RIGGED);
        let rig = model.rig.as_ref().expect("the player is rigged");
        let walk = rig.clip("walk").expect("a walk clip");

        let mut pose = rig::Pose::rest(rig);
        walk.sample(0.25, &mut pose);
        let posed = model.mesh.bake_posed(
            rig.parts(),
            &rig.matrices(&pose),
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            mesh::UvWindow::FULL,
        );
        let rest = model.mesh.bake(Mat4::IDENTITY);

        let moved = rest
            .vertices
            .iter()
            .zip(&posed.vertices)
            .filter(|(a, b)| a.position != b.position)
            .count();
        assert!(moved > 0, "the walk clip should displace vertices");
        assert!(
            moved < rest.vertices.len(),
            "but not every one — the head is still"
        );
    }

    /// A flat file must stay flat: no groups means no rig, and the geometry
    /// still comes out of the same walk it always did.
    #[test]
    fn a_model_with_no_hierarchy_has_no_rig() {
        const PIXEL: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        let source = MapSource::new().with(
            "assets/models/items/flat.bbmodel",
            format!(
                r#"{{"resolution":{{"width":16,"height":16}},
                    "elements":[{{"uuid":"a","type":"cube","from":[0,0,0],"to":[16,16,16],
                      "faces":{{"north":{{"uv":[0,0,16,16]}}}}}}],
                    "outliner":["a"],
                    "textures":[{{"source":"{PIXEL}"}}]}}"#
            ),
        );
        let mut registry = ModelRegistry::new();
        let id = registry
            .load("assets/models/items/flat.bbmodel", &source)
            .expect("a flat bbmodel still loads");
        let model = registry.get(id).expect("registered");
        assert!(model.rig.is_none(), "no groups, no rig");
        assert_eq!(model.vertex_count(), 4, "and the one face is still there");
    }
}
