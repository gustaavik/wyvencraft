//! The normalised geometry every loader produces, and its conversion into the
//! renderer's [`CpuMesh`].
//!
//! Normalising here is what makes the loaders substitutable: `.gltf` and
//! `.bbmodel` describe the same sword in different coordinate conventions, and
//! by the time either reaches this type both are Y-up, right-handed, one block
//! per unit, with UVs in `[0,1]` measured from the texture's top-left corner.
//! Nothing downstream can tell which file a model came from.

use glam::{Mat3, Mat4, Vec3};

use crate::core::math::yaw_matrix;
use crate::render::mesh::CpuMesh;
use crate::render::vertex::ChunkVertex;

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
    /// matrix — this mirrors what [`crate::entity::model`] does for box parts.
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
            }
        });
        mesh.push_indexed(vertices, self.indices.iter().copied());
        mesh
    }

    /// Bake this model standing at `origin` and turned to face `yaw`.
    pub fn to_cpu_mesh(&self, origin: Vec3, yaw: f32, scale: f32, offset: Vec3) -> CpuMesh {
        self.bake(placement(origin, yaw, 0.0, scale, offset))
    }
}

/// Model→world transform: shift by `offset` and scale in the model's own space,
/// pitch about X, turn by the engine's `yaw`, then translate to `origin`.
///
/// `offset` applies before `scale` so a Blockbench author can re-centre a model
/// within its own frame without the correction changing when they rescale it.
/// `pitch` exists for geometry that rides an articulated joint — an item held in
/// a swinging hand.
pub fn placement(origin: Vec3, yaw: f32, pitch: f32, scale: f32, offset: Vec3) -> Mat4 {
    Mat4::from_translation(origin)
        * yaw_matrix(yaw)
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_scale(Vec3::splat(scale))
        * Mat4::from_translation(offset)
}

/// Directional shading for an arbitrary normal, generalising the per-face
/// constants the box models bake in (`entity::model::face_shade`: +Y 1.0, −Y
/// 0.68, ±X 0.86, ±Z 0.80). Axis-aligned normals reproduce those values exactly,
/// so an imported model sits in the same light as the voxels around it instead
/// of reading as a flat sticker.
fn shade(normal: Vec3) -> f32 {
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
        let mesh = tri().to_cpu_mesh(origin, 0.0, 2.0, Vec3::ZERO);
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
        let mesh = tri().to_cpu_mesh(Vec3::ZERO, 0.0, 3.0, Vec3::X);
        // (0 + 1) * 3 = 3, not 0 * 3 + 1 = 1.
        assert_eq!(mesh.vertices[0].position[0], 3.0);
    }

    #[test]
    fn yaw_turns_geometry_and_normals_together() {
        let mesh = tri().to_cpu_mesh(Vec3::ZERO, FRAC_PI_2, 1.0, Vec3::ZERO);
        // The engine's yaw convention sends +X to +Z (see core::math::rotate_y).
        let p = mesh.vertices[1].position;
        assert!(p[0].abs() < 1e-6 && (p[2] - 1.0).abs() < 1e-6, "got {p:?}");
        let n = mesh.vertices[0].normal;
        assert!((n[0] + 1.0).abs() < 1e-6 && n[2].abs() < 1e-6, "got {n:?}");
    }

    #[test]
    fn uvs_survive_the_bake_untouched() {
        let mesh = tri().to_cpu_mesh(Vec3::ONE, 1.2, 0.5, Vec3::Y);
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
}
