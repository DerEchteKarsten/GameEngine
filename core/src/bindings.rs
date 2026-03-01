
use std::collections::HashMap;
use std::sync::{OnceLock, Mutex}; 
use glam::*;
use bytemuck::{Pod, Zeroable};
use lava::command_buffer::{Binding, ResourceHandle, ResourceState, ShaderHash, RasterHash, ComputePass, RasterPass, RayTracingPass, RasterMeshShaderPass, RasterVertexShaderPass};
use lava::bindless::BindlessHandle;
use lava::buffer::slice::BufferSlice;
use std::cell::{LazyCell};
use lava::{PipelineStageFlags2, AccessFlags2, ImageLayout, VkPipeline, VkShaderModule};
use lava::image::slice::{StorageImageViewBinding, SampledImageViewBinding};

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
    type CpuBinding = BvhCullBindings;

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {
        Self {
            bvh_nodes: bindings.bvh_nodes.gpu_address() as u64,
instance_transforms: bindings.instance_transforms.gpu_address() as u64,
cull_data: bindings.cull_data.gpu_address() as u64,
bvh_node_stack: bindings.bvh_node_stack.gpu_address() as u64,
clusters: bindings.clusters.gpu_address() as u64,
dp: bindings.dp.gpu_address() as u64,
        }
    }

    fn resources(
        bindings: &Self::CpuBinding,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.bvh_nodes.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.instance_transforms.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.cull_data.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.bvh_node_stack.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.clusters.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.dp.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct BvhCull;


impl ComputePass for BvhCull {
    type GpuBinding = CBvhCullBindings;

    const ENTRY: &'static str = "bvhcull\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/bvh_cull.slang.spv");
    fn cache() -> &'static OnceLock<VkPipeline> {
        static CACHE: OnceLock<VkPipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CMeshshaderBindings {
    pub proj: Mat4,
    pub view: Mat4,
    pub model: Mat4,
    pub meshlets: u64,
}

pub struct MeshshaderBindings {
    pub proj: Mat4,
    pub view: Mat4,
    pub model: Mat4,
    pub meshlets: BufferSlice<Meshlet>,
}

unsafe impl bytemuck::Pod for CMeshshaderBindings {}
unsafe impl bytemuck::Zeroable for CMeshshaderBindings {}

impl Binding for CMeshshaderBindings {
    type CpuBinding = MeshshaderBindings;

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {
        Self {
            proj: bindings.proj,
view: bindings.view,
model: bindings.model,
meshlets: bindings.meshlets.gpu_address() as u64,
        }
    }

    fn resources(
        bindings: &Self::CpuBinding,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.meshlets.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct Meshshader;
impl RasterPass for Meshshader {
    type GpuBinding = CMeshshaderBindings;
}

impl RasterMeshShaderPass for Meshshader {
    const MESH: &'static str = "mesh\0";
    const FRAGMENT: &'static str = "mesh_fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/meshshader.slang.spv");
    const TASK: Option<&'static str> = Some("amp\0");

    fn module_cache() -> &'static OnceLock<VkShaderModule> {
        static CACHE: OnceLock<VkShaderModule> = OnceLock::new();
        &CACHE
    }

    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> {
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CRasterBindings {
    pub view: Mat4,
    pub proj: Mat4,
    pub instances: u64,
    pub instance: u32,
    pub offset: u32,
}

pub struct RasterBindings {
    pub view: Mat4,
    pub proj: Mat4,
    pub instances: BufferSlice<u64>,
    pub instance: u32,
    pub offset: u32,
}

unsafe impl bytemuck::Pod for CRasterBindings {}
unsafe impl bytemuck::Zeroable for CRasterBindings {}

impl Binding for CRasterBindings {
    type CpuBinding = RasterBindings;

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {
        Self {
            view: bindings.view,
proj: bindings.proj,
instances: bindings.instances.gpu_address() as u64,
instance: bindings.instance,
offset: bindings.offset,
        }
    }

    fn resources(
        bindings: &Self::CpuBinding,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.instances.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct Raster;


impl RasterPass for Raster {
    type GpuBinding = CRasterBindings;
}

impl RasterVertexShaderPass for Raster {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/raster.slang.spv");
    
    fn module_cache() -> &'static OnceLock<VkShaderModule> {
        static CACHE: OnceLock<VkShaderModule> = OnceLock::new();
        &CACHE
    }
    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> {
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
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
    pub aabbs: BufferSlice<AabbError>,
    pub instance_bvh_root_nodes: BufferSlice<u32>,
    pub instance_transforms: BufferSlice<Mat4>,
    pub dp: BufferSlice<DispatchParams>,
    pub bvh_node_stack: BufferSlice<InstancedOffset>,
}

unsafe impl bytemuck::Pod for CInstanceCullBindings {}
unsafe impl bytemuck::Zeroable for CInstanceCullBindings {}

impl Binding for CInstanceCullBindings {
    type CpuBinding = InstanceCullBindings;

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {
        Self {
            num_instances: bindings.num_instances,
aabbs: bindings.aabbs.gpu_address() as u64,
instance_bvh_root_nodes: bindings.instance_bvh_root_nodes.gpu_address() as u64,
instance_transforms: bindings.instance_transforms.gpu_address() as u64,
dp: bindings.dp.gpu_address() as u64,
bvh_node_stack: bindings.bvh_node_stack.gpu_address() as u64,
        }
    }

    fn resources(
        bindings: &Self::CpuBinding,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.aabbs.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.instance_bvh_root_nodes.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.instance_transforms.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.dp.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.bvh_node_stack.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct InstanceCull;


impl ComputePass for InstanceCull {
    type GpuBinding = CInstanceCullBindings;

    const ENTRY: &'static str = "instance_cull\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/instance_cull.slang.spv");
    fn cache() -> &'static OnceLock<VkPipeline> {
        static CACHE: OnceLock<VkPipeline> = OnceLock::new();
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

pub struct PostBindings {
    pub inverse_proj: Mat4,
    pub inverse_view: Mat4,
    pub window_size: Vec4,
    pub depth: SampledImageViewBinding,
    pub color: SampledImageViewBinding,
    pub out: StorageImageViewBinding,
}

unsafe impl bytemuck::Pod for CPostBindings {}
unsafe impl bytemuck::Zeroable for CPostBindings {}

impl Binding for CPostBindings {
    type CpuBinding = PostBindings;

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {
        Self {
            inverse_proj: bindings.inverse_proj,
inverse_view: bindings.inverse_view,
window_size: bindings.window_size,
depth: bindings.depth.handle,
color: bindings.color.handle,
out: bindings.out.handle,
        }
    }

    fn resources(
        bindings: &Self::CpuBinding,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Image(bindings.depth.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_SAMPLED_READ,
    layout: bindings.depth.prefered_layout,
    ..Default::default()
}),
(ResourceHandle::Image(bindings.color.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_SAMPLED_READ,
    layout: bindings.color.prefered_layout,
    ..Default::default()
}),
(ResourceHandle::Image(bindings.out.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,
    layout: bindings.out.prefered_layout,
    ..Default::default()
}),
        ]
    }
}
pub struct Post;


impl ComputePass for Post {
    type GpuBinding = CPostBindings;

    const ENTRY: &'static str = "post\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/post.slang.spv");
    fn cache() -> &'static OnceLock<VkPipeline> {
        static CACHE: OnceLock<VkPipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CRasterUiBindings {
    pub verticies: u64,
    pub indicies: u64,
    pub font_atlas: BindlessHandle,
}

pub struct RasterUiBindings {
    pub verticies: BufferSlice<UIVertex>,
    pub indicies: BufferSlice<u32>,
    pub font_atlas: SampledImageViewBinding,
}

unsafe impl bytemuck::Pod for CRasterUiBindings {}
unsafe impl bytemuck::Zeroable for CRasterUiBindings {}

impl Binding for CRasterUiBindings {
    type CpuBinding = RasterUiBindings;

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {
        Self {
            verticies: bindings.verticies.gpu_address() as u64,
indicies: bindings.indicies.gpu_address() as u64,
font_atlas: bindings.font_atlas.handle,
        }
    }

    fn resources(
        bindings: &Self::CpuBinding,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.verticies.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Buffer(bindings.indicies.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Image(bindings.font_atlas.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_SAMPLED_READ,
    layout: bindings.font_atlas.prefered_layout,
    ..Default::default()
}),
        ]
    }
}
pub struct RasterUi;


impl RasterPass for RasterUi {
    type GpuBinding = CRasterUiBindings;
}

impl RasterVertexShaderPass for RasterUi {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/raster_ui.slang.spv");
    
    fn module_cache() -> &'static OnceLock<VkShaderModule> {
        static CACHE: OnceLock<VkShaderModule> = OnceLock::new();
        &CACHE
    }
    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> {
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }
}
    
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CDrawVerticiesBindings {
    pub view: Mat4,
    pub proj: Mat4,
    pub instances: u64,
    pub instance: u32,
    pub offset: u32,
    pub window_size: UVec2,
    pub out: BindlessHandle,
    pub num_verticies: u32,
}

pub struct DrawVerticiesBindings {
    pub view: Mat4,
    pub proj: Mat4,
    pub instances: BufferSlice<u64>,
    pub instance: u32,
    pub offset: u32,
    pub window_size: UVec2,
    pub out: StorageImageViewBinding,
    pub num_verticies: u32,
}

unsafe impl bytemuck::Pod for CDrawVerticiesBindings {}
unsafe impl bytemuck::Zeroable for CDrawVerticiesBindings {}

impl Binding for CDrawVerticiesBindings {
    type CpuBinding = DrawVerticiesBindings;

    fn from_cpu_binding(bindings: &Self::CpuBinding) -> Self {
        Self {
            view: bindings.view,
proj: bindings.proj,
instances: bindings.instances.gpu_address() as u64,
instance: bindings.instance,
offset: bindings.offset,
window_size: bindings.window_size,
out: bindings.out.handle,
num_verticies: bindings.num_verticies,
        }
    }

    fn resources(
        bindings: &Self::CpuBinding,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (ResourceHandle::Buffer(bindings.instances.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(ResourceHandle::Image(bindings.out.into()),
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,
    layout: bindings.out.prefered_layout,
    ..Default::default()
}),
        ]
    }
}
pub struct DrawVerticies;


impl ComputePass for DrawVerticies {
    type GpuBinding = CDrawVerticiesBindings;

    const ENTRY: &'static str = "draw\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/Documents/code/GameEngine/core/../shaders/bin/draw_verticies.slang.spv");
    fn cache() -> &'static OnceLock<VkPipeline> {
        static CACHE: OnceLock<VkPipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct AabbPtr {
    pub center_and_offset_high: Vec4,
    pub half_extent_and_offset_low: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct AabbError {
    pub center_and_error: Vec4,
    pub half_extent: Vec4,
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
pub struct Meshlet {
    pub vertex_index: u64,
    pub triangle_index: u64,
    pub vertex_count: u32,
    pub triangle_count: u32,
    pub pad: UVec2,
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
pub struct Vertex {
    pub position_and_uv1: Vec4,
    pub normal_and_uv2: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct InstancedOffset {
    pub instance: u32,
    pub offset: i32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct UIVertex {
    pub pos: Vec2,
    pub uv: Vec2,
    pub color: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct CullData {
    pub aabb: AabbError,
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
pub struct BvhNode {
    pub aabb_and_offsets: [AabbPtr; 8],
    pub errors: [f32; 8],
    pub lod_bounds: [Vec4; 8],
    pub child_counts: u64,
    pub pad: u64,
}