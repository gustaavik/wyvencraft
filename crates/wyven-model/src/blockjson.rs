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
//! overlay, bottom), where [`super::Model`] carries exactly one, and a
//! `display` block saying where the model sits in each context it is drawn in.
//! So this is not itself a [`super::ModelLoader`]: it produces a different
//! shape, and the chunk mesher wants the `cullface`/`tintindex` a `Model` has
//! nowhere to put. [`super::javamodel`] is the adapter that turns what comes
//! out of here into a `Model` for the *item* path, by packing the textures into
//! one image; that is what claims `.json` in the registry, and blocks still
//! come through here directly.
//!
//! Everything about how a UV rect maps onto a box corner is shared verbatim
//! with [`super::bbmodel`], so the two formats cannot disagree about a face
//! that was authored once and exported twice.
//!
//! Boundaries: pure. Geometry comes out in block-local `0..1` space with no
//! notion of atlas layers, block ids or neighbours — turning that into
//! something the chunk mesher can emit is the game's block-model baker.

use std::collections::HashMap;

use glam::Vec3;
use serde::Deserialize;

use wyven_assets::AssetSource as ContentSource;
use wyven_assets::Rgba8;
use wyven_assets::decode_png;
use wyven_core::Direction;

use super::bbmodel::{
    FACES, FaceDir, PIXELS_PER_BLOCK, Resolution, Transform, face_corners, face_uvs,
};
use super::display::DisplayTransforms;
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
    /// From `tintindex`: which biome colour to multiply into this face, instead
    /// of drawing the texture's own. Minecraft's index selects among tint
    /// sources — `0` grass, `1` foliage — and `None` here is its `-1`.
    pub tint: Option<u8>,
    /// A second texture drawn *over* [`Self::texture`], alpha-blended, with its
    /// own tint in [`Self::overlay_tint`].
    ///
    /// This is how a face that Minecraft would author as two coincident quads
    /// on separate render layers — the grass block's dirt side and its tinted
    /// grass overlay — becomes one quad here. Merging them is not an
    /// optimisation: two coplanar quads have no dependable depth order wherever
    /// the depth buffer cannot separate them, which is what made distant grass
    /// crawl as the camera turned. It also upgrades the overlay from the
    /// fragment shader's hard alpha test to a blend, so its edge filters down
    /// smoothly with the mip chain instead of snapping on and off.
    pub overlay: Option<usize>,
    /// `tintindex` of [`Self::overlay`], independent of [`Self::tint`] — the
    /// grass block tints its overlay and not the side beneath it.
    pub overlay_tint: Option<u8>,
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
    /// The model's `display` block, empty when it declares none. A block in a
    /// chunk has nowhere to put this; an item drawn from the same file does.
    pub display: DisplayTransforms,
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

    // `item/generated` is the one parent that carries no geometry to inherit —
    // it says "my shape is my sprite", which is something we can honour without
    // resolving a parent chain. Anything else still warns.
    let generate = super::generated::claims(&doc.parent) && doc.elements.is_empty();
    if !generate && !doc.parent.trim().is_empty() {
        log::warn!(
            "block model declares parent {:?}; model inheritance is not supported, \
             the model must be self-contained",
            doc.parent
        );
    }

    let mut textures = TextureSet::default();
    let mut quads = Vec::new();
    let mut planes: HashMap<[i32; 4], Vec<usize>> = HashMap::new();

    // A generated model's texture has to be resolved *before* its elements
    // exist, because the alpha is what decides where the geometry goes.
    let synthesized;
    let elements: &[Element] = if generate {
        let layer = format!("#{}", super::generated::LAYER);
        let index = textures
            .resolve(&layer, &doc.textures, dir, source)?
            .ok_or_else(|| {
                format!(
                    "generated model names no {:?} texture to take its shape from",
                    super::generated::LAYER
                )
            })?;
        synthesized = super::generated::elements(&textures.images[index])?;
        &synthesized
    } else {
        &doc.elements
    };

    for element in elements {
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

    // A generated stub that says nothing about placement takes Minecraft's
    // standard one, which is the same for every flat sprite. One that does
    // declare a `display` block keeps it.
    let display = match generate && doc.display.is_empty() {
        true => super::generated::default_display(),
        false => doc.display,
    };

    Ok(BlockJsonModel {
        quads,
        textures: textures.images,
        texture_paths: textures.paths,
        display,
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
    /// Where the model sits in each context it can be drawn in. Meaningless to
    /// a block in a chunk, which is why this rides through untouched for the
    /// item loader to pick up.
    #[serde(default)]
    display: DisplayTransforms,
}

#[derive(Debug, Deserialize)]
pub struct Element {
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
pub struct Face {
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

impl Element {
    /// A box carrying exactly one textured face, built in code rather than
    /// parsed. [`super::generated`] is the only caller: it derives geometry from
    /// a sprite's alpha and hands it back through this door so the result still
    /// goes through [`Element::build`], and cannot drift from an authored model.
    pub(crate) fn synthetic(from: [f32; 3], to: [f32; 3], face: &str, spec: Face) -> Self {
        Element {
            from,
            to,
            rotation: None,
            shade: true,
            faces: HashMap::from([(face.to_string(), spec)]),
        }
    }
}

impl Face {
    /// An untinted, unculled face naming the sole texture of a generated model.
    pub(crate) fn synthetic(uv: [f32; 4]) -> Self {
        Face {
            uv: Some(uv),
            texture: Some(format!("#{}", super::generated::LAYER)),
            cullface: None,
            tintindex: no_tint(),
            rotation: 0.0,
        }
    }
}

// --- Geometry ---------------------------------------------------------------

impl Element {
    fn build(
        &self,
        refs: &HashMap<String, String>,
        dir: &str,
        source: &dyn ContentSource,
        textures: &mut TextureSet,
        planes: &mut HashMap<[i32; 4], Vec<usize>>,
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
            let Some(texture) = textures.resolve(reference, refs, dir, source)? else {
                continue;
            };

            let corners = face_corners(face_dir, lo, hi);
            // A zero-thickness element — the two crossed planes a flower is
            // made of — still declares six faces, four of which collapse to
            // nothing. Emitting them would cost vertices and draw no pixels.
            if face_area(&corners) <= AREA_EPSILON {
                continue;
            }
            let uvs = face_uvs(rect, face.rotation, UV_RESOLUTION);
            let normal = transform.apply_normal(face_dir.normal());

            let positions: [Vec3; 4] =
                std::array::from_fn(|i| transform.apply(corners[i]) / PIXELS_PER_BLOCK);

            let tint = u8::try_from(face.tintindex).ok();
            let shade = if self.shade {
                super::mesh::shade(normal)
            } else {
                1.0
            };
            let cull = match face.cullface.as_deref() {
                Some(cull) => {
                    let cull = FaceDir::from_name(cull)
                        .ok_or_else(|| format!("unknown cullface {cull:?}"))?;
                    // A face lying flat on a cell boundary can only ever be
                    // hidden by the neighbour it faces, so a `cullface` naming
                    // any other direction is an authoring slip — and a costly
                    // one, since the block then vanishes whenever something
                    // solid sits in that unrelated direction.
                    if cull != face_dir && covers_own_cell_face(face_dir, lo, hi) {
                        log::warn!(
                            "block model: the {name} face fills its side of the cell but \
                             culls against {:?}; it should cull against {name}",
                            cull.name()
                        );
                    }
                    Some(cull.direction())
                }
                None => None,
            };

            // Faces that land on the same plane need separating, one way or
            // another. A face that covers an earlier one *exactly* — same
            // corners, same UVs, same culling and shading, differing only in
            // texture and tint — becomes that quad's overlay and emits no
            // geometry at all. That is the grass block, and it is the case worth
            // catching: coincident geometry has no dependable depth order.
            let coplanar = planes.entry(plane_key(normal, positions[0])).or_default();
            if let Some(&under) = coplanar.iter().find(|&&i| {
                let q: &BlockQuad = &out[i];
                q.overlay.is_none()
                    && q.positions == positions
                    && q.uvs == uvs
                    && q.cull == cull
                    && q.shade == shade
            }) {
                out[under].overlay = Some(texture);
                out[under].overlay_tint = tint;
                continue;
            }

            // Otherwise the faces genuinely differ, and the later one is pushed
            // clear along its normal so it wins the depth test rather than
            // fighting for it. One nudge per quad already standing on the plane.
            let nudge = normal * (coplanar.len() as f32 * COPLANAR_EPSILON);
            coplanar.push(out.len());

            out.push(BlockQuad {
                positions: positions.map(|p| p + nudge),
                normal,
                uvs,
                texture,
                cull,
                tint,
                overlay: None,
                overlay_tint: None,
                shade,
            });
        }
        Ok(())
    }
}

/// Below this a face has no area worth drawing, in square pixels of the 0..16
/// authoring grid.
const AREA_EPSILON: f32 = 1e-4;

/// Area of an axis-aligned box face, from its untransformed corners. A rotation
/// preserves area, so this is measured before the transform for simplicity.
fn face_area(corners: &[Vec3; 4]) -> f32 {
    let lo = corners
        .iter()
        .fold(Vec3::splat(f32::INFINITY), |m, c| m.min(*c));
    let hi = corners
        .iter()
        .fold(Vec3::splat(f32::NEG_INFINITY), |m, c| m.max(*c));
    let extent = hi - lo;
    // Exactly one component is zero for a box face; the other two are its sides.
    let mut sides = [extent.x, extent.y, extent.z];
    sides.sort_by(|a, b| a.total_cmp(b));
    sides[1] * sides[2]
}

/// Whether this face lies flat on its own side of the cell and covers all of it
/// — the only case where a `cullface` can be checked against the face itself.
fn covers_own_cell_face(dir: FaceDir, lo: Vec3, hi: Vec3) -> bool {
    const CELL: f32 = PIXELS_PER_BLOCK;
    let (axis, plane) = match dir {
        FaceDir::West => (0, lo.x),
        FaceDir::East => (0, hi.x),
        FaceDir::Down => (1, lo.y),
        FaceDir::Up => (1, hi.y),
        FaceDir::North => (2, lo.z),
        FaceDir::South => (2, hi.z),
    };
    let on_boundary = match dir {
        FaceDir::West | FaceDir::Down | FaceDir::North => plane == 0.0,
        FaceDir::East | FaceDir::Up | FaceDir::South => plane == CELL,
    };
    on_boundary
        && (0..3)
            .filter(|&a| a != axis)
            .all(|a| lo[a] == 0.0 && hi[a] == CELL)
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
    /// The index `reference` resolves to, or `None` when the face has no
    /// texture at all and should simply not be drawn.
    fn resolve(
        &mut self,
        reference: &str,
        refs: &HashMap<String, String>,
        dir: &str,
        source: &dyn ContentSource,
    ) -> Result<Option<usize>, String> {
        let Some(path) = texture_path(reference, refs, dir)? else {
            return Ok(None);
        };
        if let Some(&index) = self.by_path.get(&path) {
            return Ok(Some(index));
        }
        // Blockbench writes the reference relative to the exported file when the
        // texture was linked from disk, and as a bare name when it was not.
        // Both mean the same thing here, so the relative form is a hint rather
        // than a contract: fall back to this model's own texture directory
        // before giving up.
        let (resolved, bytes) = match source.read_bytes(&path) {
            Ok(bytes) => (path.clone(), bytes),
            Err(first) => {
                let fallback = texture_dir_path(dir, &path);
                match source.read_bytes(&fallback) {
                    Ok(bytes) => {
                        log::debug!("texture {path} not found; using {fallback}");
                        (fallback, bytes)
                    }
                    Err(_) => return Err(format!("could not read texture {path}: {first}")),
                }
            }
        };
        // Remember the reference we were *given* as well as where it landed, or
        // every face naming the same bare texture would take the fallback path
        // again and decode its own copy.
        if let Some(&index) = self.by_path.get(&resolved) {
            self.by_path.insert(path, index);
            return Ok(Some(index));
        }
        let image = decode_png(&bytes).map_err(|e| format!("texture {resolved}: {e}"))?;
        let index = self.images.len();
        self.images.push(image);
        self.paths.push(resolved.clone());
        self.by_path.insert(resolved, index);
        self.by_path.insert(path, index);
        Ok(Some(index))
    }
}

/// The same texture, looked for in the model's own texture directory instead.
///
/// Art sits beside the models that name it, one tree mirroring the other:
/// `assets/models/items/` is textured out of `assets/textures/items/`. So the
/// fallback is the model's own directory with its `models` segment swapped for
/// `textures` — which is how a bare `"dirt"` still finds its PNG without this
/// crate having to know that Wyvencraft keeps blocks in one folder and items in
/// another. A directory with no `models` segment has no mirror, and the
/// reference is left to fail on its own terms.
fn texture_dir_path(dir: &str, path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let mirrored: Vec<&str> = dir
        .split('/')
        .map(|segment| {
            if segment == "models" {
                "textures"
            } else {
                segment
            }
        })
        .collect();
    format!("{}/{name}", mirrored.join("/"))
}

/// Follow a `"#key"` reference through the document's texture map to a file
/// path, then make it `assets/`-relative.
///
/// Blockbench writes `"../../textures/blocks/dirt"` — relative to the exported file, and
/// without the extension.
fn texture_path(
    reference: &str,
    refs: &HashMap<String, String>,
    dir: &str,
) -> Result<Option<String>, String> {
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
            return Ok(Some(resolve_sibling(dir, &path)));
        };
        let Some(next) = refs.get(key) else {
            // `#missing` is Minecraft's marker for a face with no texture, and
            // Blockbench writes it for faces the author cleared. Dropping just
            // that face is the useful reading; failing the model over it would
            // cost the block its whole appearance.
            log::debug!("texture {reference:?} is undefined; that face is not drawn");
            return Ok(None);
        };
        value = next;
    }
    Err(format!(
        "texture {reference:?} still unresolved after {MAX_TEXTURE_INDIRECTION} hops (a loop?)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyven_assets::{AssetSource, FsSource, MapSource};

    /// `assets/` sits at the workspace root; a test runner starts in the
    /// crate directory, so the shipped-export fixtures need it named.
    fn assets() -> FsSource {
        FsSource::rooted(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
    }

    const DIRT: &str = "assets/models/blocks/dirt.json";
    const GRASS: &str = "assets/models/blocks/grass.json";

    fn load_file(path: &str) -> BlockJsonModel {
        let source = assets();
        let bytes = source
            .read_bytes(path)
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        let dir = path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        load(&bytes, dir, &source).unwrap_or_else(|e| panic!("{path}: {e}"))
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
            .with_bytes("assets/models/blocks/t.json", json.as_bytes().to_vec())
            .with_bytes("assets/textures/blocks/a.png", tiny_png())
            .with_bytes("assets/textures/blocks/b.png", tiny_png())
    }

    fn load_inline(json: &str) -> Result<BlockJsonModel, String> {
        load(json.as_bytes(), "assets/models/blocks", &source_with(json))
    }

    const FULL_CUBE: &str = r##"{
        "textures": { "0": "../../textures/blocks/a" },
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
        assert_eq!(model.texture_paths, vec!["assets/textures/blocks/a.png"]);
    }

    #[test]
    fn a_texture_key_may_point_at_another_key() {
        let json = r##"{
            "textures": { "all": "../../textures/blocks/a", "0": "#all" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.texture_paths, vec!["assets/textures/blocks/a.png"]);
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

    /// `#missing` is what Minecraft — and Blockbench, for a face whose texture
    /// was cleared — writes when a face has no texture. Losing the whole model
    /// over one such face would cost the block its entire appearance.
    #[test]
    fn an_undefined_texture_key_drops_only_its_own_face() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": {
                    "up":   {"uv": [0,0,16,16], "texture": "#0"},
                    "down": {"uv": [0,0,16,16], "texture": "#missing"}
                } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.quads.len(), 1, "only the textured face survives");
        assert!(model.quads[0].normal.y > 0.5, "and it is the up face");
    }

    #[test]
    fn a_missing_texture_file_fails_the_model() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/gone" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let err = load_inline(json).expect_err("should fail");
        assert!(err.contains("could not read texture"), "{err}");
    }

    /// Two elements occupying the same box is how Minecraft spells the grass
    /// block's tinted side overlay. Coincident geometry has no dependable depth
    /// order, so the covering face must not become a second quad at all.
    #[test]
    fn a_face_exactly_covering_another_becomes_its_overlay() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/a", "1": "../../textures/blocks/b" },
            "elements": [
                { "from": [0,0,0], "to": [16,16,16],
                  "faces": { "north": {"uv": [0,0,16,16], "texture": "#0"} } },
                { "from": [0,0,0], "to": [16,16,16],
                  "faces": { "north": {"uv": [0,0,16,16], "texture": "#1", "tintindex": 0} } }
            ]
        }"##;
        let model = load_inline(json).expect("loads");

        assert_eq!(model.quads.len(), 1, "the two faces merged into one quad");
        let quad = &model.quads[0];
        assert_eq!(quad.texture, 0, "the earlier face is the base");
        assert_eq!(quad.overlay, Some(1), "the later face is the overlay");
        assert_eq!(quad.tint, None, "the base keeps its own (absent) tint");
        assert_eq!(quad.overlay_tint, Some(0), "and the overlay keeps its own");
        for p in quad.positions {
            assert!(
                p.min_element() >= 0.0 && p.max_element() <= 1.0,
                "{p} left the cell — a merged face needs no nudge"
            );
        }
    }

    /// The fallback, for coplanar faces that are *not* interchangeable. Here the
    /// second face covers only half the first, so it cannot be folded in and has
    /// to be pushed clear to win the depth test instead.
    #[test]
    fn a_partly_coplanar_face_is_nudged_clear_of_the_one_it_overlaps() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/a", "1": "../../textures/blocks/b" },
            "elements": [
                { "from": [0,0,0], "to": [16,16,16],
                  "faces": { "north": {"uv": [0,0,16,16], "texture": "#0"} } },
                { "from": [0,0,0], "to": [8,16,16],
                  "faces": { "north": {"uv": [0,0,8,16], "texture": "#1"} } }
            ]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.quads.len(), 2, "no merge — the faces differ");
        assert!(model.quads.iter().all(|q| q.overlay.is_none()));

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
            "textures": { "0": "../../textures/blocks/a" },
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
            "textures": { "0": "../../textures/blocks/a" },
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
            "textures": { "0": "../../textures/blocks/a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16], "shade": false,
                "faces": { "north": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.quads[0].shade, 1.0);
    }

    #[test]
    fn tintindex_marks_a_face_biome_coloured() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/a" },
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
        assert_eq!(up.tint, Some(0));
        assert_eq!(down.tint, None, "an absent tintindex must not tint");
    }

    #[test]
    fn a_face_without_uv_derives_it_from_the_element() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/a" },
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
        let err =
            load(b"{ not json", "assets/models/blocks", &MapSource::new()).expect_err("bad json");
        assert!(err.contains("invalid block model JSON"), "{err}");
    }

    /// Blockbench writes a bare name when the texture was not linked from disk.
    /// It means the same thing as the relative form, so the loader looks in the
    /// one directory textures actually live in before giving up.
    #[test]
    fn a_bare_texture_name_falls_back_to_the_textures_directory() {
        let json = r##"{
            "textures": { "0": "a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#0"} } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.texture_paths, vec!["assets/textures/blocks/a.png"]);
    }

    /// Every face naming the same bare texture must share one entry — the
    /// fallback path is easy to accidentally take once per face.
    #[test]
    fn a_bare_name_used_by_every_face_is_decoded_once() {
        let json = r##"{
            "textures": { "0": "a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": {
                    "north": {"uv": [0,0,16,16], "texture": "#0"},
                    "east":  {"uv": [0,0,16,16], "texture": "#0"},
                    "south": {"uv": [0,0,16,16], "texture": "#0"},
                    "west":  {"uv": [0,0,16,16], "texture": "#0"},
                    "up":    {"uv": [0,0,16,16], "texture": "#0"},
                    "down":  {"uv": [0,0,16,16], "texture": "#0"}
                } }]
        }"##;
        let model = load_inline(json).expect("loads");
        assert_eq!(model.quads.len(), 6);
        assert_eq!(model.textures.len(), 1, "one PNG, one entry");
    }

    /// A flower is two zero-thickness planes; four of each element's six faces
    /// collapse to nothing and would draw no pixels at all.
    #[test]
    fn a_zero_area_face_is_not_emitted() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/a" },
            "elements": [{
                "from": [8, 0, 0], "to": [8, 16, 16],
                "faces": {
                    "north": {"uv": [0,0,16,16], "texture": "#0"},
                    "east":  {"uv": [0,0,16,16], "texture": "#0"},
                    "west":  {"uv": [0,0,16,16], "texture": "#0"},
                    "up":    {"uv": [0,0,16,16], "texture": "#0"}
                }
            }]
        }"##;
        let model = load_inline(json).expect("loads");
        // Only the two faces of the plane itself have area.
        assert_eq!(model.quads.len(), 2);
        for quad in &model.quads {
            assert!(
                quad.normal.x.abs() > 0.5,
                "normal {} is not the plane's",
                quad.normal
            );
        }
    }

    /// Minecraft's numbering, which the biome tint tables follow: 0 grass,
    /// 1 foliage.
    #[test]
    fn the_tint_index_is_carried_through_verbatim() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/a" },
            "elements": [{ "from": [0,0,0], "to": [16,16,16],
                "faces": {
                    "up":    {"uv": [0,0,16,16], "texture": "#0", "tintindex": 0},
                    "north": {"uv": [0,0,16,16], "texture": "#0", "tintindex": 1},
                    "down":  {"uv": [0,0,16,16], "texture": "#0"}
                } }]
        }"##;
        let model = load_inline(json).expect("loads");
        let tint_of =
            |pick: fn(&BlockQuad) -> bool| model.quads.iter().find(|q| pick(q)).expect("face").tint;
        assert_eq!(tint_of(|q| q.normal.y > 0.5), Some(0), "grass");
        assert_eq!(tint_of(|q| q.normal.z < -0.5), Some(1), "foliage");
        assert_eq!(tint_of(|q| q.normal.y < -0.5), None, "untinted");
    }

    /// The shipped copper and iron ores were exported with `cullface: "west"`
    /// on every face — a slip that would have hidden the whole block whenever
    /// something solid sat to its west.
    #[test]
    fn a_full_cube_face_culls_against_its_own_direction() {
        for path in [
            "assets/models/blocks/copper_ore.json",
            "assets/models/blocks/iron_ore.json",
        ] {
            let model = load_file(path);
            assert_eq!(model.quads.len(), 6, "{path}");
            for quad in &model.quads {
                let cull = quad.cull.expect("every face of a cube culls");
                assert!(
                    cull.normal().dot(quad.normal) > 0.99,
                    "{path}: a face pointing {} culls against {cull:?}",
                    quad.normal
                );
            }
        }
    }

    // --- The shipped files ---------------------------------------------------

    #[test]
    fn the_shipped_dirt_block_loads() {
        let model = load_file(DIRT);
        assert_eq!(model.quads.len(), 6, "a cube has six faces");
        assert_eq!(model.texture_paths, vec!["assets/textures/blocks/dirt.png"]);
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
                "assets/textures/blocks/dirt.png",
                "assets/textures/blocks/grass_side.png",
                "assets/textures/blocks/grass_side_overlay.png",
                "assets/textures/blocks/grass_top.png",
            ],
            "the single-texture Model type could never have carried this"
        );
        assert!(
            model.quads.iter().any(|q| q.tint.is_some()),
            "grass should have at least one tinted face"
        );
    }
}
