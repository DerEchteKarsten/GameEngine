use std::collections::{HashMap, HashSet};

use anyhow::{Error, Result};
use ash::vk::{self, BufferUsageFlags, ImageUsageFlags};
use glam::{UVec2, UVec3};
use lava::pipelines::{PipelineModel, RasterPipelineHandle, RayTracingPipelineHandle, ShaderPath};
use lava::state::Ctx;


use super::executions::{ComputePass, RasterPass, RayTracingPass, WorkSize2D};
use super::{
    EdgeType, Execution, ExecutionTrait, Node, NodeEdge, NodeHandle, RenderGraph,
    ResourceDescriptionType, ResourceHandle, ResourceType, IMPORTED,
};

use std::ffi::{self, c_void};

pub struct NodeBuilder<'a, T>
where
    T: ExecutionTrait + 'static,
{
    pub(super) name: &'static str,
    pub(super) rg: &'a mut RenderGraph,
    pub(super) execution: Option<T>,
    pub(super) edges: Vec<NodeEdge>,
    pub(super) constants_offset: Option<u32>,
    pub(super) constants_size: usize,
}

impl<'b, T> NodeBuilder<'b, T>
where
    T: ExecutionTrait + 'static,
    Execution: From<T>,
{
    pub(super) fn new<E: ExecutionTrait + 'static>(
        rg: &'b mut RenderGraph,
        name: &'static str,
    ) -> NodeBuilder<'b, E> {
        if rg.nodes.iter().find(|e| e.name == name).is_some() {
            panic!("Node name {name} allready used")
        }
        NodeBuilder::<'b, E> {
            name,
            rg,
            execution: Option::None,
            edges: Vec::new(),
            constants_offset: None,
            constants_size: 0,
        }
    }

    pub fn constants<C: Copy>(mut self, constants: &'b C) -> Self {
        let offset = self.rg.constants_offset;
        self.rg.constants_offset += size_of::<C>() as u32;
        self.rg
            .constants_buffer
            .copy_value_to_buffer_offset(constants, offset as u64)
            .unwrap();
        self.constants_offset = Some(offset);
        self
    }

    pub fn read(mut self, origin: NodeHandle, handle: ResourceHandle) -> Self {
        if origin != IMPORTED {
            let prev = self.rg.nodes[origin]
                .edges
                .iter()
                .find(|e| e.resource == handle)
                .ok_or(Error::msg("Origin doesnt write to handle".to_owned()))
                .unwrap();
            if prev.edge_type == EdgeType::ShaderRead {
                panic!("Origin contains handle but does not write to it")
            }
        }
        if let ResourceType::Uninitilized(index) = self.rg.resources[handle].ty {
            match &mut self.rg.resource_cache[index].ty {
                super::ResourceDescriptionType::Buffer { size, usage } => {
                    *usage |= BufferUsageFlags::STORAGE_BUFFER
                }
                ResourceDescriptionType::Image {
                    size,
                    usage,
                    format,
                } => {
                    if !usage.contains(ImageUsageFlags::STORAGE) {
                        *usage |= ImageUsageFlags::SAMPLED;
                    } else {
                        *usage |= ImageUsageFlags::STORAGE;
                        *usage &= ImageUsageFlags::SAMPLED;
                    }
                }
            }
        }
        self.edges.push(NodeEdge {
            origin: if origin == IMPORTED {
                None
            } else {
                Some(origin)
            },
            edge_type: EdgeType::ShaderRead,
            resource: handle,
        });
        self
    }

    pub fn write(mut self, last_read: NodeHandle, handle: ResourceHandle) -> Self {
        if let ResourceType::Uninitilized(index) = self.rg.resources[handle].ty {
            match &mut self.rg.resource_cache[index].ty {
                super::ResourceDescriptionType::Buffer { size, usage } => {
                    *usage |= BufferUsageFlags::STORAGE_BUFFER
                }
                ResourceDescriptionType::Image {
                    size,
                    usage,
                    format,
                } => {
                    *usage |= ImageUsageFlags::STORAGE;
                    *usage &= !ImageUsageFlags::SAMPLED;
                }
            }
        }
        self.edges.push(NodeEdge {
            origin: if last_read == IMPORTED {
                None
            } else {
                Some(last_read)
            },
            edge_type: EdgeType::ShaderWrite,
            resource: handle,
        });
        self
    }

    pub fn read_write(mut self, origin: NodeHandle, handle: ResourceHandle) -> Self {
        if let ResourceType::Uninitilized(index) = self.rg.resources[handle].ty {
            match &mut self.rg.resource_cache[index].ty {
                super::ResourceDescriptionType::Buffer { size, usage } => {
                    *usage |= BufferUsageFlags::STORAGE_BUFFER
                }
                ResourceDescriptionType::Image {
                    size,
                    usage,
                    format,
                } => {
                    *usage |= ImageUsageFlags::STORAGE;
                    *usage &= !ImageUsageFlags::SAMPLED;
                }
            }
        }
        self.edges.push(NodeEdge {
            origin: if origin == IMPORTED {
                None
            } else {
                Some(origin)
            },
            edge_type: super::EdgeType::ShaderReadWrite,
            resource: handle,
        });
        self
    }
    fn build(self) -> NodeHandle {
        let mut seen = HashSet::new();
        if let Some(edge) = self.edges.iter().find(|x| !seen.insert(x.resource.clone())) {
            panic!("resource: {:?} is duplicate", edge.resource);
        }

        let handle = self.rg.nodes.len();
        self.rg.nodes.push(Node {
            name: self.name,
            execution: self.execution.unwrap().into(),
            constant_offset: self.constants_offset,
            edges: self.edges,
        });
        handle
    }
}

#[derive(Default, Clone, Copy, PartialEq)]
pub enum DispatchSize {
    #[default]
    FullScreen,
    FractionalFullScreen(u32, u32),
    X(u32),
    XY(u32, u32),
    VertexCountInstanceCount(u32,u32),
    XYZ(u32, u32, u32),
    Custom(fn() -> UVec3),
}

impl DispatchSize {
    pub(super) fn size(&self) -> (u32, u32, u32) {
        match self {
            DispatchSize::Custom(func) => {
                let res = func();
                (res.x, res.y, res.z)
            }
            DispatchSize::FractionalFullScreen(x, y) => (
                (Ctx::window_width().unwrap()).div_ceil(*x),
                (Ctx::window_height().unwrap()).div_ceil(*y),
                1,
            ),
            DispatchSize::FullScreen => (
                (Ctx::window_width().unwrap()).div_ceil(8),
                (Ctx::window_height().unwrap()).div_ceil(8),
                1,
            ),
            DispatchSize::X(x) => (*x, 1, 1),
            DispatchSize::XY(x, y) => (*x, *y, 1),
            DispatchSize::XYZ(x, y, z) => (*x, *y, *z),
            DispatchSize::VertexCountInstanceCount(x, y) => (*x, *y, 0)
        }
    }
}

impl<'b> NodeBuilder<'b, RasterPass> {
    pub fn mesh_path(mut self, path: &'static str) -> Self {
        if let PipelineModel::Mesh { task, mesh } = &mut self.execution.as_mut().unwrap().pipeline.model {
            mesh.path = path;
            mesh.entry = "main";
        }else {
            self.execution.as_mut().unwrap().pipeline.model = PipelineModel::Mesh { task: None, mesh: ShaderPath {
                entry: "main",
                path,
            }}
        }
        self
    }
    pub fn mesh(mut self, entry: &'static str, path: &'static str) -> Self {
        if let PipelineModel::Mesh { task, mesh } = &mut self.execution.as_mut().unwrap().pipeline.model {
            mesh.entry = entry;
            mesh.path = path;
        }else {
            self.execution.as_mut().unwrap().pipeline.model = PipelineModel::Mesh { task: None, mesh: ShaderPath {
                entry,
                path,
            }}
        }
        self
    }
    pub fn vertex_path(mut self, path: &'static str) -> Self {
        if let PipelineModel::Vertex { vertex } = &mut self.execution.as_mut().unwrap().pipeline.model {
            vertex.path = path;
            vertex.entry = "main";
        }else {
            self.execution.as_mut().unwrap().pipeline.model = PipelineModel::Vertex { vertex: ShaderPath {
                entry: "main",
                path,
            }}
        }
        self
    }
    pub fn vertex(mut self, entry: &'static str, path: &'static str) -> Self {
        if let PipelineModel::Vertex { vertex } = &mut self.execution.as_mut().unwrap().pipeline.model {
            vertex.path = path;
            vertex.entry = entry;
        }else {
            self.execution.as_mut().unwrap().pipeline.model = PipelineModel::Vertex { vertex: ShaderPath {
                entry,
                path,
            }}
        }
        self
    }
    pub fn fragment_path(mut self, path: &'static str) -> Self {
        self.execution.as_mut().unwrap().pipeline.fragment.path = path;
        self
    }
    pub fn fragment(mut self, entry: &'static str, path: &'static str) -> Self {
        self.execution.as_mut().unwrap().pipeline.fragment = ShaderPath { path, entry };
        self
    }
    pub fn task_path(mut self, path: &'static str) -> Self {
        if let PipelineModel::Mesh { task, mesh } = &mut self.execution.as_mut().unwrap().pipeline.model {
            if let Some(task) = task {
                task.path = path;
            }else {
                *task = Some(ShaderPath {
                    entry: "main",
                    path,
                })
            }
        }else {
            self.execution.as_mut().unwrap().pipeline.model = PipelineModel::Mesh { task: None, mesh: ShaderPath {
                entry: "main",
                path,
            }}
        }
        self
    }
    pub fn task(mut self, entry: &'static str, path: &'static str) -> Self {
        if let PipelineModel::Mesh { task, mesh } = &mut self.execution.as_mut().unwrap().pipeline.model {
            *task = Some(ShaderPath {entry, path});
        }else {
            self.execution.as_mut().unwrap().pipeline.model = PipelineModel::Mesh { task: Some(ShaderPath { path, entry }), mesh: ShaderPath::default()}
        }
        self
    }
    pub fn render_area(mut self, render_area: WorkSize2D) -> Self {
        self.execution.as_mut().unwrap().render_area = render_area;
        self
    }
    pub fn draw(mut self, dispatch_size: DispatchSize) -> NodeHandle {
        self.execution.as_mut().unwrap().dispatch = dispatch_size;
        self.build()
    }

    pub fn color_attachment(
        mut self,
        last_read: NodeHandle,
        handle: ResourceHandle,
        clear_color: Option<[f32; 4]>,
    ) -> Self {
        if let ResourceType::Uninitilized(index) = self.rg.resources[handle].ty {
            match &mut self.rg.resource_cache[index].ty {
                super::ResourceDescriptionType::Buffer { size, usage } => {
                    panic!("Exspected Image")
                }
                ResourceDescriptionType::Image {
                    size,
                    usage,
                    format,
                } => *usage |= ImageUsageFlags::COLOR_ATTACHMENT,
            }
        }
        self.edges.push(NodeEdge {
            origin: if last_read == IMPORTED {
                None
            } else {
                Some(last_read)
            },
            edge_type: EdgeType::ColorAttachmentOutput { clear_color },
            resource: handle,
        });
        self
    }
    pub fn depth_attachment(mut self, origin: NodeHandle, handle: ResourceHandle) -> Self {
        if let ResourceType::Uninitilized(index) = self.rg.resources[handle].ty {
            match &mut self.rg.resource_cache[index].ty {
                super::ResourceDescriptionType::Buffer { size, usage } => {
                    panic!("Exspected Image")
                }
                ResourceDescriptionType::Image {
                    size,
                    usage,
                    format,
                } => *usage |= ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
            }
        }
        self.edges.push(NodeEdge {
            origin: if origin == IMPORTED {
                None
            } else {
                Some(origin)
            },
            edge_type: EdgeType::DepthAttachment,
            resource: handle,
        });
        self
    }
    pub fn stencil_attachment(mut self, origin: NodeHandle, handle: ResourceHandle) -> Self {
        self.edges.push(NodeEdge {
            origin: if origin == IMPORTED {
                None
            } else {
                Some(origin)
            },
            edge_type: EdgeType::StencilAttachment,
            resource: handle,
        });
        self
    }
    pub fn backface_culling(mut self, value: bool) -> Self {
        self.execution.as_mut().unwrap().pipeline.backface_culling = value;
        self
    }
}

impl<'b> NodeBuilder<'b, RayTracingPass> {
    pub fn shader(mut self, path: &'static str) -> Self {
        self.execution.as_mut().unwrap().pipeline.path.path = path;
        self
    }
    pub fn entry(mut self, entry: &'static str) -> Self {
        self.execution.as_mut().unwrap().pipeline.path.entry = entry;
        self
    }
    pub fn launch(mut self, launch: WorkSize2D) -> NodeHandle {
        self.execution.as_mut().unwrap().launch = launch;
        self.build()
    }
}

impl<'b> NodeBuilder<'b, ComputePass> {
    pub fn shader(mut self, path: &'static str) -> Self {
        self.execution.as_mut().unwrap().pipeline.path.path = path;
        self
    }
    pub fn entry(mut self, entry: &'static str) -> Self {
        self.execution.as_mut().unwrap().pipeline.path.entry = entry;
        self
    }
    pub fn dispatch(mut self, dispatch: DispatchSize) -> NodeHandle {
        self.execution.as_mut().unwrap().dispatch = dispatch;
        self.build()
    }
}

impl RasterPass {
    pub fn new<'a>(
        rg: &'a mut RenderGraph,
        name: &'static str,
    ) -> NodeBuilder<'a, RasterPass> {
        let mut builder = NodeBuilder::<RasterPass>::new::<RasterPass>(rg, name);
        builder.execution = Some(RasterPass {
            dispatch: DispatchSize::FullScreen,
            render_area: WorkSize2D::FullScreen,
            pipeline: RasterPipelineHandle::default()
        });
        builder
    }
}

impl RayTracingPass {
    pub fn new<'a>(
        rg: &'a mut RenderGraph,
        name: &'static str,
    ) -> NodeBuilder<'a, RayTracingPass> {
        let mut builder = NodeBuilder::<RayTracingPass>::new::<RayTracingPass>(rg, name);
        builder.execution = Some(RayTracingPass {
            launch: WorkSize2D::FullScreen,
            pipeline: RayTracingPipelineHandle {
                path: ShaderPath::default(),
            }
        });
        builder
    }
}

impl ComputePass {
    pub fn new<'a>(
        rg: &'a mut RenderGraph,
        name: &'static str,
    ) -> NodeBuilder<'a, ComputePass> {
        let mut builder = NodeBuilder::<ComputePass>::new::<ComputePass>(rg, name);
        builder.execution = Some(ComputePass::default());
        builder
    }
}
