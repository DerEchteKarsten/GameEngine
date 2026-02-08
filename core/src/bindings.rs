
use std::collections::HashMap;
use std::sync::{OnceLock, Mutex}; 
use glam::*;
use bytemuck::{Pod, Zeroable};
use lava::command_buffer::{Binding, ResourceHandle, ResourceState, ShaderHash, RasterHash, ComputePass, RasterPass, RayTracingPass, RasterMeshShaderPass, RasterVertexShaderPass};
use lava::bindless::BindlessHandle;
use lava::vkobjects::image::Image;
use lava::buffer::slice::BufferSlice;
use std::cell::{LazyCell};
use ash::vk;
use lava::vkobjects::image::get_aspects;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct CBvhCullBindings {
    pub bvh_nodes: u64,
    pub instance_transforms: u64,
    pub cull_data: u64,
    pub bvh_node_stack: u64,
    pub clusters: u64,
    pub dp: u64,
}

pub struct BvhCullBindings {
    pub bvh_nodes: BufferSlice<BvhNode>,
    pub instance_transforms: BufferSlice<Mat4>,
    pub cull_data: BufferSlice<CullData>,
    pub bvh_node_stack: BufferSlice<InstancedOffset>,
    pub clusters: BufferSlice<InstancedOffset>,
    pub dp: BufferSlice<DispatchParams>,
}

unsafe impl bytemuck::Pod for CBvhCullBindings {}
unsafe impl bytemuck::Zeroable for CBvhCullBindings {}

impl Binding for CBvhCullBindings {
    type CpuBinding<'a> = BvhCullBindings;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            bvh_nodes: bindings.bvh_nodes.gpu_address() as u64,
instance_transforms: bindings.instance_transforms.gpu_address() as u64,
cull_data: bindings.cull_data.gpu_address() as u64,
bvh_node_stack: bindings.bvh_node_stack.gpu_address() as u64,
clusters: bindings.clusters.gpu_address() as u64,
dp: bindings.dp.gpu_address() as u64,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.bvh_nodes.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_transforms.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.cull_data.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.bvh_node_stack.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.clusters.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.dp.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
        ]
    }
}
pub struct BvhCull;


impl ComputePass for BvhCull {
    type GpuBinding = CBvhCullBindings;

    const ENTRY: &'static str = "bvh_cull\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/bvh_cull.slang.spv");
    fn cache() -> &'static OnceLock<vk::Pipeline> {
        static CACHE: OnceLock<vk::Pipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CInstanceCullBindings {
    pub num_instances: u64,
    pub aabbs: u64,
    pub instance_bvh_root_nodes: u64,
    pub instance_transforms: u64,
    pub dp: u64,
    pub bvh_node_stack: u64,
}

pub struct InstanceCullBindings {
    pub num_instances: u64,
    pub aabbs: BufferSlice<Aabb>,
    pub instance_bvh_root_nodes: BufferSlice<u32>,
    pub instance_transforms: BufferSlice<Mat4>,
    pub dp: BufferSlice<DispatchParams>,
    pub bvh_node_stack: BufferSlice<InstancedOffset>,
}

unsafe impl bytemuck::Pod for CInstanceCullBindings {}
unsafe impl bytemuck::Zeroable for CInstanceCullBindings {}

impl Binding for CInstanceCullBindings {
    type CpuBinding<'a> = InstanceCullBindings;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            num_instances: bindings.num_instances,
aabbs: bindings.aabbs.gpu_address() as u64,
instance_bvh_root_nodes: bindings.instance_bvh_root_nodes.gpu_address() as u64,
instance_transforms: bindings.instance_transforms.gpu_address() as u64,
dp: bindings.dp.gpu_address() as u64,
bvh_node_stack: bindings.bvh_node_stack.gpu_address() as u64,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.aabbs.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_bvh_root_nodes.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_transforms.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.dp.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.bvh_node_stack.handle),
ResourceState {
    stages,
    access: vk::AccessFlags2::SHADER_STORAGE_WRITE | vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: vk::ImageLayout::UNDEFINED,
aspect: vk::ImageAspectFlags::empty(), 

}),
        ]
    }
}
pub struct InstanceCull;


impl ComputePass for InstanceCull {
    type GpuBinding = CInstanceCullBindings;

    const ENTRY: &'static str = "instance_cull\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/instance_cull.slang.spv");
    fn cache() -> &'static OnceLock<vk::Pipeline> {
        static CACHE: OnceLock<vk::Pipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct DispatchIndirectCommand {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct AabbErrorOffset {
    pub center_and_error: Vec4,
    pub half_extent_and_offset: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct BvhNode {
    pub aabbs: [AabbErrorOffset; 8],
    pub lod_bounds: [Vec4; 8],
    pub child_counts: u64,
    pub pad: u64,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct CullData {
    pub aabb: AabbErrorOffset,
    pub lod_group_sphere: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct DrawIndirectCommand {
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct InstancedOffset {
    pub instance: u32,
    pub offset: i32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct DispatchParams {
    pub node_head: u32,
    pub node_tail: u32,
    pub done: u32,
    pub meshlet_count: u32,
    pub indirect_draw: DrawIndirectCommand,
    pub indirect_dispatch: DispatchIndirectCommand,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct Aabb {
    pub center: Vec4,
    pub half_extent: Vec4,
}