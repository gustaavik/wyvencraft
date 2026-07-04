//! Mesh data: a CPU-side builder ([`CpuMesh`]) produced by the chunk mesher, and
//! a GPU-resident ([`GpuMesh`]) holding the uploaded vertex/index buffers.

use std::sync::Arc;

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
