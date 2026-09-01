//! The normalised geometry every loader produces, and its conversion into the
//! renderer's [`CpuMesh`].
//!
//! Normalising here is what makes the loaders substitutable: `.gltf` and
//! `.bbmodel` describe the same sword in different coordinate conventions, and
//! by the time either reaches this type both are Y-up, right-handed, one block
//! per unit, with UVs in `[0,1]` measured from the texture's top-left corner.
//! Nothing downstream can tell which file a model came from.

use glam::{EulerRot, Mat3, Mat4, Vec3};

use wyven_core::math::yaw_matrix;
use wyven_render::mesh::CpuMesh;

use super::display::ItemTransform;
use super::rig::BonePart;
use wyven_render::vertex::{ChunkVertex, NO_TINT};

/// Triangle geometry in model space: Y-up, right-handed, one block = 1.0, UVs
/// with a top-left origin. `positions`, `normals` and `uvs` are parallel arrays
/// indexed by `indices`.
#[derive(Debug, Clone, Default)]
pub struct ModelMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Where a model's `[0,1]` UV space lands inside the texture actually bound.
///
/// A model normally owns its texture outright and samples all of it, which is
/// [`UvWindow::FULL`]. A model drawn against a shared sheet — an entity skin
/// living in the block atlas — occupies one rectangle of it instead, and says
/// so here rather than by rewriting its UVs afterwards. Folding it into the
/// bake costs nothing: the vertices are being written anyway.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UvWindow {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
}

impl UvWindow {
    /// The whole texture — what every model used before there was an atlas.
    pub const FULL: Self = Self {
        offset: [0.0, 0.0],
        scale: [1.0, 1.0],
    };

    pub fn apply(&self, uv: [f32; 2]) -> [f32; 2] {
        [
            self.offset[0] + uv[0] * self.scale[0],
            self.offset[1] + uv[1] * self.scale[1],
        ]
    }
}

impl Default for UvWindow {
    fn default() -> Self {
        Self::FULL
    }
}

impl ModelMesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Append `other`'s geometry, shifting its indices to follow ours. Loaders
    /// build one of these per cube/primitive and merge them into the whole.
    pub fn merge(&mut self, other: ModelMesh) {
        let base = self.positions.len() as u32;
        self.indices.extend(other.indices.iter().map(|i| i + base));
        self.positions.extend(other.positions);
        self.normals.extend(other.normals);
        self.uvs.extend(other.uvs);
    }

    /// Axis-aligned bounds in model space, or `None` for empty geometry.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut iter = self.positions.iter().copied();
        let first = iter.next()?;
        Some(iter.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p))))
    }

    /// Reject geometry whose parallel arrays disagree or whose indices point
    /// past the end — a malformed file must fail loudly at load, not produce a
    /// panic deep inside the per-frame mesh bake.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.positions.len();
        if self.normals.len() != n || self.uvs.len() != n {
            return Err(format!(
                "attribute count mismatch: {n} positions, {} normals, {} uvs",
                self.normals.len(),
                self.uvs.len()
            ));
        }
        if !self.indices.len().is_multiple_of(3) {
            return Err(format!(
                "{} indices is not a whole triangle count",
                self.indices.len()
            ));
        }
        if let Some(bad) = self.indices.iter().find(|&&i| i as usize >= n) {
            return Err(format!("index {bad} out of range for {n} vertices"));
        }
        Ok(())
    }

    /// Bake this model into renderer geometry under an arbitrary model→world
    /// transform.
    ///
    /// Vertices come out in world space because the voxel pipeline has no model
    /// matrix — this mirrors what the game does when it bakes box parts.
    pub fn bake(&self, transform: Mat4) -> CpuMesh {
        let normal_matrix = Mat3::from_mat4(transform).inverse().transpose();
        let mut mesh = CpuMesh::new();
        let vertices = (0..self.positions.len()).map(|i| {
            let normal = (normal_matrix * self.normals[i]).normalize_or_zero();
            ChunkVertex {
                position: transform.transform_point3(self.positions[i]).to_array(),
                normal: normal.to_array(),
                uv: self.uvs[i],
                ao: shade(normal),
                flags: 0,
                // A file-loaded model binds its own texture rather than a layer
                // of the block texture array, and is never biome-tinted.
                layer: 0,
                tint: NO_TINT,
            }
        });
        mesh.push_indexed(vertices, self.indices.iter().copied());
        mesh
    }

    /// Bake the vertices `parts` names, each moved by its bone's matrix.
    ///
    /// The counterpart of [`ModelMesh::bake`] for a rigged model: `bones` is one
    /// matrix per bone (from [`Rig::matrices`](super::rig::Rig::matrices)) and
    /// `parts` says which vertices each owns. Passing only some of a rig's parts
    /// is how a caller draws one limb — the first-person arm — out of a body.
    ///
    /// `normal_basis` is separate from `transform` for the same reason
    /// [`CpuMesh::transformed`] splits them: geometry carried by the camera is
    /// placed by a matrix containing the camera's own rotation, and lighting
    /// taken from that would pulse as the player turned on the spot. Where the
    /// two are the same thing, pass the same matrix twice.
    pub fn bake_posed<'a>(
        &self,
        parts: impl IntoIterator<Item = &'a BonePart>,
        bones: &[Mat4],
        transform: Mat4,
        normal_basis: Mat4,
        uv: UvWindow,
    ) -> CpuMesh {
        let normal_matrix = Mat3::from_mat4(normal_basis).inverse().transpose();
        let mut mesh = CpuMesh::new();
        // Where each source vertex ended up, so the index walk below stays a
        // single pass whichever subset of the parts was asked for.
        let mut moved = vec![u32::MAX; self.positions.len()];
        for part in parts {
            let bone = part
                .bone
                .and_then(|b| bones.get(b.0 as usize).copied())
                .unwrap_or(Mat4::IDENTITY);
            let place = transform * bone;
            let orient = normal_matrix * Mat3::from_mat4(bone);
            let lo = part.start as usize;
            let hi = (part.end as usize).min(self.positions.len());
            for (i, slot) in (lo..hi).zip(moved[lo..hi].iter_mut()) {
                *slot = mesh.vertices.len() as u32;
                let normal = (orient * self.normals[i]).normalize_or_zero();
                mesh.vertices.push(ChunkVertex {
                    position: place.transform_point3(self.positions[i]).to_array(),
                    normal: normal.to_array(),
                    uv: uv.apply(self.uvs[i]),
                    ao: shade(normal),
                    flags: 0,
                    layer: 0,
                    tint: NO_TINT,
                });
            }
        }
        // A triangle survives only if all three corners were baked, so asking
        // for one limb yields that limb and no stray connecting faces.
        for tri in self.indices.chunks_exact(3) {
            let mapped = [
                moved[tri[0] as usize],
                moved[tri[1] as usize],
                moved[tri[2] as usize],
            ];
            if mapped.iter().all(|&i| i != u32::MAX) {
                mesh.indices.extend_from_slice(&mapped);
            }
        }
        mesh
    }

    /// Bake this model standing at `origin` and turned to face `yaw`.
    pub fn to_cpu_mesh(
        &self,
        origin: Vec3,
        yaw: f32,
        scale: f32,
        rotation: Vec3,
        offset: Vec3,
    ) -> CpuMesh {
        self.bake(placement(origin, yaw, 0.0, scale, rotation, offset))
    }
}

/// Model→world transform: shift by `offset`, turn about the model's own axes by
/// `rotation`, and scale — all in the model's own space — then pitch about X,
/// turn by the engine's `yaw`, and translate to `origin`.
///
/// `offset` applies before `rotation` and `scale` so a Blockbench author can
/// re-centre a model within its own frame without the correction changing when
/// they rescale or re-orient it — and so `rotation` turns the model about
/// itself rather than about the point it is drawn at. `rotation` is in radians
/// (`ModelSpec` authors it in degrees) and exists because exports disagree on
/// which plane a flat object lies in. `pitch` is separate: it belongs to
/// geometry riding an articulated joint — an item held in a swinging hand.
pub fn placement(
    origin: Vec3,
    yaw: f32,
    pitch: f32,
    scale: f32,
    rotation: Vec3,
    offset: Vec3,
) -> Mat4 {
    anchor(origin, yaw, pitch) * local_transform(None, scale, rotation, offset)
}

/// The world half of a [`placement`]: where the thing carrying the model is,
/// and how it is turned. Split out so a caller that has its own model-space
/// transform — a `display` entry, a viewmodel's camera-relative pose — can
/// compose against the same anchor every other model uses.
pub fn anchor(origin: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    Mat4::from_translation(origin) * yaw_matrix(yaw) * Mat4::from_rotation_x(pitch)
}

/// The model half of a [`placement`]: where the model sits within the space of
/// whatever carries it.
///
/// This is the one place that decides between an authored `display` entry and
/// the data file's own `scale`/`rotation`/`offset`. A model that declares a
/// placement for the context being drawn wins outright — its author measured it
/// against that context, where the `ModelSpec` numbers are one compromise
/// stretched across the hand, the ground and the inventory slot at once.
pub fn local_transform(
    display: Option<ItemTransform>,
    scale: f32,
    rotation: Vec3,
    offset: Vec3,
) -> Mat4 {
    match display {
        Some(transform) => transform.matrix(),
        None => {
            Mat4::from_scale(Vec3::splat(scale))
                * Mat4::from_euler(EulerRot::YXZ, rotation.y, rotation.x, rotation.z)
                * Mat4::from_translation(offset)
        }
    }
}

/// Directional shading for an arbitrary normal, generalising the per-face
/// constants the box models bake in (`entity::model::face_shade`: +Y 1.0, −Y
/// 0.68, ±X 0.86, ±Z 0.80). Axis-aligned normals reproduce those values exactly,
/// so an imported model sits in the same light as the voxels around it instead
/// of reading as a flat sticker.
pub(super) fn shade(normal: Vec3) -> f32 {
    let n = normal.normalize_or_zero();
    let weights = n.abs();
    let total = weights.x + weights.y + weights.z;
    if total <= f32::EPSILON {
        return 1.0;
    }
    let vertical = if n.y >= 0.0 { 1.0 } else { 0.68 };
    (weights.x * 0.86 + weights.y * vertical + weights.z * 0.80) / total
}

#[cfg(test)]
mod tests {
    use super::super::display::ItemTransform;

    use super::*;
    use std::f32::consts::FRAC_PI_2;

    /// One triangle in the XY plane, facing +Z.
    fn tri() -> ModelMesh {
        ModelMesh {
            positions: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            normals: vec![Vec3::Z; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
        }
    }

    #[test]
    fn axis_aligned_normals_match_the_box_model_shading() {
        assert!((shade(Vec3::Y) - 1.0).abs() < 1e-6);
        assert!((shade(Vec3::NEG_Y) - 0.68).abs() < 1e-6);
        assert!((shade(Vec3::X) - 0.86).abs() < 1e-6);
        assert!((shade(Vec3::NEG_X) - 0.86).abs() < 1e-6);
        assert!((shade(Vec3::Z) - 0.80).abs() < 1e-6);
        assert!((shade(Vec3::NEG_Z) - 0.80).abs() < 1e-6);
    }

    #[test]
    fn shading_stays_in_range_for_arbitrary_normals() {
        for n in [
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(-0.3, -0.9, 0.2),
            Vec3::new(0.0, 0.0, 0.0),
        ] {
            let s = shade(n);
            assert!((0.0..=1.0).contains(&s), "shade({n}) = {s} out of range");
        }
    }

    #[test]
    fn baking_translates_scales_and_turns_the_model() {
        let origin = Vec3::new(10.0, 2.0, -4.0);
        let mesh = tri().to_cpu_mesh(origin, 0.0, 2.0, Vec3::ZERO, Vec3::ZERO);
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(mesh.vertices[0].position, origin.to_array());
        // The +X corner is scaled to 2 blocks out, then translated.
        assert_eq!(
            mesh.vertices[1].position,
            (origin + Vec3::X * 2.0).to_array()
        );
    }

    #[test]
    fn offset_applies_in_model_space_before_scale() {
        let mesh = tri().to_cpu_mesh(Vec3::ZERO, 0.0, 3.0, Vec3::ZERO, Vec3::X);
        // (0 + 1) * 3 = 3, not 0 * 3 + 1 = 1.
        assert_eq!(mesh.vertices[0].position[0], 3.0);
    }

    #[test]
    fn yaw_turns_geometry_and_normals_together() {
        let mesh = tri().to_cpu_mesh(Vec3::ZERO, FRAC_PI_2, 1.0, Vec3::ZERO, Vec3::ZERO);
        // The engine's yaw convention sends +X to +Z (see core::math::rotate_y).
        let p = mesh.vertices[1].position;
        assert!(p[0].abs() < 1e-6 && (p[2] - 1.0).abs() < 1e-6, "got {p:?}");
        let n = mesh.vertices[0].normal;
        assert!((n[0] + 1.0).abs() < 1e-6 && n[2].abs() < 1e-6, "got {n:?}");
    }

    /// A zero rotation must compose to exactly what `placement` produced before
    /// the parameter existed — every entity prop and the vine sword depend on
    /// that, and a sign or ordering slip here would move all of them at once.
    #[test]
    fn a_zero_rotation_leaves_the_transform_untouched() {
        let expected = Mat4::from_translation(Vec3::new(3.0, 1.0, -2.0))
            * yaw_matrix(0.9)
            * Mat4::from_rotation_x(-0.4)
            * Mat4::from_scale(Vec3::splat(0.35))
            * Mat4::from_translation(Vec3::new(-0.5, 0.75, -0.5));
        let actual = placement(
            Vec3::new(3.0, 1.0, -2.0),
            0.9,
            -0.4,
            0.35,
            Vec3::ZERO,
            Vec3::new(-0.5, 0.75, -0.5),
        );
        assert!(expected.abs_diff_eq(actual, 1e-6), "{expected} vs {actual}");
    }

    /// The quarter-turn the tool models carry: a model flat in XY is stood up
    /// flat in YZ, which is what puts a blade broadside in the fist.
    #[test]
    fn a_model_rotation_turns_geometry_about_the_models_own_axis() {
        let quarter = Vec3::new(0.0, FRAC_PI_2, 0.0);
        let mesh = tri().to_cpu_mesh(Vec3::ZERO, 0.0, 1.0, quarter, Vec3::ZERO);
        // The +X corner swings onto -Z; the model is turned, not displaced.
        let p = mesh.vertices[1].position;
        assert!(p[0].abs() < 1e-6 && (p[2] + 1.0).abs() < 1e-6, "got {p:?}");
        // The vertex at the model's own origin does not move.
        assert_eq!(mesh.vertices[0].position, [0.0, 0.0, 0.0]);
    }

    /// `offset` re-centres first, so the rotation pivots about the re-centred
    /// model rather than sweeping it around the point it is drawn at.
    #[test]
    fn rotation_pivots_after_the_offset_recentres_the_model() {
        let quarter = Vec3::new(0.0, FRAC_PI_2, 0.0);
        // Offset puts the model's origin vertex at (-1, 0, 0); a quarter turn
        // about the model's axis sends that to (0, 0, 1).
        let mesh = tri().to_cpu_mesh(Vec3::ZERO, 0.0, 1.0, quarter, Vec3::new(-1.0, 0.0, 0.0));
        let p = mesh.vertices[0].position;
        assert!(p[0].abs() < 1e-6 && (p[2] - 1.0).abs() < 1e-6, "got {p:?}");
    }

    #[test]
    fn uvs_survive_the_bake_untouched() {
        let mesh = tri().to_cpu_mesh(Vec3::ONE, 1.2, 0.5, Vec3::ZERO, Vec3::Y);
        assert_eq!(mesh.vertices[1].uv, [1.0, 0.0]);
        assert_eq!(mesh.vertices[2].uv, [0.0, 1.0]);
    }

    #[test]
    fn merge_shifts_the_second_meshs_indices() {
        let mut mesh = tri();
        mesh.merge(tri());
        assert_eq!(mesh.positions.len(), 6);
        assert_eq!(mesh.indices, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(mesh.triangle_count(), 2);
    }

    #[test]
    fn bounds_cover_every_vertex() {
        let (lo, hi) = tri().bounds().expect("non-empty");
        assert_eq!(lo, Vec3::ZERO);
        assert_eq!(hi, Vec3::new(1.0, 1.0, 0.0));
        assert!(ModelMesh::default().bounds().is_none());
    }

    #[test]
    fn validate_rejects_malformed_geometry() {
        assert!(tri().validate().is_ok());

        let mut short = tri();
        short.normals.pop();
        assert!(short.validate().is_err(), "attribute mismatch");

        let mut partial = tri();
        partial.indices.pop();
        assert!(partial.validate().is_err(), "incomplete triangle");

        let mut oob = tri();
        oob.indices = vec![0, 1, 99];
        assert!(oob.validate().is_err(), "index out of range");
    }

    /// The split must be exact, or every model in the game shifts.
    #[test]
    fn the_anchor_and_the_local_transform_rebuild_the_placement() {
        let origin = Vec3::new(3.0, 1.5, -2.0);
        let (yaw, pitch) = (0.7, -0.3);
        let rotation = Vec3::new(0.2, 1.1, -0.4);
        let offset = Vec3::new(-0.5, 0.25, -0.5);
        let split = anchor(origin, yaw, pitch) * local_transform(None, 0.45, rotation, offset);
        let whole = placement(origin, yaw, pitch, 0.45, rotation, offset);
        assert!(
            split.abs_diff_eq(whole, 1e-6),
            "split {split} != placement {whole}"
        );
    }

    /// An authored placement replaces the data file's numbers rather than
    /// composing with them: the two centre a model differently (`display` in all
    /// three axes, `offset` only in XZ), so combining them would mean nothing.
    #[test]
    fn an_authored_placement_ignores_the_spec() {
        let display = ItemTransform {
            translation: [0.0, 16.0, 0.0],
            ..Default::default()
        };
        let with_spec = local_transform(Some(display), 0.45, Vec3::new(0.0, 1.5, 0.0), Vec3::X);
        let without = local_transform(Some(display), 1.0, Vec3::ZERO, Vec3::ZERO);
        assert!(with_spec.abs_diff_eq(without, 1e-6));
        assert!(
            with_spec
                .transform_point3(Vec3::splat(0.5))
                .abs_diff_eq(Vec3::Y, 1e-6)
        );
    }
}
