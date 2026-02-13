use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::num::NonZeroU64;
use std::ops::{Deref, DerefMut, Range};
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;
use async_std::sync::Mutex;
use bevy::asset::{AsAssetId, LoadState};
use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use bevy::tasks::futures::check_ready;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, Scope, Task, TaskPool, block_on};
use bytemuck::Pod;
use futures::join;
use glam::Mat4;
use gpu_allocator::vulkan::Allocation;
use lava::buffer::Buffer;

use lava::buffer::slice::{BufferSlice, BufferView};
use lava::buffer::{AsBuffer, BufferUsageFlags, CpuBuffer, GpuBuffer, Location};
use lava::command_buffer::CommandBuffer;
use lava::image::format::R8Uint;
use lava::image::slice::{ImageSlice, TypeLessImageView};
use lava::image::usage::UsageSet;
use lava::state::Ctx;
use lava::vkobjects;
use lava::vkobjects::acceleration_structure::AccelerationStructure;
use lava::vkobjects::queue::{CommandBufferMemory, CommandPool, Fence};
use rand::random;
use smallvec::SmallVec;

use crate::assets::mesh::MeshletMesh;
use crate::assets::{Mesh, material::Material};
use crate::bindings::{AabbError, BvhNode, CullData, Meshlet, Vertex};
use crate::render::extract_param::Extract;
use crate::render::render::{CommandPools, FrameCount, Swapchain, SynchronizationResources, extract_camera};
use crate::render::{ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, Render, RenderStartup, RenderSystems};
use crate::ui::UiContext;

#[derive(Component, Clone)]
pub struct Model {
    pub model: Handle<Mesh>,
}

impl AsAssetId for Model {
    type Asset = Mesh;
    fn as_asset_id(&self) -> AssetId<Self::Asset> {
        self.model.id()
    }
}

#[derive(Resource)]
pub struct InstanceManager {
    pub transforms: Buffer<Mat4, CpuBuffer>,
    pub materials: Buffer<u32, CpuBuffer>,
    pub bvh_root_nodes: Buffer<u64, CpuBuffer>,
    pub aabbs: Buffer<AabbError, CpuBuffer>,
    pub max_bvh_depth: u32,
    pub max_instance_count: usize,
}

#[derive(Clone, Copy)]
struct Instance {
    transform: Mat4,
    material: u32,
    bvh_root: u64,
    aabb: AabbError,
}

impl InstanceManager {
    fn add_instance(&mut self, instance: Instance) {
        let slot = self.max_instance_count;
        self.max_instance_count += 1;
        self.transforms
            .whole()
            .mem_copy_from(BufferSlice::from(&[instance.transform]));
        self.materials
            .whole()
            .mem_copy_from(BufferSlice::from(&[instance.material]));
        self.bvh_root_nodes
            .whole()
            .mem_copy_from(BufferSlice::from(&[instance.bvh_root]));
        self.aabbs
            .whole()
            .mem_copy_from(BufferSlice::from(&[instance.aabb]));
    }
}

const STAGING_BUFFER_SIZE: usize = 16 * 1024 * 1024;


enum Dst {
    Buffer(BufferSlice),
    Image(ImageSlice)
}
struct CopyRegion {
    src: BufferSlice<u8, CpuBuffer>,
    dst: Dst,
}

#[derive(Resource)]
pub struct UploadQueue {
    pool: CommandPool,
    fence: Fence,
    cmd: CommandBufferMemory,
    buffer: Buffer<u8, CpuBuffer>,
    queue: Vec<CopyRegion>,
    task: Option<Task<()>>,
}

impl UploadQueue {
    pub fn new() -> Self {
        let buffer = Buffer::new(STAGING_BUFFER_SIZE).unwrap();
        let pool = Ctx::transfer_queue().create_pool();
        Self {
            queue: Vec::new(),
            buffer,
            cmd: pool.create_command_buffer(),
            pool: pool,
            fence: Fence::new(),
            task: None,
        }
    }
    fn resolve_writes(&mut self) {
        if self.task.is_none() || self.queue.is_empty() {
            return;
        }

        if check_ready(self.task.as_mut().unwrap()).is_some() {
            self.task = None;
        }

        let mut queue = std::mem::take(&mut self.queue);
        let whole = self.buffer.whole();
        let fence = self.fence.clone();
        let cmd = self.cmd.clone();
        let pool = self.pool.clone();
        self.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            let mut staging = whole.clone();
            let mut slice_start = 0;

            async fn flush(
                queue: &[CopyRegion],
                fence: Fence,
                cmd: CommandBufferMemory,
                pool: CommandPool,
                range: std::ops::Range<usize>,
            ) {
                pool.reset();
                fence.reset();
                Ctx::transfer_queue().execute_command(cmd.clone(), Some(fence), &[], &[], |cmd| {
                    for entry in &queue[range] {
                        match entry.dst {
                            Dst::Buffer(buff) => {
                                cmd.copy_buffer(entry.src, buff);
                                cmd.release_buffer(buff, Ctx::gfx_queue_index());
                            },
                            Dst::Image(img) => {
                                cmd.copy_buffer_to_image(entry.src, img);
                            }
                        }
                    }
                }).unwrap();
                fence.wait_async().await;
            }

            for i in 0..queue.len() {
                while staging.push(queue[i].src).is_err() {
                    let remaining = staging.size;
                    staging.mem_copy_from(queue[i].src.num_bytes(remaining));
                    queue[i].src.size -= remaining;
                    queue[i].src.offset += remaining;

                    flush(&queue, fence, cmd, pool, slice_start..(i + 1)).await;

                    staging = whole.clone();
                    slice_start = i;
                }
            }

            if slice_start < queue.len() {
                flush(&queue, fence, cmd, pool, slice_start..queue.len()).await;
            }
            for e in queue {
                unsafe { Arc::decrement_strong_count(e.src.cpu_base_ptr as *const u32) };
            }
        }));
    }

    pub fn push_buffer<T: Copy+Pod+Send+Sync>(&mut self, src: &Arc<[T]>, buffer: BufferSlice<T, GpuBuffer>) {
        unsafe { Arc::increment_strong_count(Arc::as_ptr(src)) };
        self.queue.push(CopyRegion { src: BufferSlice::from(src).cast(), dst: Dst::Buffer(buffer.cast()) });
    }
    pub fn push_image<F: lava::image::format::Format, U: UsageSet>(&mut self, src: &Arc<[u8]>, image: ImageSlice<F, U>) {
        unsafe { Arc::increment_strong_count(Arc::as_ptr(src)) };
        self.queue.push(CopyRegion { src: BufferSlice::from(src).cast(), dst: Dst::Image(image.cast()) });
    }
}

#[derive(Resource, Default)]
pub struct MeshletManager {
    pub mesh_buffers: Vec<Buffer<u8>>,
    pub asset_instance_aabbs: Vec<AabbError>,
    pub asset_instance_transforms: Vec<Mat4>,
    pub asset_instance_meshes: Vec<u32>,
    pub asset_instance_materials: Vec<u32>,
    pub assets: HashMap<AssetId<Mesh>, (u32, u32)>,
    pub _acceleration_structure_scratch_memory: Option<Buffer<u8>>,
    pub _acceleration_structure_memory: Option<Buffer<u8>>,
    pub _tlas: Option<AccelerationStructure>,
}

fn size<T>(t: &Arc<[T]>) -> u64 {
    (t.len() * size_of::<T>()) as u64
}

impl MeshletManager {
    fn queue_upload_if_needed(
        &mut self,
        id: AssetId<Mesh>,
        assets: &mut Assets<Mesh>,
        instance_manager: &mut InstanceManager,
        upload_queue: &mut UploadQueue,
    ) {
        let queue_meshlet_mesh = |asset_id: &AssetId<Mesh>| {
            let meshlet_mesh = assets.remove_untracked(*asset_id).expect(
                "MeshletMesh asset was already unloaded but is not registered with MeshletManager",
            );
            let mesh_offset = self.mesh_buffers.len() as u32;
            self.mesh_buffers
                .extend(meshlet_mesh.meshes.iter().map(|mesh| {
                    let vert_size = size(&mesh.vertices);
                    let ind_size = size(&mesh.indices);
                    let meshlet_size = size(&mesh.meshlets);
                    let bvh_size = size(&mesh.bvh);
                    let cull_size = size(&mesh.cull_data);

                    let vertices_offset = bvh_size;
                    let indices_offset = vert_size + vertices_offset;
                    let meshlets_offset = ind_size + indices_offset;
                    let cull_data_offset = meshlet_size + meshlets_offset;

                    let buffer: Buffer<u8> = Buffer::new(
                        (cull_data_offset + cull_size) as usize,
                    )
                    .unwrap();
                    let address = buffer.address;

                    assert!(Arc::is_unique(&mesh.meshlets));
                    let meshlet_count = mesh.meshlets.len();
                    let meshlet_ptr = mesh.meshlets.as_ptr().cast_mut();
                    for i in 0..meshlet_count {
                        let m = unsafe { meshlet_ptr.add(i).as_mut().unwrap() };
                        m.triangle_index += indices_offset as u64 + address;
                        m.vertex_index += vertices_offset as u64 + address;
                    }

                    assert!(Arc::is_unique(&mesh.bvh));
                    let bvh_count = mesh.bvh.len();
                    let bvh_ptr = mesh.bvh.as_ptr().cast_mut();
                    for i in 0..bvh_count {
                        let n = unsafe { bvh_ptr.add(i).as_mut().unwrap() };
                        n.aabb_and_offsets
                            .iter_mut()
                            .enumerate()
                            .for_each(|(i, aabb)| {
                                let offset = aabb.offset();
                                aabb.set_offset(
                                    offset
                                        + if ((n.child_counts >> (i * 8)) & 0xFF) as u8 == 255 {
                                            address
                                        } else {
                                            meshlets_offset as u64 + address
                                        },
                                );
                            });
                    }

                    upload_queue.push_buffer(&mesh.bvh, buffer.whole().num_bytes(bvh_size).cast());
                    upload_queue.push_buffer(&mesh.vertices, buffer.whole().num_bytes(vert_size).byte_offset(vertices_offset as u64).cast());
                    upload_queue.push_buffer(&mesh.indices, buffer.whole().num_bytes(ind_size).byte_offset(indices_offset as u64).cast());
                    upload_queue.push_buffer(&mesh.meshlets, buffer.whole().num.byte_offset(meshlets_offset as u64).cast());
                    upload_queue.push_buffer(&mesh.cull_data, buffer.whole().byte_offset(cull_data_offset as u64).cast());

                    buffer
                }));
            let instance_offset = self.asset_instance_meshes.len() as u32;
            let instance_count = meshlet_mesh.instance_mesh.len() as u32;
            let material_offset = 0; //TODO
            self.asset_instance_materials.extend(
                meshlet_mesh
                    .instance_materials
                    .iter()
                    .map(|e| e + material_offset),
            );
            self.asset_instance_meshes
                .extend(meshlet_mesh.instance_mesh.iter().map(|e| e + mesh_offset));
            self.asset_instance_transforms
                .extend_from_slice(&meshlet_mesh.instance_transforms);
            self.asset_instance_aabbs.extend(
                meshlet_mesh
                    .instance_mesh
                    .iter()
                    .map(|e| meshlet_mesh.meshes[*e as usize].aabb),
            );
            (instance_offset, instance_count)
        };

        let (instance_offset, instance_count) = self
            .assets
            .entry(id)
            .or_insert_with_key(queue_meshlet_mesh)
            .clone();
        (instance_offset as usize..instance_count as usize).for_each(|i| {
            let mesh = self.asset_instance_meshes[i] as usize;

            instance_manager.add_instance(Instance {
                aabb: self.asset_instance_aabbs[mesh],
                bvh_root: self.mesh_buffers[mesh].address,
                transform: self.asset_instance_transforms[i],
                material: self.asset_instance_materials[i],
            });
        });
    }
}

pub(super) fn init_world(mut cmd: Commands) {
    cmd.insert_resource(InstanceManager {
        aabbs: Buffer::new(1024 * 10).unwrap(),
        transforms: Buffer::new(1024 * 10).unwrap(),
        bvh_root_nodes: Buffer::new(1024 * 10).unwrap(),
        materials: Buffer::new(1024 * 10).unwrap(),
        max_bvh_depth: 5,
        max_instance_count: 1024 * 10,
    });
    cmd.init_resource::<FrameCount>();
}

fn extract_meshlet_instances(
    mut instance_manager: ResMut<InstanceManager>,
    mut meshlet_manager: ResMut<MeshletManager>,
    mut main_world: ResMut<MainWorld>,
    mut upload_queue: ResMut<UploadQueue>,
    mut system_state: Local<
        Option<
            SystemState<(
                Query<(&Model, &GlobalTransform)>,
                Res<AssetServer>,
                ResMut<Assets<Mesh>>,
                MessageReader<AssetEvent<Mesh>>,
            )>,
        >,
    >,
) {
    if system_state.is_none() {
        *system_state = Some(SystemState::new(&mut main_world));
    }
    let system_state = system_state.as_mut().unwrap();
    let (instances_query, asset_server, mut assets, mut asset_events) =
        system_state.get_mut(&mut main_world);

    instance_manager.max_bvh_depth = 0;

    for asset_event in asset_events.read() {
        if let AssetEvent::Unused { id } | AssetEvent::Modified { id } = asset_event {
            todo!();
        }
    }

    for (instance, transform) in &instances_query {
        if asset_server.is_managed(instance.model.id())
            && !asset_server.is_loaded_with_dependencies(instance.model.id())
        {
            continue;
        }

        let transform = transform.affine();
        meshlet_manager.queue_upload_if_needed(
            instance.model.id(),
            &mut assets,
            &mut instance_manager,
            &mut upload_queue,
        );
    }
}

fn apply_writes(mut buffer: ResMut<UploadQueue>) {
    buffer.resolve_writes();
}

pub fn WorldPlugin(app: &mut App) {
    app.add_systems(RenderStartup, init_world)
        .add_systems(ExtractSchedule, (extract_meshlet_instances, extract_camera))
        .add_systems(Render, apply_writes);
}
