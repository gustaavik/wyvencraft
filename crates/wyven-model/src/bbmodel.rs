//! Blockbench (`.bbmodel`) loader.
//!
//! A bbmodel is a list of cuboid `elements`, each with `from`/`to` corners in
//! sixteenths of a block, an optional rotation about its `origin`, and six faces
//! carrying explicit UV rects in texture-resolution space. Groups in the
//! `outliner` nest elements and add their own pivoted rotation.
//!
//! Every convention below (which box corner each UV corner lands on, which way a
//! face `rotation` turns, the sign of an element rotation) was originally
//! derived by diffing this loader against Blockbench's own glTF export of the
//! same model, vertex for vertex.
//!
//! **That cross-check no longer runs.** The glTF export it compared against was
//! removed from `assets/`, and with it the test that asserted the two agreed. The
//! conventions here are still correct — they are what the shipped models are
//! authored against — but nothing mechanically pins them any more, so a change to
//! corner order or UV-rotation direction will not fail a test. Re-adding a
//! second export of one model, and a test that the two loaders agree on it, is
//! the way to get that guarantee back.
//!
//! The walk over the outliner also *keeps* what it discovers — which joint owns
//! which vertices, and where it pivots — as a [`Rig`](super::rig::Rig), and reads
//! the file's `animations` into [`Clip`]s. Both are optional: a file with no
//! groups and no animations loads exactly as it always did, and the rest-pose
//! bake of a rigged model is vertex-for-vertex the flat mesh below.

use std::collections::HashMap;

use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::Deserialize;

use wyven_assets::AssetSource as ContentSource;
use wyven_assets::decode_png;
use wyven_core::Direction;

use super::clip::{Channel, Clip, Interpolation, Keyframe, LoopMode, Track};
use super::datauri::{self, Uri};
use super::mesh::ModelMesh;
use super::rig::{BoneId, RigBuilder};
use super::{Model, ModelLoader, resolve_sibling};

/// Blockbench authors in sixteenths of a block.
pub(super) const PIXELS_PER_BLOCK: f32 = 16.0;

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
        // Blockbench 5.0 moved group definitions out of the outliner into a
        // table of their own, leaving the node with nothing but a uuid.
        let group_defs: HashMap<&str, &GroupDef> =
            doc.groups.iter().map(|g| (g.uuid.as_str(), g)).collect();
        let resolution = doc.resolution.unwrap_or(Resolution {
            width: PIXELS_PER_BLOCK,
            height: PIXELS_PER_BLOCK,
        });

        let mut mesh = ModelMesh::default();
        let mut walk = Walk {
            by_uuid: &by_uuid,
            group_defs: &group_defs,
            resolution,
            rig: RigBuilder::new(),
            bones: HashMap::new(),
        };
        if doc.outliner.is_empty() {
            // No hierarchy declared: every element is a root.
            for element in doc.elements.iter().filter(|e| e.kind == "cube") {
                mesh.merge(element.build(resolution, Transform::IDENTITY));
            }
        } else {
            for node in &doc.outliner {
                walk.visit(node, Transform::IDENTITY, None, 0, &mut mesh)?;
            }
        }
        mesh.validate()?;

        let Walk { rig, bones, .. } = walk;
        let rig = (!rig.is_empty()).then(|| rig.build(doc.clips(&bones)));

        let texture = doc.resolve_texture(dir, source)?;
        Model::new(mesh, texture).map(|model| model.with_rig(rig))
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
    /// Blockbench 5.0's group table, keyed by uuid. Empty in older files, which
    /// inline the same fields on the outliner node instead.
    #[serde(default)]
    groups: Vec<GroupDef>,
    #[serde(default)]
    textures: Vec<TextureEntry>,
    #[serde(default)]
    animations: Vec<AnimationDef>,
}

#[derive(Deserialize, Clone, Copy)]
pub(super) struct Resolution {
    pub(super) width: f32,
    pub(super) height: f32,
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

/// A group as the outliner spells it.
///
/// Every identifying field is optional because two Blockbench layouts have to
/// work: up to 4.x the node carries `name`/`origin`/`rotation` itself, and from
/// 5.0 it carries only `uuid` and the rest lives in the document's `groups`
/// table. Reading them as `Option` rather than `#[serde(default)]` is what
/// distinguishes "authored as zero" from "not here, look it up" — the older code
/// could not, so every 5.0 file loaded with all its pivots silently at the
/// origin.
#[derive(Deserialize)]
struct Group {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    origin: Option<[f32; 3]>,
    #[serde(default)]
    rotation: Option<[f32; 3]>,
    #[serde(default)]
    children: Vec<OutlinerNode>,
}

/// A group as the 5.0 `groups` table spells it.
#[derive(Deserialize)]
struct GroupDef {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    origin: Option<[f32; 3]>,
    #[serde(default)]
    rotation: Option<[f32; 3]>,
}

impl Group {
    /// This group's name, pivot and rest rotation: whatever the node declares,
    /// then whatever the `groups` table says, then nothing.
    fn resolve(&self, defs: &HashMap<&str, &GroupDef>) -> (String, [f32; 3], [f32; 3]) {
        let def = defs.get(self.uuid.as_str());
        let name = self
            .name
            .clone()
            .or_else(|| def.and_then(|d| d.name.clone()))
            .unwrap_or_default();
        let origin = self
            .origin
            .or_else(|| def.and_then(|d| d.origin))
            .unwrap_or([0.0; 3]);
        let rotation = self
            .rotation
            .or_else(|| def.and_then(|d| d.rotation))
            .unwrap_or([0.0; 3]);
        (name, origin, rotation)
    }
}

// --- Animation schema -------------------------------------------------------

#[derive(Deserialize)]
struct AnimationDef {
    #[serde(default)]
    name: String,
    /// `"loop"`, `"hold"` or `"once"`.
    #[serde(default, rename = "loop")]
    loop_mode: Option<String>,
    #[serde(default)]
    length: f32,
    /// Keyed by the uuid of the group each one drives.
    #[serde(default)]
    animators: HashMap<String, AnimatorDef>,
}

#[derive(Deserialize)]
struct AnimatorDef {
    #[serde(rename = "type", default)]
    kind: Option<String>,
    #[serde(default)]
    keyframes: Vec<KeyframeDef>,
}

#[derive(Deserialize)]
struct KeyframeDef {
    #[serde(default)]
    channel: String,
    #[serde(default)]
    time: f32,
    #[serde(default)]
    interpolation: Option<String>,
    #[serde(default)]
    data_points: Vec<DataPoint>,
}

/// One keyframe's value. Blockbench stores each component as a **string**,
/// because any of them may be a molang expression rather than a number — so
/// these are `Value` and parsed, not deserialized as `f32`.
#[derive(Deserialize, Default)]
struct DataPoint {
    #[serde(default)]
    x: serde_json::Value,
    #[serde(default)]
    y: serde_json::Value,
    #[serde(default)]
    z: serde_json::Value,
}

impl DataPoint {
    fn vec3(&self) -> Vec3 {
        Vec3::new(number(&self.x), number(&self.y), number(&self.z))
    }
}

/// A keyframe component, whether the file wrote it as a number or a string.
/// An expression this cannot evaluate contributes nothing rather than refusing
/// the whole file — one warning, and the rest of the animation still plays.
fn number(value: &serde_json::Value) -> f32 {
    match value {
        serde_json::Value::Number(n) => n.as_f64().unwrap_or(0.0) as f32,
        serde_json::Value::String(s) => s.trim().parse().unwrap_or_else(|_| {
            if !s.trim().is_empty() {
                log::warn!("bbmodel keyframe value {s:?} is not a number; using 0");
            }
            0.0
        }),
        _ => 0.0,
    }
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
pub(super) struct Transform(Mat4);

impl Transform {
    pub(super) const IDENTITY: Self = Self(Mat4::IDENTITY);

    /// Nest a rotation of `degrees` about `pivot` inside this one.
    pub(super) fn then(self, degrees: [f32; 3], pivot: [f32; 3]) -> Self {
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

    pub(super) fn apply(self, p: Vec3) -> Vec3 {
        self.0.transform_point3(p)
    }

    /// Rotations preserve length and angle, so normals need no inverse-transpose
    /// here — only the rotational part, applied as a direction.
    pub(super) fn apply_normal(self, n: Vec3) -> Vec3 {
        self.0.transform_vector3(n).normalize_or_zero()
    }
}

/// One pass over the outliner, accumulating both the flat mesh and the rig.
///
/// A struct rather than a pile of parameters because the walk now carries two
/// results and three lookups; the recursion itself is unchanged.
struct Walk<'a> {
    by_uuid: &'a HashMap<&'a str, &'a Element>,
    group_defs: &'a HashMap<&'a str, &'a GroupDef>,
    resolution: Resolution,
    rig: RigBuilder,
    /// Group uuid → bone, so an animator can find the bone it drives.
    bones: HashMap<String, BoneId>,
}

impl Walk<'_> {
    fn visit(
        &mut self,
        node: &OutlinerNode,
        parent: Transform,
        parent_bone: Option<BoneId>,
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
                if let Some(element) = self.by_uuid.get(uuid.as_str()) {
                    let start = out.positions.len() as u32;
                    out.merge(element.build(self.resolution, parent));
                    self.rig
                        .attach(parent_bone, start, out.positions.len() as u32);
                }
            }
            OutlinerNode::Group(group) => {
                let (name, origin, rotation) = group.resolve(self.group_defs);
                // The pivot is stored *after* the ancestors' rest rotations, in
                // blocks, because that is the space the baked vertices are in —
                // which is what makes an unanimated bone exactly the identity.
                let pivot = parent.apply(Vec3::from(origin)) / PIXELS_PER_BLOCK;
                let bone = self.rig.push_bone(&name, parent_bone, pivot);
                self.bones.insert(group.uuid.clone(), bone);

                let transform = parent.then(rotation, origin);
                for child in &group.children {
                    self.visit(child, transform, Some(bone), depth + 1, out)?;
                }
            }
        }
        Ok(())
    }
}

impl Document {
    /// Read `animations` into clips over the bones the walk found.
    ///
    /// Units are converted here, at the edge: degrees become radians and
    /// Blockbench pixels become blocks, so nothing downstream carries a
    /// Blockbench convention.
    fn clips(&self, bones: &HashMap<String, BoneId>) -> Vec<Clip> {
        self.animations
            .iter()
            .filter_map(|animation| {
                let tracks: Vec<Track> = animation
                    .animators
                    .iter()
                    .filter(|(_, a)| a.kind.as_deref().unwrap_or("bone") == "bone")
                    .filter_map(|(uuid, animator)| Some((*bones.get(uuid)?, animator)))
                    .flat_map(|(bone, animator)| tracks_for(bone, animator))
                    .collect();
                if tracks.is_empty() {
                    // An animation naming only bones this file has no geometry
                    // for would otherwise sit in the registry doing nothing.
                    log::warn!(
                        "bbmodel animation {:?} drives no known bone; ignoring it",
                        animation.name
                    );
                    return None;
                }
                Some(Clip::new(
                    animation.name.clone(),
                    animation.length,
                    loop_mode(animation.loop_mode.as_deref()),
                    tracks,
                ))
            })
            .collect()
    }
}

/// One track per channel this animator actually keyframes.
fn tracks_for(bone: BoneId, animator: &AnimatorDef) -> Vec<Track> {
    [
        (Channel::Rotation, "rotation"),
        (Channel::Position, "position"),
    ]
    .into_iter()
    .filter_map(|(channel, name)| {
        let keys: Vec<Keyframe> = animator
            .keyframes
            .iter()
            .filter(|k| k.channel == name)
            .filter_map(|k| {
                let point = k.data_points.first()?;
                let value = match channel {
                    // Degrees on the wire, radians everywhere after.
                    Channel::Rotation => Vec3::from(point.vec3().to_array().map(f32::to_radians)),
                    Channel::Position => point.vec3() / PIXELS_PER_BLOCK,
                };
                Some(Keyframe {
                    time: k.time,
                    value,
                    interpolation: interpolation(k.interpolation.as_deref()),
                })
            })
            .collect();
        (!keys.is_empty()).then(|| Track::new(bone, channel, keys))
    })
    .collect()
}

/// Blockbench's spelling of a loop mode. Anything unknown loops, which is what
/// an ambient animation almost always wants.
fn loop_mode(name: Option<&str>) -> LoopMode {
    match name {
        Some("once") => LoopMode::Once,
        Some("hold") => LoopMode::Hold,
        _ => LoopMode::Loop,
    }
}

/// Blockbench's spelling of an interpolation. `bezier` and anything else this
/// does not implement degrade to linear with a warning rather than refusing the
/// file — a clip playing slightly stiffly beats a model that will not load.
fn interpolation(name: Option<&str>) -> Interpolation {
    match name {
        None | Some("linear") => Interpolation::Linear,
        Some("catmullrom") | Some("smooth") => Interpolation::CatmullRom,
        Some("step") => Interpolation::Step,
        Some(other) => {
            log::warn!("bbmodel interpolation {other:?} is not supported; using linear");
            Interpolation::Linear
        }
    }
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

/// The six cube faces, in Blockbench's naming. Minecraft's Java block-model
/// JSON spells them identically, so [`super::blockjson`] shares this table.
pub(super) const FACES: [(&str, FaceDir); 6] = [
    ("north", FaceDir::North),
    ("south", FaceDir::South),
    ("east", FaceDir::East),
    ("west", FaceDir::West),
    ("up", FaceDir::Up),
    ("down", FaceDir::Down),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FaceDir {
    North,
    South,
    East,
    West,
    Up,
    Down,
}

impl FaceDir {
    pub(super) fn normal(self) -> Vec3 {
        self.direction().normal()
    }

    /// The engine axis-direction this face points along. Blockbench and
    /// Minecraft agree on the naming: north is `-Z`, south `+Z`, east `+X`,
    /// west `-X`.
    pub(super) fn direction(self) -> Direction {
        match self {
            FaceDir::North => Direction::NegZ,
            FaceDir::South => Direction::PosZ,
            FaceDir::East => Direction::PosX,
            FaceDir::West => Direction::NegX,
            FaceDir::Up => Direction::PosY,
            FaceDir::Down => Direction::NegY,
        }
    }

    /// This face's name, as Minecraft and Blockbench spell it.
    pub(super) fn name(self) -> &'static str {
        FACES
            .iter()
            .find(|&&(_, dir)| dir == self)
            .map(|&(name, _)| name)
            .unwrap_or("?")
    }

    /// Parse the name Minecraft's `cullface` uses.
    pub(super) fn from_name(name: &str) -> Option<Self> {
        FACES.iter().find(|(n, _)| *n == name).map(|&(_, dir)| dir)
    }
}

/// The four corners of a face, in the cycle order that the UV rect's corners
/// `(u1,v1) → (u2,v1) → (u2,v2) → (u1,v2)` map onto at face rotation 0.
pub(super) fn face_corners(dir: FaceDir, lo: Vec3, hi: Vec3) -> [Vec3; 4] {
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
pub(super) fn face_uvs(rect: [f32; 4], rotation: f32, resolution: Resolution) -> [[f32; 2]; 4] {
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
    ) -> Result<wyven_assets::Rgba8, String> {
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

        // Verified against element 7 of Blockbench's glTF export of vine_sword: at 270°
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
