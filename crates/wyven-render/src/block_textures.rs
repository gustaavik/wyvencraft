//! The block texture array: one 256×256 layer per texture a block model names.
//!
//! Blocks authored in Blockbench ship their textures as ordinary PNGs, at a
//! resolution the 16×16-tile atlas in [`super::texture`] cannot hold — a 16×16
//! grid of 256-pixel tiles would be a 4096×4096 image with room for 256 of
//! them, and the atlas already spends most of its slots on the player skin,
//! armor and mob sheets, which are pinned to 16-pixel tiles.
//!
//! So block textures get their own image: a **2D array**, one layer per
//! texture, addressed by a layer index baked into the vertex rather than by a
//! UV offset. That buys three things an atlas cannot:
//!
//! - **No capacity cliff.** `maxImageArrayLayers` is 2048 on the target device,
//!   against an atlas's 256 tiles.
//! - **No bleeding.** Layers are separate images, so a UV of exactly `1.0` and
//!   `ClampToEdge` cannot wander into the neighbouring texture. The atlas gets
//!   away with this only because it is nearest-filtered with no mips.
//! - **Mipmaps**, which follow from the above. At 256 pixels a block face
//!   aliases badly at distance without them, and generating them for an atlas
//!   would need gutters the atlas does not have.
//!
//! Mip levels are built here on the CPU rather than blitted on the GPU: the
//! data is a few hundred kilobytes per layer, the filter is then testable
//! without a device, and it keeps the upload a single plain buffer copy.
//!
//! An **animated** texture is a strip of frames that takes one layer each,
//! consecutively ([`BlockTextureSet::resolve_strip`]); the shader steps the
//! vertex's layer index through them. Packing the frames into one layer instead
//! would be far cheaper, but it would give back both properties above — frames
//! would bleed into each other and their shared mip chain would be meaningless.
//! Water's two 64-frame columns are 128 layers, about 45 MB with mips.

use std::collections::HashMap;
use std::sync::Arc;

use vulkano::buffer::{Buffer, BufferCreateInfo, BufferUsage};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, BufferImageCopy, CommandBufferUsage, CopyBufferToImageInfo,
    PrimaryCommandBufferAbstract,
};
use vulkano::descriptor_set::{DescriptorSet, WriteDescriptorSet};
use vulkano::format::Format;
use vulkano::image::sampler::{
    Filter, Sampler, SamplerAddressMode, SamplerCreateInfo, SamplerMipmapMode,
};
use vulkano::image::view::{ImageView, ImageViewCreateInfo, ImageViewType};
use vulkano::image::{Image, ImageCreateInfo, ImageSubresourceLayers, ImageType, ImageUsage};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter};
use vulkano::sync::GpuFuture;

use super::context::RenderContext;
use super::pipeline::voxel_array;
use super::texture::{Rgba8, TILE_SIZE};
use super::tile_registry::TileRgba;

/// Edge length every block texture must have. One extent covers all layers of
/// an array image, so this is an equality, not a maximum.
pub const BLOCK_TEXTURE_SIZE: u32 = 256;

/// Anisotropy ceiling for the block texture array, clamped to the device limit.
/// 8 is the usual knee in the quality-per-bandwidth curve, and well under the 16
/// every desktop driver offers.
const MAX_ANISOTROPY: f32 = 8.0;

/// Full mip chain down to 1×1: `log2(256) + 1`.
pub const BLOCK_MIP_LEVELS: u32 = 9;

/// Sanity ceiling on the array. At 256 KB per layer this is 128 MB of base
/// level, and well under the device's 2048-layer limit — a content set that
/// hits it has almost certainly gone wrong.
pub const MAX_BLOCK_LAYERS: u32 = 512;

/// Layer 0, so a texture that failed to load is loudly wrong rather than
/// invisible. Matches the atlas's reserved tile 0.
const MISSING_TEXTURE: [u8; 4] = [255, 0, 255, 255];

/// Where an animation's frames live: `frames` consecutive layers starting at
/// `first`. A `frames` of 1 is an ordinary still texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimatedLayers {
    pub first: u32,
    pub frames: u8,
}

impl AnimatedLayers {
    /// The missing-texture marker, as one static frame.
    pub const MISSING: Self = Self {
        first: 0,
        frames: 1,
    };
}

/// Which part of an animation strip to cut out, and how opaque to make it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Strip {
    /// Column index; the column count follows from the frame size.
    pub column: u32,
    /// How many square frames are stacked top to bottom.
    pub frames: u32,
    /// Alpha the most opaque texel ends up at, everything else scaled with it.
    ///
    /// Fluid art is a greyscale *template* — the game decides its colour, and
    /// for the same reason it decides how much of the riverbed shows through.
    /// `None` keeps the alpha as authored.
    pub alpha: Option<u8>,
}

/// The CPU side: which layer each texture path was given, and its pixels.
///
/// Keyed by `assets/`-relative path, so two block models naming the same PNG
/// share one layer — the grass block and a future grass slab pay for
/// `grass_top.png` once.
pub struct BlockTextureSet {
    layers: Vec<Rgba8>,
    /// Whether each layer is fully opaque, which is what lets a face built from
    /// it occlude its neighbour.
    opaque: Vec<bool>,
    by_path: HashMap<String, u32>,
    /// Animation strips, keyed `<path>#<column>`. Separate from `by_path`
    /// because a strip's entry is a *run* of layers, and the run length has to
    /// come back from the cache rather than from the caller — two blocks naming
    /// the same strip with different frame counts must not read past it.
    strips: HashMap<String, AnimatedLayers>,
}

impl Default for BlockTextureSet {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockTextureSet {
    pub fn new() -> Self {
        Self {
            layers: vec![solid(MISSING_TEXTURE)],
            opaque: vec![true],
            by_path: HashMap::new(),
            strips: HashMap::new(),
        }
    }

    /// The layer `path` was given, adding `image` on first use.
    ///
    /// Art smaller than [`BLOCK_TEXTURE_SIZE`] is scaled up to it — an array
    /// image has one extent for every layer, so 16-pixel and 256-pixel textures
    /// can only coexist if the small ones are enlarged. Nearest replication at
    /// an integer factor is lossless in the sense that matters: the sampler
    /// magnifies with `Nearest`, so a 16×16 texture blown up 16× is
    /// pixel-identical to the original on screen, and mip level 4 of the result
    /// *is* the original.
    ///
    /// Fail-soft, like every other content load: art that cannot be scaled warns
    /// and takes the missing-texture layer, so one bad PNG costs its own block's
    /// appearance rather than the boot.
    pub fn resolve(&mut self, path: &str, image: &Rgba8) -> u32 {
        if let Some(&layer) = self.by_path.get(path) {
            return layer;
        }
        let layer = match self.prepare(path, image) {
            Ok(image) => {
                let index = self.layers.len() as u32;
                self.opaque.push(is_opaque(&image));
                self.layers.push(image);
                index
            }
            Err(err) => {
                log::warn!("ignoring block texture {path}: {err}");
                0
            }
        };
        self.by_path.insert(path.to_string(), layer);
        layer
    }

    /// The consecutive layers a `columns`-wide animation strip's `column` was
    /// given, adding its frames on first use.
    ///
    /// The strip is `frames` square frames stacked top to bottom, side by side
    /// in as many columns as its width allows — so the frame size, and with it
    /// the column count, are derived from the image rather than authored.
    /// Each frame then goes through [`BlockTextureSet::resolve`]'s own
    /// validation and scaling, so a 16-pixel frame is enlarged exactly like a
    /// 16-pixel texture.
    ///
    /// Fail-soft: a strip whose geometry does not work out warns and takes the
    /// missing-texture layer as a single static frame, so one bad asset costs
    /// its own block's appearance rather than the boot.
    pub fn resolve_strip(&mut self, path: &str, image: &Rgba8, strip: Strip) -> AnimatedLayers {
        // The requested opacity is part of the identity: two fluids sharing one
        // PNG at different opacities are two different runs of layers.
        let key = match strip.alpha {
            Some(alpha) => format!("{path}#{}@{alpha}", strip.column),
            None => format!("{path}#{}", strip.column),
        };
        if let Some(&cached) = self.strips.get(&key) {
            return cached;
        }
        let layers = match self.slice_strip(path, image, strip) {
            Ok(strip) => strip,
            Err(err) => {
                log::warn!("ignoring block texture strip {path}: {err}");
                AnimatedLayers::MISSING
            }
        };
        self.strips.insert(key, layers);
        layers
    }

    /// Cut one column out of the strip and register every frame of it.
    fn slice_strip(
        &mut self,
        path: &str,
        image: &Rgba8,
        strip: Strip,
    ) -> Result<AnimatedLayers, String> {
        let Strip {
            column,
            frames,
            alpha,
        } = strip;
        let [width, height] = image.size;
        if !(2..=u32::from(u8::MAX)).contains(&frames) {
            return Err(format!("frames must be 2..=255, got {frames}"));
        }
        if height == 0 || !height.is_multiple_of(frames) {
            return Err(format!(
                "{height}px tall does not divide into {frames} frames"
            ));
        }
        let size = height / frames;
        if size == 0 || !width.is_multiple_of(size) {
            return Err(format!(
                "{width}x{height} is not a whole number of {size}px columns"
            ));
        }
        let columns = width / size;
        if column >= columns {
            return Err(format!("column {column} of a {columns}-column strip"));
        }
        // Reserve the whole run before taking any of it: a strip half-registered
        // against a full array would animate into whatever came next.
        if self.layers.len() as u32 + frames > MAX_BLOCK_LAYERS {
            return Err(format!(
                "block texture array has no room for {frames} frames                  ({MAX_BLOCK_LAYERS} layers)"
            ));
        }
        // Scaled from the whole strip, not per frame, so every frame keeps the
        // same relationship to the others.
        let gain = alpha.and_then(|target| alpha_gain(image, target));
        let first = self.layers.len() as u32;
        for frame in 0..frames {
            let mut cropped = crop(image, column * size, frame * size, size);
            if let Some(gain) = gain {
                apply_alpha_gain(&mut cropped, gain);
            }
            let prepared = self.prepare(path, &cropped)?;
            self.opaque.push(is_opaque(&prepared));
            self.layers.push(prepared);
        }
        Ok(AnimatedLayers {
            first,
            frames: frames as u8,
        })
    }

    /// Validate `image` and bring it up to the array's extent.
    fn prepare(&self, path: &str, image: &Rgba8) -> Result<Rgba8, String> {
        if self.layers.len() as u32 >= MAX_BLOCK_LAYERS {
            return Err(format!(
                "block texture array is full ({MAX_BLOCK_LAYERS} layers), {path}"
            ));
        }
        let [width, height] = image.size;
        if width != height {
            return Err(format!(
                "block textures must be square, got {width}x{height}"
            ));
        }
        if width == BLOCK_TEXTURE_SIZE {
            return Ok(image.clone());
        }
        // Only an exact integer factor keeps every source texel a whole block of
        // destination texels; anything else would blur pixel art or leave a
        // partial row, and is far more likely a mistake than an intention.
        if width == 0 || !BLOCK_TEXTURE_SIZE.is_multiple_of(width) {
            return Err(format!(
                "block textures must be {BLOCK_TEXTURE_SIZE}px or an exact fraction of it, \
                 got {width}x{height}"
            ));
        }
        Ok(upscale(image, BLOCK_TEXTURE_SIZE / width))
    }

    /// Whether `layer`'s texture has no transparent texel — a face drawn from it
    /// hides whatever is behind.
    pub fn is_opaque(&self, layer: u32) -> bool {
        self.opaque.get(layer as usize).copied().unwrap_or(false)
    }

    /// A layer's pixels, for deriving the small atlas tile a dropped item and an
    /// inventory icon still sample.
    pub fn layer(&self, layer: u32) -> Option<&Rgba8> {
        self.layers.get(layer as usize)
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        // Layer 0 is always the missing marker, so the set is never truly empty.
        false
    }

    /// Every layer's full mip chain, ordered the way the GPU upload wants it:
    /// mip level outermost, layer innermost, tightly packed.
    fn upload_bytes(&self) -> Vec<u8> {
        let chains: Vec<Vec<Rgba8>> = self.layers.iter().map(mip_chain).collect();
        let mut out = Vec::new();
        for level in 0..BLOCK_MIP_LEVELS as usize {
            for chain in &chains {
                out.extend_from_slice(&chain[level].pixels);
            }
        }
        out
    }
}

/// A uniform image of `BLOCK_TEXTURE_SIZE` square.
fn solid(rgba: [u8; 4]) -> Rgba8 {
    let count = (BLOCK_TEXTURE_SIZE * BLOCK_TEXTURE_SIZE) as usize;
    Rgba8 {
        pixels: rgba.repeat(count),
        size: [BLOCK_TEXTURE_SIZE; 2],
    }
}

fn is_opaque(image: &Rgba8) -> bool {
    image.pixels.chunks_exact(4).all(|px| px[3] == 255)
}

/// Factor that brings `image`'s most opaque texel to `target`, or `None` when
/// there is nothing to scale (fully transparent art, or already exact).
fn alpha_gain(image: &Rgba8, target: u8) -> Option<f32> {
    let peak = image.pixels.chunks_exact(4).map(|px| px[3]).max()?;
    (peak > 0 && peak != target).then(|| f32::from(target) / f32::from(peak))
}

/// Scale every texel's alpha by `gain`, saturating at fully opaque.
fn apply_alpha_gain(image: &mut Rgba8, gain: f32) {
    for px in image.pixels.chunks_exact_mut(4) {
        px[3] = (f32::from(px[3]) * gain).round().clamp(0.0, 255.0) as u8;
    }
}

/// The `size`-square region of `image` with its top-left corner at (`x`, `y`).
fn crop(image: &Rgba8, x: u32, y: u32, size: u32) -> Rgba8 {
    let [w, _] = image.size;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for row in y..y + size {
        let start = ((row * w + x) * 4) as usize;
        pixels.extend_from_slice(&image.pixels[start..start + (size * 4) as usize]);
    }
    Rgba8 {
        pixels,
        size: [size; 2],
    }
}

/// Replicate every texel of `image` into a `factor`×`factor` block.
///
/// The inverse of [`downsample_half`] applied `log2(factor)` times, and exact:
/// downsampling the result back reproduces the input bit-for-bit.
pub fn upscale(image: &Rgba8, factor: u32) -> Rgba8 {
    let [w, h] = image.size;
    let (dw, dh) = (w * factor, h * factor);
    let mut pixels = Vec::with_capacity((dw * dh * 4) as usize);
    for y in 0..dh {
        let row = ((y / factor) * w) as usize;
        for x in 0..dw {
            let i = (row + (x / factor) as usize) * 4;
            pixels.extend_from_slice(&image.pixels[i..i + 4]);
        }
    }
    Rgba8 {
        pixels,
        size: [dw, dh],
    }
}

/// Halve `image` with a box filter.
///
/// The colour average is weighted by alpha so a cutout texture's transparent
/// texels — which typically hold garbage colour — cannot bleed into the visible
/// half of an edge as it shrinks.
pub fn downsample_half(image: &Rgba8) -> Rgba8 {
    let [w, h] = image.size;
    let (dw, dh) = ((w / 2).max(1), (h / 2).max(1));
    let mut pixels = Vec::with_capacity((dw * dh * 4) as usize);
    let at = |x: u32, y: u32| {
        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
        &image.pixels[i..i + 4]
    };
    for y in 0..dh {
        for x in 0..dw {
            let quad = [
                at(x * 2, y * 2),
                at(x * 2 + 1, y * 2),
                at(x * 2, y * 2 + 1),
                at(x * 2 + 1, y * 2 + 1),
            ];
            let alpha: u32 = quad.iter().map(|p| p[3] as u32).sum();
            let channel = |c: usize| -> u8 {
                let weighted: u32 = quad.iter().map(|p| p[c] as u32 * p[3] as u32).sum();
                match weighted.checked_div(alpha) {
                    Some(average) => average as u8,
                    // Nothing visible here; keep the plain average so the colour
                    // stays meaningful if something later reads it.
                    None => (quad.iter().map(|p| p[c] as u32).sum::<u32>() / 4) as u8,
                }
            };
            pixels.extend_from_slice(&[channel(0), channel(1), channel(2), (alpha / 4) as u8]);
        }
    }
    Rgba8 {
        pixels,
        size: [dw, dh],
    }
}

/// `image` plus every halving of it down to 1×1, `BLOCK_MIP_LEVELS` in all.
pub fn mip_chain(image: &Rgba8) -> Vec<Rgba8> {
    let mut chain = Vec::with_capacity(BLOCK_MIP_LEVELS as usize);
    chain.push(image.clone());
    for level in 1..BLOCK_MIP_LEVELS as usize {
        chain.push(downsample_half(&chain[level - 1]));
    }
    chain
}

/// Reduce a block texture to one atlas tile.
///
/// Item icons and dropped-item cubes still sample the 16-pixel atlas, so a
/// block modelled in Blockbench needs a small stand-in for each of its faces.
/// Deriving it from the model's own texture is what keeps the two from drifting.
pub fn to_atlas_tile(image: &Rgba8) -> TileRgba {
    let mut current = image.clone();
    while current.size[0] > TILE_SIZE && current.size[1] > TILE_SIZE {
        current = downsample_half(&current);
    }
    let [w, h] = current.size;
    std::array::from_fn(|y| {
        std::array::from_fn(|x| {
            let sx = (x as u32).min(w - 1);
            let sy = (y as u32).min(h - 1);
            let i = ((sy * w + sx) * 4) as usize;
            [
                current.pixels[i],
                current.pixels[i + 1],
                current.pixels[i + 2],
                current.pixels[i + 3],
            ]
        })
    })
}

/// The GPU side: the array image, its sampler, and the descriptor set binding
/// them as set 0 of the array voxel pipeline.
pub struct BlockTextureArray {
    pub image_view: Arc<ImageView>,
    pub sampler: Arc<Sampler>,
    pub set: Arc<DescriptorSet>,
}

impl BlockTextureArray {
    pub fn create(ctx: &RenderContext, textures: &BlockTextureSet) -> Result<Self, String> {
        let layers = textures.len() as u32;
        let pixels = textures.upload_bytes();

        let staging = Buffer::from_iter(
            ctx.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_SRC,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            pixels.iter().copied(),
        )
        .map_err(|e| format!("block texture staging buffer: {e}"))?;

        let image = Image::new(
            ctx.memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                format: Format::R8G8B8A8_UNORM,
                extent: [BLOCK_TEXTURE_SIZE, BLOCK_TEXTURE_SIZE, 1],
                array_layers: layers,
                mip_levels: BLOCK_MIP_LEVELS,
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo::default(),
        )
        .map_err(|e| format!("block texture array image: {e}"))?;

        // One region per mip level, each covering every layer — which is exactly
        // how `upload_bytes` orders the pixels.
        let mut offset = 0u64;
        let mut regions = Vec::with_capacity(BLOCK_MIP_LEVELS as usize);
        for level in 0..BLOCK_MIP_LEVELS {
            let size = (BLOCK_TEXTURE_SIZE >> level).max(1);
            regions.push(BufferImageCopy {
                buffer_offset: offset,
                image_subresource: ImageSubresourceLayers {
                    mip_level: level,
                    array_layers: 0..layers,
                    ..ImageSubresourceLayers::from_parameters(Format::R8G8B8A8_UNORM, layers)
                },
                image_extent: [size, size, 1],
                ..Default::default()
            });
            offset += (size as u64) * (size as u64) * 4 * (layers as u64);
        }

        let mut builder = AutoCommandBufferBuilder::primary(
            ctx.command_allocator.clone(),
            ctx.graphics_queue().queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|e| format!("block texture upload command buffer: {e}"))?;
        builder
            .copy_buffer_to_image(CopyBufferToImageInfo {
                regions: regions.into(),
                ..CopyBufferToImageInfo::buffer_image(staging, image.clone())
            })
            .map_err(|e| format!("record block texture copy: {e}"))?;
        builder
            .build()
            .map_err(|e| format!("build block texture copy: {e}"))?
            .execute(ctx.graphics_queue().clone())
            .map_err(|e| format!("submit block texture copy: {e}"))?
            .then_signal_fence_and_flush()
            .map_err(|e| format!("flush block texture copy: {e}"))?
            .wait(None)
            .map_err(|e| format!("wait block texture copy: {e}"))?;

        // Always an array view, even at one layer: the shader binds
        // `sampler2DArray` and a plain 2D view would not match it.
        let image_view = ImageView::new(
            image.clone(),
            ImageViewCreateInfo {
                view_type: ImageViewType::Dim2dArray,
                ..ImageViewCreateInfo::from_image(&image)
            },
        )
        .map_err(|e| format!("block texture array view: {e}"))?;

        // Nearest magnification keeps the art pixel-crisp up close; linear
        // minification through the mip chain is what stops distant terrain
        // shimmering, which 256-pixel faces do badly without it.
        //
        // Anisotropy is the other half of that. A mip level is chosen for the
        // worse-compressed axis, so a ground plane seen edge-on — most of what a
        // voxel world shows at range — is blurred along one axis and still
        // aliased along the other, and the level flips as the camera turns.
        // Gated on the feature actually being enabled so the sampler stays valid
        // if that request is ever relaxed, and clamped to what the device offers.
        let anisotropy = ctx
            .device()
            .enabled_features()
            .sampler_anisotropy
            .then(|| {
                let limit = ctx
                    .device()
                    .physical_device()
                    .properties()
                    .max_sampler_anisotropy;
                MAX_ANISOTROPY.min(limit)
            });
        let sampler = Sampler::new(
            ctx.device().clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Nearest,
                min_filter: Filter::Linear,
                mipmap_mode: SamplerMipmapMode::Linear,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                lod: 0.0..=(BLOCK_MIP_LEVELS as f32),
                anisotropy,
                ..Default::default()
            },
        )
        .map_err(|e| format!("block texture sampler: {e}"))?;

        let set = DescriptorSet::new(
            ctx.descriptor_allocator.clone(),
            voxel_array::texture_set_layout(ctx.device()),
            [WriteDescriptorSet::image_view_sampler(
                0,
                image_view.clone(),
                sampler.clone(),
            )],
            [],
        )
        .map_err(|e| format!("block texture descriptor set: {e}"))?;

        log::info!(
            "block texture array: {layers} layers of {BLOCK_TEXTURE_SIZE}px, \
             {BLOCK_MIP_LEVELS} mips, anisotropy {}",
            match anisotropy {
                Some(n) => n.to_string(),
                None => "off".to_string(),
            }
        );

        Ok(Self {
            image_view,
            sampler,
            set,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(size: u32, fill: impl Fn(u32, u32) -> [u8; 4]) -> Rgba8 {
        let mut pixels = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                pixels.extend_from_slice(&fill(x, y));
            }
        }
        Rgba8 {
            pixels,
            size: [size; 2],
        }
    }

    fn block_texture(rgba: [u8; 4]) -> Rgba8 {
        image(BLOCK_TEXTURE_SIZE, |_, _| rgba)
    }

    /// The plain, as-authored cut of one column.
    fn cut(column: u32, frames: u32) -> Strip {
        Strip {
            column,
            frames,
            alpha: None,
        }
    }

    /// An animation strip: `columns` columns of `frames` square frames, every
    /// texel of a frame painted `[column, frame, 0, alpha]` so a single sample
    /// identifies which crop it came from.
    fn strip_art(size: u32, columns: u32, frames: u32, alpha: u8) -> Rgba8 {
        let (w, h) = (size * columns, size * frames);
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[(x / size) as u8, (y / size) as u8, 0, alpha]);
            }
        }
        Rgba8 {
            pixels,
            size: [w, h],
        }
    }

    #[test]
    fn layer_zero_is_the_missing_marker() {
        let set = BlockTextureSet::new();
        assert_eq!(set.len(), 1);
        assert_eq!(&set.layer(0).expect("marker").pixels[..4], &MISSING_TEXTURE);
    }

    #[test]
    fn the_same_path_shares_one_layer() {
        let mut set = BlockTextureSet::new();
        let art = block_texture([1, 2, 3, 255]);
        let first = set.resolve("assets/textures/blocks/a.png", &art);
        let second = set.resolve("assets/textures/blocks/a.png", &art);
        assert_eq!(first, second);
        assert_eq!(set.len(), 2, "one marker plus one texture");
    }

    #[test]
    fn different_paths_get_different_layers() {
        let mut set = BlockTextureSet::new();
        let a = set.resolve(
            "assets/textures/blocks/a.png",
            &block_texture([1, 1, 1, 255]),
        );
        let b = set.resolve(
            "assets/textures/blocks/b.png",
            &block_texture([2, 2, 2, 255]),
        );
        assert_ne!(a, b);
        assert!(a != 0 && b != 0, "content must not land on the marker");
    }

    /// 16-pixel and 256-pixel art has to coexist in one array, so the small
    /// art is enlarged rather than rejected — and enlarging must not change a
    /// single pixel of what ends up on screen.
    #[test]
    fn a_smaller_texture_is_scaled_up_to_the_array_extent() {
        let mut set = BlockTextureSet::new();
        let art = image(16, |x, y| [x as u8, y as u8, 0, 255]);
        let layer = set.resolve("assets/textures/blocks/small.png", &art);
        assert_ne!(layer, 0, "it should take a real layer");

        let stored = set.layer(layer).expect("layer");
        assert_eq!(stored.size, [BLOCK_TEXTURE_SIZE; 2]);
        // Every source texel became a 16x16 block of destination texels.
        let at = |x: u32, y: u32| {
            let i = ((y * BLOCK_TEXTURE_SIZE + x) * 4) as usize;
            stored.pixels[i..i + 4].to_vec()
        };
        assert_eq!(at(0, 0), vec![0, 0, 0, 255]);
        assert_eq!(
            at(15, 15),
            vec![0, 0, 0, 255],
            "still the first source texel"
        );
        assert_eq!(at(16, 0), vec![1, 0, 0, 255], "the next one over");
        assert_eq!(at(255, 255), vec![15, 15, 0, 255]);
    }

    /// Downsampling is how the inventory stand-in tile is derived, so the two
    /// have to be exact inverses or a 16px block's icon would drift.
    #[test]
    fn scaling_up_and_back_down_is_lossless() {
        let art = image(16, |x, y| [x as u8 * 3, y as u8 * 5, 7, 255]);
        let mut scaled = upscale(&art, 16);
        for _ in 0..4 {
            scaled = downsample_half(&scaled);
        }
        assert_eq!(scaled.size, [16, 16]);
        assert_eq!(scaled.pixels, art.pixels);
    }

    /// A size that is not an exact fraction would have to blur or leave a
    /// partial row, and is far likelier a mistake than an intention.
    #[test]
    fn art_that_does_not_divide_the_extent_falls_back_to_the_marker() {
        let mut set = BlockTextureSet::new();
        for (name, art) in [
            ("odd", image(17, |_, _| [0, 0, 0, 255])),
            ("huge", image(512, |_, _| [0, 0, 0, 255])),
        ] {
            assert_eq!(set.resolve(name, &art), 0, "{name} should be refused");
        }
        assert_eq!(set.len(), 1, "no bad texture may take a layer");
    }

    #[test]
    fn non_square_art_falls_back_to_the_marker() {
        let mut set = BlockTextureSet::new();
        let art = Rgba8 {
            pixels: vec![255; 16 * 32 * 4],
            size: [16, 32],
        };
        assert_eq!(set.resolve("strip.png", &art), 0);
    }

    #[test]
    fn opacity_is_recorded_per_layer() {
        let mut set = BlockTextureSet::new();
        let solid = set.resolve("a.png", &block_texture([0, 0, 0, 255]));
        let cutout = set.resolve("b.png", &block_texture([0, 0, 0, 0]));
        assert!(set.is_opaque(solid));
        assert!(!set.is_opaque(cutout));
    }

    #[test]
    fn the_mip_chain_halves_down_to_one_pixel() {
        let chain = mip_chain(&block_texture([9, 9, 9, 255]));
        assert_eq!(chain.len(), BLOCK_MIP_LEVELS as usize);
        assert_eq!(chain[0].size, [256, 256]);
        assert_eq!(chain[1].size, [128, 128]);
        assert_eq!(chain.last().expect("last").size, [1, 1]);
    }

    #[test]
    fn downsampling_averages_the_four_source_texels() {
        let art = image(2, |x, _| {
            if x == 0 {
                [0, 0, 0, 255]
            } else {
                [100, 100, 100, 255]
            }
        });
        let half = downsample_half(&art);
        assert_eq!(half.size, [1, 1]);
        assert_eq!(half.pixels, vec![50, 50, 50, 255]);
    }

    /// A cutout texture's invisible texels usually hold arbitrary colour; an
    /// unweighted average would drag it into the visible edge as the mip shrinks.
    #[test]
    fn transparent_texels_do_not_tint_the_average() {
        let art = image(2, |x, _| {
            if x == 0 {
                [200, 0, 0, 255]
            } else {
                [0, 0, 255, 0]
            }
        });
        let half = downsample_half(&art);
        assert_eq!(half.pixels[0], 200, "the visible red must survive intact");
        assert_eq!(half.pixels[2], 0, "the invisible blue must not bleed in");
        assert_eq!(half.pixels[3], 127, "alpha still averages plainly");
    }

    #[test]
    fn a_block_texture_reduces_to_one_atlas_tile() {
        let tile = to_atlas_tile(&block_texture([7, 8, 9, 255]));
        assert_eq!(tile.len(), TILE_SIZE as usize);
        assert_eq!(tile[0].len(), TILE_SIZE as usize);
        assert_eq!(tile[0][0], [7, 8, 9, 255]);
        assert_eq!(tile[15][15], [7, 8, 9, 255]);
    }

    #[test]
    fn the_upload_is_ordered_mip_major_and_tightly_packed() {
        let mut set = BlockTextureSet::new();
        set.resolve("a.png", &block_texture([1, 1, 1, 255]));
        let layers = set.len() as u64;
        let expected: u64 = (0..BLOCK_MIP_LEVELS)
            .map(|level| {
                let size = (BLOCK_TEXTURE_SIZE >> level).max(1) as u64;
                size * size * 4 * layers
            })
            .sum();
        assert_eq!(set.upload_bytes().len() as u64, expected);
    }

    /// A strip of animation frames takes one layer each, consecutively — the
    /// shader steps the layer index, so the run must be unbroken.
    #[test]
    fn a_strip_column_takes_one_layer_per_frame() {
        let mut set = BlockTextureSet::new();
        let art = strip_art(16, 2, 4, 255);
        let flow = set.resolve_strip("water.png", &art, cut(0, 4));
        assert_eq!(flow.frames, 4);
        assert_eq!(flow.first, 1, "straight after the missing marker");
        assert_eq!(set.len(), 5);

        let still = set.resolve_strip("water.png", &art, cut(1, 4));
        assert_eq!(still.first, 5, "the second column follows the first");
        assert_eq!(still.frames, 4);
        assert_eq!(set.len(), 9, "the two columns share no layer");
    }

    /// Each frame is the cropped sub-image, enlarged exactly as a texture of
    /// that size would have been.
    #[test]
    fn every_frame_is_its_own_crop_scaled_up() {
        let mut set = BlockTextureSet::new();
        let art = strip_art(16, 2, 4, 255);
        let column = set.resolve_strip("water.png", &art, cut(1, 4));
        for frame in 0..4u32 {
            let layer = set.layer(column.first + frame).expect("frame layer");
            assert_eq!(layer.size, [BLOCK_TEXTURE_SIZE; 2]);
            // The strip paints every texel with its column and frame index, so
            // one sample identifies which crop landed here.
            assert_eq!(&layer.pixels[..4], &[1, frame as u8, 0, 255]);
        }
    }

    #[test]
    fn the_same_column_of_the_same_strip_is_resolved_once() {
        let mut set = BlockTextureSet::new();
        let art = strip_art(16, 2, 4, 255);
        let first = set.resolve_strip("water.png", &art, cut(0, 4));
        let second = set.resolve_strip("water.png", &art, cut(0, 4));
        assert_eq!(first, second);
        assert_eq!(set.len(), 5, "one marker plus four frames");
        // The run length comes back from the cache, not from the caller: a
        // second block asking for more frames than were reserved would
        // otherwise animate off the end of the run.
        assert_eq!(set.resolve_strip("water.png", &art, cut(0, 64)), first);
    }

    #[test]
    fn a_strip_that_does_not_divide_evenly_falls_back_to_the_marker() {
        let mut set = BlockTextureSet::new();
        for (name, art, strip) in [
            ("uneven height", strip_art(16, 2, 4, 255), cut(0, 3)),
            (
                "frame size not a fraction of the extent",
                strip_art(17, 2, 4, 255),
                cut(0, 4),
            ),
            ("column past the end", strip_art(16, 2, 4, 255), cut(2, 4)),
            ("not an animation", strip_art(16, 2, 4, 255), cut(0, 1)),
            (
                "more frames than the format holds",
                strip_art(1, 1, 256, 255),
                cut(0, 256),
            ),
        ] {
            assert_eq!(
                set.resolve_strip(name, &art, strip),
                AnimatedLayers::MISSING,
                "{name} should be refused"
            );
        }
        assert_eq!(set.len(), 1, "no bad strip may take a layer");
    }

    /// Fluid art is a template: the game decides how much of the riverbed shows
    /// through, so a strip's alpha is rescaled to what the block asked for —
    /// upward as readily as downward.
    #[test]
    fn opacity_rescales_the_strips_alpha() {
        let mut set = BlockTextureSet::new();
        let art = strip_art(16, 2, 4, 89);
        let column = set.resolve_strip(
            "water.png",
            &art,
            Strip {
                column: 0,
                frames: 4,
                alpha: Some(217),
            },
        );
        let layer = set.layer(column.first).expect("frame layer");
        assert_eq!(layer.pixels[3], 217);
        assert!(
            !set.is_opaque(column.first),
            "still blends, so it occludes nothing"
        );
    }

    /// Two fluids sharing one PNG at different opacities are two different runs
    /// of layers — one must not be served the other's cached alpha.
    #[test]
    fn opacity_is_part_of_a_strips_identity() {
        let mut set = BlockTextureSet::new();
        let art = strip_art(16, 2, 4, 89);
        let strip = |alpha| Strip {
            column: 0,
            frames: 4,
            alpha,
        };
        let thin = set.resolve_strip("water.png", &art, strip(Some(60)));
        let thick = set.resolve_strip("water.png", &art, strip(Some(217)));
        let plain = set.resolve_strip("water.png", &art, strip(None));
        assert_ne!(thin.first, thick.first);
        assert_ne!(plain.first, thick.first);
        assert_eq!(set.layer(thin.first).expect("layer").pixels[3], 60);
        assert_eq!(set.layer(thick.first).expect("layer").pixels[3], 217);
        assert_eq!(set.layer(plain.first).expect("layer").pixels[3], 89);
    }

    /// A strip half-registered against a full array would animate into whatever
    /// texture came after it, so the whole run is reserved or none of it is.
    #[test]
    fn a_strip_with_no_room_takes_no_layers_at_all() {
        let mut set = BlockTextureSet::new();
        while (set.len() as u32) < MAX_BLOCK_LAYERS - 2 {
            let name = format!("filler{}.png", set.len());
            set.resolve(&name, &block_texture([0, 0, 0, 255]));
        }
        let before = set.len();
        assert_eq!(
            set.resolve_strip("water.png", &strip_art(16, 2, 4, 255), cut(0, 4)),
            AnimatedLayers::MISSING
        );
        assert_eq!(set.len(), before, "the partial run must not be kept");
    }
}
