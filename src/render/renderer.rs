//! The world renderer: owns the voxel pipeline, the block-atlas descriptor, and
//! a depth buffer, and records the 3D scene pass into a target image using
//! dynamic rendering. egui is drawn as an overlay afterwards by the app.

use std::sync::Arc;

use glam::Mat4;
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, RenderingAttachmentInfo, RenderingInfo,
};
use vulkano::descriptor_set::{DescriptorSet, WriteDescriptorSet};
use vulkano::format::{ClearValue, Format};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::memory::allocator::AllocationCreateInfo;
use vulkano::pipeline::graphics::viewport::Viewport;
use vulkano::pipeline::graphics::GraphicsPipeline;
use vulkano::pipeline::{Pipeline, PipelineBindPoint};
use vulkano::render_pass::{AttachmentLoadOp, AttachmentStoreOp};
use vulkano::sync::GpuFuture;

use super::context::RenderContext;
use super::mesh::GpuMesh;
use super::pipeline::voxel;
use super::shaders;
use super::texture::TextureAtlas;

/// Sky/clear colour for the world pass.
const SKY_COLOR: [f32; 4] = [0.52, 0.70, 0.96, 1.0];
const DEPTH_FORMAT: Format = Format::D32_SFLOAT;

/// Everything the renderer needs to draw one frame of the 3D scene. Holds
/// borrowed GPU meshes owned by the active game state.
pub struct SceneFrame<'a> {
    pub view_proj: Mat4,
    pub opaque: Vec<&'a GpuMesh>,
}

pub struct Renderer {
    ctx: Arc<RenderContext>,
    voxel_pipeline: Arc<GraphicsPipeline>,
    atlas_set: Arc<DescriptorSet>,
    /// Cached depth buffer + the size it was created for.
    depth: Option<(Arc<ImageView>, [u32; 2])>,
    #[allow(dead_code)]
    atlas: TextureAtlas,
}

impl Renderer {
    pub fn new(ctx: Arc<RenderContext>, color_format: Format) -> Self {
        let voxel_pipeline = voxel::create(ctx.device().clone(), color_format, DEPTH_FORMAT);
        let atlas = TextureAtlas::create(&ctx);

        let set_layout = voxel_pipeline.layout().set_layouts()[0].clone();
        let atlas_set = DescriptorSet::new(
            ctx.descriptor_allocator.clone(),
            set_layout,
            [WriteDescriptorSet::image_view_sampler(
                0,
                atlas.image_view.clone(),
                atlas.sampler.clone(),
            )],
            [],
        )
        .expect("atlas descriptor set");

        Self {
            ctx,
            voxel_pipeline,
            atlas_set,
            depth: None,
            atlas,
        }
    }

    /// Create or reuse a depth buffer matching `size`.
    fn ensure_depth(&mut self, size: [u32; 2]) -> Arc<ImageView> {
        if let Some((view, cached)) = &self.depth {
            if *cached == size {
                return view.clone();
            }
        }
        let image = Image::new(
            self.ctx.memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                format: DEPTH_FORMAT,
                extent: [size[0], size[1], 1],
                usage: ImageUsage::DEPTH_STENCIL_ATTACHMENT,
                ..Default::default()
            },
            AllocationCreateInfo::default(),
        )
        .expect("depth image");
        let view = ImageView::new_default(image).expect("depth view");
        self.depth = Some((view.clone(), size));
        view
    }

    /// Record the world pass into `target`, chaining after `before`. Always
    /// clears (so it doubles as the background for menus when `scene` is `None`).
    pub fn draw(
        &mut self,
        before: Box<dyn GpuFuture>,
        target: Arc<ImageView>,
        scene: Option<&SceneFrame>,
    ) -> Box<dyn GpuFuture> {
        let extent = target.image().extent();
        let size = [extent[0], extent[1]];
        let depth = self.ensure_depth(size);

        let mut builder = AutoCommandBufferBuilder::primary(
            self.ctx.command_allocator.clone(),
            self.ctx.graphics_queue().queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .expect("scene command buffer");

        builder
            .begin_rendering(RenderingInfo {
                color_attachments: vec![Some(RenderingAttachmentInfo {
                    load_op: AttachmentLoadOp::Clear,
                    store_op: AttachmentStoreOp::Store,
                    clear_value: Some(ClearValue::Float(SKY_COLOR)),
                    ..RenderingAttachmentInfo::image_view(target.clone())
                })],
                depth_attachment: Some(RenderingAttachmentInfo {
                    load_op: AttachmentLoadOp::Clear,
                    store_op: AttachmentStoreOp::DontCare,
                    clear_value: Some(ClearValue::Depth(1.0)),
                    ..RenderingAttachmentInfo::image_view(depth)
                }),
                ..Default::default()
            })
            .expect("begin rendering");

        let viewport = Viewport {
            offset: [0.0, 0.0],
            extent: [size[0] as f32, size[1] as f32],
            depth_range: 0.0..=1.0,
        };
        builder
            .set_viewport(0, [viewport].into_iter().collect())
            .expect("set viewport");

        if let Some(scene) = scene {
            if !scene.opaque.is_empty() {
                let layout = self.voxel_pipeline.layout().clone();
                builder
                    .bind_pipeline_graphics(self.voxel_pipeline.clone())
                    .expect("bind voxel pipeline");
                builder
                    .bind_descriptor_sets(
                        PipelineBindPoint::Graphics,
                        layout.clone(),
                        0,
                        self.atlas_set.clone(),
                    )
                    .expect("bind atlas set");
                let push = shaders::voxel_vs::PushConstants {
                    view_proj: scene.view_proj.to_cols_array_2d(),
                };
                builder
                    .push_constants(layout, 0, push)
                    .expect("push view_proj");

                for mesh in &scene.opaque {
                    builder
                        .bind_vertex_buffers(0, mesh.vertex_buffer.clone())
                        .expect("bind vbuf");
                    builder
                        .bind_index_buffer(mesh.index_buffer.clone())
                        .expect("bind ibuf");
                    // SAFETY: pipeline, descriptor set, push constants and buffers
                    // are all bound and match the shader interface.
                    unsafe {
                        builder
                            .draw_indexed(mesh.index_count, 1, 0, 0, 0)
                            .expect("draw chunk");
                    }
                }
            }
        }

        builder.end_rendering().expect("end rendering");
        let command_buffer = builder.build().expect("build scene cb");

        before
            .then_execute(self.ctx.graphics_queue().clone(), command_buffer)
            .expect("execute scene cb")
            .boxed()
    }
}
