//! glTF 2.0 (`.gltf`) loader.
//!
//! Supports the static, self-contained subset that matters here: triangle
//! primitives with `POSITION`/`NORMAL`/`TEXCOORD_0`, TRS or matrix node
//! transforms composed down the scene graph, and buffers/images supplied either
//! inline as `data:` URIs or as files beside the `.gltf`. Skinning, morph
//! targets and animation are rejected rather than silently ignored — the
//! renderer has no way to express them, and a model that quietly loses its rig
//! is worse than one that reports why it cannot load.
//!
//! `.glb` (the binary container) is not handled; exporters offer `.gltf` and it
//! keeps this parser to plain JSON.

use std::collections::HashMap;

use glam::{Mat3, Mat4, Quat, Vec2, Vec3};
use serde::Deserialize;

use crate::content::ContentSource;
use crate::render::texture::decode_png;

use super::datauri::{self, Uri};
use super::mesh::ModelMesh;
use super::{Model, ModelLoader, resolve_sibling};

/// Component types we can read (glTF's OpenGL enum values).
const BYTE: u32 = 5120;
const UNSIGNED_BYTE: u32 = 5121;
const SHORT: u32 = 5122;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const FLOAT: u32 = 5126;

/// `primitive.mode` for a triangle list.
const MODE_TRIANGLES: u32 = 4;

pub struct GltfLoader;

impl ModelLoader for GltfLoader {
    fn extensions(&self) -> &'static [&'static str] {
        &["gltf"]
    }

    fn load(&self, bytes: &[u8], dir: &str, source: &dyn ContentSource) -> Result<Model, String> {
        let doc: Document =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid glTF JSON: {e}"))?;
        doc.check_supported()?;

        let buffers = doc.resolve_buffers(dir, source)?;
        let mut mesh = ModelMesh::default();
        doc.walk_scene(&buffers, &mut mesh)?;
        mesh.validate()?;

        let texture = doc.resolve_texture(dir, source, &buffers)?;
        Model::new(mesh, texture)
    }
}

// --- glTF JSON schema (only the fields we read) -----------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    #[serde(default)]
    scene: usize,
    #[serde(default)]
    scenes: Vec<Scene>,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    meshes: Vec<Mesh>,
    #[serde(default)]
    accessors: Vec<Accessor>,
    #[serde(default)]
    buffer_views: Vec<BufferView>,
    #[serde(default)]
    buffers: Vec<Buffer>,
    #[serde(default)]
    images: Vec<Image>,
    #[serde(default)]
    textures: Vec<TextureRef>,
    #[serde(default)]
    materials: Vec<Material>,
    #[serde(default)]
    skins: Vec<serde_json::Value>,
    #[serde(default)]
    animations: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct Scene {
    #[serde(default)]
    nodes: Vec<usize>,
}

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    children: Vec<usize>,
    mesh: Option<usize>,
    matrix: Option<[f32; 16]>,
    translation: Option<[f32; 3]>,
    rotation: Option<[f32; 4]>,
    scale: Option<[f32; 3]>,
}

#[derive(Deserialize)]
struct Mesh {
    #[serde(default)]
    primitives: Vec<Primitive>,
}

#[derive(Deserialize)]
struct Primitive {
    #[serde(default)]
    attributes: HashMap<String, usize>,
    indices: Option<usize>,
    #[serde(default = "default_mode")]
    mode: u32,
    // `material` is deliberately not read: the renderer binds one texture per
    // mesh, so the base-colour texture is resolved once for the whole document
    // (see `Document::resolve_texture`) rather than per primitive.
}

fn default_mode() -> u32 {
    MODE_TRIANGLES
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Accessor {
    buffer_view: Option<usize>,
    #[serde(default)]
    byte_offset: usize,
    component_type: u32,
    count: usize,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferView {
    buffer: usize,
    #[serde(default)]
    byte_offset: usize,
    byte_length: usize,
    #[serde(default)]
    byte_stride: Option<usize>,
}

#[derive(Deserialize)]
struct Buffer {
    uri: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Image {
    uri: Option<String>,
    buffer_view: Option<usize>,
}

#[derive(Deserialize)]
struct TextureRef {
    source: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Material {
    #[serde(default)]
    pbr_metallic_roughness: Option<PbrMetallicRoughness>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PbrMetallicRoughness {
    base_color_texture: Option<TextureInfo>,
}

#[derive(Deserialize)]
struct TextureInfo {
    index: usize,
}

// --- Loading ---------------------------------------------------------------

impl Document {
    fn check_supported(&self) -> Result<(), String> {
        if !self.skins.is_empty() {
            return Err(format!(
                "skinned models are not supported ({} skin(s)); export with the rig baked",
                self.skins.len()
            ));
        }
        if !self.animations.is_empty() {
            log::warn!(
                "glTF carries {} animation(s); they are ignored (models render in their rest pose)",
                self.animations.len()
            );
        }
        Ok(())
    }

    /// Decode every buffer up front. glTF accessors index buffers freely, so
    /// there is no streaming to be had — and Blockbench exports one buffer.
    fn resolve_buffers(
        &self,
        dir: &str,
        source: &dyn ContentSource,
    ) -> Result<Vec<Vec<u8>>, String> {
        self.buffers
            .iter()
            .enumerate()
            .map(|(i, buffer)| {
                let uri = buffer
                    .uri
                    .as_deref()
                    .ok_or_else(|| format!("buffer {i} has no uri (.glb is not supported)"))?;
                read_uri(uri, dir, source).map_err(|e| format!("buffer {i}: {e}"))
            })
            .collect()
    }

    /// Compose node transforms down from the scene roots, appending every mesh
    /// primitive it finds in world (model) space.
    fn walk_scene(&self, buffers: &[Vec<u8>], out: &mut ModelMesh) -> Result<(), String> {
        let roots: &[usize] = match self.scenes.get(self.scene) {
            Some(scene) => &scene.nodes,
            // No scene declared: every node is a root. Cycles would recurse
            // forever, so depth is bounded below regardless.
            None => &[],
        };
        if roots.is_empty() && self.scenes.is_empty() {
            for index in 0..self.nodes.len() {
                self.walk_node(index, Mat4::IDENTITY, 0, buffers, out)?;
            }
            return Ok(());
        }
        for &index in roots {
            self.walk_node(index, Mat4::IDENTITY, 0, buffers, out)?;
        }
        Ok(())
    }

    fn walk_node(
        &self,
        index: usize,
        parent: Mat4,
        depth: usize,
        buffers: &[Vec<u8>],
        out: &mut ModelMesh,
    ) -> Result<(), String> {
        // glTF node graphs are trees, but a malformed file can contain a cycle;
        // bound the recursion rather than blowing the stack on bad input.
        const MAX_DEPTH: usize = 64;
        if depth > MAX_DEPTH {
            return Err(format!("node hierarchy deeper than {MAX_DEPTH} levels"));
        }
        let node = self
            .nodes
            .get(index)
            .ok_or_else(|| format!("node {index} out of range"))?;
        let transform = parent * node.local_transform();

        if let Some(mesh_index) = node.mesh {
            let mesh = self
                .meshes
                .get(mesh_index)
                .ok_or_else(|| format!("mesh {mesh_index} out of range"))?;
            for primitive in &mesh.primitives {
                out.merge(self.read_primitive(primitive, transform, buffers)?);
            }
        }
        for &child in &node.children {
            self.walk_node(child, transform, depth + 1, buffers, out)?;
        }
        Ok(())
    }

    fn read_primitive(
        &self,
        primitive: &Primitive,
        transform: Mat4,
        buffers: &[Vec<u8>],
    ) -> Result<ModelMesh, String> {
        if primitive.mode != MODE_TRIANGLES {
            return Err(format!(
                "primitive mode {} is not supported (only triangles, mode {MODE_TRIANGLES})",
                primitive.mode
            ));
        }
        let position_index = *primitive
            .attributes
            .get("POSITION")
            .ok_or("primitive has no POSITION attribute")?;
        let positions = self.read_vec3(position_index, buffers)?;

        let normals = match primitive.attributes.get("NORMAL") {
            Some(&i) => self.read_vec3(i, buffers)?,
            // glTF says normal-less triangles are flat-shaded; derive them so
            // lighting still reads correctly.
            None => flat_normals(&positions),
        };
        let uvs = match primitive.attributes.get("TEXCOORD_0") {
            Some(&i) => self.read_vec2(i, buffers)?,
            None => vec![Vec2::ZERO; positions.len()],
        };
        let indices = match primitive.indices {
            Some(i) => self.read_indices(i, buffers)?,
            None => (0..positions.len() as u32).collect(),
        };

        // Normals need the inverse-transpose so non-uniform scale doesn't skew
        // them; Blockbench never emits scale, but other exporters do.
        let normal_matrix = Mat3::from_mat4(transform).inverse().transpose();
        Ok(ModelMesh {
            positions: positions
                .iter()
                .map(|p| transform.transform_point3(*p))
                .collect(),
            normals: normals
                .iter()
                .map(|n| (normal_matrix * *n).normalize_or_zero())
                .collect(),
            uvs: uvs.iter().map(|uv| uv.to_array()).collect(),
            indices,
        })
    }

    fn read_vec3(&self, index: usize, buffers: &[Vec<u8>]) -> Result<Vec<Vec3>, String> {
        let floats = self.read_floats(index, "VEC3", 3, buffers)?;
        Ok(floats.chunks_exact(3).map(Vec3::from_slice).collect())
    }

    fn read_vec2(&self, index: usize, buffers: &[Vec<u8>]) -> Result<Vec<Vec2>, String> {
        let floats = self.read_floats(index, "VEC2", 2, buffers)?;
        Ok(floats.chunks_exact(2).map(Vec2::from_slice).collect())
    }

    fn read_floats(
        &self,
        index: usize,
        kind: &str,
        components: usize,
        buffers: &[Vec<u8>],
    ) -> Result<Vec<f32>, String> {
        let accessor = self.accessor(index)?;
        if accessor.kind != kind {
            return Err(format!(
                "accessor {index} is {:?}, expected {kind}",
                accessor.kind
            ));
        }
        if accessor.component_type != FLOAT {
            return Err(format!(
                "accessor {index} has component type {}, expected float ({FLOAT})",
                accessor.component_type
            ));
        }
        let mut out = Vec::with_capacity(accessor.count * components);
        self.for_each_element(index, components * 4, buffers, |element| {
            for i in 0..components {
                let bytes: [u8; 4] = element[i * 4..i * 4 + 4].try_into().expect("4 bytes");
                out.push(f32::from_le_bytes(bytes));
            }
            Ok(())
        })?;
        Ok(out)
    }

    fn read_indices(&self, index: usize, buffers: &[Vec<u8>]) -> Result<Vec<u32>, String> {
        let accessor = self.accessor(index)?;
        if accessor.kind != "SCALAR" {
            return Err(format!(
                "index accessor {index} is {:?}, expected SCALAR",
                accessor.kind
            ));
        }
        let width = match accessor.component_type {
            UNSIGNED_BYTE | BYTE => 1,
            UNSIGNED_SHORT | SHORT => 2,
            UNSIGNED_INT => 4,
            other => return Err(format!("index accessor {index} has component type {other}")),
        };
        let mut out = Vec::with_capacity(accessor.count);
        self.for_each_element(index, width, buffers, |element| {
            out.push(match width {
                1 => element[0] as u32,
                2 => u16::from_le_bytes([element[0], element[1]]) as u32,
                _ => u32::from_le_bytes(element.try_into().expect("4 bytes")),
            });
            Ok(())
        })?;
        Ok(out)
    }

    /// Walk an accessor's elements, honouring the buffer view's `byteStride`
    /// (interleaved attributes) and both offsets.
    fn for_each_element(
        &self,
        index: usize,
        element_size: usize,
        buffers: &[Vec<u8>],
        mut visit: impl FnMut(&[u8]) -> Result<(), String>,
    ) -> Result<(), String> {
        let accessor = self.accessor(index)?;
        let view_index = accessor.buffer_view.ok_or_else(|| {
            format!("accessor {index} has no bufferView (sparse accessors are not supported)")
        })?;
        let view = self
            .buffer_views
            .get(view_index)
            .ok_or_else(|| format!("bufferView {view_index} out of range"))?;
        let buffer = buffers
            .get(view.buffer)
            .ok_or_else(|| format!("buffer {} out of range", view.buffer))?;

        let stride = view.byte_stride.unwrap_or(element_size).max(element_size);
        let view_end = view.byte_offset + view.byte_length;
        if view_end > buffer.len() {
            return Err(format!(
                "bufferView {view_index} spans {}..{view_end} of a {}-byte buffer",
                view.byte_offset,
                buffer.len()
            ));
        }
        for i in 0..accessor.count {
            let start = view.byte_offset + accessor.byte_offset + i * stride;
            let end = start + element_size;
            if end > view_end {
                return Err(format!(
                    "accessor {index} element {i} runs past the end of bufferView {view_index}"
                ));
            }
            visit(&buffer[start..end])?;
        }
        Ok(())
    }

    fn accessor(&self, index: usize) -> Result<&Accessor, String> {
        self.accessors
            .get(index)
            .ok_or_else(|| format!("accessor {index} out of range"))
    }

    /// The base-colour texture of the first material that has one, falling back
    /// to the first image in the file.
    ///
    /// The renderer binds one texture per mesh, so a multi-material model gets
    /// the first material's texture everywhere. That is a real limitation, and
    /// it is logged rather than hidden.
    fn resolve_texture(
        &self,
        dir: &str,
        source: &dyn ContentSource,
        buffers: &[Vec<u8>],
    ) -> Result<crate::render::Rgba8, String> {
        if self.materials.len() > 1 {
            log::warn!(
                "glTF has {} materials; all geometry will sample the first one's texture",
                self.materials.len()
            );
        }
        let image_index = self
            .materials
            .iter()
            .filter_map(|m| {
                m.pbr_metallic_roughness
                    .as_ref()?
                    .base_color_texture
                    .as_ref()
            })
            .find_map(|info| self.textures.get(info.index)?.source)
            .or(if self.images.is_empty() {
                None
            } else {
                Some(0)
            })
            .ok_or("model has no base-colour texture")?;

        let image = self
            .images
            .get(image_index)
            .ok_or_else(|| format!("image {image_index} out of range"))?;

        let bytes = match (&image.uri, image.buffer_view) {
            (Some(uri), _) => read_uri(uri, dir, source)?,
            (None, Some(view_index)) => {
                let view = self
                    .buffer_views
                    .get(view_index)
                    .ok_or_else(|| format!("image bufferView {view_index} out of range"))?;
                let buffer = buffers
                    .get(view.buffer)
                    .ok_or_else(|| format!("buffer {} out of range", view.buffer))?;
                let end = view.byte_offset + view.byte_length;
                buffer
                    .get(view.byte_offset..end)
                    .ok_or("image bufferView runs past the end of its buffer")?
                    .to_vec()
            }
            (None, None) => return Err("image has neither a uri nor a bufferView".into()),
        };
        decode_png(&bytes).map_err(|e| format!("model texture: {e}"))
    }
}

impl Node {
    fn local_transform(&self) -> Mat4 {
        if let Some(m) = self.matrix {
            // glTF stores matrices column-major, same as glam.
            return Mat4::from_cols_array(&m);
        }
        Mat4::from_scale_rotation_translation(
            self.scale.map(Vec3::from).unwrap_or(Vec3::ONE),
            self.rotation
                .map(|[x, y, z, w]| Quat::from_xyzw(x, y, z, w))
                .unwrap_or(Quat::IDENTITY),
            self.translation.map(Vec3::from).unwrap_or(Vec3::ZERO),
        )
    }
}

/// Read a glTF `uri`: inline `data:` payload, or a file beside the model.
fn read_uri(uri: &str, dir: &str, source: &dyn ContentSource) -> Result<Vec<u8>, String> {
    match datauri::parse(uri)? {
        Uri::Inline(bytes) => Ok(bytes),
        Uri::Relative(path) => {
            let full = resolve_sibling(dir, path);
            source
                .read_bytes(&full)
                .map_err(|e| format!("could not read {full}: {e}"))
        }
    }
}

/// Per-triangle geometric normals, for primitives that ship without any.
fn flat_normals(positions: &[Vec3]) -> Vec<Vec3> {
    let mut normals = vec![Vec3::Y; positions.len()];
    for tri in 0..positions.len() / 3 {
        let [a, b, c] = [
            positions[tri * 3],
            positions[tri * 3 + 1],
            positions[tri * 3 + 2],
        ];
        let n = (b - a).cross(c - a).normalize_or_zero();
        normals[tri * 3..tri * 3 + 3].fill(n);
    }
    normals
}
