//! The opaque voxel/chunk graphics pipeline (dynamic rendering).

use std::sync::Arc;

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

use crate::render::shaders;
use crate::render::vertex::ChunkVertex;

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
    let vs = shaders::voxel_vs::load(device.clone())
        .unwrap()
        .entry_point("main")
        .unwrap();
    let fs = shaders::voxel_fs::load(device.clone())
        .unwrap()
        .entry_point("main")
        .unwrap();

    let vertex_input_state = ChunkVertex::per_vertex()
        .definition(&vs)
        .expect("voxel vertex input");

    let stages = [
        PipelineShaderStageCreateInfo::new(vs),
        PipelineShaderStageCreateInfo::new(fs),
    ];

    let layout = PipelineLayout::new(
        device.clone(),
        PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
            .into_pipeline_layout_create_info(device.clone())
            .expect("voxel pipeline layout info"),
    )
    .expect("voxel pipeline layout");

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
