//! Mesh data: a CPU-side builder ([`CpuMesh`]) produced by the chunk mesher, and
//! a GPU-resident ([`GpuMesh`]) holding the uploaded vertex/index buffers.

use std::sync::Arc;

use glam::{Mat4, Vec3};

use vulkano::Validated;
use vulkano::buffer::{Buffer, BufferCreateInfo, BufferUsage, Subbuffer};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter, StandardMemoryAllocator};

use super::vertex::{ChunkVertex, LineVertex};

/// Plain, render-agnostic geometry built off-thread by the mesher.
#[derive(Default, Clone)]
pub struct CpuMesh {
    pub vertices: Vec<ChunkVertex>,
    pub indices: Vec<u32>,
}

impl CpuMesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Append a quad as two triangles. `verts` are given counter-clockwise.
    pub fn push_quad(&mut self, verts: [ChunkVertex; 4]) {
        let base = self.vertices.len() as u32;
        self.vertices.extend_from_slice(&verts);
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
    }

    /// Append an indexed triangle list, rebasing `indices` onto the vertices
    /// already here. This is the entry point for geometry that arrives as a
    /// triangle soup — imported model files — rather than as voxel quads.
    pub fn push_indexed(
        &mut self,
        vertices: impl IntoIterator<Item = ChunkVertex>,
        indices: impl IntoIterator<Item = u32>,
    ) {
        let base = self.vertices.len() as u32;
        self.vertices.extend(vertices);
        self.indices.extend(indices.into_iter().map(|i| i + base));
    }

    /// This mesh moved by `transform`, with normals turned by `normal_basis`.
    ///
    /// Everything else — UVs, the baked face shade, flags, layer and tint —
    /// rides through untouched, because none of it depends on where the geometry
    /// ended up.
    ///
    /// `normal_basis` is separate from `transform` for the reason a view model
    /// exists at all: geometry held in front of the camera is placed by a matrix
    /// that *contains the camera's own rotation*, and normals taken from it would
    /// swing relative to the sun as the player turns on the spot — the held item
    /// would pulse bright and dim, which reads as a rendering fault rather than
    /// as lighting. Passing just the placement's rotation pins the shading to the
    /// object instead. Where the two genuinely are the same thing, pass the same
    /// matrix twice.
    pub fn transformed(&self, transform: Mat4, normal_basis: Mat4) -> CpuMesh {
        let vertices = self
            .vertices
            .iter()
            .map(|v| ChunkVertex {
                position: transform
                    .transform_point3(Vec3::from(v.position))
                    .to_array(),
                normal: normal_basis
                    .transform_vector3(Vec3::from(v.normal))
                    .to_array(),
                ..*v
            })
            .collect();
        CpuMesh {
            vertices,
            indices: self.indices.clone(),
        }
    }
}

/// Vertex + index buffers uploaded to the GPU. Dropping it frees the buffers
/// (RAII via vulkano's `Arc`-backed allocations).
pub struct GpuMesh {
    pub vertex_buffer: Subbuffer<[ChunkVertex]>,
    pub index_buffer: Subbuffer<[u32]>,
    pub index_count: u32,
}

impl GpuMesh {
    /// Upload a [`CpuMesh`]. Returns `None` for empty meshes (nothing to draw).
    pub fn upload(
        allocator: &Arc<StandardMemoryAllocator>,
        mesh: &CpuMesh,
    ) -> Result<Option<GpuMesh>, Validated<vulkano::buffer::AllocateBufferError>> {
        if mesh.is_empty() {
            return Ok(None);
        }

        let vertex_buffer = Buffer::from_iter(
            allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            mesh.vertices.iter().copied(),
        )?;

        let index_buffer = Buffer::from_iter(
            allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::INDEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            mesh.indices.iter().copied(),
        )?;

        Ok(Some(GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
        }))
    }
}

/// A vertex buffer of [`LineVertex`] pairs for the debug-line pipeline
/// (block selection outline, wireframes). Unindexed `LineList` geometry.
pub struct GpuLines {
    pub vertex_buffer: Subbuffer<[LineVertex]>,
    pub vertex_count: u32,
}

impl GpuLines {
    /// Upload line-list vertices. Returns `None` for an empty slice.
    pub fn upload(
        allocator: &Arc<StandardMemoryAllocator>,
        vertices: &[LineVertex],
    ) -> Result<Option<GpuLines>, Validated<vulkano::buffer::AllocateBufferError>> {
        if vertices.is_empty() {
            return Ok(None);
        }

        let vertex_buffer = Buffer::from_iter(
            allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            vertices.iter().copied(),
        )?;

        Ok(Some(GpuLines {
            vertex_buffer,
            vertex_count: vertices.len() as u32,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(position: [f32; 3], normal: [f32; 3]) -> ChunkVertex {
        ChunkVertex {
            position,
            normal,
            uv: [0.25, 0.75],
            ao: 0.6,
            flags: 7,
            layer: 3,
            tint: [1, 2, 3, 4],
            overlay_layer: 9,
            overlay_tint: [5, 6, 7, 8],
        }
    }

    fn one_triangle() -> CpuMesh {
        let mut mesh = CpuMesh::new();
        mesh.push_indexed(
            [
                vertex([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
                vertex([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
                vertex([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ],
            [0, 1, 2],
        );
        mesh
    }

    #[test]
    fn transforming_moves_positions_and_keeps_the_index_list() {
        let mesh = one_triangle();
        let moved = mesh.transformed(
            Mat4::from_translation(Vec3::new(5.0, 0.0, -2.0)),
            Mat4::IDENTITY,
        );
        assert_eq!(moved.indices, mesh.indices);
        assert_eq!(moved.vertices[0].position, [5.0, 0.0, -2.0]);
        assert_eq!(moved.vertices[1].position, [6.0, 0.0, -2.0]);
    }

    /// The whole reason `normal_basis` is a separate argument: a held item is
    /// placed by a matrix carrying the camera's rotation, but must be lit as
    /// though it were not, or it pulses as the player spins.
    #[test]
    fn normals_follow_the_basis_and_not_the_transform() {
        let mesh = one_triangle();
        let spun = mesh.transformed(
            Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Mat4::IDENTITY,
        );
        assert_eq!(
            spun.vertices[0].normal,
            [0.0, 0.0, 1.0],
            "the identity basis must leave the normal alone"
        );

        let turned = mesh.transformed(
            Mat4::IDENTITY,
            Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
        );
        let n = Vec3::from(turned.vertices[0].normal);
        assert!(
            n.abs_diff_eq(Vec3::X, 1e-5),
            "normal not turned by the basis: {n}"
        );
        assert_eq!(
            turned.vertices[0].position,
            [0.0, 0.0, 0.0],
            "the identity transform must leave the position alone"
        );
    }

    /// Everything that does not depend on where the geometry ended up rides
    /// through untouched — a transformed mesh must still sample the same texel.
    #[test]
    fn everything_but_position_and_normal_survives() {
        let mesh = one_triangle();
        let out = mesh.transformed(Mat4::from_scale(Vec3::splat(3.0)), Mat4::IDENTITY);
        for (before, after) in mesh.vertices.iter().zip(&out.vertices) {
            assert_eq!(after.uv, before.uv);
            assert_eq!(after.ao, before.ao);
            assert_eq!(after.flags, before.flags);
            assert_eq!(after.layer, before.layer);
            assert_eq!(after.tint, before.tint);
        }
    }
}
