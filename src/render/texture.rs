//! Block texture atlas.
//!
//! The atlas pixels are assembled at startup by the tile registry
//! ([`super::tile_registry::TileRegistry::atlas_rgba`]) from PNGs and the
//! procedural pixel art in [`super::tiles`]. This module owns the atlas
//! layout (`tile index -> UV`) and the GPU upload ([`TextureAtlas`]).

use std::sync::Arc;

use vulkano::buffer::{Buffer, BufferCreateInfo, BufferUsage};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, CopyBufferToImageInfo,
    PrimaryCommandBufferAbstract,
};
use vulkano::format::Format;
use vulkano::image::sampler::{Filter, Sampler, SamplerAddressMode, SamplerCreateInfo};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::memory::allocator::{AllocationCreateInfo, MemoryTypeFilter};
use vulkano::sync::GpuFuture;

use super::context::RenderContext;

/// Atlas is a square grid of `ATLAS_COLUMNS x ATLAS_COLUMNS` tiles.
pub const ATLAS_COLUMNS: u32 = 16;
/// Pixels per tile edge.
pub const TILE_SIZE: u32 = 16;
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

/// GPU-resident block atlas: the sampled image view + a nearest-filtered sampler
/// (for the crisp, pixelated voxel look).
pub struct TextureAtlas {
    pub image_view: Arc<ImageView>,
    pub sampler: Arc<Sampler>,
}

impl TextureAtlas {
    /// Upload prebuilt atlas pixels (`ATLAS_SIZE^2 * 4` RGBA8 bytes) to
    /// device-local memory.
    pub fn create(ctx: &RenderContext, pixels: Vec<u8>) -> Self {
        assert_eq!(
            pixels.len(),
            (ATLAS_SIZE * ATLAS_SIZE * 4) as usize,
            "atlas pixel buffer has the wrong size"
        );

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
            pixels,
        )
        .expect("create atlas staging buffer");

        // Device-local sampled image.
        let image = Image::new(
            ctx.memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                format: Format::R8G8B8A8_UNORM,
                extent: [ATLAS_SIZE, ATLAS_SIZE, 1],
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo::default(),
        )
        .expect("create atlas image");

        // Record + submit the buffer→image copy and wait for it to finish.
        let mut builder = AutoCommandBufferBuilder::primary(
            ctx.command_allocator.clone(),
            ctx.graphics_queue().queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .expect("atlas upload command buffer");
        builder
            .copy_buffer_to_image(CopyBufferToImageInfo::buffer_image(staging, image.clone()))
            .expect("record atlas copy");
        builder
            .build()
            .expect("build atlas copy")
            .execute(ctx.graphics_queue().clone())
            .expect("submit atlas copy")
            .then_signal_fence_and_flush()
            .expect("flush atlas copy")
            .wait(None)
            .expect("wait atlas copy");

        let image_view = ImageView::new_default(image).expect("atlas image view");
        let sampler = Sampler::new(
            ctx.device().clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Nearest,
                min_filter: Filter::Nearest,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                ..Default::default()
            },
        )
        .expect("atlas sampler");

        Self {
            image_view,
            sampler,
        }
    }
}
