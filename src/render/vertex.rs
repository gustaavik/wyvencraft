//! GPU vertex layouts. These derive vulkano's [`Vertex`] (for pipeline vertex
//! input) and [`BufferContents`] (for safe buffer uploads).

use vulkano::buffer::BufferContents;
use vulkano::pipeline::graphics::vertex_input::Vertex;

/// Vertex for voxel/chunk geometry: world position, face normal, atlas UV, and a
/// baked ambient-occlusion term.
#[derive(BufferContents, Vertex, Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct ChunkVertex {
    #[format(R32G32B32_SFLOAT)]
    pub position: [f32; 3],
    #[format(R32G32B32_SFLOAT)]
    pub normal: [f32; 3],
    #[format(R32G32_SFLOAT)]
    pub uv: [f32; 2],
    /// Ambient occlusion / shading multiplier in `[0,1]`.
    #[format(R32_SFLOAT)]
    pub ao: f32,
}

/// Vertex for entity models and simple coloured geometry.
#[derive(BufferContents, Vertex, Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct EntityVertex {
    #[format(R32G32B32_SFLOAT)]
    pub position: [f32; 3],
    #[format(R32G32B32_SFLOAT)]
    pub normal: [f32; 3],
    #[format(R32G32_SFLOAT)]
    pub uv: [f32; 2],
}

/// Vertex for debug lines (block selection outline, wireframes).
#[derive(BufferContents, Vertex, Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct LineVertex {
    #[format(R32G32B32_SFLOAT)]
    pub position: [f32; 3],
    #[format(R32G32B32_SFLOAT)]
    pub color: [f32; 3],
}
