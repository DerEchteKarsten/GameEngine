#![feature(f16)]
#![feature(random)]

use std::{
    collections::{HashMap, HashSet}, fs, io::{BufReader, BufWriter, Read, Seek, Write}, ops::Deref, path::PathBuf, random::random
};

use ash::vk::{self, BufferUsageFlags, Format, VideoChromaSubsamplingFlagsKHR};
use bevy_a11y::AccessibilityPlugin;
use bevy_app::{App, PostUpdate, PreStartup, PreUpdate, Startup, TaskPoolPlugin, Update};
use bevy_asset::AssetPlugin;
use bevy_ecs::{
    event::EventReader, resource::Resource, schedule::IntoScheduleConfigs, system::{Commands, Local, Query, Res, ResMut}, world::World
};
use bevy_input::InputPlugin;
use bevy_log::LogPlugin;
use bevy_time::TimePlugin;
use bevy_window::{
    ExitCondition, Window, WindowEvent, WindowPlugin, WindowResized, WindowResolution,
    WindowScaleFactorChanged,
};
use bevy_winit::{WinitPlugin, WinitWindows};
use fastnbt::{DeOpts, Value, from_bytes};
use glam::{IVec3, Mat4, Vec2, Vec3, Vec3Swizzles, Vec4};
use gpu_allocator::MemoryLocation;
use image::{DynamicImage, ImageBuffer, Rgb};
use lava::{
    state::Ctx,
    vkobjects::{
        buffer::{Buffer, DynamicBuffer},
        image::{Image, ImageSize},
    },
};
use noise::{MultiFractal, NoiseFn, Perlin};
use smallvec::SmallVec;

use crate::{
    assets::{MeshAssets},
    components::camera::{Camera, CameraPlugin},
    world::{
        add_instance, init_world, load_assets, transform_child_changed, transform_parent_changed, RenderWorld, StagingBuffer, STAGING_BUFFER_SIZE
    },
};

pub mod assets;
pub mod components;
pub mod world;

pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(2000.0, 2000.0 * 9.0/16.0);

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
}

fn render(query: Query<&Camera>, world: Res<RenderWorld>, mut resources: Local<Option<RenderResources>>) {
    let camera = query.single().unwrap();

    let resources = resources.get_or_insert_with(|| {RenderResources {
        depth_attachment: Image::new_2d(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED, MemoryLocation::GpuOnly, Format::D32_SFLOAT, ImageSize::XY(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32)).unwrap(),
        color_attachment: Image::new_2d(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED, MemoryLocation::GpuOnly, Format::R32G32B32A32_SFLOAT, ImageSize::XY(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32)).unwrap(),
    }});

    Ctx::next_frame(&mut |cmd, swapchain_image| {
        cmd.compute()
            .shader_path("instance_cull")
            .read(world.instance_transforms)
            .read(world.instance_aabbs)
            .read(world.instance_bvh_root_nodes)
            .read(world.bvh_nodes)
            .read(world.cull_data)
            .read(world.)
            .dispatch(1, 1, 1);
        

        cmd.present(swapchain_image);
        Ok(())
    })
    .unwrap();
}

pub fn CorePlugin(app: &mut App) {
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
