//! The world renderer: owns the voxel pipeline, the block-atlas descriptor, and
//! a depth buffer, and records the 3D scene pass into a target image using
//! dynamic rendering. egui is drawn as an overlay afterwards by the app.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, ClearAttachment, ClearRect, CommandBufferUsage,
    PrimaryAutoCommandBuffer, RenderingAttachmentInfo, RenderingInfo,
};
use vulkano::descriptor_set::DescriptorSet;
use vulkano::format::{ClearValue, Format};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::memory::allocator::AllocationCreateInfo;
use vulkano::pipeline::graphics::GraphicsPipeline;
use vulkano::pipeline::graphics::viewport::Viewport;
use vulkano::pipeline::{Pipeline, PipelineBindPoint};
use vulkano::render_pass::{AttachmentLoadOp, AttachmentStoreOp};
use vulkano::sync::GpuFuture;

use super::block_textures::{BlockTextureArray, BlockTextureSet};
use super::context::RenderContext;
use super::icons;
use super::mesh::{GpuLines, GpuMesh};
use super::pipeline::{line, sky, voxel, voxel_array};
use super::shaders;
use super::texture::Texture;

/// Fallback clear colour used as the menu background (when there is no scene to
/// draw the procedural sky behind).
const SKY_COLOR: [f32; 4] = [0.52, 0.70, 0.96, 1.0];
/// Depth attachment format for every pass here.
///
/// A float format is what makes **reversed-Z** worth having: the near plane maps
/// to `1.0` and the far plane to `0.0` (see `Camera::projection_matrix`), so the
/// depths that a conventional range would bunch against 1.0 instead spread
/// across the exponent range near 0. Consequently every depth attachment below
/// clears to `DEPTH_CLEAR` rather than `1.0`, and every pipeline that tests
/// depth uses `CompareOp::Greater`.
const DEPTH_FORMAT: Format = Format::D32_SFLOAT;
/// "Nothing has been drawn here yet" under reversed-Z — the far plane.
const DEPTH_CLEAR: f32 = 0.0;
/// Backdrop for the inventory player preview: a near-black opaque fill matching
/// the mockup's box. Opaque (not transparent) sidesteps premultiplied-alpha
/// halos when egui composites the sampled image over the inventory panel.
const PREVIEW_BG: [f32; 4] = [0.02, 0.02, 0.03, 1.0];

/// Lighting for the item-icon sheet: mostly ambient, with a soft key from the
/// upper front-left so a model still reads as solid. Fixed rather than tied to
/// the day/night cycle — icons are rendered once, and a hotbar that dimmed at
/// dusk would just look broken.
const ICON_LIGHT: LightParams = LightParams {
    light_dir: Vec3::new(-0.35, 0.75, 0.55),
    light_color: Vec3::new(1.0, 1.0, 1.0),
    ambient: 0.62,
};

/// Parameters for the procedural sky pass, derived from the day/night cycle.
#[derive(Clone, Copy)]
pub struct SkyParams {
    /// Inverse of (projection * translation-free view) — unprojects to a view ray.
    pub inv_view_proj: Mat4,
    /// Direction toward the sun (world space).
    pub sun_dir: Vec3,
    pub zenith_color: Vec3,
    pub horizon_color: Vec3,
    pub sun_color: Vec3,
    pub star_intensity: f32,
    pub moon_intensity: f32,
}

/// World directional-lighting parameters, derived from the day/night cycle.
#[derive(Clone, Copy)]
pub struct LightParams {
    /// Direction toward the dominant light (sun by day, moon at night).
    pub light_dir: Vec3,
    pub light_color: Vec3,
    pub ambient: f32,
}

/// A mesh that samples its own texture instead of the shared block atlas — what
/// geometry loaded from a model file needs, since its texture is authored
/// alongside the model rather than allocated a slot in the atlas.
#[derive(Clone, Copy)]
pub struct TexturedMesh<'a> {
    pub mesh: &'a GpuMesh,
    pub texture: &'a Texture,
}

/// Everything the renderer needs to draw one frame of the 3D scene. Holds
/// borrowed GPU meshes owned by the active game state.
pub struct SceneFrame<'a> {
    pub view_proj: Mat4,
    pub sky: SkyParams,
    pub light: LightParams,
    /// Seconds of in-game time, driving shader animation (water frames).
    pub time: f32,
    pub opaque: Vec<&'a GpuMesh>,
    pub transparent: Vec<&'a GpuMesh>,
    /// Chunk geometry from Blockbench-authored blocks, sampling the block
    /// texture array rather than the atlas. Same two passes as `opaque` /
    /// `transparent`; a separate list only because it binds a different set 0.
    pub array_opaque: Vec<&'a GpuMesh>,
    pub array_transparent: Vec<&'a GpuMesh>,
    /// Opaque geometry that brings its own texture, drawn after `opaque`.
    pub textured: Vec<TexturedMesh<'a>>,
    /// Debug lines drawn on top of the world (block selection outline).
    pub lines: Option<&'a GpuLines>,
    /// Geometry drawn in front of the finished world under its own camera — a
    /// first-person view model. `None` when there is nothing in front.
    ///
    /// Deliberately not called "hand": what the renderer knows is that this
    /// geometry is nearer than the world and framed by a camera of its own, not
    /// that a game somewhere has arms.
    pub foreground: Option<ForegroundFrame<'a>>,
}

/// Geometry drawn after the world, with the depth buffer cleared first, so
/// nothing already drawn can cut into it.
///
/// The depth clear is what makes this different from another entry in
/// `opaque` — a view model sits a few centimetres from the eye, closer than the
/// near plane of anything it would otherwise be tested against, so a wall the
/// camera is pressed against would slice straight through it. Clearing costs one
/// command inside the render pass already in progress; a second render pass
/// instance would cost a store and reload of the whole colour attachment.
pub struct ForegroundFrame<'a> {
    /// Its own camera: a view model is framed independently of the world's
    /// field of view, so a wide-FOV setting does not distort it.
    pub view_proj: Mat4,
    /// Geometry sampling the shared atlas — a player's own skin.
    pub atlas: Vec<&'a GpuMesh>,
    /// Geometry bringing its own texture — a held model.
    pub textured: Vec<TexturedMesh<'a>>,
}

impl ForegroundFrame<'_> {
    fn is_empty(&self) -> bool {
        self.atlas.is_empty() && self.textured.is_empty()
    }
}

/// Everything the renderer needs to draw the player-model preview into the
/// inventory's offscreen image: a fixed orbit camera, a neutral light, and the
/// one model mesh. No sky, no world.
pub struct PreviewFrame<'a> {
    pub view_proj: Mat4,
    pub light: LightParams,
    pub model: &'a GpuMesh,
    /// The item model in the previewed player's hand, if they hold one.
    pub held: Option<TexturedMesh<'a>>,
    /// A held item with no model of its own — a block cube or a flat sprite,
    /// sampling the shared atlas like the player model beside it.
    pub held_atlas: Option<&'a GpuMesh>,
}

/// The state every world draw shares: which pipeline, and the camera and light
/// pushed to it. Grouped because the two mesh recorders differ only in how they
/// bind textures, and threading five positional arguments through both obscured
/// that.
#[derive(Clone, Copy)]
struct Pass<'a> {
    pipeline: &'a Arc<GraphicsPipeline>,
    view_proj: Mat4,
    light: &'a LightParams,
    time: f32,
}

pub struct Renderer {
    ctx: Arc<RenderContext>,
    sky_pipeline: Arc<GraphicsPipeline>,
    voxel_pipeline: Arc<GraphicsPipeline>,
    transparent_pipeline: Arc<GraphicsPipeline>,
    /// The same two, for chunk geometry sampling the block texture array.
    /// They collapse back into one pair when the atlas path retires.
    array_pipeline: Arc<GraphicsPipeline>,
    array_transparent_pipeline: Arc<GraphicsPipeline>,
    line_pipeline: Arc<GraphicsPipeline>,
    /// Depth buffers keyed by size. The swapchain pass and the (differently
    /// sized) preview pass each keep their own entry, so alternating between
    /// them every frame doesn't thrash a single-slot cache.
    depth_cache: Vec<(Arc<ImageView>, [u32; 2])>,
    atlas: Texture,
    block_textures: BlockTextureArray,
}

impl Renderer {
    /// `atlas_pixels` is the packed RGBA atlas assembled by the tile registry,
    /// and `block_textures` the 256-pixel layers Blockbench-authored blocks
    /// sample (the renderer stays decoupled from how content chose either).
    pub fn new(
        ctx: Arc<RenderContext>,
        color_format: Format,
        atlas_pixels: Vec<u8>,
        block_textures: &BlockTextureSet,
    ) -> Self {
        let sky_pipeline = sky::create(ctx.device().clone(), color_format, DEPTH_FORMAT);
        let voxel_pipeline = voxel::create(ctx.device().clone(), color_format, DEPTH_FORMAT, false);
        let transparent_pipeline =
            voxel::create(ctx.device().clone(), color_format, DEPTH_FORMAT, true);
        let array_pipeline =
            voxel_array::create(ctx.device().clone(), color_format, DEPTH_FORMAT, false);
        let array_transparent_pipeline =
            voxel_array::create(ctx.device().clone(), color_format, DEPTH_FORMAT, true);
        let line_pipeline = line::create(ctx.device().clone(), color_format, DEPTH_FORMAT);
        let atlas = Texture::atlas(&ctx, atlas_pixels);
        let block_textures =
            BlockTextureArray::create(&ctx, block_textures).expect("block texture array upload");

        Self {
            ctx,
            sky_pipeline,
            voxel_pipeline,
            transparent_pipeline,
            array_pipeline,
            array_transparent_pipeline,
            line_pipeline,
            depth_cache: Vec::new(),
            atlas,
            block_textures,
        }
    }

    /// The block-atlas image view, for the app to register with egui so the UI
    /// can draw item icons from the same texture the world samples.
    pub fn atlas_view(&self) -> Arc<ImageView> {
        self.atlas.image_view.clone()
    }

    /// Record the fullscreen procedural sky pass (drawn before world geometry).
    fn record_sky(
        &self,
        builder: &mut AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
        sky: &SkyParams,
    ) {
        let layout = self.sky_pipeline.layout().clone();
        builder
            .bind_pipeline_graphics(self.sky_pipeline.clone())
            .expect("bind sky pipeline");
        let push = shaders::sky_fs::PushConstants {
            inv_view_proj: sky.inv_view_proj.to_cols_array_2d(),
            sun_dir: [
                sky.sun_dir.x,
                sky.sun_dir.y,
                sky.sun_dir.z,
                sky.star_intensity,
            ],
            zenith_color: [
                sky.zenith_color.x,
                sky.zenith_color.y,
                sky.zenith_color.z,
                0.0,
            ],
            horizon_color: [
                sky.horizon_color.x,
                sky.horizon_color.y,
                sky.horizon_color.z,
                sky.moon_intensity,
            ],
            sun_color: [sky.sun_color.x, sky.sun_color.y, sky.sun_color.z, 0.0],
        };
        builder
            .push_constants(layout, 0, push)
            .expect("push sky constants");
        // SAFETY: pipeline and push constants are bound; the fullscreen triangle
        // is generated from gl_VertexIndex, so no vertex/index buffers are needed.
        unsafe {
            builder.draw(3, 1, 0, 0).expect("draw sky");
        }
    }

    /// Bind `pass`'s pipeline and push the shared per-frame constants. Split out
    /// because the two mesh recorders below differ only in how they bind
    /// textures, not in how they set the frame up.
    fn begin_pass(
        &self,
        builder: &mut AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
        pass: Pass<'_>,
    ) {
        let light = pass.light;
        builder
            .bind_pipeline_graphics(pass.pipeline.clone())
            .expect("bind pipeline");
        let push = shaders::voxel_vs::PushConstants {
            view_proj: pass.view_proj.to_cols_array_2d(),
            sun_dir: [
                light.light_dir.x,
                light.light_dir.y,
                light.light_dir.z,
                pass.time,
            ],
            light_color: [
                light.light_color.x,
                light.light_color.y,
                light.light_color.z,
                light.ambient,
            ],
        };
        builder
            .push_constants(pass.pipeline.layout().clone(), 0, push)
            .expect("push view_proj");
    }

    fn draw_mesh(
        &self,
        builder: &mut AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
        mesh: &GpuMesh,
    ) {
        builder
            .bind_vertex_buffers(0, mesh.vertex_buffer.clone())
            .expect("bind vbuf");
        builder
            .bind_index_buffer(mesh.index_buffer.clone())
            .expect("bind ibuf");
        // SAFETY: pipeline, descriptor set, push constants and buffers are
        // bound and match the shader interface.
        unsafe {
            builder
                .draw_indexed(mesh.index_count, 1, 0, 0, 0)
                .expect("draw mesh");
        }
    }

    /// Record draws for meshes that all sample one texture — the world off the
    /// block atlas or off the block texture array, and every entity built from
    /// atlas tiles or skin sheets. One descriptor bind covers the whole batch,
    /// which is the whole point of both shared textures.
    fn record_meshes(
        &self,
        builder: &mut AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
        pass: Pass<'_>,
        set: Arc<DescriptorSet>,
        meshes: &[&GpuMesh],
    ) {
        if meshes.is_empty() {
            return;
        }
        self.begin_pass(builder, pass);
        builder
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                pass.pipeline.layout().clone(),
                0,
                set,
            )
            .expect("bind texture set");
        for mesh in meshes {
            self.draw_mesh(builder, mesh);
        }
    }

    /// Record draws for meshes that each carry their own texture — geometry
    /// loaded from model files, whose textures are far too varied to live in the
    /// block atlas. Same pipeline and push constants as [`Self::record_meshes`];
    /// only the descriptor set is rebound per mesh.
    fn record_textured(
        &self,
        builder: &mut AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
        pass: Pass<'_>,
        meshes: &[TexturedMesh<'_>],
    ) {
        if meshes.is_empty() {
            return;
        }
        self.begin_pass(builder, pass);
        for textured in meshes {
            builder
                .bind_descriptor_sets(
                    PipelineBindPoint::Graphics,
                    pass.pipeline.layout().clone(),
                    0,
                    textured.texture.set.clone(),
                )
                .expect("bind model texture set");
            self.draw_mesh(builder, textured.mesh);
        }
    }

    /// Record the debug-line pass (block selection outline), drawn after the
    /// world geometry so the outline stays crisp over blended surfaces.
    fn record_lines(
        &self,
        builder: &mut AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
        view_proj: Mat4,
        lines: &GpuLines,
    ) {
        let layout = self.line_pipeline.layout().clone();
        builder
            .bind_pipeline_graphics(self.line_pipeline.clone())
            .expect("bind line pipeline");
        let push = shaders::line_vs::PushConstants {
            view_proj: view_proj.to_cols_array_2d(),
        };
        builder
            .push_constants(layout, 0, push)
            .expect("push line view_proj");
        builder
            .bind_vertex_buffers(0, lines.vertex_buffer.clone())
            .expect("bind line vbuf");
        // SAFETY: pipeline, push constants and vertex buffer are bound and
        // match the shader interface.
        unsafe {
            builder
                .draw(lines.vertex_count, 1, 0, 0)
                .expect("draw lines");
        }
    }

    /// Create or reuse a depth buffer matching `size`.
    fn ensure_depth(&mut self, size: [u32; 2]) -> Arc<ImageView> {
        if let Some((view, _)) = self.depth_cache.iter().find(|(_, s)| *s == size) {
            return view.clone();
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
        self.depth_cache.push((view.clone(), size));
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
                    clear_value: Some(ClearValue::Depth(DEPTH_CLEAR)),
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
            // Sky first (fills the background, no depth write), then opaque
            // (writes depth), then transparent (blended, no depth write).
            self.record_sky(&mut builder, &scene.sky);
            let voxel_pipeline = self.voxel_pipeline.clone();
            let transparent_pipeline = self.transparent_pipeline.clone();
            let array_pipeline = self.array_pipeline.clone();
            let array_transparent_pipeline = self.array_transparent_pipeline.clone();
            let pass = |pipeline| Pass {
                pipeline,
                view_proj: scene.view_proj,
                light: &scene.light,
                time: scene.time,
            };
            self.record_meshes(
                &mut builder,
                pass(&voxel_pipeline),
                self.atlas.set.clone(),
                &scene.opaque,
            );
            // Blockbench-authored blocks: opaque geometry too, just off a
            // different texture. One bind for every block type on screen.
            self.record_meshes(
                &mut builder,
                pass(&array_pipeline),
                self.block_textures.set.clone(),
                &scene.array_opaque,
            );
            // Model geometry is opaque too (its cutouts are alpha-tested in the
            // shader), so it goes in before the blended pass.
            self.record_textured(&mut builder, pass(&voxel_pipeline), &scene.textured);
            self.record_meshes(
                &mut builder,
                pass(&transparent_pipeline),
                self.atlas.set.clone(),
                &scene.transparent,
            );
            self.record_meshes(
                &mut builder,
                pass(&array_transparent_pipeline),
                self.block_textures.set.clone(),
                &scene.array_transparent,
            );
            if let Some(lines) = scene.lines {
                self.record_lines(&mut builder, scene.view_proj, lines);
            }
            // Last, and only after the depth of everything else is thrown away:
            // the view model is nearer than the world by construction, and must
            // not be clipped by geometry the camera is standing inside.
            if let Some(foreground) = scene.foreground.as_ref().filter(|f| !f.is_empty()) {
                builder
                    .clear_attachments(
                        [ClearAttachment::Depth(DEPTH_CLEAR)].into_iter().collect(),
                        [ClearRect {
                            offset: [0, 0],
                            extent: size,
                            array_layers: 0..1,
                        }]
                        .into_iter()
                        .collect(),
                    )
                    .expect("clear foreground depth");
                let pass = Pass {
                    pipeline: &voxel_pipeline,
                    view_proj: foreground.view_proj,
                    light: &scene.light,
                    time: scene.time,
                };
                self.record_meshes(
                    &mut builder,
                    pass,
                    self.atlas.set.clone(),
                    &foreground.atlas,
                );
                self.record_textured(&mut builder, pass, &foreground.textured);
            }
        }

        builder.end_rendering().expect("end rendering");
        let command_buffer = builder.build().expect("build scene cb");

        before
            .then_execute(self.ctx.graphics_queue().clone(), command_buffer)
            .expect("execute scene cb")
            .boxed()
    }

    /// Fill an icon sheet: render `icons[i]` into cell `i` of `target`, all in
    /// one pass, and return a future that completes when the sheet is ready.
    ///
    /// A `None` leaves its cell empty **without moving the ones after it**. The
    /// index is the identity of the icon, not merely its order — callers look a
    /// cell up by the same index they built the slice with — so a model that
    /// failed to load has to hold its place rather than be squeezed out.
    ///
    /// Every cell shares the orthographic camera and light from
    /// [`super::icons`] — the meshes arrive already framed into the unit box it
    /// covers — so a cell change is only a viewport change. Cleared fully
    /// transparent so the icons composite over inventory slots; the shader's
    /// alpha test means every pixel is either opaque model or untouched clear,
    /// with no partial-alpha edges to fringe.
    pub fn draw_icons(
        &mut self,
        before: Box<dyn GpuFuture>,
        target: Arc<ImageView>,
        icons: &[Option<TexturedMesh<'_>>],
    ) -> Box<dyn GpuFuture> {
        let extent = target.image().extent();
        let depth = self.ensure_depth([extent[0], extent[1]]);

        let mut builder = AutoCommandBufferBuilder::primary(
            self.ctx.command_allocator.clone(),
            self.ctx.graphics_queue().queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .expect("icon command buffer");

        builder
            .begin_rendering(RenderingInfo {
                color_attachments: vec![Some(RenderingAttachmentInfo {
                    load_op: AttachmentLoadOp::Clear,
                    store_op: AttachmentStoreOp::Store,
                    clear_value: Some(ClearValue::Float([0.0; 4])),
                    ..RenderingAttachmentInfo::image_view(target.clone())
                })],
                depth_attachment: Some(RenderingAttachmentInfo {
                    load_op: AttachmentLoadOp::Clear,
                    store_op: AttachmentStoreOp::DontCare,
                    clear_value: Some(ClearValue::Depth(DEPTH_CLEAR)),
                    ..RenderingAttachmentInfo::image_view(depth)
                }),
                ..Default::default()
            })
            .expect("begin icon rendering");

        let pipeline = self.voxel_pipeline.clone();
        let view_proj = icons::view_projection();
        for (index, icon) in icons.iter().enumerate() {
            let Some(icon) = icon else {
                continue;
            };
            let [x, y, w, h] = icons::cell_rect(index as u32);
            builder
                .set_viewport(
                    0,
                    [Viewport {
                        offset: [x as f32, y as f32],
                        extent: [w as f32, h as f32],
                        depth_range: 0.0..=1.0,
                    }]
                    .into_iter()
                    .collect(),
                )
                .expect("set icon viewport");
            self.record_textured(
                &mut builder,
                Pass {
                    pipeline: &pipeline,
                    view_proj,
                    light: &ICON_LIGHT,
                    time: 0.0,
                },
                std::slice::from_ref(icon),
            );
        }

        builder.end_rendering().expect("end icon rendering");
        let command_buffer = builder.build().expect("build icon cb");
        before
            .then_execute(self.ctx.graphics_queue().clone(), command_buffer)
            .expect("execute icon cb")
            .boxed()
    }

    /// Render just the player model into `target` (the inventory preview's
    /// offscreen image), chaining after `before`. Reuses the voxel pipeline and
    /// the shared atlas — the skin already lives there — so it needs no new GPU
    /// resources. The caller folds the returned future into the one passed to
    /// egui's overlay pass so the sampled image is finished first.
    pub fn draw_model(
        &mut self,
        before: Box<dyn GpuFuture>,
        target: Arc<ImageView>,
        preview: &PreviewFrame,
    ) -> Box<dyn GpuFuture> {
        let extent = target.image().extent();
        let size = [extent[0], extent[1]];
        let depth = self.ensure_depth(size);

        let mut builder = AutoCommandBufferBuilder::primary(
            self.ctx.command_allocator.clone(),
            self.ctx.graphics_queue().queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .expect("preview command buffer");

        builder
            .begin_rendering(RenderingInfo {
                color_attachments: vec![Some(RenderingAttachmentInfo {
                    load_op: AttachmentLoadOp::Clear,
                    store_op: AttachmentStoreOp::Store,
                    clear_value: Some(ClearValue::Float(PREVIEW_BG)),
                    ..RenderingAttachmentInfo::image_view(target.clone())
                })],
                depth_attachment: Some(RenderingAttachmentInfo {
                    load_op: AttachmentLoadOp::Clear,
                    store_op: AttachmentStoreOp::DontCare,
                    clear_value: Some(ClearValue::Depth(DEPTH_CLEAR)),
                    ..RenderingAttachmentInfo::image_view(depth)
                }),
                ..Default::default()
            })
            .expect("begin preview rendering");

        let viewport = Viewport {
            offset: [0.0, 0.0],
            extent: [size[0] as f32, size[1] as f32],
            depth_range: 0.0..=1.0,
        };
        builder
            .set_viewport(0, [viewport].into_iter().collect())
            .expect("set preview viewport");

        let voxel_pipeline = self.voxel_pipeline.clone();
        let pass = Pass {
            pipeline: &voxel_pipeline,
            view_proj: preview.view_proj,
            light: &preview.light,
            time: 0.0,
        };
        self.record_meshes(&mut builder, pass, self.atlas.set.clone(), &[preview.model]);
        self.record_textured(&mut builder, pass, preview.held.as_slice());
        self.record_meshes(
            &mut builder,
            pass,
            self.atlas.set.clone(),
            &preview.held_atlas.into_iter().collect::<Vec<_>>(),
        );

        builder.end_rendering().expect("end preview rendering");
        let command_buffer = builder.build().expect("build preview cb");

        before
            .then_execute(self.ctx.graphics_queue().clone(), command_buffer)
            .expect("execute preview cb")
            .boxed()
    }
}
