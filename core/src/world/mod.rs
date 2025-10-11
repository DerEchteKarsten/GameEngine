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
    assets::{material::Material, Cluster, Mesh, Meshlet, MeshletBoundingSpheres, SavedMat4, Vertex},
    components::transform::Transform,
};

pub const STAGING_BUFFER_SIZE: u64 = 16777216;


#[derive(Clone, Copy, bincode::Encode, bincode::Decode, Debug)]
#[repr(C)]
pub struct DrawTask {
    pub instance_id: u32,
    pub material_id: u32,
    pub block_id: u32,
    pub geometry_id: u32,
}

#[derive(Component, Clone)]
pub struct Instance {
    pub model: Handle<Mesh>,
}

#[derive(Component, Clone)]
pub struct UploadedInstance {
    cluster_transforms_offset: usize,
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
    for (transform, UploadedInstance { cluster_transforms_offset }, children) in query {
        let mut transforms = Vec::with_capacity(children.len());
        for &c in children {
            let child_transform = q_children.get(c).unwrap();
            transforms.push(transform.as_matrix() * child_transform.as_matrix());
        }
        staging_buffer.0.copy_data_to_buffer(&transforms).unwrap();
        world.instance_transforms.copy_from(
            &staging_buffer.0,
            (*cluster_transforms_offset * size_of::<Mat4>()) as u64,
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
    for (parent, transform, UploadedInstance { cluster_transforms_offset }) in query {
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
    let mut clusters = Vec::new();
    for (entity, mesh, transform) in loading {
        if let Some(mesh) = meshes.get_mut(&mesh) {
            if !mesh.uploaded {
                world.vertices.push(&staging_buffer.0, &mesh.vertices);
                world.indecies.push(&staging_buffer.0, &mesh.indicies);
                world.materials.push(&staging_buffer.0, &mesh.materials);

                let num_meshlets = world.meshlets.size as usize / size_of::<Meshlet>();
                world.meshlets.push(&staging_buffer.0, &mesh.meshlets);
                mesh.clusters.iter_mut().for_each(|cluster| {
                    cluster.meshlet += num_meshlets as u32;
                });
                mesh.uploaded = true;
                log::debug!("Uploaded");
                // log::debug!("{:#?}", mesh);
            }

            let cluster_offset = world.num_instances;
            transforms.extend(mesh.cluster_transforms.iter().map(|t| {
                SavedMat4(t.0 * transform)
            }));
            world.num_instances += mesh.cluster_transforms.len();

            let children = mesh
                .cluster_transforms
                .iter()
                .enumerate()
                .map(|(i, mat)| {
                    cmd.spawn((
                        UploadedInstance {
                            cluster_transforms_offset: cluster_offset + i,
                        },
                        Transform::from_matrix(mat.0),
                    ))
                    .id()
                })
                .collect::<Vec<_>>();
            cmd.entity(entity)
                .add_children(&children)
                .insert(UploadedInstance { cluster_transforms_offset: cluster_offset });

            clusters.extend(mesh.clusters.iter().map(|c| Cluster {
                transform: c.transform + cluster_offset as u32,
                meshlet: c.meshlet,
            }));

            world.num_clusters += mesh.clusters.len();
        } else {
            world.upload_queue.push((entity, mesh, transform));
        }
    }
    if !clusters.is_empty() {
        world.clusters.push(&staging_buffer.0, &clusters);
    }
    if !transforms.is_empty() {
        world.instance_transforms.push(&staging_buffer.0, &transforms);
    }
}

#[derive(Resource)]
pub struct StagingBuffer(pub Buffer);

#[derive(Resource)]
pub struct RenderWorld {
    pub vertices: DynamicBuffer<Vertex>,
    pub indecies: DynamicBuffer<u8>,
    pub meshlets: DynamicBuffer<Meshlet>,
    pub bounding_spheres: DynamicBuffer<MeshletBoundingSpheres>,
    pub materials: DynamicBuffer<Material>,
    pub instance_transforms: DynamicBuffer<SavedMat4>,
    pub clusters: DynamicBuffer<Cluster>,

    upload_queue: Vec<(Entity, Handle<Mesh>, Mat4)>,

    acceleration_structure_scratch_memory: Option<DynamicBuffer<u8>>,
    acceleration_structure_memory: Option<DynamicBuffer<u8>>,
    tlas: Option<AccelerationStructure>,
    pub num_clusters: usize,
    pub num_instances: usize,
}

pub(super) fn init_world(mut cmd: Commands) {
    let mut acceleration_structure_scratch_memory = if Ctx::features().raytracing {
        Some(
            DynamicBuffer::new(
                vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                    | vk::BufferUsageFlags::STORAGE_BUFFER,
                MemoryLocation::GpuOnly,
                None,
            )
            .unwrap(),
        )
    } else {
        None
    };

    let mut acceleration_structure_memory = if Ctx::features().raytracing {
        Some(
            DynamicBuffer::new(
                vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                    | vk::BufferUsageFlags::STORAGE_BUFFER,
                MemoryLocation::GpuOnly,
                None,
            )
            .unwrap(),
        )
    } else {
        None
    };

    let tlas = if Ctx::features().raytracing {
        Some(
            Ctx::queue()
                .execute_command_wait(|cmd| {
                    AccelerationStructure::new(
                        vk::AccelerationStructureTypeKHR::TOP_LEVEL,
                        &[],
                        &[],
                        &[],
                        acceleration_structure_memory.as_mut().unwrap(),
                        0,
                        acceleration_structure_scratch_memory.as_mut().unwrap(),
                        &cmd,
                    )
                    .unwrap()
                })
                .unwrap(),
        )
    } else {
        None
    };

    let render_world = RenderWorld {
        instance_transforms: DynamicBuffer::new_storage().unwrap(),
        bounding_spheres: DynamicBuffer::new_storage().unwrap(),
        clusters: DynamicBuffer::new_storage().unwrap(),
        indecies: DynamicBuffer::new_storage().unwrap(),
        materials: DynamicBuffer::new_storage().unwrap(),
        meshlets: DynamicBuffer::new_storage().unwrap(),
        vertices: DynamicBuffer::new_storage().unwrap(),
        
        upload_queue: Vec::new(),
        acceleration_structure_scratch_memory,
        acceleration_structure_memory,
        num_clusters: 0,
        tlas,
        num_instances: 0,
    };
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
