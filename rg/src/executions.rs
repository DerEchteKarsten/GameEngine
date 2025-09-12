use std::mem::MaybeUninit;

use anyhow::Result;
use ash::vk;
use lava::pipelines::{ComputePipelineHandle, RasterPipelineHandle, RayTracingPipelineHandle};
use lava::state::Ctx;

use super::build::{DispatchSize, NodeBuilder};
use super::{EdgeType, ExecutionTrait, NodeEdge, RenderGraph};

#[derive(Clone, PartialEq, Default)]
pub struct ComputePass {
    pub(super) pipeline: ComputePipelineHandle,
    pub(super) dispatch: DispatchSize,
}

impl ExecutionTrait for ComputePass {
    fn execute(
        &self,
        cmd: &vk::CommandBuffer,
        rg: &RenderGraph,
        _edges: &[NodeEdge],
    ) -> Result<()> {
        let (x, y, z) = self.dispatch.size();
        self.pipeline.dispatch(cmd, x, y, z);
        Ok(())
    }
    fn get_stages(&self) -> vk::PipelineStageFlags2 {
        vk::PipelineStageFlags2::COMPUTE_SHADER
    }
}

#[derive(Default, PartialEq)]
pub enum WorkSize2D {
    #[default]
    FullScreen,
    FractionalFullScreen(u32, u32),
    X(u32),
    XY(u32, u32),
}

impl WorkSize2D {
    fn size(&self) -> (u32, u32) {
        match self {
            WorkSize2D::FractionalFullScreen(x, y) => (
                Ctx::window_width().unwrap().div_ceil(*x),
                Ctx::window_height().unwrap().div_ceil(*y),
            ),
            WorkSize2D::FullScreen => (Ctx::window_width().unwrap(), Ctx::window_height().unwrap()),
            WorkSize2D::X(x) => (*x, 1),
            WorkSize2D::XY(x, y) => (*x, *y),
        }
    }
}

#[derive(PartialEq, Default)]
pub struct RayTracingPass {
    pub(super) launch: WorkSize2D,
    pub(super) pipeline: RayTracingPipelineHandle,
}

impl ExecutionTrait for RayTracingPass {
    fn execute(&self, cmd: &vk::CommandBuffer, rg: &RenderGraph, _: &[NodeEdge]) -> Result<()> {
        let (x, y) = self.launch.size();
        self.pipeline.launch(cmd, x, y);
        Ok(())
    }
    fn get_stages(&self) -> vk::PipelineStageFlags2 {
        vk::PipelineStageFlags2::RAY_TRACING_SHADER_KHR
    }
}

#[derive(PartialEq, Default)]
pub struct RasterPass {
    pub(super) dispatch: DispatchSize,
    pub(super) render_area: WorkSize2D,
    pub(super) pipeline: RasterPipelineHandle,
}

impl ExecutionTrait for RasterPass {
    fn execute(&self, cmd: &vk::CommandBuffer, rg: &RenderGraph, edges: &[NodeEdge]) -> Result<()> {
        let (x, y, z) = self.dispatch.size();
        let (width, height) = self.render_area.size();
        let color_attachments = edges
            .iter()
            .filter_map(|e| {
                if let EdgeType::ColorAttachmentOutput { clear_color } = e.edge_type {
                    Some((rg.image_handle(e.resource).unwrap(), clear_color))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let depth_attachment = edges
            .iter()
            .find(|e| e.edge_type == EdgeType::DepthAttachment)
            .and_then(|e| rg.image_handle(e.resource));
        let stencil_attachment = edges
            .iter()
            .find(|e| e.edge_type == EdgeType::StencilAttachment)
            .and_then(|e| rg.image_handle(e.resource));

        self.pipeline.dispatch(
            *cmd,
            &color_attachments,
            &depth_attachment,
            &stencil_attachment,
            width,
            height,
            x,
            y,
            z,
        );
        Ok(())
    }
    fn get_stages(&self) -> vk::PipelineStageFlags2 {
        vk::PipelineStageFlags2::FRAGMENT_SHADER
    }
}
