#![feature(let_chains)]
use std::{
    arch::x86_64, collections::{HashMap, HashSet}, ffi::c_void, sync::Arc, time::Instant
};

use anyhow::Result;
use ash::vk::{self, Format, ImageUsageFlags};
use derivative::Derivative;
use enum_dispatch::enum_dispatch;
use glam::{UVec2, Vec3};
use gpu_allocator::MemoryLocation;

pub mod bake;
pub mod build;
pub mod executions;
pub mod resources;
use executions::*;
use lava::{bindless::{BindlessDescriptorHeap, DescriptorHandle}, state::{Ctx, Functions}, vkobjects::{buffer::Buffer, image::{Image, ImageHandle, ImageSize}}, FRAMES_IN_FLIGHT};
use resources::*;

pub const IMPORTED: NodeHandle = !0;

#[derive(Debug)]
struct Barrier {
    resource: ResourceHandle,
    layout: vk::ImageLayout,
    access: vk::AccessFlags2,
    stages: vk::PipelineStageFlags2,
}

impl Barrier {
    fn need_invalidate(&self, event: &resources::Event) -> bool {
        (0..64)
            .map(|i| {
                self.access.contains(
                    event.invalidated_in_stage[((self.stages.as_raw() >> i) & 1) as usize / 2],
                )
            })
            .fold(false, |acc, a| acc || a)
    }
}

impl Barrier {
    fn new(resource: ResourceHandle) -> Self {
        Self {
            resource,
            layout: vk::ImageLayout::UNDEFINED,
            access: vk::AccessFlags2::empty(),
            stages: vk::PipelineStageFlags2::empty(),
        }
    }
}

#[derive(Debug)]
struct Barriers {
    invalidates: Vec<Barrier>,
    flushes: Vec<Barrier>,
}

type NodeHandle = usize;

#[enum_dispatch]
pub(crate) trait ExecutionTrait {
    fn execute(&self, cmd: &vk::CommandBuffer, rg: &RenderGraph, edges: &[NodeEdge]) -> Result<()>;
    fn get_stages(&self) -> vk::PipelineStageFlags2;
}

#[enum_dispatch(ExecutionTrait)]
#[derive(PartialEq)]
enum Execution {
    RayTracingPass,
    ComputePass,
    RasterPass,
}

#[derive(PartialEq)]
struct Node {
    name: &'static str,
    execution: Execution,
    constant_offset: Option<u32>,
    edges: Vec<NodeEdge>,
}

impl Node {
    fn parents<'b>(&self) -> Vec<NodeHandle> {
        self.edges
            .iter()
            .filter_map(|r| r.origin)
            .collect::<Vec<_>>()
    }

    fn bindings<'b>(
        &'b self,
    ) -> std::iter::Filter<std::slice::Iter<'b, NodeEdge>, impl FnMut(&&'b NodeEdge) -> bool> {
        self.edges.iter().filter(|e| {
            std::mem::discriminant(&e.edge_type)
                != std::mem::discriminant(&EdgeType::ColorAttachmentOutput { clear_color: None })
                && e.edge_type != EdgeType::DepthAttachment
                && e.edge_type != EdgeType::StencilAttachment
        })
    }

    fn cmd_push_constants(
        &self,
        rg: &RenderGraph,
        cmd: &vk::CommandBuffer,
        descriptor_offset: u32,
    ) {
        unsafe {
            let mut constants = [0u8; 16];
            constants[0..4].copy_from_slice(&self.constant_offset.unwrap_or(0).to_ne_bytes());

            constants[4..8].copy_from_slice(
                &if self.bindings().count() == 0 {
                    0
                } else {
                    descriptor_offset
                }
                .to_ne_bytes(),
            );
            constants[8..12].copy_from_slice(&rg.descriptor_buffer_binding.0.to_ne_bytes());
            constants[12..16].copy_from_slice(&rg.constants_buffer_binding.0.to_ne_bytes());

            Ctx::device().cmd_push_constants(
                *cmd,
                BindlessDescriptorHeap::get().layout,
                vk::ShaderStageFlags::ALL,
                0,
                &constants,
            )
        };
    }

    fn get_barriers(&self, rg: &RenderGraph) -> Barriers {
        let mut invalidates: HashMap<ResourceHandle, Barrier> = HashMap::new();
        let mut flushes: HashMap<ResourceHandle, Barrier> = HashMap::new();

        for edge in &self.edges {
            match edge.edge_type {
                EdgeType::ShaderRead => {
                    let barrier = invalidates
                        .entry(edge.resource)
                        .or_insert(Barrier::new(edge.resource));
                    barrier.stages |= self.execution.get_stages();
                    if let Some(image) = rg.image_handle(edge.resource)
                        && image.usage.contains(vk::ImageUsageFlags::STORAGE)
                    {
                        barrier.access |= vk::AccessFlags2::SHADER_STORAGE_READ;
                        barrier.layout = vk::ImageLayout::GENERAL;
                    } else {
                        barrier.access |= vk::AccessFlags2::SHADER_SAMPLED_READ;
                        barrier.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
                    }
                }
                EdgeType::ColorAttachmentOutput { clear_color: _ } => {
                    let barrier = flushes
                        .entry(edge.resource)
                        .or_insert(Barrier::new(edge.resource));
                    barrier.access |= vk::AccessFlags2::COLOR_ATTACHMENT_WRITE;
                    barrier.stages |= vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT;
                    barrier.layout = vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL;
                }
                EdgeType::DepthAttachment | EdgeType::StencilAttachment => {
                    let src = flushes
                        .entry(edge.resource)
                        .or_insert(Barrier::new(edge.resource));
                    let dst = invalidates
                        .entry(edge.resource)
                        .or_insert(Barrier::new(edge.resource));
                    dst.layout = vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
                    dst.access |= vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                        | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE;
                    dst.stages |= vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                        | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;

                    src.layout = vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL;
                    src.access |= vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE;
                    dst.stages |= vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
                }
                EdgeType::ShaderReadWrite => {
                    let flush = flushes
                        .entry(edge.resource)
                        .or_insert(Barrier::new(edge.resource));
                    flush.stages |= self.execution.get_stages();
                    flush.access |= vk::AccessFlags2::SHADER_STORAGE_WRITE;
                    flush.layout = vk::ImageLayout::GENERAL;

                    let invalidate = invalidates
                        .entry(edge.resource)
                        .or_insert(Barrier::new(edge.resource));
                    invalidate.stages |= self.execution.get_stages();
                    if let Some(image) = rg.image_handle(edge.resource)
                        && image.usage.contains(vk::ImageUsageFlags::STORAGE)
                    {
                        invalidate.access |= vk::AccessFlags2::SHADER_STORAGE_READ;
                        invalidate.layout = vk::ImageLayout::GENERAL;
                    } else {
                        invalidate.access |= vk::AccessFlags2::SHADER_SAMPLED_READ;
                        invalidate.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
                    }
                }
                EdgeType::ShaderWrite => {
                    let flush = flushes
                        .entry(edge.resource)
                        .or_insert(Barrier::new(edge.resource));
                    flush.stages |= self.execution.get_stages();
                    flush.access |= vk::AccessFlags2::SHADER_STORAGE_WRITE;
                    flush.layout = vk::ImageLayout::GENERAL;
                }
                EdgeType::TransferDst => {
                    todo!();
                }
                EdgeType::TransferSrc => {
                    todo!();
                }
            }
            if !invalidates.contains_key(&edge.resource)
                && let Some(flush) = flushes.get(&edge.resource)
                && rg.resources[edge.resource].event.layout != flush.layout
            {
                invalidates.insert(
                    edge.resource,
                    Barrier {
                        resource: edge.resource,
                        layout: flush.layout,
                        access: vk::AccessFlags2::NONE,
                        stages: self.execution.get_stages(),
                    },
                );
            }
        }

        Barriers {
            invalidates: invalidates.into_values().collect::<Vec<_>>(),
            flushes: flushes.into_values().collect::<Vec<_>>(),
        }
    }
}

pub fn depends_on(rg: &RenderGraph, other: NodeHandle, s: NodeHandle) -> bool {
    other == s
        || rg.nodes[other]
            .edges
            .iter()
            .find(|e| {
                if let Some(origin) = e.origin
                    && origin == s
                {
                    true
                } else {
                    false
                }
            })
            .is_some()
}

#[derive(Clone)]
struct FrameData {
    command_pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    frame_number: u64,
}

pub struct RenderGraph {
    pub resources: Vec<Resource>,
    pub resource_cache: Vec<ResourceDescription>,

    destroy_next_frame: Vec<(Image, u64)>,
    constants_buffer: Buffer,
    constants_buffer_binding: DescriptorHandle,
    descriptor_buffer: Buffer, //TODO
    descriptor_buffer_binding: DescriptorHandle,
    swapchain_images: Vec<ResourceHandle>,

    nodes: Vec<Node>,
    constants_offset: u32,
}

#[derive(Clone, PartialEq)]
enum EdgeType {
    ShaderRead,
    ShaderReadWrite,
    ShaderWrite,
    ColorAttachmentOutput { clear_color: Option<[f32; 4]> },
    DepthAttachment,
    StencilAttachment,
    TransferSrc,
    TransferDst,
}

#[derive(Clone, PartialEq)]
pub struct NodeEdge {
    edge_type: EdgeType,
    origin: Option<NodeHandle>,
    resource: ResourceHandle,
}

impl RenderGraph {
    pub fn new() -> Self {
        let bindless = BindlessDescriptorHeap::get();

        let mut resources = Vec::new();
        let swapchain_images = Ctx::swapchain()
            .unwrap()
            .images
            .iter()
            .map(|image| {
                let descriptor = bindless.allocate_image_handle(image);
                let index = resources.len();
                resources.push(Resource::new(
                    descriptor,
                    ResourceType::Image(image.clone()),
                ));
                index
            })
            .collect::<Vec<_>>();

        let descriptor_buffer = Buffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::CpuToGpu,
            size_of::<u32>() as u64 * 256,
        )
        .unwrap();
        let descriptor_buffer_binding = bindless.allocate_buffer_handle(&descriptor_buffer);

        let constants_buffer = Buffer::new(
                vk::BufferUsageFlags::STORAGE_BUFFER,
                MemoryLocation::CpuToGpu,
                size_of::<u32>() as u64 * 1024,
            )
            .unwrap();
        let constants_buffer_binding = bindless.allocate_buffer_handle(&constants_buffer);

        Self {
            swapchain_images,
            constants_buffer_binding,
            descriptor_buffer_binding,
            resource_cache: Vec::new(),
            constants_offset: 0,
            nodes: Vec::new(),
            resources,
            constants_buffer,
            descriptor_buffer,
            destroy_next_frame: Vec::new(),
        }
    }

    pub fn get_swapchain(&self, swapchain_image_index: usize) -> ResourceHandle {
        self.swapchain_images[swapchain_image_index]
    }

    pub fn import<T>(&mut self, value: T) -> ResourceHandle
    where
        T: Importable,
    {
        let index = self.resources.len();
        self.resources.push(value.resource());
        index
    }

    pub fn buffer(&mut self, size: u64, name: &'static str) -> ResourceHandle {
        if let Some(resource) = self.resource_cache.iter().find(|e| e.name == name) {
            resource.handle
        } else {
            let index = self.resources.len();
            let index2 = self.resource_cache.len();
            self.resource_cache.push(ResourceDescription {
                name,
                handle: index,
                ty: ResourceDescriptionType::Buffer {
                    size,
                    usage: vk::BufferUsageFlags::empty(),
                },
            });
            self.resources.push(Resource::new(
                DescriptorHandle(!0),
                ResourceType::Uninitilized(index2),
            ));
            index
        }
    }

    pub fn image(&mut self, size: ImageSize, format: Format, name: &'static str) -> ResourceHandle {
        if let Some(resource) = self.resource_cache.iter().find(|e| e.name == name) {
            resource.handle
        } else {
            let index = self.resources.len();
            let index2 = self.resource_cache.len();
            self.resource_cache.push(ResourceDescription {
                name,
                handle: index,
                ty: ResourceDescriptionType::Image {
                    format,
                    size,
                    usage: vk::ImageUsageFlags::empty(),
                },
            });
            self.resources.push(Resource::new(
                DescriptorHandle(!0),
                ResourceType::Uninitilized(index2),
            ));
            index
        }
    }

    fn resource(&mut self, desc: &ResourceDescription) -> Resource {
        match &desc.ty {
            ResourceDescriptionType::Buffer { size, usage } => {
                let buffer = Buffer::new(*usage, MemoryLocation::GpuOnly, *size).unwrap();
                Functions::set_debug_name(desc.name, buffer.buffer);
                buffer.resource()
            }
            ResourceDescriptionType::Image {
                size,
                usage,
                format,
            } => {
                let image = Image::new_2d(*usage, MemoryLocation::GpuOnly, *format, *size)
                    .unwrap();
                Functions::set_debug_name(desc.name, image.image);
                image.resource()
            }
        }
    }

    fn image_handle<'a>(&'a self, handle: ResourceHandle) -> Option<ImageHandle> {
        if let ResourceType::Image(image) = &self.resources[handle].ty {
            Some(image.handle())
        } else {
            None
        }
    }
    fn buffer_handle<'a>(&'a self, handle: ResourceHandle) -> Option<Buffer> {
        if let ResourceType::Buffer(buffer) = &self.resources[handle].ty {
            Some(buffer.clone())
        } else {
            None
        }
    }
    
    pub fn draw_frame(&mut self, mut record: impl FnMut(&mut RenderGraph, usize)) {
        self.nodes.clear();
        self.constants_offset = 0;
        
        Ctx::next_frame(&mut |cmd, swapchain_image_index| {
            self.destroy_next_frame = self.destroy_next_frame.iter_mut().filter_map(|e| {
                if Ctx::current_frame() > e.1   {
                    Some(e.clone())
                } else {
                    e.0.destroy();
                    None
                }
            }).collect::<Vec<_>>();
            
            record(self, swapchain_image_index);
            
            
            let bindless = BindlessDescriptorHeap::get();
            if Ctx::swapchain().unwrap().resized {
                for (i, res) in self.swapchain_images.iter().enumerate() {
                    if let ResourceType::Image(image) = &mut self.resources[*res].ty {
                        *image = Ctx::swapchain().unwrap().images[i].clone();
                        self.resources[*res].event.layout = vk::ImageLayout::UNDEFINED;
                    }
                }
            }
            for i in 0..self.resources.len() {
                if self.swapchain_images.contains(&i) {
                    continue;
                }
                match &mut self.resources[i].ty {
                    ResourceType::Uninitilized(index) => {
                        let desc = self.resource_cache[*index].clone();
                        self.resources[i] = self.resource(&desc);
                    },
                    ResourceType::Image(image) => {
                        if let ImageSize::FractionalFullScreen(x, y) = image.size && Ctx::swapchain().unwrap().resized {
                            self.destroy_next_frame.push((image.clone(), Ctx::current_frame()+ FRAMES_IN_FLIGHT as u64));
                            *image = Image::new_2d(image.usage, MemoryLocation::GpuOnly, image.format, image.size).unwrap();
                            self.resources[i].event.layout = vk::ImageLayout::UNDEFINED;

                        }else if let ImageSize::FullScreen = image.size && Ctx::swapchain().unwrap().resized {
                            self.destroy_next_frame.push((image.clone(), Ctx::current_frame()+ FRAMES_IN_FLIGHT as u64));
                            *image = Image::new_2d(image.usage, MemoryLocation::GpuOnly, image.format, image.size).unwrap();
                            self.resources[i].event.layout = vk::ImageLayout::UNDEFINED;
                        }
                    }
                    _ => {}
                }
            }
            self.resources.iter_mut().for_each(|resource| {
                resource.event.invalidated_in_stage = [vk::AccessFlags2::empty(); 25];
                resource.event.pipeline_barrier_src_stages = vk::PipelineStageFlags2::empty();
                resource.event.to_flush = vk::AccessFlags2::default();
            });
            let root_node = self
                .nodes
                .iter()
                .position(|e| {
                    e.edges
                        .iter()
                        .position(|e| e.resource == self.get_swapchain(swapchain_image_index))
                        .is_some()
                })
                .unwrap();
            bindless
                .bind(Ctx::features().raytracing, &cmd)
                .unwrap();
            let descriptor_offsets = self.write_bindings().unwrap();
            let execution_order = if self.nodes.len() > 2 {
                self.bake(root_node).unwrap()
            } else if self.nodes.len() == 2 {
                vec![0, 1]
            } else {
                vec![0]
            };
        
            let barriers = self.create_barriers(&execution_order, swapchain_image_index);
            for (pass_index, pass_handle) in execution_order.iter().enumerate() {
                let pass = &self.nodes[*pass_handle];
                unsafe {
                    Functions::cmd_start_label(cmd, &pass.name);
                    pass.cmd_push_constants(
                        &self,
                        cmd,
                        descriptor_offsets[*pass_handle] as u32 * size_of::<u32>() as u32,
                    );
        
                    let barrier = &barriers[pass_index];
                    // println!("{}:{:#?}", pass.name, barrier);
                    if barrier.images.len() != 0 || barrier.buffers.len() != 0 {
                        Functions::cmd_insert_label(cmd, &format!("Barrier for {}", pass.name));
                        let dependency_info = vk::DependencyInfo::default()
                            .buffer_memory_barriers(&barrier.buffers)
                            .image_memory_barriers(&barrier.images);
                        Ctx::device()
                            .cmd_pipeline_barrier2(*cmd, &dependency_info);
                    }
        
                    pass.execution
                        .execute(cmd, self, &pass.edges)
                        .unwrap();
                    Functions::cmd_end_label(cmd);
                }
            }
        
            if let Some(barrier) = barriers.get(execution_order.len()) {
                Functions::cmd_insert_label(&cmd, "Transitioning Swapchain Image");
                let dependency_info = vk::DependencyInfo::default()
                    .buffer_memory_barriers(&barrier.buffers)
                    .image_memory_barriers(&barrier.images);
                unsafe {
                    Ctx::device()
                        .cmd_pipeline_barrier2(*cmd, &dependency_info)
                };
            }
            Ok(())
        }).unwrap();
    }
}
