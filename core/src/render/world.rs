use std::collections::BTreeMap;
use std::future::AsyncDrop;
use std::ops::{Deref, DerefMut, Range};
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


const STAGING_CHUNK_SIZE: u64 = 16 * 1024 * 1024;

struct Allocator {
    total_size: u64,
    free_ranges: BTreeMap<u64, u64>,
}


async fn dealloc(alloc: Arc<Mutex<Allocator>>, start: u64, end: u64) {
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

async fn allocate(alloc: Arc<Mutex<Allocator>>, size: u64) -> Option<u64> {
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

struct LargeBuffer<T: Copy + Pod> {
    buffer: Buffer<T, GpuBuffer>,
    buffer_task: Option<Task<Option<Buffer<T, GpuBuffer>>>>,
    wirtes: Vec<(u64, Arc<[T]>)>,
    size: u64,
    queue_size: u64,
}

impl<T: Copy + Pod + Send> LargeBuffer<T> {
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
        if let Some(task) = &mut self.buffer_task {
            if let Some(buffer) = check_ready(task) {
                queue.delay_deletion.push((std::mem::replace(&mut self.buffer, buffer).cast_owned(), 0));
            }
        } else {
            if !self.wirtes.is_empty() {
                self.buffer_task = Some(AsyncComputeTaskPool::get().spawn({
                    let writes = std::mem::take(&mut self.wirtes);
                    let staging_buffer = queue.staging_buffer.handle.clone();
                    let ptr = {
                        let lock = queue.staging_buffer.allocation.read().unwrap();
                        lock.0.mapped_ptr().unwrap()
                    };
                    let allocator = queue.allocator.clone();
                    let mut buffer = self.buffer.clone();
                    let queue_size = self.queue_size;
                    async move {
                        if buffer.size < queue_size {
                            let new_buffer = 
                        }

                        for (write_offset, data) in writes {
                            let data_size = (data.len() * std::mem::size_of::<T>()) as u64;
                            let offset = allocate(allocator, data_size).await.unwrap();
                            
                            unsafe { data.as_ptr().copy_to(ptr.byte_add(offset as usize).as_ptr().cast(), data.len()); };
                            
                        }
                        buffer
                    }
                }) );
            }
        }
    } 
}

impl UploadQueue {
    pub fn new() -> Self {
        let mut free_ranges = BTreeMap::new();
        free_ranges.insert(0, total_size);
        
        Self {
            staging_buffer: Buffer::with_alignment(
                BufferUsageFlags::empty(),
                STAGING_CHUNK_SIZE,
                None,
            )
            .unwrap(),
            delay_deletion: Vec::new(),
            allocator: Arc::new(Mutex::new(Allocator {
                total_size,
                free_ranges,
            }))
        }
    }
}

#[derive(Resource, Default)]
pub struct RenderWorld {
    pub vertices: StorageBuffer<Vertex>,
    pub indecies: StorageBuffer<u8>,
    pub meshlets: StorageBuffer<Meshlet>,
    pub cull_data: StorageBuffer<CullData>,
    pub bvh_nodes: StorageBuffer<BvhNode>,
    pub materials: StorageBuffer<Material>,

    pub instance_transforms: StorageBuffer<Mat4>,
    pub instance_materials: StorageBuffer<u32>,
    pub instance_bvh_root_nodes: StorageBuffer<u32>,
    pub instance_aabbs: StorageBuffer<Aabb>,

    _acceleration_structure_scratch_memory: Option<Buffer<u8>>,
    _acceleration_structure_memory: Option<Buffer<u8>>,
    _tlas: Option<AccelerationStructure>,
    pub max_bvh_depth: u32,
}

pub(super) fn init_world(mut cmd: Commands) {
    cmd.insert_resource(RenderWorld::default());
}

fn extract_meshlet_instances(
    mut render_world: ResMut<RenderWorld>,
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
    
    render_world.max_bvh_depth = 0;
    render_world.instance_bvh_root_nodes.clear();
    render_world.instance_aabbs.clear();
    render_world.instance_transforms.clear();

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
        self.instance_aabbs.get_mut().push(aabb);
        self.instance_material_ids.get_mut().push(0);
        self.instance_bvh_root_nodes.get_mut().push(root_bvh_node);

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
        .add_systems(ExtractSchedule, extract_meshlet_instances)
        .add_systems(Render, spawn_upload_thread)        
}