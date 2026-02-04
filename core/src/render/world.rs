use std::collections::BTreeMap;
use std::ffi::c_void;
use std::future::AsyncDrop;
use std::ops::{Deref, DerefMut, Range};
use std::ptr::NonNull;
use std::sync::{Arc};

use bevy::asset::{AsAssetId, LoadState};
use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;

use bevy::tasks::futures::check_ready;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, Task, TaskPool, block_on};
use bytemuck::Pod;
use futures::lock::Mutex;
use glam::Mat4;
use lava::state::Ctx;
use lava::vkobjects::acceleration_structure::AccelerationStructure;
use lava::vkobjects::buffer::{Buffer, BufferUsageFlags, CpuBuffer, GpuBuffer, StorageBuffer};

use crate::assets::mesh::MeshletMesh;
use crate::bindings::{Aabb, BvhNode, CullData, Meshlet, Vertex};
use crate::render::{ExtractSchedule, MainWorld, Render, RenderStartup};
use crate::render::extract_param::Extract;
use crate::ui::UiContext;
use crate::{
    assets::{Mesh, material::Material},

};


#[derive(Component, Clone)]
pub struct Instance {
    pub model: Handle<Mesh>,
}

impl AsAssetId for Instance {
    type Asset = Mesh;
    fn as_asset_id(&self) -> AssetId<Self::Asset> {
        self.model.id()
    }
}


const STAGING_BUFFER_SIZE: u64 = 16 * 1024 * 1024;

struct Allocator {
    total_size: u64,
    free_ranges: BTreeMap<u64, u64>,
}


async fn dealloc(alloc: &Arc<Mutex<Allocator>>, start: u64, end: u64) {
    let mut inner = alloc.lock().await;
    inner.free_ranges.insert(start, end - start);
    let mut keys_to_merge = Vec::new();
    let mut last_start = None;
    for &start in inner.free_ranges.keys() {
        if let Some(prev) = last_start {
            if prev + inner.free_ranges[&prev] == start {
                keys_to_merge.push((prev, start));
            }
        }
        last_start = Some(start);
    }
    for (a, b) in keys_to_merge {
        let len_a = inner.free_ranges.remove(&a).unwrap();
        let len_b = inner.free_ranges.remove(&b).unwrap();
        inner.free_ranges.insert(a, len_a + len_b);
    }
}

#[derive(Resource)]
struct UploadQueue {
    allocator: Arc<Mutex<Allocator>>,
    staging_buffer: Buffer<u8, CpuBuffer>,
    delay_deletion: Vec<(Buffer<u8, GpuBuffer>, u64)>,
}

async fn allocate(alloc: &Arc<Mutex<Allocator>>, size: u64) -> Option<u64> {
    let mut inner = alloc.lock().await;
    for (&start, &length) in inner.free_ranges.iter() {
        if length >= size {
            inner.free_ranges.remove(&start);
            if length > size {
                inner.free_ranges.insert(start + size, length - size);
            }
            return Some(start);
        }
    }
    None
}

struct PersistantBuffer<T: Copy + Pod + Send + Sync> {
    buffer: Buffer<T>,
    buffer_task: Option<(u64, Task<Option<Buffer<T>>>)>,
    wirtes: Vec<(u64, Arc<[T]>)>,
    size: u64,
    queue_size: u64,
}

impl<T: Copy + Pod + Send + Sync> AsRef<Buffer<T>> for PersistantBuffer<T> {
    fn as_ref(&self) -> &Buffer<T> {
        &self.buffer
    }
}

impl<T: Copy + Pod + Send + Sync> AsMut<Buffer<T>> for PersistantBuffer<T> {
    fn as_mut(&mut self) -> &mut Buffer<T> {
        &mut self.buffer
    }
}

impl<T: Copy + Pod + Send + Sync> Default for PersistantBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Copy + Pod + Send + Sync> PersistantBuffer<T> {
    pub fn new() -> Self {
        Self {
            buffer: Buffer::with_alignment(BufferUsageFlags::STORAGE, 1024 * 1024, None).unwrap(),
            wirtes: Vec::new(),
            size: 0,
            buffer_task: None,
            queue_size: 0,
        }
    }
    
    pub fn queue_wirte(&mut self, data: Arc<[T]>) -> u64 {
        let offset = self.queue_size;
        let data_size = (data.len() * std::mem::size_of::<T>()) as u64;
        self.wirtes.push((offset, data));
        self.queue_size += data_size;
        offset
    }

    pub fn resolve_write(&mut self, queue: &mut UploadQueue) {
        if let Some((new_size, task)) = &mut self.buffer_task {
            if let Some(buffer) = check_ready(task) {
                self.size = *new_size;
                if let Some(buffer) = buffer{
                    queue.delay_deletion.push((std::mem::replace(&mut self.buffer, buffer).cast_owned(), 0));
                }
            }
        } else {
            if !self.wirtes.is_empty() {
                let queue_size = self.queue_size;
                self.buffer_task = Some((queue_size, AsyncComputeTaskPool::get().spawn({
                    let ptr = queue.cpu_ptr();
                    let writes = std::mem::take(&mut self.wirtes);
                    let staging_buffer = queue.staging_buffer.clone();
                    let allocator = queue.allocator.clone();
                    let mut buffer = self.buffer.clone();
                    let size = self.size as usize / size_of::<T>();
                    async move {
                        let new_buffer = if buffer.size < queue_size {
                            let new_buffer = Buffer::<T>::with_alignment(BufferUsageFlags::STORAGE, queue_size.next_power_of_two(), None).unwrap();
                            Ctx::transfer_queue().execute_command_wait(|cmd| {
                                cmd.copy_buffer(&buffer, &new_buffer, size, 0, 0);
                            });
                            buffer = new_buffer;
                            true 
                        }else {
                            false
                        };

                        for (dst_offset, data) in writes {
                            let data_size = (data.len() * std::mem::size_of::<T>()) as u64;
                            let src_offset = loop {
                                if let Some(v) = allocate(&allocator, data_size).await {
                                    break v;
                                }
                                log::error!("Staging Buffer Full!!");
                            };
                            let p = ptr as *mut u8;
                            unsafe { data.as_ptr().copy_to(p.byte_add(src_offset as usize).cast(), data.len()); };
                            Ctx::transfer_queue().execute_command_async(|cmd| {
                                cmd.copy_buffer(&staging_buffer, buffer.cast(), size, src_offset as u32, dst_offset as u32);
                            }).await;
                            dealloc(&allocator, src_offset, src_offset+data_size).await;
                        }
                        if new_buffer {
                            Some(buffer)
                        }else {
                            None
                        }
                    }
                }) ));
            }
        }
    } 
}

impl UploadQueue {
    pub fn new() -> Self {
        let mut free_ranges = BTreeMap::new();
        free_ranges.insert(0, STAGING_BUFFER_SIZE);
        
        Self {
            staging_buffer: Buffer::with_alignment(
                BufferUsageFlags::empty(),
                STAGING_BUFFER_SIZE,
                None,
            )
            .unwrap(),
            delay_deletion: Vec::new(),
            allocator: Arc::new(Mutex::new(Allocator {
                total_size: STAGING_BUFFER_SIZE,
                free_ranges,
            }))
        }
    }

    pub fn cpu_ptr(&self) -> usize {
        self.staging_buffer.allocation.read().unwrap().0.mapped_ptr().unwrap().as_ptr() as usize
    }
}

#[derive(Resource, Default)]
pub struct InstanceManager {
    pub transforms: StorageBuffer<Mat4>,
    pub materials: StorageBuffer<u32>,
    pub bvh_root_nodes: StorageBuffer<u32>,
    pub aabbs: StorageBuffer<Aabb>,
    pub max_bvh_depth: u32,
}

impl InstanceManager {
    fn clear(&mut self) {
        self.transforms.clear();
        self.materials.clear();
        self.bvh_root_nodes.clear();
        self.aabbs.clear();
        self.max_bvh_depth = 0;
    }
    fn add_instances(&mut self, queue: &mut UploadQueue, transforms: &[Mat4], material: &[u32], bvh_root_node_index: &[u32], aabbs: &[Aabb]) {
        let transform_size = size_of_val(transforms);
        let material_size = size_of_val(material);
        let bvh_size = size_of_val(bvh_root_node_index);
        let aabb_size = size_of_val(aabb);
        let size = transform_size + material_size + bvh_size + aabb_size;
        let ptr = queue.cpu_ptr();
        async fn copy<T>(data: &[T], offset: usize) {
            unsafe { data.as_ptr().copy_to(offset as *mut T, size_of_val(data)) }; 
        }
        futures::join!(
            ComputeTaskPool::get().spawn(copy(transforms, ptr)),
            ComputeTaskPool::get().spawn(copy(material, ptr + transform_size)),
            ComputeTaskPool::get().spawn(copy(bvh_root_node_index, ptr + transform_size + material_size)),
            ComputeTaskPool::get().spawn(copy(aabb, ptr + transform_size + material_size + bvh_size))
        );


        ComputeTaskPool::get().spawn_local(async {
            let mem = loop {
                if let Some(mem) = allocate(&queue.allocator, size as u64).await {
                    break mem;
                }
            };
            Ctx::queue()
                .execute_command_wait(|cmd| {
                    cmd.copy_buffer_regions(src, dst, num_elements, &regions);
                });
            dealloc(&queue.allocator, mem, mem+size).await;
        });
    }
}


#[derive(Resource, Default)]
pub struct MeshletManager {
    pub vertices: PersistantBuffer<Vertex>,
    pub indecies: PersistantBuffer<u8>,
    pub meshlets: PersistantBuffer<Meshlet>,
    pub cull_data: PersistantBuffer<CullData>,
    pub bvh_nodes: PersistantBuffer<BvhNode>,
    pub materials: PersistantBuffer<Material>,

    _acceleration_structure_scratch_memory: Option<Buffer<u8>>,
    _acceleration_structure_memory: Option<Buffer<u8>>,
    _tlas: Option<AccelerationStructure>,
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
                Query<(
                    Entity,
                    &Instance,
                    &GlobalTransform,
                )>,
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
    
    instance_manager.clear();

    for asset_event in asset_events.read() {
        if let AssetEvent::Unused { id } | AssetEvent::Modified { id } = asset_event {
            todo!();
        }
    }

    for(
        entity,
        instance,
        transform,
    ) in &instances_query
    {
        if asset_server.is_managed(instance.model.id())
            && !asset_server.is_loaded_with_dependencies(instance.model.id())
        {
            continue;
        }

        let (root_bvh_node, aabb, bvh_depth) =
            meshlet_mesh_manager.queue_upload_if_needed(instance.model.id(), &mut assets);

        let transform = transform.affine();
    
        render_world.instance_transforms.push(mesh_uniform);
        self.aabbs.get_mut().push(aabb);
        self.material_ids.get_mut().push(0);
        self.bvh_root_nodes.get_mut().push(root_bvh_node);

        self.scene_instance_count += 1;
        self.max_bvh_depth = self.max_bvh_depth.max(bvh_depth);
    }

    for event in &events {
        match event {
            AssetEvent::LoadedWithDependencies { id } => {
                let mesh = meshes.get_mut(&mesh).unwrap();
                for m in &mut mesh.meshes {
                    let meshlet_index = world.meshlets.len() + meshlets.len();
                    let mmeshlets = m
                        .meshlets
                        .iter()
                        .map(|m| Meshlet {
                            triangle_count: m.triangle_count,
                            vertex_count: m.vertex_count,
                            triangle_index: m.triangle_index
                                + world.indecies.len() as u32
                                + indices.len() as u32,
                            vertex_index: m.vertex_index
                                + world.vertices.len() as u32
                                + vertices.len() as u32,
                        })
                        .collect::<Vec<_>>();
                    vertices.extend(m.vertices.clone());
                    indices.extend(m.indices.clone());
                    meshlets.extend(mmeshlets);
                    cull_data.extend(m.cull_data.clone());

                    let bvh_root = world.bvh_nodes.len() + bvh.len();
                    m.bvh_root_node_index = bvh_root as u32;
                    let mbvh = m
                        .bvh
                        .iter()
                        .map(|n| {
                            let mut n = n.clone();
                            n.aabbs.iter_mut().enumerate().for_each(|(i, aabb)| {
                                let offset = aabb.offset();
                                aabb.set_offset(
                                    offset
                                        + if ((n.child_counts >> (i * 8)) & 0xFF) as u8 == 255 {
                                            bvh_root as u32
                                        } else {
                                            meshlet_index as u32
                                        },
                                );
                            });
                            n
                        })
                        .collect::<Vec<_>>();
                    bvh.extend(mbvh);

                    world.max_bvh_depth = world.max_bvh_depth.max(m.bvh_depth);
                }
                let instance_offset = world.instance_transforms.len() + transforms.len();
                let children = mesh
                    .instance_transforms
                    .iter()
                    .enumerate()
                    .map(|(i, mat)| {
                        cmd.spawn((
                            UploadedInstance {
                                instance_offset: instance_offset + i,
                            },
                            Transform::from_matrix(*mat),
                        ))
                        .id()
                    })
                    .collect::<Vec<_>>();
                cmd.entity(entity)
                    .add_children(&children)
                    .insert(UploadedInstance { instance_offset });
    
                transforms.extend(mesh.instance_transforms.clone());
                bvh_root_nodes.extend(
                    mesh.instance_mesh
                        .iter()
                        .map(|m| mesh.meshes[(*m) as usize].bvh_root_node_index),
                );
                aabbs.extend(
                    mesh.instance_mesh
                        .iter()
                        .map(|m| mesh.meshes[(*m) as usize].aabb),
                );
                material_ids.extend(mesh.instance_materials.clone());
                materials.extend(mesh.materials.clone());
            },
            AssetEvent::Removed { handle } => {
                todo!()
            },
            _ => {}
        }
    }
    world
        .instance_bvh_root_nodes
        .push(&mut staging_buffer, &bvh_root_nodes);
    world
        .instance_transforms
        .push(&mut staging_buffer, &transforms);
    world
        .instance_materials
        .push(&mut staging_buffer, &material_ids);
    world.materials.push(&mut staging_buffer, &materials);
    world.instance_aabbs.push(&mut staging_buffer, &aabbs);

    world.vertices.push(&mut staging_buffer, &vertices);
    world.indecies.push(&mut staging_buffer, &indices);
    world.meshlets.push(&mut staging_buffer, &meshlets);
    world.cull_data.push(&mut staging_buffer, &cull_data);
    world.bvh_nodes.push(&mut staging_buffer, &bvh);
}

pub fn WorldPlugin(app: &mut App) {
    app.add_systems(RenderStartup, init_world)
        .add_systems(ExtractSchedule, extract_meshlet_instances);
}