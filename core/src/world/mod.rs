use std::{
    fmt::Debug,
    marker::PhantomData,
    os::raw::c_void,
    ptr::NonNull,
    time::{Duration, Instant},
};

use anyhow::Result;
use ash::vk::{self, BufferCopy, BufferUsageFlags, Packed24_8};
use bevy_app::prelude::*;
use bevy_asset::{AssetEvent, AssetServer, Assets, Handle};
use bevy_ecs::{
    component::{ComponentId, HookContext},
    entity::EntityHashMap,
    prelude::*,
    world::DeferredWorld,
};
use bevy_log::info_span;
use glam::{Mat4, Quat, Vec3, Vec4};
use gpu_allocator::MemoryLocation;
use lava::vkobjects::buffer::{Buffer, DynamicBuffer};
use lava::{state::Ctx, vkobjects::acceleration_structure::AccelerationStructure};

use crate::{
    assets::{Mesh, material::Material, mesh::{Aabb, BvhNode, CullData, Meshlet, Vertex}},
    components::transform::Transform,
};

pub const STAGING_BUFFER_SIZE: u64 = 16777216;


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
            transform.map(|t| t.as_matrix()).unwrap_or(Mat4::IDENTITY).clone(),
        ));
    }
}

pub fn transform_parent_changed(
    mut world: ResMut<RenderWorld>,
    staging_buffer: Res<StagingBuffer>,
    query: Query<(&Transform, &UploadedInstance, &Children), (Changed<Transform>, With<Instance>)>,
    q_children: Query<&Transform>,
) {
    for (transform, UploadedInstance { instance_offset }, children) in query {
        let mut transforms = Vec::with_capacity(children.len());
        for &c in children {
            let child_transform = q_children.get(c).unwrap();
            transforms.push(transform.as_matrix() * child_transform.as_matrix());
        }
        staging_buffer.0.copy_data_to_buffer(&transforms).unwrap();
        world.instance_transforms.copy_from(
            &staging_buffer.0,
            (*instance_offset * size_of::<Mat4>()) as u64,
            (transforms.len() * size_of::<Mat4>()) as u64,
        );
    }
}

pub fn transform_child_changed(
    mut world: ResMut<RenderWorld>,
    staging_buffer: Res<StagingBuffer>,
    query: Query<
        (&ChildOf, &Transform, &UploadedInstance),
        (Changed<Transform>, Without<Instance>),
    >,
    p_query: Query<&Transform, With<Instance>>,
) {
    for (parent, transform, UploadedInstance { instance_offset: cluster_transforms_offset }) in query {
        let parent_transform = p_query.get(parent.parent()).unwrap();

        staging_buffer
            .0
            .copy_data_to_buffer(&[parent_transform.as_matrix() * transform.as_matrix()])
            .unwrap();
        world.instance_transforms.copy_from(
            &staging_buffer.0,
            (*cluster_transforms_offset * size_of::<Mat4>()) as u64,
            (1 * size_of::<Mat4>()) as u64,
        );
    }
}

pub fn load_assets(
    mut cmd: Commands,
    mut world: ResMut<RenderWorld>,
    staging_buffer: Res<StagingBuffer>,
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
                    vertices.extend(m.vertices.clone());
                    indices.extend(m.indices.clone());
                    meshlets.extend(m.meshlets.clone());
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
                .insert(UploadedInstance {
                    instance_offset 
                });
            
            transforms.extend(mesh.instance_transforms.clone());
            bvh_root_nodes.extend(mesh.instance_mesh.iter().map(|m| mesh.meshes[(*m) as usize].bvh_root_node_index));
            aabbs.extend(mesh.instance_mesh.iter().map(|m| mesh.meshes[(*m) as usize].aabb));
            material_ids.extend(mesh.instance_materials.clone());
            materials.extend(mesh.materials.clone());
        } else {
            world.upload_queue.push((entity, mesh, transform));
        }
    }
    world.instance_bvh_root_nodes.push(&staging_buffer.0, &bvh_root_nodes);
    world.instance_transforms.push(&staging_buffer.0, &transforms);
    world.instance_materials.push(&staging_buffer.0, &material_ids);
    world.materials.push(&staging_buffer.0, &materials);
    world.instance_aabbs.push(&staging_buffer.0, &aabbs);

    world.vertices.push(&staging_buffer.0, &vertices);
    world.indecies.push(&staging_buffer.0, &indices);
    world.meshlets.push(&staging_buffer.0, &meshlets);
    world.cull_data.push(&staging_buffer.0, &cull_data);
    world.bvh_nodes.push(&staging_buffer.0, &bvh);
}

#[derive(Resource)]
pub struct StagingBuffer(pub Buffer);

#[derive(Resource, Default)]
pub struct RenderWorld {
    pub vertices: DynamicBuffer<Vertex>,
    pub indecies: DynamicBuffer<u8>,
    pub meshlets: DynamicBuffer<Meshlet>,
    pub cull_data: DynamicBuffer<CullData>,
    pub bvh_nodes: DynamicBuffer<BvhNode>,
    pub materials: DynamicBuffer<Material>,
    
    pub instance_transforms: DynamicBuffer<Mat4>,
    pub instance_materials: DynamicBuffer<u32>,
    pub instance_bvh_root_nodes: DynamicBuffer<u32>,
    pub instance_aabbs: DynamicBuffer<Aabb>,

    upload_queue: Vec<(Entity, Handle<Mesh>, Mat4)>,

    acceleration_structure_scratch_memory: Option<DynamicBuffer<u8>>,
    acceleration_structure_memory: Option<DynamicBuffer<u8>>,
    tlas: Option<AccelerationStructure>,
    pub max_bvh_depth: u32,
}

pub(super) fn init_world(mut cmd: Commands) {
    let render_world = RenderWorld::default();
    cmd.insert_resource(StagingBuffer(
        Buffer::new(
            BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::CpuToGpu,
            STAGING_BUFFER_SIZE,
        )
        .unwrap(),
    ));

    cmd.insert_resource(render_world);
}
