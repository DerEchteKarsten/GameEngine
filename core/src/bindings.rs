use std::collections::HashMap;
use std::sync::{OnceLock, Mutex}; use glam::*;
use bytemuck::{Pod, Zeroable};
use lava::command_buffer::{ResourceHandle, ResourceState, ShaderHash, RasterHash};
use lava::bindless::BindlessHandle;
use std::cell::{LazyCell};
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

pub struct BvhCullBindings<'a> {
    pub bvh_nodes: &'a lava::vkobjects::buffer::Buffer<BvhNode>,
    pub instance_transforms: &'a lava::vkobjects::buffer::Buffer<Mat4>,
    pub cull_data: &'a lava::vkobjects::buffer::Buffer<CullData>,
    pub bvh_node_stack: &'a lava::vkobjects::buffer::Buffer<InstancedOffset>,
    pub clusters: &'a lava::vkobjects::buffer::Buffer<InstancedOffset>,
    pub dp: &'a lava::vkobjects::buffer::Buffer<DispatchParams>,
}

unsafe impl bytemuck::Pod for CBvhCullBindings {}
unsafe impl bytemuck::Zeroable for CBvhCullBindings {}

impl lava::command_buffer::Binding for CBvhCullBindings {
    type CpuBinding<'a> = BvhCullBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            bvh_nodes: bindings.bvh_nodes.address,
instance_transforms: bindings.instance_transforms.address,
cull_data: bindings.cull_data.address,
bvh_node_stack: bindings.bvh_node_stack.address,
clusters: bindings.clusters.address,
dp: bindings.dp.address,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: ash::vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.bvh_nodes.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_transforms.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.cull_data.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.bvh_node_stack.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.clusters.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.dp.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
        ]
    }
}
pub struct BvhCull;


impl lava::command_buffer::ComputePass for BvhCull {
    type GpuBinding = CBvhCullBindings;

    const ENTRY: &'static str = "bvh_cull\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/bvh_cull.slang.spv");
    fn cache() -> &'static OnceLock<ash::vk::Pipeline> {
        static CACHE: OnceLock<ash::vk::Pipeline> = OnceLock::new();
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

pub struct InstanceCullBindings<'a> {
    pub num_instances: u64,
    pub aabbs: &'a lava::vkobjects::buffer::Buffer<Aabb>,
    pub instance_bvh_root_nodes: &'a lava::vkobjects::buffer::Buffer<u32>,
    pub instance_transforms: &'a lava::vkobjects::buffer::Buffer<Mat4>,
    pub dp: &'a lava::vkobjects::buffer::Buffer<DispatchParams>,
    pub bvh_node_stack: &'a lava::vkobjects::buffer::Buffer<InstancedOffset>,
}

unsafe impl bytemuck::Pod for CInstanceCullBindings {}
unsafe impl bytemuck::Zeroable for CInstanceCullBindings {}

impl lava::command_buffer::Binding for CInstanceCullBindings {
    type CpuBinding<'a> = InstanceCullBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            num_instances: bindings.num_instances,
aabbs: bindings.aabbs.address,
instance_bvh_root_nodes: bindings.instance_bvh_root_nodes.address,
instance_transforms: bindings.instance_transforms.address,
dp: bindings.dp.address,
bvh_node_stack: bindings.bvh_node_stack.address,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: ash::vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.aabbs.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_bvh_root_nodes.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_transforms.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.dp.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.bvh_node_stack.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_WRITE | ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
        ]
    }
}
pub struct InstanceCull;


impl lava::command_buffer::ComputePass for InstanceCull {
    type GpuBinding = CInstanceCullBindings;

    const ENTRY: &'static str = "instance_cull\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/instance_cull.slang.spv");
    fn cache() -> &'static OnceLock<ash::vk::Pipeline> {
        static CACHE: OnceLock<ash::vk::Pipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CMeshshaderBindings {
    pub proj: Mat4,
    pub view: Mat4,
    pub model: Mat4,
    pub vertices: u64,
    pub indecies: u64,
    pub meshlets: u64,
}

pub struct MeshshaderBindings<'a> {
    pub proj: Mat4,
    pub view: Mat4,
    pub model: Mat4,
    pub vertices: &'a lava::vkobjects::buffer::Buffer<Vertex>,
    pub indecies: &'a lava::vkobjects::buffer::Buffer<u8>,
    pub meshlets: &'a lava::vkobjects::buffer::Buffer<Meshlet>,
}

unsafe impl bytemuck::Pod for CMeshshaderBindings {}
unsafe impl bytemuck::Zeroable for CMeshshaderBindings {}

impl lava::command_buffer::Binding for CMeshshaderBindings {
    type CpuBinding<'a> = MeshshaderBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            proj: bindings.proj,
view: bindings.view,
model: bindings.model,
vertices: bindings.vertices.address,
indecies: bindings.indecies.address,
meshlets: bindings.meshlets.address,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: ash::vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.vertices.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.indecies.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.meshlets.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
        ]
    }
}
pub struct Meshshader;
impl lava::command_buffer::RasterPass for Meshshader {
    type GpuBinding = CMeshshaderBindings;
}

impl lava::command_buffer::RasterMeshShaderPass for Meshshader {
    const MESH: &'static str = "mesh\0";
    const FRAGMENT: &'static str = "mesh_fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/meshshader.slang.spv");
    const TASK: Option<&'static str> = Some("amp\0");

    fn module_cache() -> &'static OnceLock<ash::vk::ShaderModule> {
        static CACHE: OnceLock<ash::vk::ShaderModule> = OnceLock::new();
        &CACHE
    }

    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> {
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CPostBindings {
    pub inverse_proj: Mat4,
    pub inverse_view: Mat4,
    pub window_size: Vec4,
    pub depth: BindlessHandle,
    pub color: BindlessHandle,
    pub out: BindlessHandle,
}

pub struct PostBindings<'a> {
    pub inverse_proj: Mat4,
    pub inverse_view: Mat4,
    pub window_size: Vec4,
    pub depth: &'a lava::vkobjects::image::Image,
    pub color: &'a lava::vkobjects::image::Image,
    pub out: &'a lava::vkobjects::image::Image,
}

unsafe impl bytemuck::Pod for CPostBindings {}
unsafe impl bytemuck::Zeroable for CPostBindings {}

impl lava::command_buffer::Binding for CPostBindings {
    type CpuBinding<'a> = PostBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            inverse_proj: bindings.inverse_proj,
inverse_view: bindings.inverse_view,
window_size: bindings.window_size,
depth: bindings.depth.bindless_handle.unwrap(),
color: bindings.color.bindless_handle.unwrap(),
out: bindings.out.bindless_handle.unwrap(),
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: ash::vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Image((bindings.depth.view, bindings.depth.handle)),
ResourceState {
    stages,
    access: bindings.depth.const_access(),
    layout: bindings.depth.prefered_layout(),
    aspect: lava::vkobjects::image::get_aspects(bindings.depth.format),
}),
(ResourceHandle::Image((bindings.color.view, bindings.color.handle)),
ResourceState {
    stages,
    access: bindings.color.const_access(),
    layout: bindings.color.prefered_layout(),
    aspect: lava::vkobjects::image::get_aspects(bindings.color.format),
}),
(ResourceHandle::Image((bindings.out.view, bindings.out.handle)),
ResourceState {
    stages,
    access: bindings.out.mut_access(),
    layout: bindings.out.prefered_layout(),
    aspect: lava::vkobjects::image::get_aspects(bindings.out.format),
}),
        ]
    }
}
pub struct Post;


impl lava::command_buffer::ComputePass for Post {
    type GpuBinding = CPostBindings;

    const ENTRY: &'static str = "post\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/post.slang.spv");
    fn cache() -> &'static OnceLock<ash::vk::Pipeline> {
        static CACHE: OnceLock<ash::vk::Pipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CRasterBindings {
    pub view: Mat4,
    pub proj: Mat4,
    pub verticies: u64,
    pub indicies: u64,
    pub meshlets: u64,
    pub instance_offsets: u64,
    pub instance_transforms: u64,
}

pub struct RasterBindings<'a> {
    pub view: Mat4,
    pub proj: Mat4,
    pub verticies: &'a lava::vkobjects::buffer::Buffer<Vertex>,
    pub indicies: &'a lava::vkobjects::buffer::Buffer<u8>,
    pub meshlets: &'a lava::vkobjects::buffer::Buffer<Meshlet>,
    pub instance_offsets: &'a lava::vkobjects::buffer::Buffer<InstancedOffset>,
    pub instance_transforms: &'a lava::vkobjects::buffer::Buffer<Mat4>,
}

unsafe impl bytemuck::Pod for CRasterBindings {}
unsafe impl bytemuck::Zeroable for CRasterBindings {}

impl lava::command_buffer::Binding for CRasterBindings {
    type CpuBinding<'a> = RasterBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            view: bindings.view,
proj: bindings.proj,
verticies: bindings.verticies.address,
indicies: bindings.indicies.address,
meshlets: bindings.meshlets.address,
instance_offsets: bindings.instance_offsets.address,
instance_transforms: bindings.instance_transforms.address,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: ash::vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.verticies.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.indicies.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.meshlets.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_offsets.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
(ResourceHandle::Buffer(bindings.instance_transforms.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
        ]
    }
}
pub struct Raster;


impl lava::command_buffer::RasterPass for Raster {
    type GpuBinding = CRasterBindings;
}

impl lava::command_buffer::RasterVertexShaderPass for Raster {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/raster.slang.spv");
    
    fn module_cache() -> &'static OnceLock<ash::vk::ShaderModule> {
        static CACHE: OnceLock<ash::vk::ShaderModule> = OnceLock::new();
        &CACHE
    }
    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> {
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }
}
    
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CRasterUiBindings {
    pub verticies: u64,
}

pub struct RasterUiBindings<'a> {
    pub verticies: &'a lava::vkobjects::buffer::Buffer<UIVertex>,
}

unsafe impl bytemuck::Pod for CRasterUiBindings {}
unsafe impl bytemuck::Zeroable for CRasterUiBindings {}

impl lava::command_buffer::Binding for CRasterUiBindings {
    type CpuBinding<'a> = RasterUiBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            verticies: bindings.verticies.address,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: ash::vk::PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.verticies.handle),
ResourceState {
    stages,
    access: ash::vk::AccessFlags2::SHADER_STORAGE_READ,
    layout: ash::vk::ImageLayout::UNDEFINED,
aspect: ash::vk::ImageAspectFlags::empty(), 

}),
        ]
    }
}
pub struct RasterUi;


impl lava::command_buffer::RasterPass for RasterUi {
    type GpuBinding = CRasterUiBindings;
}

impl lava::command_buffer::RasterVertexShaderPass for RasterUi {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/raster_ui.slang.spv");
    
    fn module_cache() -> &'static OnceLock<ash::vk::ShaderModule> {
        static CACHE: OnceLock<ash::vk::ShaderModule> = OnceLock::new();
        &CACHE
    }
    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> {
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, ash::vk::Pipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }
}
    
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct InstancedOffset {
    pub instance: u32,
    pub offset: i32,
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
pub struct DispatchIndirectCommand {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct UIVertex {
    pub pos_and_uv: Vec4,
    pub color: Vec4,
    pub texture_index: BindlessHandle,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct AabbErrorOffset {
    pub center_and_error: Vec4,
    pub half_extent_and_offset: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct Aabb {
    pub center: Vec4,
    pub half_extent: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct CullData {
    pub aabb: AabbErrorOffset,
    pub lod_group_sphere: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct Vertex {
    pub position_and_uv1: Vec4,
    pub normal_and_uv2: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct Meshlet {
    pub vertex_count: u32,
    pub vertex_index: u32,
    pub triangle_count: u32,
    pub triangle_index: u32,
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
pub struct DispatchParams {
    pub node_head: u32,
    pub node_tail: u32,
    pub done: u32,
    pub meshlet_count: u32,
    pub indirect_draw: DrawIndirectCommand,
    pub indirect_dispatch: DispatchIndirectCommand,
}