//! The chunk pipeline for Blockbench-authored blocks.
//!
//! Identical to [`super::voxel`] in every respect except set 0: this one binds
//! [`crate::render::block_textures`]'s `sampler2DArray` rather than the 16-pixel
//! atlas, and its fragment shader applies the per-vertex biome tint. It shares
//! `voxel.vert` verbatim, so the two pipelines consume the same
//! [`ChunkVertex`] buffers and a chunk's atlas and array geometry differ only
//! in which pipeline records them.
//!
//! The pair is temporary. Blocks are migrating to models one at a time; when
//! the last one moves over, [`super::voxel`] and the atlas path go with it and
//! this becomes the only chunk pipeline.

use std::sync::Arc;

use vulkano::descriptor_set::layout::DescriptorSetLayout;
use vulkano::device::Device;
use vulkano::format::Format;
use vulkano::pipeline::graphics::color_blend::{
    AttachmentBlend, ColorBlendAttachmentState, ColorBlendState,
};
use vulkano::pipeline::graphics::depth_stencil::{CompareOp, DepthState, DepthStencilState};
use vulkano::pipeline::graphics::input_assembly::InputAssemblyState;
use vulkano::pipeline::graphics::multisample::MultisampleState;
use vulkano::pipeline::graphics::rasterization::{CullMode, RasterizationState};
use vulkano::pipeline::graphics::subpass::PipelineRenderingCreateInfo;
use vulkano::pipeline::graphics::vertex_input::{Vertex, VertexDefinition};
use vulkano::pipeline::graphics::viewport::ViewportState;
use vulkano::pipeline::graphics::{GraphicsPipeline, GraphicsPipelineCreateInfo};
use vulkano::pipeline::layout::PipelineDescriptorSetLayoutCreateInfo;
use vulkano::pipeline::{DynamicState, PipelineLayout, PipelineShaderStageCreateInfo};
use vulkano::shader::EntryPoint;

use crate::render::shaders;
use crate::render::vertex::ChunkVertex;

fn entry_points(device: &Arc<Device>) -> (EntryPoint, EntryPoint) {
    let vs = shaders::voxel_vs::load(device.clone())
        .unwrap()
        .entry_point("main")
        .unwrap();
    let fs = shaders::voxel_array_fs::load(device.clone())
        .unwrap()
        .entry_point("main")
        .unwrap();
    (vs, fs)
}

/// The layout both array pipelines share, reflected from the shaders.
pub fn layout(device: &Arc<Device>) -> Arc<PipelineLayout> {
    let (vs, fs) = entry_points(device);
    let stages = [
        PipelineShaderStageCreateInfo::new(vs),
        PipelineShaderStageCreateInfo::new(fs),
    ];
    PipelineLayout::new(
        device.clone(),
        PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
            .into_pipeline_layout_create_info(device.clone())
            .expect("voxel array pipeline layout info"),
    )
    .expect("voxel array pipeline layout")
}

/// Descriptor-set layout for set 0 — the sampled texture array.
///
/// Derived from the same reflected [`layout`] the pipelines use, so the set the
/// block texture array builds cannot drift out of sync with the shader binding.
pub fn texture_set_layout(device: &Arc<Device>) -> Arc<DescriptorSetLayout> {
    layout(device).set_layouts()[0].clone()
}

/// Build an array voxel pipeline targeting the given color/depth formats.
///
/// `transparent`: when true, enables alpha blending and disables depth writes
/// (for the water/glass pass drawn after opaque geometry).
pub fn create(
    device: Arc<Device>,
    color_format: Format,
    depth_format: Format,
    transparent: bool,
) -> Arc<GraphicsPipeline> {
    let (vs, fs) = entry_points(&device);

    let vertex_input_state = ChunkVertex::per_vertex()
        .definition(&vs)
        .expect("voxel array vertex input");

    let stages = [
        PipelineShaderStageCreateInfo::new(vs),
        PipelineShaderStageCreateInfo::new(fs),
    ];

    let layout = layout(&device);

    let subpass = PipelineRenderingCreateInfo {
        color_attachment_formats: vec![Some(color_format)],
        depth_attachment_format: Some(depth_format),
        ..Default::default()
    };

    GraphicsPipeline::new(
        device,
        None,
        GraphicsPipelineCreateInfo {
            stages: stages.into_iter().collect(),
            vertex_input_state: Some(vertex_input_state),
            input_assembly_state: Some(InputAssemblyState::default()),
            viewport_state: Some(ViewportState::default()),
            // Matches the atlas pipeline: faces are emitted single-sided, and
            // culling stays off until greedy meshing guarantees a winding.
            rasterization_state: Some(RasterizationState {
                cull_mode: CullMode::None,
                ..Default::default()
            }),
            multisample_state: Some(MultisampleState::default()),
            depth_stencil_state: Some(DepthStencilState {
                depth: Some(DepthState {
                    write_enable: !transparent,
                    compare_op: CompareOp::Less,
                }),
                ..Default::default()
            }),
            color_blend_state: Some(ColorBlendState::with_attachment_states(
                subpass.color_attachment_formats.len() as u32,
                ColorBlendAttachmentState {
                    blend: transparent.then(AttachmentBlend::alpha),
                    ..Default::default()
                },
            )),
            dynamic_state: [DynamicState::Viewport].into_iter().collect(),
            subpass: Some(subpass.into()),
            ..GraphicsPipelineCreateInfo::layout(layout)
        },
    )
    .expect("voxel array pipeline")
}
