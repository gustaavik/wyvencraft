//! Sampled textures and the block-atlas layout.
//!
//! The atlas pixels are assembled at startup by the tile registry
//! ([`super::tile_registry::TileRegistry::atlas_rgba`]), which gets its art from
//! whatever [`TileSource`](super::tile_registry::TileSource) the game supplies.
//! This module owns the atlas layout (`tile index -> UV`) and the GPU upload.
//!
//! [`Texture`] is deliberately *not* atlas-shaped: the block atlas is one
//! instance of it, and a model loaded from a file brings its own. Each carries
//! the descriptor set that binds it, so the renderer can swap textures between
//! draws without knowing where the pixels came from.

use std::sync::Arc;

use vulkano::buffer::{Buffer, BufferCreateInfo, BufferUsage};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, CopyBufferToImageInfo,
    PrimaryCommandBufferAbstract,
};
use vulkano::descriptor_set::{DescriptorSet, WriteDescriptorSet};
use vulkano::format::Format;
use vulkano::image::sampler::{Filter, Sampler, SamplerAddressMode, SamplerCreateInfo};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter};
use vulkano::sync::GpuFuture;

pub use wyven_assets::{Rgba8, decode_png};

use super::context::RenderContext;
use super::pipeline::voxel;

/// Atlas is a square grid of `ATLAS_COLUMNS x ATLAS_COLUMNS` tiles.
pub const ATLAS_COLUMNS: u32 = 16;
/// Pixels per tile edge.
pub const TILE_SIZE: u32 = 32;
/// Full atlas edge length in pixels.
pub const ATLAS_SIZE: u32 = ATLAS_COLUMNS * TILE_SIZE;

/// Map an atlas tile index + a `[0,1]` local face UV into atlas texture
/// coordinates. Shared by the chunk mesher and entity-model builder.
pub fn atlas_uv(tile: u32, local_uv: [f32; 2]) -> [f32; 2] {
    let cols = ATLAS_COLUMNS as f32;
    let tx = (tile % ATLAS_COLUMNS) as f32;
    let ty = (tile / ATLAS_COLUMNS) as f32;
    [(tx + local_uv[0]) / cols, (ty + local_uv[1]) / cols]
}

/// A GPU-resident sampled texture plus the descriptor set that binds it as set 0
/// of the voxel pipeline. Nearest-filtered and clamped, for the crisp pixelated
/// look — which also matches what Blockbench's glTF exporter asks for
/// (`magFilter`/`minFilter` NEAREST, `wrapS`/`wrapT` CLAMP_TO_EDGE).
pub struct Texture {
    pub image_view: Arc<ImageView>,
    pub sampler: Arc<Sampler>,
    pub set: Arc<DescriptorSet>,
}

impl Texture {
    /// Upload `image` to device-local memory.
    pub fn create(ctx: &RenderContext, image: &Rgba8) -> Result<Self, String> {
        let [width, height] = image.size;
        let expected = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || image.pixels.len() != expected {
            return Err(format!(
                "texture is {width}x{height} but carries {} bytes (expected {expected})",
                image.pixels.len()
            ));
        }

        // Host-visible staging buffer.
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
            image.pixels.iter().copied(),
        )
        .map_err(|e| format!("staging buffer: {e}"))?;

        // Device-local sampled image.
        let gpu_image = Image::new(
            ctx.memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                format: Format::R8G8B8A8_UNORM,
                extent: [width, height, 1],
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo::default(),
        )
        .map_err(|e| format!("image: {e}"))?;

        // Record + submit the buffer→image copy and wait for it to finish.
        let mut builder = AutoCommandBufferBuilder::primary(
            ctx.command_allocator.clone(),
            ctx.graphics_queue().queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .map_err(|e| format!("upload command buffer: {e}"))?;
        builder
            .copy_buffer_to_image(CopyBufferToImageInfo::buffer_image(
                staging,
                gpu_image.clone(),
            ))
            .map_err(|e| format!("record copy: {e}"))?;
        builder
            .build()
            .map_err(|e| format!("build copy: {e}"))?
            .execute(ctx.graphics_queue().clone())
            .map_err(|e| format!("submit copy: {e}"))?
            .then_signal_fence_and_flush()
            .map_err(|e| format!("flush copy: {e}"))?
            .wait(None)
            .map_err(|e| format!("wait copy: {e}"))?;

        let image_view =
            ImageView::new_default(gpu_image).map_err(|e| format!("image view: {e}"))?;
        let sampler = Sampler::new(
            ctx.device().clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Nearest,
                min_filter: Filter::Nearest,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                ..Default::default()
            },
        )
        .map_err(|e| format!("sampler: {e}"))?;

        let set = DescriptorSet::new(
            ctx.descriptor_allocator.clone(),
            voxel::texture_set_layout(ctx.device()),
            [WriteDescriptorSet::image_view_sampler(
                0,
                image_view.clone(),
                sampler.clone(),
            )],
            [],
        )
        .map_err(|e| format!("descriptor set: {e}"))?;

        Ok(Self {
            image_view,
            sampler,
            set,
        })
    }

    /// Upload prebuilt block-atlas pixels (`ATLAS_SIZE^2 * 4` RGBA8 bytes).
    pub fn atlas(ctx: &RenderContext, pixels: Vec<u8>) -> Self {
        let image = Rgba8 {
            pixels,
            size: [ATLAS_SIZE; 2],
        };
        Self::create(ctx, &image).expect("block atlas upload")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×1 opaque-red RGBA PNG.
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00,
        0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn decodes_a_png_to_rgba8() {
        let image = decode_png(RED_PNG).expect("valid png");
        assert_eq!(image.size, [1, 1]);
        assert_eq!(image.pixels, vec![255, 0, 0, 255]);
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn atlas_uv_maps_tiles_into_the_grid() {
        assert_eq!(atlas_uv(0, [0.0, 0.0]), [0.0, 0.0]);
        let cols = ATLAS_COLUMNS as f32;
        assert_eq!(atlas_uv(1, [0.0, 0.0]), [1.0 / cols, 0.0]);
        assert_eq!(atlas_uv(ATLAS_COLUMNS, [0.0, 0.0]), [0.0, 1.0 / cols]);
    }
}
