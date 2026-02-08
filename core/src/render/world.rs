use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::ops::{Deref, DerefMut, Range};
use std::ptr::NonNull;
use std::sync::Arc;

use async_std::sync::Mutex;
use bevy::asset::{AsAssetId, LoadState};
use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use bevy::tasks::futures::{check_ready};
use bevy::tasks::futures_lite::{future};
use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, Task, TaskPool, block_on};
use bytemuck::Pod;
use futures::join;
use glam::Mat4;
use lava::FRAMES_IN_FLIGHT;
use lava::buffer::Buffer;
use lava::buffer::allocator::{ArenaAllocator, AsyncSubAllocator, QueueAllocated, RangeAllocator, SubAllocated};
use lava::buffer::slice::BufferSlice;
use lava::buffer::{AsBuffer, BufferUsageFlags, CpuBuffer, GpuBuffer, Location};
use lava::state::Ctx;
use lava::vkobjects::acceleration_structure::AccelerationStructure;
use rand::random;

use crate::assets::mesh::MeshletMesh;
use crate::assets::{Mesh, material::Material};
use crate::bindings::{Aabb, BvhNode, CullData, Meshlet, Vertex};
use crate::render::extract_param::Extract;
use crate::render::storage_buffer::StorageBuffer;
use crate::render::{ExtractSchedule, MainWorld, Render, RenderStartup, RenderSystems};
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

const STAGING_BUFFER_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Resource)]
pub struct UploadBuffer {
    pub staging_buffer: Buffer<u8, CpuBuffer>,
    pub allocator: AsyncSubAllocator<RangeAllocator>,
}

impl UploadBuffer {
    pub fn new() -> Self {
        let staging_buffer =
            Buffer::new(BufferUsageFlags::STORAGE, STAGING_BUFFER_SIZE as usize).unwrap();
        Self {
            allocator: AsyncSubAllocator::new(
                staging_buffer.whole(),
                RangeAllocator::new(STAGING_BUFFER_SIZE),
            ),
            staging_buffer,
        }
    }
}

#[derive(Resource)]
pub struct InstanceManager {
    pub transforms: QueueAllocated<Buffer<Mat4>>,
    pub materials: QueueAllocated<Buffer<u32>>,
    pub bvh_root_nodes: QueueAllocated<Buffer<u32>>,
    pub aabbs: QueueAllocated<Buffer<Aabb>>,
    pub max_bvh_depth: u32,
}

#[derive(Clone, Copy)]
struct Instance {
    transform: Mat4,
    material: u32,
    bvh_root: u32,
    aabb: Aabb,
    max_bvh_depth: u32,
}

impl InstanceManager {
    fn add_instance(&mut self, instance: Instance) {
        self.transforms.push(instance.transform);
        self.materials.push(instance.material);
        self.bvh_root_nodes.push(instance.bvh_root);
        self.aabbs.push(instance.aabb);
        self.max_bvh_depth = self.max_bvh_depth.max(instance.max_bvh_depth);
    }

    fn apply_writes(&mut self, buffer: &mut UploadBuffer) {
        self.transforms.assert_size();
        self.materials.assert_size();
        self.bvh_root_nodes.assert_size();
        self.aabbs.assert_size();

        fn copy<T: Pod+ Copy + Send>(buff: &mut QueueAllocated<Buffer<T>>, allocator: AsyncSubAllocator<RangeAllocator>) -> Task<BufferSlice<T, CpuBuffer>>{
            buff.assert_size();
            let slice = buff.whole();
            let size = buff.queue_size();
            let queue = std::mem::take(&mut buff.queue);
            AsyncComputeTaskPool::get().spawn(async move {
                let mut mem = allocator.allocate_blocking(size).await;
                mem.mem_copy_from(BufferSlice::from(queue.as_slice()));
                mem
            })
        }

        let regions = block_on(async {join!(
            copy(&mut self.transforms, buffer.allocator.clone()),
            copy(&mut self.materials, buffer.allocator.clone()),
            copy(&mut self.bvh_root_nodes, buffer.allocator.clone()),
            copy(&mut self.aabbs, buffer.allocator.clone()),
        )});

        Ctx::queue().execute_command_wait(|cmd| {
            cmd.copy_buffer(regions.0, self.transforms.whole());
            cmd.copy_buffer(regions.1, self.materials.whole());
            cmd.copy_buffer(regions.2, self.bvh_root_nodes.whole());
            cmd.copy_buffer(regions.3, self.aabbs.whole());
        }).unwrap();
    }
}

#[derive(Clone, Copy)]
struct MeshSlices {
    vertex_offset: u32,
    vertex_count: u32,
    index_offset: u32,
    index_count: u32,
    meshlet_offset: u32,
    meshlet_count: u32,
    cull_data_offset: u32,
    cull_data_count: u32,
    bvh_count: u32,

    aabb: Aabb,
    bvh_root: u32,
    max_bvh_depth: u32,
}

#[derive(Resource, Default)]
pub struct MeshletManager {
    pub vertices: StorageBuffer<Vertex>,
    pub indices: StorageBuffer<u8>,
    pub meshlets: StorageBuffer<Meshlet>,
    pub cull_data: StorageBuffer<CullData>,
    pub bvh_nodes: StorageBuffer<BvhNode>,
    pub materials: StorageBuffer<Material>,

    mesh_slices: Vec<MeshSlices>,
    asset_instance_meshes: Vec<u32>,
    asset_instance_transforms: Vec<Mat4>,
    asset_instance_materials: Vec<u32>,
    assets: HashMap<AssetId<Mesh>, (u32, u32)>,
    _acceleration_structure_scratch_memory: Option<Buffer<u8>>,
    _acceleration_structure_memory: Option<Buffer<u8>>,
    _tlas: Option<AccelerationStructure>,
}

impl MeshletManager {
    fn queue_upload_if_needed(
        &mut self,
        id: AssetId<Mesh>,
        assets: &mut Assets<Mesh>,
        instance_manager: &mut InstanceManager,
    ) {
        let queue_meshlet_mesh = |asset_id: &AssetId<Mesh>| {
            let meshlet_mesh = assets.remove_untracked(*asset_id).expect(
                "MeshletMesh asset was already unloaded but is not registered with MeshletManager",
            );

            let mesh_offset = self.mesh_slices.len() as u32;
            self.mesh_slices
                .extend(meshlet_mesh.meshes.iter().map(|mesh| {
                    let vertex_offset =
                        self.vertices.queue_wirte(Arc::clone(&mesh.vertices)) as u32;
                    let index_offset = self.indices.queue_wirte(Arc::clone(&mesh.indices)) as u32;

                    assert!(Arc::is_unique(&mesh.meshlets));
                    let meshlet_count = mesh.meshlets.len();
                    let meshlet_ptr = mesh.meshlets.as_ptr().cast_mut();
                    for i in 0..meshlet_count {
                        let m = unsafe { meshlet_ptr.add(i).as_mut().unwrap() };
                        m.triangle_index += index_offset;
                        m.vertex_index += vertex_offset;
                    }

                    let meshlet_offset =
                        self.meshlets.queue_wirte(Arc::clone(&mesh.meshlets)) as u32;

                    assert!(Arc::is_unique(&mesh.bvh));
                    let bvh_count = mesh.bvh.len();
                    let bvh_ptr = mesh.bvh.as_ptr().cast_mut();
                    let bvh_root = self.bvh_nodes.queue_size as u32;
                    for i in 0..bvh_count {
                        let n = unsafe { bvh_ptr.add(i).as_mut().unwrap() };
                        n.aabbs.iter_mut().enumerate().for_each(|(i, aabb)| {
                            let offset = aabb.offset();
                            aabb.set_offset(
                                offset
                                    + if ((n.child_counts >> (i * 8)) & 0xFF) as u8 == 255 {
                                        bvh_root
                                    } else {
                                        meshlet_offset
                                    },
                            );
                        });
                    }

                    MeshSlices {
                        cull_data_count: mesh.cull_data.len() as u32,
                        cull_data_offset: self.cull_data.queue_wirte(Arc::clone(&mesh.cull_data))
                            as u32,
                        bvh_count: mesh.bvh.len() as u32,
                        bvh_root: self.bvh_nodes.queue_wirte(Arc::clone(&mesh.bvh)) as u32,
                        vertex_count: mesh.vertices.len() as u32,
                        index_count: mesh.indices.len() as u32,
                        meshlet_count: mesh.meshlets.len() as u32,
                        meshlet_offset,
                        index_offset,
                        vertex_offset,
                        aabb: mesh.aabb,
                        max_bvh_depth: mesh.bvh_depth,
                    }
                }));
            let instance_offset = self.asset_instance_meshes.len() as u32;
            let instance_count = meshlet_mesh.instance_mesh.len() as u32;
            self.asset_instance_materials.extend(
                meshlet_mesh
                    .instance_materials
                    .iter()
                    .map(|e| e + mesh_offset),
            );
            self.asset_instance_meshes
                .extend(meshlet_mesh.instance_mesh.iter().map(|e| e + mesh_offset));
            self.asset_instance_transforms
                .extend_from_slice(&meshlet_mesh.instance_transforms);
            (instance_offset, instance_count)
        };

        let (instance_offset, instance_count) = self
            .assets
            .entry(id)
            .or_insert_with_key(queue_meshlet_mesh)
            .clone();
        (instance_offset as usize..instance_count as usize).for_each(|i| {
            let mesh = self.asset_instance_meshes[i] as usize;
            let slice = &self.mesh_slices[mesh];
            instance_manager.add_instance(Instance {
                aabb: slice.aabb,
                bvh_root: slice.bvh_root,
                max_bvh_depth: slice.max_bvh_depth,
                transform: self.asset_instance_transforms[i],
                material: self.asset_instance_materials[i],
            });
        });
    }

    fn apply_writes(&mut self, queue: &mut UploadBuffer) {
        self.bvh_nodes.resolve_write(queue);
        self.cull_data.resolve_write(queue);
        self.indices.resolve_write(queue);
        self.materials.resolve_write(queue);
        self.meshlets.resolve_write(queue);
        self.vertices.resolve_write(queue);
    }

}

pub(super) fn init_world(mut cmd: Commands) {
    cmd.insert_resource(MeshletManager::default());
}

fn extract_meshlet_instances(
    mut instance_manager: ResMut<InstanceManager>,
    mut meshlet_manager: ResMut<MeshletManager>,
    mut main_world: ResMut<MainWorld>,
    mut system_state: Local<
        Option<
            SystemState<(
                Query<(Entity, &Model, &GlobalTransform)>,
                Res<AssetServer>,
                ResMut<Assets<Mesh>>,
                MessageReader<AssetEvent<Mesh>>,
            )>,
        >,
    >,
    render_entities: &Entities,
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

    for (entity, instance, transform) in &instances_query {
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
        );
    }
}

fn apply_writes(mut buffer: ResMut<UploadBuffer>, mut meshes: ResMut<MeshletManager>) {
    meshes.apply_writes(&mut buffer);
}

fn apply_instance_writes(mut buffer: ResMut<UploadBuffer>, mut instances: ResMut<InstanceManager>) {
    instances.apply_writes(&mut buffer);
}

pub fn WorldPlugin(app: &mut App) {
    app.add_systems(RenderStartup, init_world)
        .add_systems(ExtractSchedule, extract_meshlet_instances)
        .add_systems(Render, apply_writes)
        .add_systems(Render, apply_instance_writes.in_set(RenderSystems::PreRender));
}
