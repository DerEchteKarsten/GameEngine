#![feature(f16)]
#![feature(random)]

use std::ops::Deref;

use ash::vk::{self, Format};
use bevy_a11y::AccessibilityPlugin;
use bevy_app::{App, PostUpdate, PreStartup, PreUpdate, Startup, TaskPoolPlugin, Update};
use bevy_asset::AssetPlugin;
use bevy_ecs::{
    message::MessageReader,
    schedule::IntoScheduleConfigs,
    system::{Local, Query, Res, ResMut},
    world::World,
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
    c,
    command_buffer::{DispatchIndirectCommand, DrawIndirectCommand},
    state::Ctx,
    vkobjects::{
        buffer::{Buffer, BufferUsageFlags, GpuBuffer},
        image::{Image, ImageSize},
    },
};

use crate::{
    assets::MeshAssets,
    components::camera::{Camera, CameraPlugin},
    world::{
        RenderWorld, StagingBuffer, add_instance, init_world, load_assets, transform_child_changed,
        transform_parent_changed,
    },
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

pub fn on_resize(mut event_reader: MessageReader<WindowResized>) {
    for e in event_reader.read() {
        log::info!("test, {}, {}", e.width, e.height);
        Ctx::resize_swapchain(e.width as u32, e.height as u32);
    }
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct InstancedOffset {
    instance: u32,
    offset: i32,
}

#[derive(Clone, Copy, Debug, Pod, Zeroable)]
#[repr(C)]
struct DispatchParams {
    node_head: u32,
    node_tail: u32,
    done: u32,
    meshlet_count: u32,
    indirect_draw: DrawIndirectCommand,
    indirect_dispatch: DispatchIndirectCommand,
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
            cmd.compute()
                .shader_path("instance_cull")
                .constants(c!(world.instance_bvh_root_nodes.len() as u64))
                .read(&world.instance_aabbs)
                .read(&world.instance_bvh_root_nodes)
                .read(&world.instance_transforms)
                .readwrite(&resources.dispatch_params)
                .readwrite(&resources.bvh_node_stack)
                .dispatch(
                    world.instance_bvh_root_nodes.len().div_ceil(64) as u32,
                    1,
                    1,
                );

            cmd.compute()
                .shader_path("bvh_cull")
                .read(&world.bvh_nodes)
                .read(&world.instance_transforms)
                .read(&world.cull_data)
                .readwrite(&resources.bvh_node_stack)
                .write(&resources.cluster_buffer)
                .readwrite(&resources.dispatch_params)
                .dispatch(4, 1, 1);

            let params =
                cmd.read_buffer(&resources.dispatch_params, &(**staging_buffer).cast(), 1, 0);

            log::debug!("{:#?}", params);

            // cmd.raster()
            //     .vertex("raster", "vertex")
            //     .fragment("raster", "fragment")
            //     .constants(c!(camera.view_matrix(), camera.projection_matrix()))
            //     .read(&world.vertices)
            //     .read(&world.indecies)
            //     .read(&world.meshlets)
            //     .read(&resources.cluster_buffer)
            //     .read(&world.instance_transforms)
            //     .color_attachment(&resources.color_attachment, Some([0.2, 0.2, 0.4, 1.0]))
            //     .depth_attachment(&resources.depth_attachment)
            //     .backface_culling(false)
            //     .draw_fullscreen(RasterDispatch::indirect(&resources.dispatch_params, offset_of!(DispatchParams, indirect_dispatch) as u32, 1));

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

        cmd.compute()
            .shader_path("post")
            .constants(c!(
                camera.projection_matrix().inverse(),
                camera.view_matrix().inverse(),
                Vec4::new(
                    Ctx::window_width().unwrap() as f32,
                    Ctx::window_height().unwrap() as f32,
                    0.0,
                    0.0,
                ),
            ))
            .read(&resources.depth_attachment)
            .read(&resources.color_attachment)
            .write(&swapchain_image)
            .dispatch_fullscreen();
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
                        INITIAL_WINDOW_SIZE.x as u32,
                        INITIAL_WINDOW_SIZE.y as u32,
                    ),
                    present_mode: bevy_window::PresentMode::AutoNoVsync,
                    title: "RayTracer".to_owned(),
                    resizable: true,

                    ..Default::default()
                }),
                primary_cursor_options: None,
            },
            AssetPlugin {
                mode: bevy_asset::AssetMode::Processed,
                file_path: "/home/karsten/code/GameEngine/game/assets".to_string(),
                processed_file_path: "/home/karsten/code/GameEngine/game/imported_assets"
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
