#![feature(f16)]

use std::ops::Deref;

use ash::vk::Format;
use bevy_a11y::AccessibilityPlugin;
use bevy_app::{App, PostUpdate, PreStartup, Startup, TaskPoolPlugin, Update};
use bevy_asset::AssetPlugin;
use bevy_ecs::{resource::Resource, system::{Query, Res, ResMut}, world::World};
use bevy_input::InputPlugin;
use bevy_log::LogPlugin;
use bevy_time::TimePlugin;
use bevy_window::{ExitCondition, Window, WindowPlugin, WindowResolution};
use bevy_winit::{WinitPlugin, WinitWindows};
use glam::{Vec2, Vec3};
use lava::state::Ctx;
use rg::{build::{DispatchSize, ImageSize}, executions::{RasterPass, WorkSize2D}, RenderGraph, IMPORTED};

use crate::{assets::MeshAssets, components::camera::{Camera, CameraPlugin}, world::{add_instance, init_world, load_assets, transform_child_changed, transform_parent_changed, RenderWorld, WorldResources}};

pub mod world;
pub mod assets;
pub mod components;

pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(1280.0, 720.0);


#[derive(Resource)]
struct Rg(RenderGraph);

pub fn init(world: &mut World) {
    let windows = world.get_non_send_resource::<WinitWindows>().unwrap();
    let window = windows.windows.values().into_iter().last().unwrap().deref();

    lava::init(Some(&window), true).unwrap();   

    world.insert_resource(GConst::default());
    world.insert_resource(Rg(RenderGraph::new()));
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Resource)]
struct GConst {
    pub proj: glam::Mat4,
    pub view: glam::Mat4,
    pub proj_inverse: glam::Mat4,
    pub view_inverse: glam::Mat4,
    pub window_size: glam::Vec2,
    pub frame: u32,
    pub blendfactor: f32,
    pub bounces: u32,
    pub samples: u32,
    pub proberng: u32,
    pub cell_size: f32,
    pub mouse: [u32; 2],
    pub camera_position: Vec3,
    pub camera_direction: Vec3,
    pub far: f32,
    pub near: f32,
    pub fov: f32,
    pub pad: u32,
}

fn commands(
    mut rg: ResMut<Rg>,
    world: Res<WorldResources>,
    render_world: Res<RenderWorld>,
    mut gconst: ResMut<GConst>,
    query: Query<&Camera>,
) {

    let camera = query.single().unwrap();
    gconst.proj = camera.projection_matrix();
    gconst.proj_inverse = gconst.proj.inverse();
    gconst.view = camera.view_matrix();
    gconst.view_inverse = gconst.view.inverse();
    gconst.window_size = Vec2::new(Ctx::window_width().unwrap() as f32, Ctx::window_height().unwrap() as f32);
    gconst.frame = Ctx::current_frame() as u32;
    gconst.camera_position = camera.position;
    gconst.camera_direction = camera.direction;
    gconst.far = camera.z_far;
    gconst.near = camera.z_near;
    gconst.fov = camera.fov;
    
    let depth = rg.0.image(ImageSize::FullScreen, Format::D32_SFLOAT, "depth");
    // let color = rg.0.image(ImageSize::FullScreen, Format::R32G32B32A32_SFLOAT, "color");
    
    rg.0.draw_frame(|rg, swapchain_image_index| {
        let swapchain = rg.get_swapchain(swapchain_image_index);
        // let test2 = RasterPass::new(&mut rg, "test2")
        //     .fragment("fragment", "bindless_test2")
        //     .mesh("mesh", "bindless_test2")
        //     .task("amp", "bindless_test2")
        //     .constants(gconst.as_ref())
        //     .read(IMPORTED, world.dgf_buffer)
        //     .read(IMPORTED, world.material_buffer)
        //     .read(IMPORTED, world.instance_buffer)
        //     .read(IMPORTED, world.draw_tasks)
        //     .depth_attachment(IMPORTED, depth)
        //     .color_attachment(IMPORTED, color, Some([0.1, 0.15, 0.3, 1.0]))
        //     .render_area(WorkSize2D::FullScreen)
        //     .draw(DispatchSize::X(
        //         (render_world.num_instance_indices as u32).div_ceil(64),
        //     ));
        let test2 = RasterPass::new(rg, "test2")
            .fragment("frag", "test")
            .vertex("vert", "test")
            .constants(gconst.as_ref())
            .read(IMPORTED, world.dgf_buffer)
            .read(IMPORTED, world.material_buffer)
            .read(IMPORTED, world.instance_buffer)
            .read(IMPORTED, world.draw_tasks)
            .depth_attachment(IMPORTED, depth)
            .color_attachment(IMPORTED, swapchain, Some([0.1, f32::sin(Ctx::current_frame() as f32 / 100.0), 0.3, 1.0]))
            .render_area(WorkSize2D::FullScreen)
            .backface_culling(false)
            .draw(DispatchSize::VertexCountInstanceCount(
                3, 1,
            ));
        // ComputePass::new(&mut rg, "test")
        //     .shader("bindless_test")
        //     .read(test2, depth)
        //     .read(test2, color)
        //     .write(IMPORTED, swapchain)
        //     .dispatch(DispatchSize::FullScreen);
    });

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
                    resolution: WindowResolution::new(INITIAL_WINDOW_SIZE.x as f32, INITIAL_WINDOW_SIZE.y as f32),
                    present_mode: bevy_window::PresentMode::AutoNoVsync,
                    title: "RayTracer".to_owned(),
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
        .add_systems(
            Update,
            (
                load_assets,
                add_instance,
                transform_child_changed,
                transform_parent_changed,
            ),
        )
        .add_systems(PostUpdate, commands);
}
