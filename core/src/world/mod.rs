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
use bevy_asset::{AssetEvent, Assets, Handle};
use bevy_ecs::{
    component::{ComponentId, HookContext},
    entity::EntityHashMap,
    prelude::*,
    world::DeferredWorld,
};
use bevy_log::info_span;
use glam::{Mat4, Quat, Vec3, Vec4};
#[cfg(not(feature = "no_raytracing"))]
use gpu_allocator::MemoryLocation;
use lava::vkobjects::buffer::{Buffer, DynamicBuffer};
#[cfg(not(feature = "no_raytracing"))]
use lava::{
    bindless::BindlessDescriptorHeap, state::Ctx,
    vkobjects::acceleration_structure::AccelerationStructure,
};
use rg::RenderGraph;
#[cfg(not(feature = "no_raytracing"))]
use rg::resources::ResourceHandle;

use crate::{
    Rg,
    assets::{Material, Mesh},
    components::transform::Transform,
};

const INSTANCE_BUFFER_CAPACITY: u64 = 1048576; //TODO
const VERTEX_BUFFER_CAPACITY: u64 = 1048576; //TODO
const ACCELERATION_STRUCTURE_SCRATCH_MEMORY: u64 = 1048576; //TODO
pub const STAGING_BUFFER_SIZE: u64 = 1048576; //TODO
const MESHLET_BUFFER_CAPACITY: u64 = 1048576;
const INDEX_BUFFER_CAPACITY: u64 = 1048576;
const INSTANCE_INDEX_BUFFER_CAPACITY: u64 = 1048576;

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
    instance_offset: usize,
}

pub fn add_instance(
    query: Query<(Entity, &Instance, Option<&Transform>), Added<Instance>>,
    mut world: ResMut<RenderWorld>,
) {
    for (entity, instance, transform) in query {
        world.loading.push((
            instance.clone(),
            transform.unwrap_or(&Transform::IDENTITY).clone(),
            entity,
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
        world.instances.copy_from(
            &staging_buffer.0,
            (*instance_offset * size_of::<DrawTask>()) as u64,
            (children.len() * size_of::<DrawTask>()) as u64,
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
    for (parent, transform, UploadedInstance { instance_offset }) in query {
        let parent_transform = p_query.get(parent.parent()).unwrap();

        staging_buffer
            .0
            .copy_data_to_buffer(&[parent_transform.as_matrix() * transform.as_matrix()])
            .unwrap();
        world.instances.copy_from(
            &staging_buffer.0,
            (*instance_offset * size_of::<DrawTask>()) as u64,
            (1 * size_of::<DrawTask>()) as u64,
        );
    }
}

pub fn load_assets(
    mut cmd: Commands,
    mut world: ResMut<RenderWorld>,
    staging_buffer: Res<StagingBuffer>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    if world.loading.is_empty() {
        return;
    }
    let loading = world.loading.clone();
    world.loading.clear();
    let mut instances = Vec::new();
    let mut draw_tasks = Vec::new();
    for l in loading {
        if let Some(mesh) = meshes.get_mut(&l.0.model) {
            if !mesh.uploaded {
                let num_blocks = world.dgf_blocks.size as usize / 128;
                world
                    .dgf_blocks
                    .push(&staging_buffer.0, &mesh.mesh.dgf_blocks);
                let num_materials = world.materials.size as usize / size_of::<Material>();
                world
                    .materials
                    .push(&staging_buffer.0, &mesh.mesh.materials);
                mesh.mesh.draw_tasks.iter_mut().for_each(|i| {
                    i.block_id += num_blocks as u32;
                    i.geometry_id += world.num_geometries;
                    i.material_id += num_materials as u32;
                });
                mesh.uploaded = true;
                world.num_geometries += mesh.mesh.num_geometries;
                log::debug!("Uploaded");
            }

            let instance_offset = world.num_instances;
            instances.extend(mesh.mesh.instances.iter().map(|i| {
                let mat = glam::Mat4::from_cols_array(i);
                (mat * l.1.as_matrix()).to_cols_array()
            }));
            world.num_instances += mesh.mesh.instances.len();

            let children = mesh
                .mesh
                .instances
                .iter()
                .enumerate()
                .map(|(i, mat)| {
                    cmd.spawn((
                        UploadedInstance {
                            instance_offset: instance_offset + i,
                        },
                        Transform::from_matrix(Mat4::from_cols_array(mat)),
                    ))
                    .id()
                })
                .collect::<Vec<_>>();
            cmd.entity(l.2)
                .add_children(&children)
                .insert(UploadedInstance { instance_offset });

            draw_tasks.extend(mesh.mesh.draw_tasks.iter().map(|i| DrawTask {
                block_id: i.block_id,
                geometry_id: i.geometry_id,
                instance_id: i.instance_id + instance_offset as u32,
                material_id: i.material_id,
            }));

            world.num_instance_indices += mesh.mesh.draw_tasks.len();
        } else {
            world.loading.push(l.clone());
        }
    }
    if !draw_tasks.is_empty() {
        world.draw_tasks.push(&staging_buffer.0, &draw_tasks);
    }
    if !instances.is_empty() {
        world.instances.push(&staging_buffer.0, &instances);
    }
}

#[derive(Resource)]
pub struct StagingBuffer(pub Buffer);

#[derive(Resource)]
pub struct RenderWorld {
    loading: Vec<(Instance, Transform, Entity)>,
    dgf_blocks: DynamicBuffer,
    materials: DynamicBuffer,
    instances: DynamicBuffer,
    draw_tasks: DynamicBuffer,
    acceleration_structure_scratch_memory: Option<DynamicBuffer>,
    acceleration_structure_memory: Option<DynamicBuffer>,
    tlas: Option<AccelerationStructure>,
    pub num_geometries: u32,
    pub num_instance_indices: usize,
    pub num_instances: usize,
}

#[derive(Resource, Debug)]
pub struct WorldResources {
    pub tlas: Option<ResourceHandle>,
    pub dgf_buffer: ResourceHandle,
    pub material_buffer: ResourceHandle,
    pub instance_buffer: ResourceHandle,
    pub draw_tasks: ResourceHandle,
}

pub(super) fn init_world(mut cmd: Commands, mut rg: ResMut<Rg>) {
    let mut acceleration_structure_scratch_memory = if Ctx::features().raytracing {
        Some(
            DynamicBuffer::new(
                vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                    | vk::BufferUsageFlags::STORAGE_BUFFER,
                MemoryLocation::GpuOnly,
                ACCELERATION_STRUCTURE_SCRATCH_MEMORY,
                None,
            )
            .unwrap(),
        )
    } else {
        None
    };

    let acceleration_structure_memory = if Ctx::features().raytracing {
        Some(
            DynamicBuffer::new(
                vk::BufferUsageFlags::ACCELERATION_STRUCTURE_STORAGE_KHR
                    | vk::BufferUsageFlags::STORAGE_BUFFER,
                MemoryLocation::GpuOnly,
                ACCELERATION_STRUCTURE_SCRATCH_MEMORY,
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
                        acceleration_structure_memory.as_ref().unwrap(),
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

    let tlas_descriptor = if let Some(tlas) = &tlas {
        Some(BindlessDescriptorHeap::get().allocate_acceleration_structure_handle(&tlas))
    } else {
        None
    };

    let render_world = RenderWorld {
        loading: Vec::new(),
        instances: DynamicBuffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::GpuOnly,
            INSTANCE_BUFFER_CAPACITY,
            None,
        )
        .unwrap(),
        draw_tasks: DynamicBuffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::GpuOnly,
            INSTANCE_INDEX_BUFFER_CAPACITY,
            None,
        )
        .unwrap(),
        dgf_blocks: DynamicBuffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::GpuOnly,
            MESHLET_BUFFER_CAPACITY,
            None,
        )
        .unwrap(),
        materials: DynamicBuffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER,
            MemoryLocation::GpuOnly,
            1028,
            None,
        )
        .unwrap(),

        acceleration_structure_scratch_memory,
        acceleration_structure_memory,
        num_instance_indices: 0,
        tlas,
        num_instances: 0,
        num_geometries: 0,
    };
    cmd.insert_resource(StagingBuffer(
        Buffer::new(
            BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::CpuToGpu,
            STAGING_BUFFER_SIZE,
        )
        .unwrap(),
    ));

    cmd.insert_resource(WorldResources {
        dgf_buffer: rg.0.import(render_world.dgf_blocks.bindless_handle),
        material_buffer: rg.0.import(render_world.materials.bindless_handle),
        instance_buffer: rg.0.import(render_world.instances.bindless_handle),
        draw_tasks: rg.0.import(render_world.draw_tasks.bindless_handle),
        tlas: if let Some(desc) = tlas_descriptor {
            Some(rg.0.import(desc))
        } else {
            None
        },
    });
    cmd.insert_resource(render_world);
}
