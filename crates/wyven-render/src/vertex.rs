//! GPU vertex layouts. These derive vulkano's [`Vertex`] (for pipeline vertex
//! input) and [`BufferContents`] (for safe buffer uploads).

use vulkano::buffer::BufferContents;
use vulkano::pipeline::graphics::vertex_input::Vertex;

/// Bit layout of [`ChunkVertex::flags`]. Bits `0..8` are reserved for boolean
/// effect flags (there are none today); the animation of a block texture is
/// packed into the two bytes above them, so an animated face costs no extra
/// vertex attribute and no extra descriptor.
///
/// The array fragment shader mirrors these three constants — keep them in step
/// with `shaders/voxel_array.frag`.
pub const ANIM_FRAMES_SHIFT: u32 = 8;
/// Frames per second, in the byte above the frame count.
pub const ANIM_FPS_SHIFT: u32 = 16;
/// Width of both animation fields.
pub const ANIM_FIELD_MASK: u32 = 0xff;

/// Pack an animation into [`ChunkVertex::flags`].
///
/// `frames` is how many consecutive block-texture-array layers the animation
/// occupies; the shader steps the vertex's layer through them at `fps`. Fewer
/// than two frames is static, and packs to no animation at all.
#[inline]
pub const fn anim_flags(frames: u8, fps: u8) -> u32 {
    if frames < 2 {
        return 0;
    }
    ((frames as u32) << ANIM_FRAMES_SHIFT) | ((fps as u32) << ANIM_FPS_SHIFT)
}

/// A vertex that draws its texture's own colour.
///
/// Tint multiplies, so white is the identity — which is why it is spelled out
/// rather than left to `Default`, whose zeroes would paint everything black.
pub const NO_TINT: [u8; 4] = [255; 4];

/// [`ChunkVertex::overlay_layer`] for a face that has no overlay — almost all of
/// them. Spelled as a sentinel rather than a flag bit so the eight reserved low
/// bits of [`ChunkVertex::flags`] stay free; `voxel_array.frag` mirrors it.
pub const NO_OVERLAY: u32 = u32::MAX;

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
    /// Bit flags for shader effects (see [`anim_flags`]).
    #[format(R32_UINT)]
    pub flags: u32,
    /// Layer of [`crate::block_textures`] to sample. Ignored by the
    /// atlas pipeline, which addresses its texture through `uv` alone.
    #[format(R32_UINT)]
    pub layer: u32,
    /// Multiplied into the sampled colour — biome tint for the faces a block
    /// model marked `tintindex`. [`NO_TINT`] leaves the texture as authored.
    #[format(R8G8B8A8_UNORM)]
    pub tint: [u8; 4],
    /// A second block-texture-array layer, alpha-blended over `layer` by
    /// `voxel_array.frag`, or [`NO_OVERLAY`] for none.
    ///
    /// This is what lets the grass block's tinted side overlay ride on the same
    /// quad as the dirt side beneath it, instead of being a second quad in the
    /// same plane. Coincident geometry cannot be depth-ordered reliably — under
    /// the old forward-Z range the 2 mm the mesher pushes such faces apart was
    /// worth a fifth of a depth ULP at 128 blocks, and distant grass crawled as
    /// the camera turned. Blending sidesteps the ordering question entirely, and
    /// along the way replaces the fragment shader's hard alpha test on the
    /// overlay with a filtered edge.
    #[format(R32_UINT)]
    pub overlay_layer: u32,
    /// Biome tint for [`Self::overlay_layer`], independent of [`Self::tint`] —
    /// the grass block tints its overlay and not the side under it.
    #[format(R8G8B8A8_UNORM)]
    pub overlay_tint: [u8; 4],
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
            overlay_layer: NO_OVERLAY,
            overlay_tint: NO_TINT,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_packs_into_the_high_bytes() {
        let flags = anim_flags(64, 12);
        assert_eq!((flags >> ANIM_FRAMES_SHIFT) & ANIM_FIELD_MASK, 64);
        assert_eq!((flags >> ANIM_FPS_SHIFT) & ANIM_FIELD_MASK, 12);
        assert_eq!(flags & ANIM_FIELD_MASK, 0, "the low byte stays free");
    }

    /// A single frame is not an animation; the shader must see zero so it does
    /// not step the layer at all.
    #[test]
    fn a_static_texture_packs_to_no_animation() {
        assert_eq!(anim_flags(1, 12), 0);
        assert_eq!(anim_flags(0, 12), 0);
    }
}
