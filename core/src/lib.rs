#![feature(f16)]
#![feature(random)]

use std::{any::type_name, mem::offset_of, ops::Deref, time::{Duration, Instant}};

use ash::vk::{self, Format};
use bevy_a11y::AccessibilityPlugin;
use bevy_app::{App, PostUpdate, PreStartup, PreUpdate, Startup, TaskPoolPlugin, Update};
use bevy_asset::AssetPlugin;
use bevy_ecs::{
    event::EventReader, schedule::IntoScheduleConfigs, system::{Local, Query, Res, ResMut}, world::World
};
use bevy_input::InputPlugin;
use bevy_log::LogPlugin;
use bevy_tasks::{AsyncComputeTaskPool, TaskPoolBuilder};
use bevy_time::TimePlugin;
use bevy_window::{ExitCondition, Window, WindowPlugin, WindowResized, WindowResolution};
use bevy_winit::{WinitPlugin, WinitWindows};
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec4};
use lava::{
    command_buffer::RasterVertexDispatch, state::Ctx, vkobjects::{
        buffer::{Buffer, BufferUsageFlags, GpuBuffer},
        image::{Image, ImageSize},
    }
};

mod bindings;

use crate::{
    assets::MeshAssets, bindings::{BvhCull, BvhCullBindings, DispatchIndirectCommand, DispatchParams, DrawIndirectCommand, InstanceCull, InstanceCullBindings, InstancedOffset, Post, PostBindings, Raster, RasterBindings}, components::camera::{Camera, CameraPlugin}, world::{
        RenderWorld, StagingBuffer, add_instance, init_world, load_assets, transform_child_changed,
        transform_parent_changed,
    }
};

pub mod assets;
pub mod components;
pub mod world;

pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(2000.0, 2000.0 * 9.0 / 16.0);

pub fn init(world: &mut World) {
    let windows = world.get_non_send_resource::<WinitWindows>().unwrap();
    let window = windows.windows.values().into_iter().last().unwrap().deref();

    lava::init(Some(&window), true).unwrap();
}

pub fn on_resize(mut event_reader: EventReader<WindowResized>) {
    for e in event_reader.read() {
        log::info!("test, {}, {}", e.width, e.height);
        Ctx::resize_swapchain(e.width as u32, e.height as u32);
    }
}


struct RenderResources {
    depth_attachment: Image,
    color_attachment: Image,
    cluster_buffer: Buffer<InstancedOffset>,
    dispatch_params: Buffer<DispatchParams>,
    bvh_node_stack: Buffer<InstancedOffset>,
}


fn render(
    query: Query<&Camera>,
    world: Res<RenderWorld>,
    mut resources: Local<Option<RenderResources>>,
    mut staging_buffer: ResMut<StagingBuffer>,
) {
    let camera = query.single().unwrap();

    let resources = resources.get_or_insert_with(|| RenderResources {
        depth_attachment: Image::new_2d(
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            Format::D32_SFLOAT,
            ImageSize::XY(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32),
        )
        .unwrap(),
        color_attachment: Image::new_2d(
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            Format::R32G32B32A32_SFLOAT,
            ImageSize::XY(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32),
        )
        .unwrap(),
        cluster_buffer: Buffer::new(BufferUsageFlags::STORAGE, 1 << 14).unwrap(),
        dispatch_params: Buffer::<_, GpuBuffer>::from_data(
            BufferUsageFlags::INDIRECT_COMMAND | BufferUsageFlags::STORAGE,
            &mut staging_buffer.0,
            &[DispatchParams {
                node_head: 0,
                node_tail: 0,
                done: 0,
                meshlet_count: 0,
                indirect_draw: DrawIndirectCommand {
                    vertex_count: 128 * 3,
                    instance_count: 0,
                    first_instance: 0,
                    first_vertex: 0,
                },
                indirect_dispatch: DispatchIndirectCommand { x: 0, y: 1, z: 1 },
            }],
        )
        .unwrap(),
        bvh_node_stack: Buffer::new(BufferUsageFlags::STORAGE, 10000).unwrap(),
    });

    Ctx::next_frame(&mut |cmd, swapchain_image| {
        cmd.update_buffer_element(
            &resources.dispatch_params,
            0,
            &DispatchParams {
                node_head: 0,
                node_tail: 0,
                done: 0,
                meshlet_count: 0,
                indirect_draw: DrawIndirectCommand {
                    vertex_count: 128 * 3,
                    instance_count: 0,
                    first_instance: 0,
                    first_vertex: 0,
                },
                indirect_dispatch: DispatchIndirectCommand { x: 0, y: 1, z: 1 },
            },
        );

        cmd.fill_buffer(&resources.bvh_node_stack, 0, 0);
        cmd.fill_buffer(&resources.cluster_buffer, 0, 0);
        if world.instance_bvh_root_nodes.len() > 0 {
            cmd.compute::<InstanceCull>()
                .bind(InstanceCullBindings {
                    num_instances: world.instance_bvh_root_nodes.len() as u64,
                    aabbs: &world.instance_aabbs,
                    instance_bvh_root_nodes: &world.instance_bvh_root_nodes,
                    bvh_node_stack: &resources.bvh_node_stack,
                    dp: &resources.dispatch_params,
                    instance_transforms: &world.instance_transforms, 
                })
                .dispatch(
                    world.instance_bvh_root_nodes.len().div_ceil(64) as u32,
                    1,
                    1,
                );

            cmd.compute::<BvhCull>()
                .bind(BvhCullBindings {
                    bvh_node_stack: &resources.bvh_node_stack,
                    bvh_nodes: &world.bvh_nodes,
                    clusters: &resources.cluster_buffer,
                    cull_data: &world.cull_data,
                    dp: &resources.dispatch_params,
                    instance_transforms: &world.instance_transforms,
                })
                .dispatch(4, 1, 1);
                
            // let params =
            //     cmd.read_buffer(&resources.dispatch_params, &(**staging_buffer).cast(), 1, 0);


            cmd.raster::<Raster>()
                .bind(RasterBindings {
                    indicies: &world.indecies,
                    instance_offsets: &resources.cluster_buffer,
                    instance_transforms: &world.instance_transforms,
                    meshlets: &world.meshlets,
                    verticies: &world.vertices,
                    proj: camera.projection_matrix(),
                    view: camera.view_matrix(),
                })
                .color_attachment(&resources.color_attachment, Some([0.2, 0.2, 0.4, 1.0]))
                .depth_attachment(&resources.depth_attachment)
                .backface_culling(false)
                .draw_fullscreen(RasterVertexDispatch::indirect(&resources.dispatch_params, offset_of!(DispatchParams, indirect_draw) as u32, 1));

            // cmd.raster()
            //     .mesh("meshshader", "mesh")
            //     .fragment("meshshader", "fragment")
            //     .constants(c!(
            //         camera.projection_matrix(),
            //         camera.view_matrix(),
            //         Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::from_euler(glam::EulerRot::XYZ, PI/2.0, 0.0, 0.0), Vec3::ZERO),
            //     ))
            //     .read(&world.vertices)
            //     .read(&world.indecies)
            //     .read(&world.meshlets)
            //     .color_attachment(&resources.color_attachment, Some([0.2, 0.2, 0.4, 1.0]))
            //     .depth_attachment(&resources.depth_attachment)
            //     .backface_culling(false)
            //     .draw_fullscreen(RasterDispatch::launch_mesh(world.meshlets.len() as u32, 1, 1));
        }

        cmd.compute::<Post>()
            .bind(PostBindings {
                color: &resources.color_attachment,
                depth: &resources.depth_attachment,
                out: &swapchain_image,
                inverse_proj: camera.projection_matrix().inverse(),
                inverse_view: camera.view_matrix().inverse(),
                window_size: Vec4::new(Ctx::window_width().unwrap() as f32, Ctx::window_height().unwrap() as f32, 0.0, 0.0),
            }).dispatch_fullscreen();

        cmd.present(swapchain_image);
        Ok(())
    })
    .unwrap();
}

#[allow(non_snake_case)]
pub fn CorePlugin(app: &mut App) {
    AsyncComputeTaskPool::get_or_init(|| TaskPoolBuilder::default().num_threads(4).build());
    app.add_systems(PreStartup, init)
        .add_systems(Startup, init_world)
        .add_plugins((
            LogPlugin {
                filter: "".to_owned(),
                level: bevy_log::Level::DEBUG,
                ..Default::default()
            },
            AccessibilityPlugin,
            InputPlugin,
            WindowPlugin {
                close_when_requested: true,
                exit_condition: ExitCondition::OnPrimaryClosed,
                primary_window: Some(Window {
                    resolution: WindowResolution::new(
                        INITIAL_WINDOW_SIZE.x as f32,
                        INITIAL_WINDOW_SIZE.y as f32,
                    ),
                    present_mode: bevy_window::PresentMode::AutoNoVsync,
                    title: "RayTracer".to_owned(),
                    resizable: true,

                    ..Default::default()
                }),
            },
            AssetPlugin {
                mode: bevy_asset::AssetMode::Processed,
                file_path: "/home/karsten/Documents/code/GameEngine/game/assets".to_string(),
                processed_file_path: "/home/karsten/Documents/code/GameEngine/game/imported_assets"
                    .to_string(),
                ..Default::default()
            },
            WinitPlugin::<bevy_winit::WakeUp>::default(),
            TimePlugin,
            CameraPlugin,
            TaskPoolPlugin::default(),
            MeshAssets,
        ))
        .add_systems(PreUpdate, on_resize)
        .add_systems(
            Update,
            (
                add_instance,
                load_assets.after(add_instance),
                transform_child_changed,
                transform_parent_changed,
            ),
        )
        .add_systems(PostUpdate, render);
}
