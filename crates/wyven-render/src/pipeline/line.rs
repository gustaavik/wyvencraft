//! The debug-line graphics pipeline (block selection outline, wireframes).
//!
//! Draws [`LineVertex`] geometry as a `LineList`: depth-tested against the
//! opaque world (hidden lines stay hidden) but without depth writes, and with
//! no blending — lines render as solid colour.

use std::sync::Arc;

use vulkano::device::Device;
use vulkano::format::Format;
use vulkano::pipeline::graphics::color_blend::{ColorBlendAttachmentState, ColorBlendState};
use vulkano::pipeline::graphics::depth_stencil::{CompareOp, DepthState, DepthStencilState};
use vulkano::pipeline::graphics::input_assembly::{InputAssemblyState, PrimitiveTopology};
use vulkano::pipeline::graphics::multisample::MultisampleState;
use vulkano::pipeline::graphics::rasterization::RasterizationState;
use vulkano::pipeline::graphics::subpass::PipelineRenderingCreateInfo;
use vulkano::pipeline::graphics::vertex_input::{Vertex, VertexDefinition};
use vulkano::pipeline::graphics::viewport::ViewportState;
use vulkano::pipeline::graphics::{GraphicsPipeline, GraphicsPipelineCreateInfo};
use vulkano::pipeline::layout::PipelineDescriptorSetLayoutCreateInfo;
use vulkano::pipeline::{DynamicState, PipelineLayout, PipelineShaderStageCreateInfo};

use crate::shaders;
use crate::vertex::LineVertex;

/// Build the line pipeline targeting the given color/depth formats.
pub fn create(
    device: Arc<Device>,
    color_format: Format,
    depth_format: Format,
) -> Arc<GraphicsPipeline> {
    let vs = shaders::line_vs::load(device.clone())
        .unwrap()
        .entry_point("main")
        .unwrap();
    let fs = shaders::line_fs::load(device.clone())
        .unwrap()
        .entry_point("main")
        .unwrap();

    let vertex_input_state = LineVertex::per_vertex()
        .definition(&vs)
        .expect("line vertex input");

    let stages = [
        PipelineShaderStageCreateInfo::new(vs),
        PipelineShaderStageCreateInfo::new(fs),
    ];

    let layout = PipelineLayout::new(
        device.clone(),
        PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
            .into_pipeline_layout_create_info(device.clone())
            .expect("line pipeline layout info"),
    )
    .expect("line pipeline layout");

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
            input_assembly_state: Some(InputAssemblyState {
                topology: PrimitiveTopology::LineList,
                ..Default::default()
            }),
            viewport_state: Some(ViewportState::default()),
            rasterization_state: Some(RasterizationState::default()),
            multisample_state: Some(MultisampleState::default()),
            depth_stencil_state: Some(DepthStencilState {
                depth: Some(DepthState {
                    // Test against the opaque world so lines don't show through
                    // terrain, but don't write: lines shouldn't occlude anything.
                    write_enable: false,
                    // `Greater`, not `Less`: the camera is reversed-Z, so a
                    // nearer surface has the *larger* depth. See
                    // `Camera::projection_matrix`.
                    compare_op: CompareOp::Greater,
                }),
                ..Default::default()
            }),
            color_blend_state: Some(ColorBlendState::with_attachment_states(
                subpass.color_attachment_formats.len() as u32,
                ColorBlendAttachmentState::default(),
            )),
            dynamic_state: [DynamicState::Viewport].into_iter().collect(),
            subpass: Some(subpass.into()),
            ..GraphicsPipelineCreateInfo::layout(layout)
        },
    )
    .expect("line pipeline")
}
