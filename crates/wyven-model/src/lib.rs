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
pub mod datauri;
pub mod display;
pub mod gltf;
pub mod javamodel;
pub mod mesh;

use std::collections::HashMap;

use glam::Vec3;

use wyven_assets::AssetSource as ContentSource;
use wyven_assets::Rgba8;

pub use display::{DisplayContext, DisplayTransforms, ItemTransform};
pub use mesh::ModelMesh;

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
    /// `assets/`-relative path, e.g. `"assets/models/vine_sword.gltf"`.
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
}

impl Model {
    pub(crate) fn new(mesh: ModelMesh, texture: Rgba8) -> Result<Self, String> {
        let bounds = mesh.bounds().ok_or("model has no geometry")?;
        Ok(Self {
            mesh,
            texture,
            bounds,
            display: DisplayTransforms::default(),
        })
    }

    /// Attach the placements the file declared. Only [`javamodel`] has any.
    pub(crate) fn with_display(mut self, display: DisplayTransforms) -> Self {
        self.display = display;
        self
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
/// model in `assets/blocks/` naming `"../textures/dirt.png"` only resolves the
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
    use wyven_assets::{FsSource, MapSource};

    /// `assets/` sits at the workspace root; a test runner starts in the
    /// crate directory, so the shipped-export fixtures need it named.
    fn assets() -> FsSource {
        FsSource::rooted(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    const GLTF: &str = "assets/models/vine_sword.gltf";
    const BBMODEL: &str = "assets/models/vine_sword.bbmodel";
    const JAVA: &str = "assets/models/wooden_sword.json";

    /// Measured from the two exports of `vine_sword`, which describe the same
    /// object: 21 cubes, 6 faces each, 4 unwelded vertices per face.
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
    /// file, so `assets/blocks/x.json` names `../textures/dirt.png`.
    #[test]
    fn resolve_sibling_collapses_parent_segments() {
        assert_eq!(
            resolve_sibling("assets/blocks", "../textures/dirt.png"),
            "assets/textures/dirt.png"
        );
        assert_eq!(
            resolve_sibling("assets/blocks/nested", "../../textures/a.png"),
            "assets/textures/a.png"
        );
        // Climbing above the root is kept, so the path stays visibly wrong.
        assert_eq!(resolve_sibling("assets", "../../x.png"), "../x.png");
    }

    #[test]
    fn loads_the_gltf_export() {
        let model = load(GLTF);
        assert_eq!(model.vertex_count(), EXPECTED_VERTS);
        assert_eq!(model.triangle_count(), EXPECTED_TRIS);
        assert_eq!(model.texture.size, [32, 32]);
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

    /// The strongest check available: both files are Blockbench exports of the
    /// same sword, so the two loaders must agree on where every vertex and UV
    /// ends up. This is what pins down the bbmodel face-corner order, the UV
    /// rotation direction, the element-rotation sign and the 1/16 scale.
    #[test]
    fn the_two_formats_describe_the_same_model() {
        let a = load(GLTF);
        let b = load(BBMODEL);

        let (a_lo, a_hi) = a.bounds;
        let (b_lo, b_hi) = b.bounds;
        assert!(
            a_lo.abs_diff_eq(b_lo, 1e-4) && a_hi.abs_diff_eq(b_hi, 1e-4),
            "bounds differ: gltf {a_lo}..{a_hi} vs bbmodel {b_lo}..{b_hi}"
        );

        // Compare as unordered sets of (position, uv): the exporters emit faces
        // in a different order, but the surface they describe is identical.
        let key = |m: &Model| {
            let mut rows: Vec<[i32; 5]> = (0..m.vertex_count())
                .map(|i| {
                    let p = m.mesh.positions[i];
                    let uv = m.mesh.uvs[i];
                    // Quantise to 1/1024 block and 1/1024 UV to absorb the
                    // float noise of two independent transform paths.
                    [
                        (p.x * 1024.0).round() as i32,
                        (p.y * 1024.0).round() as i32,
                        (p.z * 1024.0).round() as i32,
                        (uv[0] * 1024.0).round() as i32,
                        (uv[1] * 1024.0).round() as i32,
                    ]
                })
                .collect();
            rows.sort_unstable();
            rows
        };
        assert_eq!(
            key(&a),
            key(&b),
            "the gltf and bbmodel exports disagree on geometry or UVs"
        );
    }

    #[test]
    fn the_sword_lands_where_blockbench_authored_it() {
        // Measured from the files: the blade runs above the block and the hilt
        // dips below it, so a model is emphatically not confined to 0..1.
        let (lo, hi) = load(GLTF).bounds;
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
        let first = registry.load(GLTF, &assets()).expect("loads");
        let second = registry.load(GLTF, &assets()).expect("cached");
        assert_eq!(first, second);
        assert_eq!(registry.len(), 1, "the file should be parsed once");
        assert!(registry.find(GLTF).is_some());
    }

    #[test]
    fn a_failed_path_is_remembered_so_it_warns_once() {
        let mut registry = ModelRegistry::new();
        let source = MapSource::new();
        assert!(registry.load("assets/models/gone.gltf", &source).is_none());
        assert!(registry.load("assets/models/gone.gltf", &source).is_none());
        assert_eq!(registry.by_path.len(), 1);
    }
}
