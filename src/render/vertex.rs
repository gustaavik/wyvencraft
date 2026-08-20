//! GPU vertex layouts. These derive vulkano's [`Vertex`] (for pipeline vertex
//! input) and [`BufferContents`] (for safe buffer uploads).

use vulkano::buffer::BufferContents;
use vulkano::pipeline::graphics::vertex_input::Vertex;

/// Marks water faces: the fragment shader cycles their tile through the
/// [`crate::render::tiles::WATER_FRAMES`] animation frames.
pub const FLAG_WATER: u32 = 1;

/// A vertex that draws its texture's own colour.
///
/// Tint multiplies, so white is the identity — which is why it is spelled out
/// rather than left to `Default`, whose zeroes would paint everything black.
pub const NO_TINT: [u8; 4] = [255; 4];

/// Vertex for voxel/chunk geometry: world position, face normal, texture UV, a
/// baked ambient-occlusion term, per-face shader-effect flags, and — for
/// geometry drawn against the block texture array — which layer to sample and
/// what to tint it by.
#[derive(BufferContents, Vertex, Clone, Copy, Debug)]
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
    /// Bit flags for shader effects (see [`FLAG_WATER`]).
    #[format(R32_UINT)]
    pub flags: u32,
    /// Layer of [`crate::render::block_textures`] to sample. Ignored by the
    /// atlas pipeline, which addresses its texture through `uv` alone.
    #[format(R32_UINT)]
    pub layer: u32,
    /// Multiplied into the sampled colour — biome tint for the faces a block
    /// model marked `tintindex`. [`NO_TINT`] leaves the texture as authored.
    #[format(R8G8B8A8_UNORM)]
    pub tint: [u8; 4],
}

impl Default for ChunkVertex {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            normal: [0.0; 3],
            uv: [0.0; 2],
            ao: 1.0,
            flags: 0,
            layer: 0,
            tint: NO_TINT,
        }
    }
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
