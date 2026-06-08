
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
    pub queue: u64,
    pub queue_state: u64,
    pub visible_meshlets: u64,
    pub canidate_meshlets: u64,
    pub meshlet_batch_buffer: u64,
    pub instance_transforms: u64,
    pub instance_headers: u64,
    pub camera_pos: Vec4,
    pub proj: Mat4,
    pub clip_from_world: Mat4,
    pub window_height: f32,
}

pub struct BvhCullBindings<'a> {
    pub queue: BufferSlice<'a, InstanceBvhRoot>,
    pub queue_state: BufferSlice<'a, TraversalVariables>,
    pub visible_meshlets: BufferSlice<'a, InstancedMeshlet>,
    pub canidate_meshlets: BufferSlice<'a, InstanceMeshletIndex>,
    pub meshlet_batch_buffer: BufferSlice<'a, u32>,
    pub instance_transforms: BufferSlice<'a, Mat4>,
    pub instance_headers: BufferSlice<'a, InstanceHeader>,
    pub camera_pos: Vec4,
    pub proj: Mat4,
    pub clip_from_world: Mat4,
    pub window_height: f32,
}

unsafe impl bytemuck::Pod for CBvhCullBindings {}
unsafe impl bytemuck::Zeroable for CBvhCullBindings {}

impl Binding for CBvhCullBindings {
    type CpuBinding<'a> = BvhCullBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            queue: bindings.queue.gpu_ptr,
queue_state: bindings.queue_state.gpu_ptr,
visible_meshlets: bindings.visible_meshlets.gpu_ptr,
canidate_meshlets: bindings.canidate_meshlets.gpu_ptr,
meshlet_batch_buffer: bindings.meshlet_batch_buffer.gpu_ptr,
instance_transforms: bindings.instance_transforms.gpu_ptr,
instance_headers: bindings.instance_headers.gpu_ptr,
camera_pos: bindings.camera_pos,
proj: bindings.proj,
clip_from_world: bindings.clip_from_world,
window_height: bindings.window_height,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.queue.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.queue_state.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.visible_meshlets.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.canidate_meshlets.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.meshlet_batch_buffer.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.instance_transforms.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.instance_headers.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct BvhCull;


impl ComputePass for BvhCull {
    type GpuBinding = CBvhCullBindings;

    const ENTRY: &'static str = "bvh_cull\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/bvh_cull.slang.spv");
    fn cache() -> &'static OnceLock<VkPipeline> {
        static CACHE: OnceLock<VkPipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CDrawAabbsBindings {
    pub world_to_clip: Mat4,
    pub gizzmos: u64,
}

pub struct DrawAabbsBindings<'a> {
    pub world_to_clip: Mat4,
    pub gizzmos: BufferSlice<'a, Gizzmo>,
}

unsafe impl bytemuck::Pod for CDrawAabbsBindings {}
unsafe impl bytemuck::Zeroable for CDrawAabbsBindings {}

impl Binding for CDrawAabbsBindings {
    type CpuBinding<'a> = DrawAabbsBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            world_to_clip: bindings.world_to_clip,
gizzmos: bindings.gizzmos.gpu_ptr,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.gizzmos.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct DrawAabbs;


impl RasterPass for DrawAabbs {
    type GpuBinding = CDrawAabbsBindings;
}

impl RasterVertexShaderPass for DrawAabbs {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/draw_aabbs.slang.spv");
    
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
pub struct CDrawArrowsBindings {
    pub world_to_clip: Mat4,
    pub gizzmos: u64,
}

pub struct DrawArrowsBindings<'a> {
    pub world_to_clip: Mat4,
    pub gizzmos: BufferSlice<'a, Gizzmo>,
}

unsafe impl bytemuck::Pod for CDrawArrowsBindings {}
unsafe impl bytemuck::Zeroable for CDrawArrowsBindings {}

impl Binding for CDrawArrowsBindings {
    type CpuBinding<'a> = DrawArrowsBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            world_to_clip: bindings.world_to_clip,
gizzmos: bindings.gizzmos.gpu_ptr,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.gizzmos.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct DrawArrows;


impl RasterPass for DrawArrows {
    type GpuBinding = CDrawArrowsBindings;
}

impl RasterVertexShaderPass for DrawArrows {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/draw_arrows.slang.spv");
    
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
pub struct CDrawOutlineBindings {
    pub depth: BindlessHandle,
    pub out: BindlessHandle,
    pub outline_color_and_radius: Vec4,
    pub view_port_offset: IVec2,
    pub view_port_size: UVec2,
    pub swpachain_size: UVec2,
}

pub struct DrawOutlineBindings<'a> {
    pub depth: SampledImageViewBinding<'a>,
    pub out: StorageImageViewBinding<'a>,
    pub outline_color_and_radius: Vec4,
    pub view_port_offset: IVec2,
    pub view_port_size: UVec2,
    pub swpachain_size: UVec2,
}

unsafe impl bytemuck::Pod for CDrawOutlineBindings {}
unsafe impl bytemuck::Zeroable for CDrawOutlineBindings {}

impl Binding for CDrawOutlineBindings {
    type CpuBinding<'a> = DrawOutlineBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            depth: bindings.depth.handle,
out: bindings.out.handle,
outline_color_and_radius: bindings.outline_color_and_radius,
view_port_offset: bindings.view_port_offset,
view_port_size: bindings.view_port_size,
swpachain_size: bindings.swpachain_size,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.depth.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_SAMPLED_READ,
    layout: bindings.depth.prefered_layout,
    ..Default::default()
}),
(bindings.out.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,
    layout: bindings.out.prefered_layout,
    ..Default::default()
}),
        ]
    }
}
pub struct DrawOutline;


impl ComputePass for DrawOutline {
    type GpuBinding = CDrawOutlineBindings;

    const ENTRY: &'static str = "computeMain\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/draw_outline.slang.spv");
    fn cache() -> &'static OnceLock<VkPipeline> {
        static CACHE: OnceLock<VkPipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CDrawSpheresBindings {
    pub world_to_clip: Mat4,
    pub gizzmos: u64,
}

pub struct DrawSpheresBindings<'a> {
    pub world_to_clip: Mat4,
    pub gizzmos: BufferSlice<'a, Gizzmo>,
}

unsafe impl bytemuck::Pod for CDrawSpheresBindings {}
unsafe impl bytemuck::Zeroable for CDrawSpheresBindings {}

impl Binding for CDrawSpheresBindings {
    type CpuBinding<'a> = DrawSpheresBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            world_to_clip: bindings.world_to_clip,
gizzmos: bindings.gizzmos.gpu_ptr,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.gizzmos.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct DrawSpheres;


impl RasterPass for DrawSpheres {
    type GpuBinding = CDrawSpheresBindings;
}

impl RasterVertexShaderPass for DrawSpheres {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/draw_spheres.slang.spv");
    
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
    pub instance_bvh_root_nodes: u64,
    pub instance_aabbs: u64,
    pub instance_transforms: u64,
    pub bvh_node_stack: u64,
    pub variables: u64,
    pub clip_from_world: Mat4,
}

pub struct InstanceCullBindings<'a> {
    pub num_instances: u64,
    pub instance_bvh_root_nodes: BufferSlice<'a, u64>,
    pub instance_aabbs: BufferSlice<'a, AabbError>,
    pub instance_transforms: BufferSlice<'a, Mat4>,
    pub bvh_node_stack: BufferSlice<'a, InstanceBvhRoot>,
    pub variables: BufferSlice<'a, TraversalVariables>,
    pub clip_from_world: Mat4,
}

unsafe impl bytemuck::Pod for CInstanceCullBindings {}
unsafe impl bytemuck::Zeroable for CInstanceCullBindings {}

impl Binding for CInstanceCullBindings {
    type CpuBinding<'a> = InstanceCullBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            num_instances: bindings.num_instances,
instance_bvh_root_nodes: bindings.instance_bvh_root_nodes.gpu_ptr,
instance_aabbs: bindings.instance_aabbs.gpu_ptr,
instance_transforms: bindings.instance_transforms.gpu_ptr,
bvh_node_stack: bindings.bvh_node_stack.gpu_ptr,
variables: bindings.variables.gpu_ptr,
clip_from_world: bindings.clip_from_world,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.instance_bvh_root_nodes.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.instance_aabbs.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.instance_transforms.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.bvh_node_stack.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.variables.into(), 
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
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/instance_cull.slang.spv");
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
    pub cull_data: u64,
}

pub struct MeshshaderBindings<'a> {
    pub proj: Mat4,
    pub view: Mat4,
    pub model: Mat4,
    pub meshlets: BufferSlice<'a, Meshlet>,
    pub cull_data: BufferSlice<'a, CullData>,
}

unsafe impl bytemuck::Pod for CMeshshaderBindings {}
unsafe impl bytemuck::Zeroable for CMeshshaderBindings {}

impl Binding for CMeshshaderBindings {
    type CpuBinding<'a> = MeshshaderBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            proj: bindings.proj,
view: bindings.view,
model: bindings.model,
meshlets: bindings.meshlets.gpu_ptr,
cull_data: bindings.cull_data.gpu_ptr,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.meshlets.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.cull_data.into(), 
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
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/meshshader.slang.spv");
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
    pub instance_transforms: u64,
    pub meshlets: u64,
}

pub struct RasterBindings<'a> {
    pub view: Mat4,
    pub proj: Mat4,
    pub instance_transforms: BufferSlice<'a, Mat4>,
    pub meshlets: BufferSlice<'a, InstancedMeshlet>,
}

unsafe impl bytemuck::Pod for CRasterBindings {}
unsafe impl bytemuck::Zeroable for CRasterBindings {}

impl Binding for CRasterBindings {
    type CpuBinding<'a> = RasterBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            view: bindings.view,
proj: bindings.proj,
instance_transforms: bindings.instance_transforms.gpu_ptr,
meshlets: bindings.meshlets.gpu_ptr,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.instance_transforms.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.meshlets.into(), 
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
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/raster.slang.spv");
    
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
pub struct CRasterUiBindings {
    pub verticies: u64,
    pub font_atlas: BindlessHandle,
}

pub struct RasterUiBindings<'a> {
    pub verticies: BufferSlice<'a, UIVertex>,
    pub font_atlas: SampledImageViewBinding<'a>,
}

unsafe impl bytemuck::Pod for CRasterUiBindings {}
unsafe impl bytemuck::Zeroable for CRasterUiBindings {}

impl Binding for CRasterUiBindings {
    type CpuBinding<'a> = RasterUiBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            verticies: bindings.verticies.gpu_ptr,
font_atlas: bindings.font_atlas.handle,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.verticies.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.font_atlas.into(), 
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
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/raster_ui.slang.spv");
    
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
pub struct CSkyboxBindings {
    pub inverse_proj: Mat4,
    pub inverse_view: Mat4,
    pub out: BindlessHandle,
    pub view_port_offset: IVec2,
    pub view_port_size: UVec2,
    pub swpachain_size: UVec2,
}

pub struct SkyboxBindings<'a> {
    pub inverse_proj: Mat4,
    pub inverse_view: Mat4,
    pub out: StorageImageViewBinding<'a>,
    pub view_port_offset: IVec2,
    pub view_port_size: UVec2,
    pub swpachain_size: UVec2,
}

unsafe impl bytemuck::Pod for CSkyboxBindings {}
unsafe impl bytemuck::Zeroable for CSkyboxBindings {}

impl Binding for CSkyboxBindings {
    type CpuBinding<'a> = SkyboxBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            inverse_proj: bindings.inverse_proj,
inverse_view: bindings.inverse_view,
out: bindings.out.handle,
view_port_offset: bindings.view_port_offset,
view_port_size: bindings.view_port_size,
swpachain_size: bindings.swpachain_size,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.out.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_WRITE | AccessFlags2::SHADER_STORAGE_READ,
    layout: bindings.out.prefered_layout,
    ..Default::default()
}),
        ]
    }
}
pub struct Skybox;


impl ComputePass for Skybox {
    type GpuBinding = CSkyboxBindings;

    const ENTRY: &'static str = "skybox\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/skybox.slang.spv");
    fn cache() -> &'static OnceLock<VkPipeline> {
        static CACHE: OnceLock<VkPipeline> = OnceLock::new();
        &CACHE
    }
}
#[derive(Clone, Copy)]
#[repr(C)]
pub struct CRasterOutlineBindings {
    pub view: Mat4,
    pub proj: Mat4,
    pub instance_transforms: u64,
    pub meshlets: u64,
    pub instance_flags: u64,
}

pub struct RasterOutlineBindings<'a> {
    pub view: Mat4,
    pub proj: Mat4,
    pub instance_transforms: BufferSlice<'a, Mat4>,
    pub meshlets: BufferSlice<'a, InstancedMeshlet>,
    pub instance_flags: BufferSlice<'a, u32>,
}

unsafe impl bytemuck::Pod for CRasterOutlineBindings {}
unsafe impl bytemuck::Zeroable for CRasterOutlineBindings {}

impl Binding for CRasterOutlineBindings {
    type CpuBinding<'a> = RasterOutlineBindings<'a>;

    fn from_cpu_binding<'a>(bindings: &Self::CpuBinding<'a>) -> Self {
        Self {
            view: bindings.view,
proj: bindings.proj,
instance_transforms: bindings.instance_transforms.gpu_ptr,
meshlets: bindings.meshlets.gpu_ptr,
instance_flags: bindings.instance_flags.gpu_ptr,
        }
    }

    fn resources<'a>(
        bindings: &Self::CpuBinding<'a>,
        stages: PipelineStageFlags2,
    ) -> Vec<(ResourceHandle, ResourceState)> {
        vec![
            (bindings.instance_transforms.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.meshlets.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
(bindings.instance_flags.into(), 
ResourceState {
    stages,
    access: AccessFlags2::SHADER_STORAGE_READ,

    ..Default::default()
}),
        ]
    }
}
pub struct RasterOutline;


impl RasterPass for RasterOutline {
    type GpuBinding = CRasterOutlineBindings;
}

impl RasterVertexShaderPass for RasterOutline {
    const VERTEX: &'static str = "vertex\0";
    const FRAGMENT: &'static str = "fragment\0";
    const BYTES: &[u8] = include_bytes!("/home/karsten/code/GameEngine/core/../shaders/bin/raster_outline.slang.spv");
    
    fn module_cache() -> &'static OnceLock<VkShaderModule> {
        static CACHE: OnceLock<VkShaderModule> = OnceLock::new();
        &CACHE
    }
    fn pipeline_cache() -> &'static Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> {
        static CACHE: Mutex<LazyCell<HashMap<RasterHash, VkPipeline>>> = Mutex::new(LazyCell::new(|| HashMap::new()));
        &CACHE
    }
}
    
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct Meshlet {
    pub vertex_index: u64,
    pub triangle_index: u64,
    pub vertex_count: u32,
    pub triangle_count: u32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct InstanceHeader {
    pub meshlet_offset: u64,
    pub cull_data_offset: u64,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct AabbError {
    pub center_and_error: Vec4,
    pub half_extent: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct CullData {
    pub aabb: AabbError,
    pub lod_group_sphere: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct BvhNode {
    pub aabb_and_offsets: [AabbPtr; 8],
    pub errors: [f32; 8],
    pub lod_bounds: [Vec4; 8],
    pub child_counts: u64,
    pub pad: UVec2,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct InstancedMeshlet {
    pub instance: u64,
    pub meshlet: u64,
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
pub struct InstanceBvhRoot {
    pub instance: u64,
    pub node: u64,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct AabbPtr {
    pub center_and_offset_high: Vec4,
    pub half_extent_and_offset_low: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct InstanceMeshletIndex {
    pub instance: u32,
    pub meshlet: u32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct Vertex {
    pub position_and_uv1: Vec4,
    pub normal_and_uv2: Vec4,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct TraversalVariables {
    pub node_batch_read_offset: u32,
    pub total_meshlets: u32,
    pub node_write_offset: u32,
    pub node_count: u32,
    pub vertex_count: u32,
    pub visible_meshlet_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
    pub candidate_meshlet_write_offset: u32,
    pub meshlet_batch_read_offset: u32,
}
#[derive(Pod, Copy, Clone, Zeroable, Debug)]
#[repr(C)]
pub struct Gizzmo {
    pub transform: Mat4,
    pub color: Vec4,
}