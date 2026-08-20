//! Blockbench "Java Block/Item" model loader (`.json`).
//!
//! This is Minecraft's block-model format, which Blockbench exports natively:
//! a list of axis-aligned `elements` in sixteenths of a block, each with six
//! optional `faces` carrying a UV rect, a texture reference, and two pieces of
//! information a `.bbmodel` has no room for —
//!
//! - `cullface`: the neighbour direction that hides this face, which is what
//!   lets a modelled full cube take part in chunk face culling instead of
//!   emitting all six faces of every buried block;
//! - `tintindex`: the face is biome-coloured rather than carrying its colour in
//!   the texture.
//!
//! It also names **several** textures per model (grass block: top, side, side
//! overlay, bottom), where [`super::Model`] carries exactly one. So this is
//! deliberately *not* a [`super::ModelLoader`]: it produces a different shape,
//! and `.json` is far too generic an extension to claim in a registry that
//! dispatches on extension alone with no content sniffing.
//!
//! Everything about how a UV rect maps onto a box corner is shared verbatim
//! with [`super::bbmodel`], whose conventions are pinned by a test asserting it
//! agrees vertex-for-vertex with Blockbench's own glTF export.
//!
//! Boundaries: pure. Geometry comes out in block-local `0..1` space with no
//! notion of atlas layers, block ids or neighbours — turning that into
//! something the chunk mesher can emit is [`crate::world::blockmodel`].

use std::collections::HashMap;

use glam::Vec3;
use serde::Deserialize;

use crate::content::ContentSource;
use crate::core::Direction;
use crate::render::Rgba8;
use crate::render::texture::decode_png;

use super::bbmodel::{
    FACES, FaceDir, PIXELS_PER_BLOCK, Resolution, Transform, face_corners, face_uvs,
};
use super::resolve_sibling;

/// Minecraft's UV space is always `0..16`, whatever the texture's pixel size —
/// a 256×256 PNG is just a 16× resource pack, not a different UV grid.
const UV_RESOLUTION: Resolution = Resolution {
    width: 16.0,
    height: 16.0,
};

/// How far a face is pushed out along its normal per co-planar face already
/// emitted beneath it.
///
/// Minecraft draws the grass block as two elements occupying *exactly* the same
/// box — the dirt-and-grass cube, and a tinted side overlay — and separates
/// them with render layers. We have one depth-tested pass, so without a nudge
/// the overlay loses the depth test and never draws at all. Same order as the
/// crack overlay's `INFLATE`, i.e. far below a pixel at any sane view distance.
const COPLANAR_EPSILON: f32 = 0.002;

/// How many `#name` hops a texture reference may take before we call it a loop.
const MAX_TEXTURE_INDIRECTION: usize = 8;

/// One textured quad of a block model, in block-local `0..1` coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockQuad {
    /// Corners in winding order, already divided down from pixel space.
    pub positions: [Vec3; 4],
    pub normal: Vec3,
    pub uvs: [[f32; 2]; 4],
    /// Index into [`BlockJsonModel::textures`].
    pub texture: usize,
    /// From `cullface`: the neighbour that hides this quad when it is solid.
    pub cull: Option<Direction>,
    /// From `tintindex`: multiply by the biome tint instead of drawing the
    /// texture's own colour. Minecraft's index selects among several tint
    /// sources; we have one, so any non-negative index means "tinted".
    pub tinted: bool,
    /// Baked directional face shade, or `1.0` where the element opted out with
    /// `"shade": false`.
    pub shade: f32,
}

/// A parsed block model: its geometry, and every texture that geometry samples.
#[derive(Debug, Clone)]
pub struct BlockJsonModel {
    pub quads: Vec<BlockQuad>,
    /// Decoded textures, in first-use order.
    pub textures: Vec<Rgba8>,
    /// Where each texture came from, `assets/`-relative. Used to key the layer
    /// registry so two blocks naming the same PNG share one array layer.
    pub texture_paths: Vec<String>,
}

impl BlockJsonModel {
    /// Model-space bounds, or `None` when the model has no geometry.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut points = self.quads.iter().flat_map(|q| q.positions.iter().copied());
        let first = points.next()?;
        Some(points.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }
}

/// Parse a Blockbench Java Block/Item export.
///
/// `dir` is the `assets/`-relative directory the file came from; texture
/// references are resolved against it through `source`, so the loader never
/// touches the filesystem itself.
pub fn load(bytes: &[u8], dir: &str, source: &dyn ContentSource) -> Result<BlockJsonModel, String> {
    let doc: Document =
        serde_json::from_slice(bytes).map_err(|e| format!("invalid block model JSON: {e}"))?;

    if !doc.parent.trim().is_empty() {
        log::warn!(
            "block model declares parent {:?}; model inheritance is not supported, \
             the model must be self-contained",
            doc.parent
        );
    }

    let mut textures = TextureSet::default();
    let mut quads = Vec::new();
    let mut planes: HashMap<[i32; 4], u32> = HashMap::new();

    for element in &doc.elements {
        element.build(
            &doc.textures,
            dir,
            source,
            &mut textures,
            &mut planes,
            &mut quads,
        )?;
    }

    if quads.is_empty() {
        return Err("block model has no geometry".into());
    }

    Ok(BlockJsonModel {
        quads,
        textures: textures.images,
        texture_paths: textures.paths,
    })
}

// --- JSON schema (only the fields we read) ----------------------------------

#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    parent: String,
    /// `{"0": "../textures/dirt", "particle": "#0"}` — an object, unlike
    /// `.bbmodel`'s array.
    #[serde(default)]
    textures: HashMap<String, String>,
    #[serde(default)]
    elements: Vec<Element>,
}

#[derive(Deserialize)]
struct Element {
    #[serde(default)]
    from: [f32; 3],
    #[serde(default)]
    to: [f32; 3],
    /// A single pivoted rotation about one axis — an object here, where
    /// `.bbmodel` uses a bare `[x, y, z]` triple.
    rotation: Option<ElementRotation>,
    /// Opting out of directional shading, for faces that should read flat.
    #[serde(default = "yes")]
    shade: bool,
    #[serde(default)]
    faces: HashMap<String, Face>,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct ElementRotation {
    #[serde(default)]
    angle: f32,
    #[serde(default = "y_axis")]
    axis: String,
    #[serde(default)]
    origin: [f32; 3],
}

fn y_axis() -> String {
    "y".into()
}

impl ElementRotation {
    /// As the `[x, y, z]` degree triple [`Transform::then`] expects. Only one
    /// component is ever non-zero — the format allows exactly one axis.
    fn degrees(&self) -> Result<[f32; 3], String> {
        match self.axis.as_str() {
            "x" => Ok([self.angle, 0.0, 0.0]),
            "y" => Ok([0.0, self.angle, 0.0]),
            "z" => Ok([0.0, 0.0, self.angle]),
            other => Err(format!(
                "element rotation axis must be x, y or z, got {other:?}"
            )),
        }
    }
}

#[derive(Deserialize)]
struct Face {
    /// `[u1, v1, u2, v2]` in Minecraft's fixed `0..16` space. Optional: when
    /// absent it is derived from the element's own extents.
    uv: Option<[f32; 4]>,
    /// `"#0"` — a key into the document's `textures` map, possibly via other
    /// keys.
    texture: Option<String>,
    cullface: Option<String>,
    /// `-1` (or absent) means "not tinted".
    #[serde(default = "no_tint")]
    tintindex: i32,
    #[serde(default)]
    rotation: f32,
}

fn no_tint() -> i32 {
    -1
}

// --- Geometry ---------------------------------------------------------------

impl Element {
    fn build(
        &self,
        refs: &HashMap<String, String>,
        dir: &str,
        source: &dyn ContentSource,
        textures: &mut TextureSet,
        planes: &mut HashMap<[i32; 4], u32>,
        out: &mut Vec<BlockQuad>,
    ) -> Result<(), String> {
        let a = Vec3::from(self.from);
        let b = Vec3::from(self.to);
        // Authors can drag a box "inside out"; min/max keeps faces outward.
        let lo = a.min(b);
        let hi = a.max(b);

        let transform = match &self.rotation {
            Some(r) => Transform::IDENTITY.then(r.degrees()?, r.origin),
            None => Transform::IDENTITY,
        };

        for (name, face_dir) in FACES {
            let Some(face) = self.faces.get(name) else {
                continue;
            };
            // Blockbench writes an empty rect for a face it considers unused.
            let rect = face.uv.unwrap_or_else(|| default_uv(face_dir, lo, hi));
            if rect[0] == rect[2] || rect[1] == rect[3] {
                continue;
            }
            let Some(reference) = face.texture.as_deref() else {
                continue;
            };

            let texture = textures.resolve(reference, refs, dir, source)?;
            let corners = face_corners(face_dir, lo, hi);
            let uvs = face_uvs(rect, face.rotation, UV_RESOLUTION);
            let normal = transform.apply_normal(face_dir.normal());

            let positions: [Vec3; 4] =
                std::array::from_fn(|i| transform.apply(corners[i]) / PIXELS_PER_BLOCK);

            // Push co-planar faces apart in draw order so the later one wins
            // the depth test instead of z-fighting with what it overlays.
            let depth = planes.entry(plane_key(normal, positions[0])).or_insert(0);
            let nudge = normal * (*depth as f32 * COPLANAR_EPSILON);
            *depth += 1;

            let cull = match face.cullface.as_deref() {
                Some(name) => Some(
                    FaceDir::from_name(name)
                        .ok_or_else(|| format!("unknown cullface {name:?}"))?
                        .direction(),
                ),
                None => None,
            };

            out.push(BlockQuad {
                positions: positions.map(|p| p + nudge),
                normal,
                uvs,
                texture,
                cull,
                tinted: face.tintindex >= 0,
                shade: if self.shade {
                    super::mesh::shade(normal)
                } else {
                    1.0
                },
            });
        }
        Ok(())
    }
}

/// Identify the plane a quad lies in, quantised so two faces authored at the
/// same coordinates land on the same key despite float noise.
fn plane_key(normal: Vec3, point: Vec3) -> [i32; 4] {
    let q = |v: f32| (v * 1024.0).round() as i32;
    [q(normal.x), q(normal.y), q(normal.z), q(normal.dot(point))]
}

/// Minecraft's rule for a face that omits its `uv`: project the element's own
/// extents onto the face.
fn default_uv(dir: FaceDir, lo: Vec3, hi: Vec3) -> [f32; 4] {
    const FULL: f32 = 16.0;
    match dir {
        FaceDir::Down => [lo.x, FULL - hi.z, hi.x, FULL - lo.z],
        FaceDir::Up => [lo.x, lo.z, hi.x, hi.z],
        FaceDir::North => [FULL - hi.x, FULL - hi.y, FULL - lo.x, FULL - lo.y],
        FaceDir::South => [lo.x, FULL - hi.y, hi.x, FULL - lo.y],
        FaceDir::West => [lo.z, FULL - hi.y, hi.z, FULL - lo.y],
        FaceDir::East => [FULL - hi.z, FULL - hi.y, FULL - lo.z, FULL - lo.y],
    }
}

// --- Textures ---------------------------------------------------------------

/// The textures a single model references, deduplicated by resolved path.
#[derive(Default)]
struct TextureSet {
    images: Vec<Rgba8>,
    paths: Vec<String>,
    by_path: HashMap<String, usize>,
}

impl TextureSet {
    fn resolve(
        &mut self,
        reference: &str,
        refs: &HashMap<String, String>,
        dir: &str,
        source: &dyn ContentSource,
    ) -> Result<usize, String> {
        let path = texture_path(reference, refs, dir)?;
        if let Some(&index) = self.by_path.get(&path) {
            return Ok(index);
        }
        let bytes = source
            .read_bytes(&path)
            .map_err(|e| format!("could not read texture {path}: {e}"))?;
        let image = decode_png(&bytes).map_err(|e| format!("texture {path}: {e}"))?;
        let index = self.images.len();
        self.images.push(image);
        self.paths.push(path.clone());
        self.by_path.insert(path, index);
        Ok(index)
    }
}

/// Follow a `"#key"` reference through the document's texture map to a file
/// path, then make it `assets/`-relative.
///
/// Blockbench writes `"../textures/dirt"` — relative to the exported file, and
/// without the extension.
fn texture_path(
    reference: &str,
    refs: &HashMap<String, String>,
    dir: &str,
) -> Result<String, String> {
    let mut value = reference;
    for _ in 0..MAX_TEXTURE_INDIRECTION {
        let Some(key) = value.strip_prefix('#') else {
            // A namespaced name (`minecraft:block/dirt`) has no meaning for us;
            // the path after the colon is the best guess available.
            let path = value.rsplit(':').next().unwrap_or(value);
            let path = match path.rsplit_once('.') {
                Some((_, "png")) => path.to_string(),
                _ => format!("{path}.png"),
            };
            return Ok(resolve_sibling(dir, &path));
        };
        value = refs
            .get(key)
            .ok_or_else(|| format!("texture {reference:?} names undefined key {key:?}"))?;
    }
    Err(format!(
        "texture {reference:?} still unresolved after {MAX_TEXTURE_INDIRECTION} hops (a loop?)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{FsSource, MapSource};

    const DIRT: &str = "assets/blocks/dirt_block.json";
    const GRASS: &str = "assets/blocks/grass_block.json";

    fn load_file(path: &str) -> BlockJsonModel {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        load(&bytes, dir, &FsSource).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// A 1×1×1-pixel cube so a model can be written inline without a PNG the
    /// size of a real block texture.
    fn tiny_png() -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, 1, 1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[10, 20, 30, 255])
            .unwrap();
        out
    }

    fn source_with(json: &str) -> MapSource {
        MapSource::new()
            .with_bytes("assets/blocks/t.json", json.as_bytes().to_vec())
            .with_bytes("assets/textures/a.png", tiny_png())
            .with_bytes("assets/textures/b.png", tiny_png())
    }

    fn load_inline(json: &str) -> Result<BlockJsonModel, String> {
        load(json.as_bytes(), "assets/blocks", &source_with(json))
    }

    const FULL_CUBE: &str = r##"{
        "textures": { "0": "../textures/a" },
        "elements": [{
            "from": [0, 0, 0], "to": [16, 16, 16],
            "faces": {
                "north": {"uv": [0,0,16,16], "texture": "#0", "cullface": "north"},
                "south": {"uv": [0,0,16,16], "texture": "#0", "cullface": "south"},
                "east":  {"uv": [0,0,16,16], "texture": "#0", "cullface": "east"},
                "west":  {"uv": [0,0,16,16], "texture": "#0", "cullface": "west"},
                "up":    {"uv": [0,0,16,16], "texture": "#0", "cullface": "up"},
                "down":  {"uv": [0,0,16,16], "texture": "#0", "cullface": "down"}
            }
        }]
    }"##;

    #[test]
    fn a_full_cube_spans_the_cell_and_names_its_cullfaces() {
        let model = load_inline(FULL_CUBE).expect("loads");
        assert_eq!(model.quads.len(), 6);
        assert_eq!(model.textures.len(), 1, "one texture, referenced six times");

        let (lo, hi) = model.bounds().expect("has geometry");
        assert!(lo.abs_diff_eq(Vec3::ZERO, 1e-6), "lo = {lo}");
        assert!(
            lo.abs_diff_eq(Vec3::ZERO, 1e-6) && hi.abs_diff_eq(Vec3::ONE, 1e-6),
            "hi = {hi}"
        );

        for dir in Direction::ALL {
            assert!(
                model.quads.iter().any(|q| q.cull == Some(dir)),
                "no quad culls against {dir:?}"
            );
        }
    }

    /// The 0..16 authoring space is a *fixed* grid, not the texture's pixel
    /// size — a 256×256 PNG is a 16× resource pack, not a different UV space.
    #[test]
    fn uvs_are_divided_by_sixteen_not_by_the_texture_size() {
        let model = load_inline(FULL_CUBE).expect("loads");
        for quad in &model.quads {
            for uv in quad.uvs {
                assert!(
                    (uv[0] == 0.0 || uv[0] == 1.0) && (uv[1] == 0.0 || uv[1] == 1.0),
                    "uv {uv:?} is not a full-texture corner"
                );
            }
        }
    }

    #[test]
    fn texture_paths_resolve_relative_to_the_model_file() {
        let model = load_inline(FULL_CUBE).expect("loads");
        assert_eq!(model.texture_paths, vec!["assets/textures/a.png"]);
    }

    #[test]
    fn a_texture_key_may_point_at_another_key() {
        let json = r##"{
            "textures": { "all": "../textures/a", "0": "#all" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.texture_paths, vec!["assets/textures/a.png"]);
    }

    #[test]
    fn a_texture_reference_loop_is_reported_not_followed_forever() {
        let json = r##"{
            "textures": { "a": "#b", "b": "#a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#a"} } }]
        }"##;
        let err = load_inline(json).expect_err("should not loop");
        assert!(err.contains("loop"), "{err}");
    }

    #[test]
    fn an_undefined_texture_key_is_reported() {
        let json = r##"{
            "textures": {},
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#9"} } }]
        }"##;
        let err = load_inline(json).expect_err("should fail");
        assert!(err.contains("undefined key"), "{err}");
    }

    #[test]
    fn a_missing_texture_file_fails_the_model() {
        let json = r##"{
            "textures": { "0": "../textures/gone" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let err = load_inline(json).expect_err("should fail");
        assert!(err.contains("could not read texture"), "{err}");
    }

    /// Two elements occupying the same box is how Minecraft spells the grass
    /// block's tinted side overlay. Without the nudge the second one loses the
    /// depth test everywhere and is simply invisible.
    #[test]
    fn a_coplanar_face_is_nudged_clear_of_the_one_it_overlays() {
        let json = r##"{
            "textures": { "0": "../textures/a", "1": "../textures/b" },
            "elements": [
                { "from": [0,0,0], "to": [16,16,16],
                  "faces": { "north": {"uv": [0,0,16,16], "texture": "#0"} } },
                { "from": [0,0,0], "to": [16,16,16],
                  "faces": { "north": {"uv": [0,0,16,16], "texture": "#1"} } }
            ]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.quads.len(), 2);

        // North is -Z, so the overlay sits slightly further out (smaller z).
        let base = model.quads[0].positions[0].z;
        let overlay = model.quads[1].positions[0].z;
        assert!(
            (base - overlay - COPLANAR_EPSILON).abs() < 1e-6,
            "base z {base}, overlay z {overlay}"
        );
    }

    #[test]
    fn faces_in_different_planes_are_not_nudged() {
        let model = load_inline(FULL_CUBE).expect("loads");
        for quad in &model.quads {
            for p in quad.positions {
                assert!(
                    p.min_element() >= 0.0 && p.max_element() <= 1.0,
                    "{p} left the cell"
                );
            }
        }
    }

    #[test]
    fn an_element_rotation_turns_the_box_about_its_origin() {
        let json = r##"{
            "textures": { "0": "../textures/a" },
            "elements": [{
                "from": [0, 0, 8], "to": [16, 16, 8],
                "rotation": {"angle": 45, "axis": "y", "origin": [8, 8, 8]},
                "faces": { "north": {"uv": [0,0,16,16], "texture": "#0"} }
            }]
        }"##;
        let model = load_inline(json).expect("loads");
        let normal = model.quads[0].normal;
        // A 45° turn about Y takes -Z to (-sin45, 0, -cos45).
        assert!(
            normal.abs_diff_eq(Vec3::new(-0.70710677, 0.0, -0.70710677), 1e-5),
            "normal = {normal}"
        );
    }

    #[test]
    fn an_unknown_rotation_axis_is_reported() {
        let json = r##"{
            "textures": { "0": "../textures/a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "rotation": {"angle": 45, "axis": "w", "origin": [8,8,8]},
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let err = load_inline(json).expect_err("should fail");
        assert!(err.contains("axis"), "{err}");
    }

    #[test]
    fn shade_false_flattens_the_face_lighting() {
        let json = r##"{
            "textures": { "0": "../textures/a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16], "shade": false,
                "faces": { "north": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.quads[0].shade, 1.0);
    }

    #[test]
    fn tintindex_marks_a_face_biome_coloured() {
        let json = r##"{
            "textures": { "0": "../textures/a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": {
                    "up":   {"uv": [0,0,16,16], "texture": "#0", "tintindex": 0},
                    "down": {"uv": [0,0,16,16], "texture": "#0"}
                } }]
        }"##;
        let model = load_inline(json).expect("loads");
        let up = model
            .quads
            .iter()
            .find(|q| q.normal.y > 0.5)
            .expect("up face");
        let down = model
            .quads
            .iter()
            .find(|q| q.normal.y < -0.5)
            .expect("down face");
        assert!(up.tinted);
        assert!(!down.tinted, "an absent tintindex must not tint");
    }

    #[test]
    fn a_face_without_uv_derives_it_from_the_element() {
        let json = r##"{
            "textures": { "0": "../textures/a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"texture": "#0"} } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.quads.len(), 1);
        let uvs = model.quads[0].uvs;
        assert!(uvs.iter().any(|uv| uv == &[0.0, 0.0]));
        assert!(uvs.iter().any(|uv| uv == &[1.0, 1.0]));
    }

    #[test]
    fn a_model_without_geometry_is_rejected() {
        let err = load_inline(r##"{ "textures": {}, "elements": [] }"##).expect_err("no geometry");
        assert!(err.contains("no geometry"), "{err}");
    }

    #[test]
    fn malformed_json_is_reported() {
        let err = load(b"{ not json", "assets/blocks", &MapSource::new()).expect_err("bad json");
        assert!(err.contains("invalid block model JSON"), "{err}");
    }

    // --- The shipped files ---------------------------------------------------

    #[test]
    fn the_shipped_dirt_block_loads() {
        let model = load_file(DIRT);
        assert_eq!(model.quads.len(), 6, "a cube has six faces");
        assert_eq!(model.texture_paths, vec!["assets/textures/dirt.png"]);
        assert_eq!(model.textures[0].size, [256, 256]);
    }

    #[test]
    fn the_shipped_grass_block_loads_all_four_of_its_textures() {
        let model = load_file(GRASS);
        let mut paths = model.texture_paths.clone();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "assets/textures/dirt.png",
                "assets/textures/grass_block_side.png",
                "assets/textures/grass_block_side_overlay.png",
                "assets/textures/grass_top.png",
            ],
            "the single-texture Model type could never have carried this"
        );
        assert!(
            model.quads.iter().any(|q| q.tinted),
            "grass should have at least one tinted face"
        );
    }
}
