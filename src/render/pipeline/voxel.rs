//! The opaque voxel/chunk graphics pipeline (dynamic rendering).

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
    let fs = shaders::voxel_fs::load(device.clone())
        .unwrap()
        .entry_point("main")
        .unwrap();
    (vs, fs)
}

/// The layout both voxel pipelines share, reflected from the shaders: set 0 is
/// the sampled texture, plus the push-constant block.
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
            .expect("voxel pipeline layout info"),
    )
    .expect("voxel pipeline layout")
}

/// Descriptor-set layout for set 0 — the sampled texture.
///
/// Anything this pipeline can bind builds its descriptor set from here: the
/// block atlas and every model's own texture alike. Deriving it from the same
/// reflected [`layout`] the pipelines use is what guarantees the sets stay
/// compatible, rather than a second hand-written copy of the binding drifting
/// out of sync with the shader.
pub fn texture_set_layout(device: &Arc<Device>) -> Arc<DescriptorSetLayout> {
    layout(device).set_layouts()[0].clone()
}

/// Build a voxel pipeline targeting the given color/depth formats.
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
        .expect("voxel vertex input");

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
            // Back-face culling is left off until greedy meshing guarantees a
            // consistent winding; faces are emitted single-sided regardless.
            rasterization_state: Some(RasterizationState {
                cull_mode: CullMode::None,
                ..Default::default()
            }),
            multisample_state: Some(MultisampleState::default()),
            depth_stencil_state: Some(DepthStencilState {
                depth: Some(DepthState {
                    // Transparent geometry tests depth but doesn't write it, so it
                    // doesn't occlude other transparent surfaces incorrectly.
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
    .expect("voxel pipeline")
}
