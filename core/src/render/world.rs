//! Render-world instance management: extracting scene instances into per-frame GPU buffers.
use std::fmt::Debug;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use bevy::app::App;
use bevy::asset::Assets;
use bevy::ecs::query::Has;
use bevy::ecs::resource::Resource;
use bevy::ecs::system::{Commands, Query, Res, ResMut, Single};
use bevy::math::Rect;
use bevy::transform::components::GlobalTransform;
use bevy::window::Window;
use futures::channel::oneshot;

use bevy::log::{error, warn_once};
use bytemuck::Pod;
use glam::{Mat4, UVec2, Vec2, Vec3};
use lava::buffer::Buffer;
use lava::buffer::slice::BufferSlice;
use lava::image::slice::{AsImage, ImageSlice};
use lava::image::usage::ImageUsage;
use lava::state::{Ctx, Functions, raw_vulkan};
use lava::vkobjects::queue::{CommandBufferMemory, CommandPool, Fence, Gfx, Queue, Transfer};
use lava::{AccessFlags2, ImageLayout, PipelineStageFlags2};

use crate::INITIAL_WINDOW_SIZE;
use crate::assets::material::GpuMaterial;
use crate::assets::mesh::{GpuMesh, MeshHeader};
use crate::bindless;
use crate::editor::picking::Selected;
use crate::editor::viewport::ViewPort;
use crate::render::MainWorld;
use crate::render::extract_param::Extract;
use crate::render::render::{
    FrameCount, QueueStrategie, Queues, Swapchain, ViewPortTarget, extract_camera, extract_skybox,
};
use crate::render::{ExtractSchedule, FRAMES_IN_FLIGHT, RenderStartup, RenderSystems};
use crate::scene::Instance;
use lava::bindings::{self, AabbError};
use lava::bindless::NULL_HANDLE;
use lava::image::Image;

#[derive(Resource)]
pub struct InstanceManager {
    pub transforms: Buffer<Mat4>,
    pub bvh_root_nodes: Buffer<u64>,
    pub headers: Buffer<bindings::InstanceHeader>,
    pub aabbs: Buffer<AabbError>,
    pub flags: Buffer<u32>,
    pub instance_materials: Buffer<u64>,
    pub instance_count: usize,
    pub any_outlined: bool,
    pending_instances: Vec<TempInstance>,
}

#[derive(Clone, Copy)]
pub struct InstanceFlags(pub u32);

impl InstanceFlags {
    const OUTLINE: InstanceFlags = InstanceFlags(0b00000001);
    pub fn contains(&self, v: InstanceFlags) -> bool {
        (self.0 & v.0) > 0
    }
    pub fn insert(&mut self, v: InstanceFlags) {
        self.0 |= v.0;
    }
    pub fn remove(&mut self, v: InstanceFlags) {
        self.0 &= !v.0;
    }
    pub fn empty() -> Self {
        Self(0)
    }
}

impl std::ops::BitOr for InstanceFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy)]
struct TempInstance {
    flags: InstanceFlags,
    material: u64,
    transform: Mat4,
    bvh_root: u64,
    header: MeshHeader,
}

pub const MAX_INSTANCES: usize = 8 * 1024;

pub(super) fn init_world(mut cmd: Commands) {
    cmd.insert_resource(InstanceManager {
        instance_materials: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        headers: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        aabbs: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        transforms: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        bvh_root_nodes: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        instance_count: 0,
        any_outlined: false,
        flags: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        pending_instances: Vec::with_capacity(MAX_INSTANCES),
    });
    cmd.init_resource::<FrameCount>();
}

pub(super) fn extract_meshlet_instances(
    mut instance_manager: ResMut<InstanceManager>,
    instances: Extract<Query<(&Instance, &GlobalTransform, Has<Selected>)>>,
    meshes: Extract<Res<Assets<GpuMesh>>>,
    materials: Extract<Res<Assets<GpuMaterial>>>,
) {
    instance_manager.any_outlined = false;
    for (instance, transform, selected) in &instances {
        if let Some(mesh) = meshes.get(&instance.mesh)
            && let Some(material) = materials.get(&instance.material)
        {
            if instance_manager.pending_instances.len() >= MAX_INSTANCES {
                warn_once!("more than {MAX_INSTANCES} instances, the rest are not drawn");
                break;
            }
            let mat = transform.to_matrix();
            let flags = if selected {
                InstanceFlags::OUTLINE
            } else {
                InstanceFlags::empty()
            };
            instance_manager.any_outlined |= flags.contains(InstanceFlags::OUTLINE);
            instance_manager.pending_instances.push(TempInstance {
                bvh_root: mesh.buffer.address,
                header: mesh.header,
                transform: mat,
                material: material.gpu,
                flags,
            });
        }
    }
}

pub(super) fn wirte_instances(mut instances: ResMut<InstanceManager>, frame: Res<FrameCount>) {
    let frame_in_flight = frame.frame_in_flight();
    for slot in 0..instances.pending_instances.len() {
        let instance = instances.pending_instances[slot];
        instances.transforms[slot + frame_in_flight * MAX_INSTANCES] = instance.transform;
        instances.bvh_root_nodes[slot + frame_in_flight * MAX_INSTANCES] = instance.bvh_root;
        instances.aabbs[slot + frame_in_flight * MAX_INSTANCES] = AabbError {
            center_and_error: Vec3::from_array(instance.header.aabb.center).extend(0.0),
            half_extent: Vec3::from_array(instance.header.aabb.half_extend).extend(0.0),
        };
        instances.headers[slot + frame_in_flight * MAX_INSTANCES] = bindings::InstanceHeader {
            meshlet_offset: instance.header.meshlet_offset as u64 + instance.bvh_root,
            cull_data_offset: instance.header.cull_data_offset as u64 + instance.bvh_root,
        };
        instances.flags[slot + frame_in_flight * MAX_INSTANCES] = instance.flags.0;
        instances.instance_materials[slot + frame_in_flight * MAX_INSTANCES] = instance.material;
    }
    instances.instance_count = instances.pending_instances.len();
    instances.pending_instances.clear();
}

pub(super) fn extract_view_port(
    mut world: ResMut<MainWorld>,
    mut target: ResMut<ViewPortTarget>,
    swapchain: Res<Swapchain>,
) {
    if let Some(mut view_port) = world.get_resource_mut::<ViewPort>() {
        // Starts at the initial window size; `record_frame` grows it when the tab outgrows it.
        let size = view_port
            .rect
            .size()
            .as_uvec2()
            .max(INITIAL_WINDOW_SIZE.as_uvec2());
        let image = target.image.get_or_insert_with(|| {
            Image::new_storage(size.x, size.y, 1, bindless::storage_slots(1).unwrap()).unwrap()
        });
        view_port.image = image.handle;
        view_port.image_size = image.extent.as_vec2();
        target.rect = view_port.rect;
    } else {
        // No editor, so no viewport tab: the scene fills the swapchain image.
        target.rect = Rect::from_corners(Vec2::ZERO, UVec2::from(swapchain.size).as_vec2());
    }
}

#[allow(non_snake_case)]
pub fn WorldPlugin(app: &mut App) {
    app.add_systems(RenderStartup, init_world)
        .add_systems(
            ExtractSchedule,
            (extract_meshlet_instances, extract_camera, extract_skybox),
        )
        .add_systems(RenderSystems::PreRender, wirte_instances);
}
