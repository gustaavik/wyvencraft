//! The animated texture a fluid cell draws from.
//!
//! A fluid cannot be an ordinary block model — its surface height is per-corner
//! and per-level, which baked quads cannot express — so it keeps the cube
//! mesher's fluid branch and takes its layers from here instead.

use wyven_render::block_textures::AnimatedLayers;

/// Where a fluid block's animation lives in the block texture array.
///
/// Both columns of the strip are resolved even for a source block: the
/// auto-registered flowing blocks share this entry, and which column a face
/// takes is a per-face decision the mesher makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidTexture {
    /// Column 1 — top and bottom faces, and every face of a source block.
    pub still: AnimatedLayers,
    /// Column 0 — the side faces of a flowing block.
    pub flowing: AnimatedLayers,
    pub fps: u8,
    /// `tintindex` into the biome colours; `2` is water.
    pub tint: Option<u8>,
}
