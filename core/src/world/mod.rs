use std::{
    fmt::Debug, marker::PhantomData, ops::{Deref, DerefMut}, os::raw::c_void, ptr::NonNull, time::{Duration, Instant}
};

use anyhow::Result;
use ash::vk::{self, BufferCopy, Packed24_8};
use bevy_app::prelude::*;
use bevy_asset::{AssetEvent, AssetServer, Assets, Handle};
use bevy_ecs::{
    component::{ComponentId},
    entity::EntityHashMap,
    prelude::*,
    world::DeferredWorld,
};
use bevy_log::info_span;
use glam::{Mat4, Quat, Vec3, Vec4};
use gpu_allocator::MemoryLocation;
use lava::{
    pipelines::Vertex,
    vkobjects::buffer::{Buffer, BufferUsageFlags, CpuBuffer, StorageBuffer},
};
use lava::{state::Ctx, vkobjects::acceleration_structure::AccelerationStructure};

use crate::{
    assets::{
        Mesh,
        material::Material,
        mesh::{Aabb, BvhNode, CullData, Meshlet},
    },
    components::transform::Transform,
};

pub const STAGING_BUFFER_SIZE: usize = 16777216;

#[derive(Component, Clone)]
pub struct Instance {
    pub model: Handle<Mesh>,
}

#[derive(Component, Clone)]
pub struct UploadedInstance {
    instance_offset: usize,
}

pub fn add_instance(
    query: Query<(Entity, &Instance, Option<&Transform>), Added<Instance>>,
    mut world: ResMut<RenderWorld>,
) {
    for (entity, instance, transform) in query {
        world.upload_queue.push((
            entity,
            instance.model.clone(),
            transform
                .map(|t| t.as_matrix())
                .unwrap_or(Mat4::IDENTITY)
                .clone(),
        ));
    }
}

pub fn transform_parent_changed(
    mut world: ResMut<RenderWorld>,
    mut staging_buffer: ResMut<StagingBuffer>,
    query: Query<(&Transform, &UploadedInstance, &Children), (Changed<Transform>, With<Instance>)>,
    q_children: Query<&Transform>,
) {
    for (transform, UploadedInstance { instance_offset }, children) in query {
        let mut transforms = Vec::with_capacity(children.len());
        for &c in children {
            let child_transform = q_children.get(c).unwrap();
            transforms.push(transform.as_matrix() * child_transform.as_matrix());
        }
        staging_buffer.0.copy_from_slice(&transforms).unwrap();
        world.instance_transforms.copy_from(
            staging_buffer.0.cast_mut(),
            (*instance_offset * size_of::<Mat4>()) as u64,
            (transforms.len() * size_of::<Mat4>()) as u64,
        );
    }
}

pub fn transform_child_changed(
    mut world: ResMut<RenderWorld>,
    mut staging_buffer: ResMut<StagingBuffer>,
    query: Query<
        (&ChildOf, &Transform, &UploadedInstance),
        (Changed<Transform>, Without<Instance>),
    >,
    p_query: Query<&Transform, With<Instance>>,
) {
    for (
        parent,
        transform,
        UploadedInstance {
            instance_offset: cluster_transforms_offset,
        },
    ) in query
    {
        let parent_transform = p_query.get(parent.parent()).unwrap();

        staging_buffer
            .0
            .copy_from_slice(&[parent_transform.as_matrix() * transform.as_matrix()])
            .unwrap();
        world.instance_transforms.copy_from(
            staging_buffer.0.cast_mut(),
            (*cluster_transforms_offset * size_of::<Mat4>()) as u64,
            (1 * size_of::<Mat4>()) as u64,
        );
    }
}

pub fn load_assets(
    mut cmd: Commands,
    mut world: ResMut<RenderWorld>,
    mut staging_buffer: ResMut<StagingBuffer>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if world.upload_queue.is_empty() {
        return;
    }
    let loading = world.upload_queue.clone();
    world.upload_queue.clear();
    let mut transforms = Vec::new();
    let mut bvh_root_nodes = Vec::new();
    let mut materials = Vec::new();
    let mut material_ids = Vec::new();
    let mut aabbs = Vec::new();

    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut meshlets = Vec::new();
    let mut cull_data = Vec::new();
    let mut bvh = Vec::new();

    for (entity, mesh, transform) in loading {
        if let Some(mesh) = meshes.get_mut(&mesh) {
            if !mesh.uploaded {
                for m in &mut mesh.meshes {
                    let mmeshlets = m.meshlets
                        .iter()
                        .map(|m| Meshlet {
                            triangle_count: m.triangle_count,
                            vertex_count: m.vertex_count,
                            triangle_index: m.triangle_index + world.indecies.len() as u32 + indices.len() as u32,
                            vertex_index: m.vertex_index + world.vertices.len() as u32 + vertices.len() as u32,
                        })
                        .collect::<Vec<_>>();
                    vertices.extend(m.vertices.clone());
                    indices.extend(m.indices.clone());
                    meshlets.extend(mmeshlets);
                    cull_data.extend(m.cull_data.clone());

                    let bvh_root = world.bvh_nodes.len() + bvh.len();
                    m.bvh_root_node_index = bvh_root as u32;
                    bvh.extend(m.bvh.clone());

                    world.max_bvh_depth = world.max_bvh_depth.max(m.bvh_depth);
                }
                mesh.uploaded = true;
                log::debug!("Uploaded");
                // log::debug!("{:#?}", mesh);
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
        } else {
            world.upload_queue.push((entity, mesh, transform));
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

#[derive(Resource)]
pub struct StagingBuffer(pub Buffer<u8, CpuBuffer>);

impl Deref for StagingBuffer {
    type Target = Buffer<u8, CpuBuffer>;
    fn deref(&self) -> &Buffer<u8, CpuBuffer> {
        &self.0
    } 
}
impl DerefMut for StagingBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
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

    upload_queue: Vec<(Entity, Handle<Mesh>, Mat4)>,

    acceleration_structure_scratch_memory: Option<Buffer<u8>>,
    acceleration_structure_memory: Option<Buffer<u8>>,
    tlas: Option<AccelerationStructure>,
    pub max_bvh_depth: u32,
}

pub(super) fn init_world(mut cmd: Commands) {
    cmd.insert_resource(StagingBuffer(Buffer::new(BufferUsageFlags::empty(), STAGING_BUFFER_SIZE).unwrap()));
    cmd.insert_resource(RenderWorld::default());
}
