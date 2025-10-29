#![feature(f16)]
#![feature(random)]

use std::{
    collections::{HashMap, HashSet}, f32::consts::PI, fs, io::{BufReader, BufWriter, Read, Seek, Write}, ops::Deref, path::PathBuf, random::random
};

use ash::vk::{self, Format, VideoChromaSubsamplingFlagsKHR};
use bevy_a11y::AccessibilityPlugin;
use bevy_app::{
    App, PostUpdate, PreStartup, PreUpdate, Startup, TaskPoolOptions, TaskPoolPlugin, Update,
};
use bevy_asset::AssetPlugin;
use bevy_ecs::{
    event::EventReader,
    resource::{self, Resource},
    schedule::IntoScheduleConfigs,
    system::{Commands, Local, Query, Res, ResMut},
    world::World,
};
use bevy_input::InputPlugin;
use bevy_log::LogPlugin;
use bevy_tasks::{AsyncComputeTaskPool, TaskPoolBuilder};
use bevy_time::TimePlugin;
use bevy_window::{
    ExitCondition, Window, WindowEvent, WindowPlugin, WindowResized, WindowResolution,
    WindowScaleFactorChanged,
};
use bevy_winit::{WinitPlugin, WinitWindows};
use bytemuck::{Pod, Zeroable};
use fastnbt::{DeOpts, Value, from_bytes};
use glam::{IVec3, Mat4, Quat, Vec2, Vec3, Vec3Swizzles, Vec4};
use gpu_allocator::MemoryLocation;
use image::{DynamicImage, ImageBuffer, Rgb};
use lava::{
    c, pipelines::{RasterDispatch, Vertex}, state::Ctx, vkobjects::{
        buffer::{Buffer, BufferUsageFlags, GpuBuffer},
        image::{Image, ImageSize},
    }
};
use noise::{MultiFractal, NoiseFn, Perlin};
use smallvec::SmallVec;

use crate::{
    assets::{MeshAssets, mesh::BvhNode},
    components::camera::{Camera, CameraPlugin},
    world::{
        RenderWorld, STAGING_BUFFER_SIZE, StagingBuffer, add_instance, init_world, load_assets,
        transform_child_changed, transform_parent_changed,
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

pub fn on_resize(mut event_reader: EventReader<WindowResized>) {
    for e in event_reader.read() {
        log::info!("test, {}, {}", e.width, e.height);
        Ctx::resize_swapchain(e.width as u32, e.height as u32);
    }
}

#[derive(Clone, Copy)]
struct Cluster {
    instance: u32,
    meshlet: u32,
}

struct RenderResources {
    depth_attachment: Image,
    color_attachment: Image,
    cluster_buffer: Buffer<Cluster>,
    indirect_draw: Buffer<vk::DrawIndirectCommand>,
    bvh_node_stack: Buffer<i32>,
    dispatch_indirect: Buffer<vk::DispatchIndirectCommand>,
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
        cluster_buffer: Buffer::new(BufferUsageFlags::STORAGE, 100000).unwrap(),
        indirect_draw: Buffer::<_, GpuBuffer>::from_data(
            BufferUsageFlags::INDIRECT_COMMAND | BufferUsageFlags::STORAGE,
            &mut staging_buffer.0,
            &[vk::DrawIndirectCommand {
                vertex_count: 128 * 3,
                instance_count: 0,
                first_instance: 0,
                first_vertex: 0,
            }],
        )
        .unwrap(),
        bvh_node_stack: Buffer::new(BufferUsageFlags::STORAGE, 10000).unwrap(),
        dispatch_indirect: Buffer::<_, GpuBuffer>::from_data(BufferUsageFlags::STORAGE | BufferUsageFlags::INDIRECT_COMMAND, &mut staging_buffer.0, &[
            vk::DispatchIndirectCommand {
                x: 0,
                y: 1,
                z: 1,
            }
        ]).unwrap(),
    });

    Ctx::next_frame(&mut |cmd, swapchain_image| {
        cmd.fill_buffer(&resources.indirect_draw, 4, 0);
        cmd.fill_buffer(&resources.dispatch_indirect, 4, 1);

        cmd.compute()
            .shader_path("instance_cull")
            .read(&world.instance_aabbs)
            .read(&world.instance_bvh_root_nodes)
            .read(&world.instance_transforms)
            .readwrite(&resources.dispatch_indirect)
            .readwrite(&resources.bvh_node_stack)
            .dispatch(world.instance_bvh_root_nodes.len().div_ceil(64) as u32, 1, 1);

        cmd.compute()
            .shader_path("bvh_cull")
            .read(&world.bvh_nodes)
            .read(&world.instance_transforms)
            .read(&world.cull_data)
            .write(&resources.cluster_buffer)
            .readwrite(&resources.indirect_draw)
            .dispatch_indirect(&resources.dispatch_indirect);

        cmd.raster()
            .vertex("raster", "vertex")
            .fragment("raster", "fragment")
            .constants(c!(camera.view_matrix(), camera.projection_matrix()))
            .read(&world.vertices)
            .read(&world.indecies)
            .read(&world.meshlets)
            .read(&resources.cluster_buffer)
            .read(&world.instance_transforms)
            .color_attachment(&resources.color_attachment, Some([0.2, 0.2, 0.4, 1.0]))
            .depth_attachment(&resources.depth_attachment)
            .backface_culling(false)
            .draw_fullscreen(RasterDispatch::indirect(&resources.indirect_draw, 0, 1));


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
