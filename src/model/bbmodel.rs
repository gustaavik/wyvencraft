//! Blockbench (`.bbmodel`) loader.
//!
//! A bbmodel is a list of cuboid `elements`, each with `from`/`to` corners in
//! sixteenths of a block, an optional rotation about its `origin`, and six faces
//! carrying explicit UV rects in texture-resolution space. Groups in the
//! `outliner` nest elements and add their own pivoted rotation.
//!
//! Every convention below (which box corner each UV corner lands on, which way a
//! face `rotation` turns, the sign of an element rotation) was verified against
//! Blockbench's own glTF export of the same model — the two must agree, and the
//! test module keeps them honest.

use std::collections::HashMap;

use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::Deserialize;

use crate::content::ContentSource;
use crate::render::texture::decode_png;

use super::datauri::{self, Uri};
use super::mesh::ModelMesh;
use super::{Model, ModelLoader, resolve_sibling};

/// Blockbench authors in sixteenths of a block.
const PIXELS_PER_BLOCK: f32 = 16.0;

pub struct BbmodelLoader;

impl ModelLoader for BbmodelLoader {
    fn extensions(&self) -> &'static [&'static str] {
        &["bbmodel"]
    }

    fn load(&self, bytes: &[u8], dir: &str, source: &dyn ContentSource) -> Result<Model, String> {
        let doc: Document =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid bbmodel JSON: {e}"))?;

        let by_uuid: HashMap<&str, &Element> = doc
            .elements
            .iter()
            .filter(|e| e.kind == "cube")
            .map(|e| (e.uuid.as_str(), e))
            .collect();
        let resolution = doc.resolution.unwrap_or(Resolution {
            width: PIXELS_PER_BLOCK,
            height: PIXELS_PER_BLOCK,
        });

        let mut mesh = ModelMesh::default();
        if doc.outliner.is_empty() {
            // No hierarchy declared: every element is a root.
            for element in doc.elements.iter().filter(|e| e.kind == "cube") {
                mesh.merge(element.build(resolution, Transform::IDENTITY));
            }
        } else {
            for node in &doc.outliner {
                walk(
                    node,
                    &by_uuid,
                    resolution,
                    Transform::IDENTITY,
                    0,
                    &mut mesh,
                )?;
            }
        }
        mesh.validate()?;

        let texture = doc.resolve_texture(dir, source)?;
        Model::new(mesh, texture)
    }
}

// --- bbmodel JSON schema (only the fields we read) --------------------------

#[derive(Deserialize)]
struct Document {
    resolution: Option<Resolution>,
    #[serde(default)]
    elements: Vec<Element>,
    #[serde(default)]
    outliner: Vec<OutlinerNode>,
    #[serde(default)]
    textures: Vec<TextureEntry>,
}

#[derive(Deserialize, Clone, Copy)]
struct Resolution {
    width: f32,
    height: f32,
}

#[derive(Deserialize)]
struct Element {
    #[serde(default)]
    uuid: String,
    #[serde(rename = "type", default = "default_kind")]
    kind: String,
    #[serde(default)]
    from: [f32; 3],
    #[serde(default)]
    to: [f32; 3],
    #[serde(default)]
    origin: [f32; 3],
    #[serde(default)]
    rotation: [f32; 3],
    /// Grows the cube by this many pixels on every side (Blockbench "inflate").
    #[serde(default)]
    inflate: f32,
    #[serde(default)]
    faces: HashMap<String, Face>,
}

fn default_kind() -> String {
    "cube".into()
}

#[derive(Deserialize)]
struct Face {
    /// `[u1, v1, u2, v2]` in texture-resolution space. The rect may be
    /// *reversed* (`u1 > u2`), which is how Blockbench encodes mirroring — so
    /// these are used as given, never normalised.
    uv: Option<[f32; 4]>,
    #[serde(default)]
    rotation: f32,
}

/// An outliner entry: either a bare element UUID, or a group with children.
#[derive(Deserialize)]
#[serde(untagged)]
enum OutlinerNode {
    Element(String),
    Group(Group),
}

#[derive(Deserialize)]
struct Group {
    #[serde(default)]
    origin: [f32; 3],
    #[serde(default)]
    rotation: [f32; 3],
    #[serde(default)]
    children: Vec<OutlinerNode>,
}

#[derive(Deserialize)]
struct TextureEntry {
    source: Option<String>,
    relative_path: Option<String>,
    name: Option<String>,
}

// --- Geometry ---------------------------------------------------------------

/// Accumulated pivoted rotations, in Blockbench pixel space.
///
/// A group or element rotation turns its subtree about its own `origin`, so each
/// level contributes `translate(pivot) · rotate · translate(-pivot)` and the
/// levels simply multiply. Keeping it as a matrix rather than hand-composing
/// quaternions and offsets is what makes nesting obviously right.
#[derive(Clone, Copy)]
struct Transform(Mat4);

impl Transform {
    const IDENTITY: Self = Self(Mat4::IDENTITY);

    /// Nest a rotation of `degrees` about `pivot` inside this one.
    fn then(self, degrees: [f32; 3], pivot: [f32; 3]) -> Self {
        if degrees == [0.0; 3] {
            return self;
        }
        let pivot = Vec3::from(pivot);
        let rotation = Mat4::from_quat(Quat::from_euler(
            EulerRot::XYZ,
            degrees[0].to_radians(),
            degrees[1].to_radians(),
            degrees[2].to_radians(),
        ));
        Self(self.0 * Mat4::from_translation(pivot) * rotation * Mat4::from_translation(-pivot))
    }

    fn apply(self, p: Vec3) -> Vec3 {
        self.0.transform_point3(p)
    }

    /// Rotations preserve length and angle, so normals need no inverse-transpose
    /// here — only the rotational part, applied as a direction.
    fn apply_normal(self, n: Vec3) -> Vec3 {
        self.0.transform_vector3(n).normalize_or_zero()
    }
}

fn walk(
    node: &OutlinerNode,
    by_uuid: &HashMap<&str, &Element>,
    resolution: Resolution,
    parent: Transform,
    depth: usize,
    out: &mut ModelMesh,
) -> Result<(), String> {
    const MAX_DEPTH: usize = 64;
    if depth > MAX_DEPTH {
        return Err(format!("outliner nested deeper than {MAX_DEPTH} levels"));
    }
    match node {
        OutlinerNode::Element(uuid) => {
            // A UUID with no matching cube is normal — meshes, locators and
            // nulls all live in the outliner and none of them are geometry.
            if let Some(element) = by_uuid.get(uuid.as_str()) {
                out.merge(element.build(resolution, parent));
            }
        }
        OutlinerNode::Group(group) => {
            let transform = parent.then(group.rotation, group.origin);
            for child in &group.children {
                walk(child, by_uuid, resolution, transform, depth + 1, out)?;
            }
        }
    }
    Ok(())
}

impl Element {
    fn build(&self, resolution: Resolution, parent: Transform) -> ModelMesh {
        let inflate = Vec3::splat(self.inflate);
        let a = Vec3::from(self.from);
        let b = Vec3::from(self.to);
        // Authors can drag a cube "inside out"; min/max keeps faces outward.
        let lo = a.min(b) - inflate;
        let hi = a.max(b) + inflate;
        let transform = parent.then(self.rotation, self.origin);

        let mut mesh = ModelMesh::default();
        for (name, dir) in FACES {
            let Some(face) = self.faces.get(name) else {
                continue;
            };
            let Some(rect) = face.uv else {
                continue;
            };
            let corners = face_corners(dir, lo, hi);
            let uvs = face_uvs(rect, face.rotation, resolution);
            let base = mesh.positions.len() as u32;
            for i in 0..4 {
                mesh.positions
                    .push(transform.apply(corners[i]) / PIXELS_PER_BLOCK);
                mesh.normals.push(transform.apply_normal(dir.normal()));
                mesh.uvs.push(uvs[i]);
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        mesh
    }
}

/// The six cube faces, in Blockbench's naming.
const FACES: [(&str, FaceDir); 6] = [
    ("north", FaceDir::North),
    ("south", FaceDir::South),
    ("east", FaceDir::East),
    ("west", FaceDir::West),
    ("up", FaceDir::Up),
    ("down", FaceDir::Down),
];

#[derive(Clone, Copy)]
enum FaceDir {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

impl FaceDir {
    fn normal(self) -> Vec3 {
        match self {
            FaceDir::North => Vec3::NEG_Z,
            FaceDir::South => Vec3::Z,
            FaceDir::East => Vec3::X,
            FaceDir::West => Vec3::NEG_X,
            FaceDir::Up => Vec3::Y,
            FaceDir::Down => Vec3::NEG_Y,
        }
    }
}

/// The four corners of a face, in the cycle order that the UV rect's corners
/// `(u1,v1) → (u2,v1) → (u2,v2) → (u1,v2)` map onto at face rotation 0.
fn face_corners(dir: FaceDir, lo: Vec3, hi: Vec3) -> [Vec3; 4] {
    let p = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
    match dir {
        FaceDir::North => [
            p(hi.x, hi.y, lo.z),
            p(lo.x, hi.y, lo.z),
            p(lo.x, lo.y, lo.z),
            p(hi.x, lo.y, lo.z),
        ],
        FaceDir::South => [
            p(lo.x, hi.y, hi.z),
            p(hi.x, hi.y, hi.z),
            p(hi.x, lo.y, hi.z),
            p(lo.x, lo.y, hi.z),
        ],
        FaceDir::East => [
            p(hi.x, hi.y, hi.z),
            p(hi.x, hi.y, lo.z),
            p(hi.x, lo.y, lo.z),
            p(hi.x, lo.y, hi.z),
        ],
        FaceDir::West => [
            p(lo.x, hi.y, lo.z),
            p(lo.x, hi.y, hi.z),
            p(lo.x, lo.y, hi.z),
            p(lo.x, lo.y, lo.z),
        ],
        FaceDir::Up => [
            p(lo.x, hi.y, lo.z),
            p(hi.x, hi.y, lo.z),
            p(hi.x, hi.y, hi.z),
            p(lo.x, hi.y, hi.z),
        ],
        FaceDir::Down => [
            p(lo.x, lo.y, hi.z),
            p(hi.x, lo.y, hi.z),
            p(hi.x, lo.y, lo.z),
            p(lo.x, lo.y, lo.z),
        ],
    }
}

/// Normalised UVs for the four face corners, honouring the face's `rotation`
/// (0/90/180/270, turning the texture on the face).
fn face_uvs(rect: [f32; 4], rotation: f32, resolution: Resolution) -> [[f32; 2]; 4] {
    let [u1, v1, u2, v2] = rect;
    let corners = [[u1, v1], [u2, v1], [u2, v2], [u1, v2]];
    // Verified against the glTF export: 270° shifts the mapping by one step.
    let steps = (4 - (rotation.rem_euclid(360.0) / 90.0).round() as usize % 4) % 4;
    std::array::from_fn(|i| {
        let [u, v] = corners[(i + steps) % 4];
        [u / resolution.width, v / resolution.height]
    })
}

// --- Texture ----------------------------------------------------------------

impl Document {
    fn resolve_texture(
        &self,
        dir: &str,
        source: &dyn ContentSource,
    ) -> Result<crate::render::Rgba8, String> {
        let entry = self.textures.first().ok_or("bbmodel has no textures")?;
        if self.textures.len() > 1 {
            log::warn!(
                "bbmodel has {} textures; all geometry will sample the first",
                self.textures.len()
            );
        }
        // `source` is normally an inline data: URI (Blockbench embeds the PNG),
        // which is why a .bbmodel is self-contained. The absolute `path` field
        // is the author's machine and is deliberately ignored.
        let bytes = match entry.source.as_deref() {
            Some(uri) => match datauri::parse(uri)? {
                Uri::Inline(bytes) => bytes,
                Uri::Relative(path) => read_sibling(dir, path, source)?,
            },
            None => {
                let name = entry
                    .relative_path
                    .as_deref()
                    .or(entry.name.as_deref())
                    .ok_or("bbmodel texture has neither embedded data nor a file name")?;
                read_sibling(dir, name, source)?
            }
        };
        decode_png(&bytes).map_err(|e| format!("model texture: {e}"))
    }
}

fn read_sibling(dir: &str, path: &str, source: &dyn ContentSource) -> Result<Vec<u8>, String> {
    let full = resolve_sibling(dir, path);
    source
        .read_bytes(&full)
        .map_err(|e| format!("could not read {full}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_rotation_shifts_the_uv_corners() {
        let res = Resolution {
            width: 32.0,
            height: 32.0,
        };
        let rect = [16.0, 18.0, 18.0, 15.0];
        let none = face_uvs(rect, 0.0, res);
        assert_eq!(
            none[0],
            [16.0 / 32.0, 18.0 / 32.0],
            "u1,v1 lands on corner 0"
        );

        // Verified against element 7 of assets/models/vine_sword.gltf: at 270°
        // the first corner takes (u2, v1).
        let turned = face_uvs(rect, 270.0, res);
        assert_eq!(turned[0], [18.0 / 32.0, 18.0 / 32.0]);
        assert_eq!(turned[1], [18.0 / 32.0, 15.0 / 32.0]);
        assert_eq!(turned[2], [16.0 / 32.0, 15.0 / 32.0]);
        assert_eq!(turned[3], [16.0 / 32.0, 18.0 / 32.0]);

        // A full turn is the identity, and 360 folds back to 0.
        assert_eq!(face_uvs(rect, 360.0, res), none);
    }

    #[test]
    fn reversed_uv_rects_are_preserved_as_mirroring() {
        let res = Resolution {
            width: 32.0,
            height: 32.0,
        };
        // u1 > u2 is how Blockbench encodes a mirrored face; normalising it
        // would silently un-mirror the texture.
        let uvs = face_uvs([15.0, 6.0, 14.0, 15.0], 0.0, res);
        assert!(uvs[0][0] > uvs[1][0], "u must still run backwards");
    }

    #[test]
    fn face_corners_share_the_boxs_extents() {
        let lo = Vec3::new(-1.0, -2.0, -3.0);
        let hi = Vec3::new(1.0, 2.0, 3.0);
        for (_, dir) in FACES {
            let corners = face_corners(dir, lo, hi);
            for c in corners {
                assert!(
                    c.cmpge(lo).all() && c.cmple(hi).all(),
                    "{c} outside the box"
                );
            }
            // A face is planar: it pins exactly one axis to one extreme.
            let pinned = (0..3)
                .filter(|&i| corners.iter().all(|c| c[i] == corners[0][i]))
                .count();
            assert_eq!(pinned, 1, "a face must be flat in exactly one axis");
        }
    }
}
