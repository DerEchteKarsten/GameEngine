#![feature(f16)]
#![feature(random)]

use std::{
    any::type_name,
    mem::offset_of,
    ops::Deref,
    time::{Duration, Instant},
};

use ash::vk::{self, Format};
#[cfg(feature = "bevy_window")]
use bevy::a11y::AccessibilityPlugin;
use bevy::{a11y::AccessibilityPlugin, app::{AppLabel, PanicHandlerPlugin}, diagnostic::{DiagnosticsPlugin, FrameCountPlugin}, ecs::system::NonSendMarker, input::InputPlugin, log::LogPlugin, prelude::*, time::TimePlugin, window::{PrimaryWindow, WindowResized, WindowResolution}, winit::{WinitPlugin, WinitWindows}};
use bevy::prelude::*;
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec4};
use lava::{
    command_buffer::RasterVertexDispatch,
    state::Ctx,
    vkobjects::{
        buffer::{Buffer, BufferUsageFlags, GpuBuffer},
        image::{Image, ImageSize},
    },
};

mod bindings;

use crate::{
    assets::MeshAssets, bindings::{
        BvhCull, BvhCullBindings, DispatchIndirectCommand, DispatchParams, DrawIndirectCommand,
        InstanceCull, InstanceCullBindings, InstancedOffset, Post, PostBindings, Raster,
        RasterBindings, RasterUi, RasterUiBindings,
    }, components::camera::{Camera, CameraPlugin}, ui::{UiContext, UiPlugin, UiResources}, world::{
        RenderWorld, StagingBuffer, add_instance, init_world, load_assets, transform_child_changed,
        transform_parent_changed,
    }
};

pub mod assets;
pub mod components;
pub mod ui;
pub mod world;

pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(2000.0, 2000.0 * 9.0 / 16.0);

pub fn init(
    _non_send_marker: NonSendMarker,
) {
    bevy::winit::WINIT_WINDOWS.with_borrow(|window| {
        let window = window.windows.iter().last().unwrap().1.deref();
        #[cfg(debug_assertions)]
        let validation = true;

        #[cfg(not(debug_assertions))]
        let validation = false;

        lava::init(&window, validation, false).unwrap();
    });
}

pub fn on_resize(mut event_reader: MessageReader<WindowResized>) {
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

#[derive(Resource)]
struct UiState {
    delta_time_histogram: [f32; 300],
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            delta_time_histogram: [0.0; 300],
        }
    }
}

fn ui(
    mut ui: NonSendMut<UiContext>,
    mut state: ResMut<UiState>,
    world: Res<RenderWorld>,
    time: Res<Time>,
) {
    state.delta_time_histogram.rotate_left(1);
    state.delta_time_histogram[299] = time.delta_secs() * 1000.0;
    let average = state.delta_time_histogram.iter().cloned().reduce(|acc, e| acc + e).unwrap_or(0.0) / state.delta_time_histogram.len() as f32;
    if let Some(ui) = ui.ui() {
        if let Some(window) = ui.window("Frame Stats")
            .size([100.0, 300.0], imgui::Condition::FirstUseEver)
            .begin() {
                ui.plot_histogram(format!("Frame Time: {:?}", average), &state.delta_time_histogram)
                    .scale_max(16.0)
                    .scale_min(0.0)
                    .build();
                ui.text(format!("Verticies: {}, capacity: {}Kb", world.vertices.len(), world.vertices.buffer.size / 1000));
                ui.text(format!("Indicies: {}, capacity: {}Kb", world.indecies.len(), world.indecies.buffer.size / 1000));
                ui.text(format!("Meshlets: {}, capacity: {}Kb", world.meshlets.len(), world.meshlets.buffer.size / 1000));
                ui.text(format!("Materials: {}, capacity: {}Kb", world.materials.len(), world.materials.buffer.size / 1000));
                ui.text(format!("Bvh Nodes: {}, capacity: {}Kb", world.bvh_nodes.len(), world.bvh_nodes.buffer.size / 1000));
                ui.text(format!("Cull Data: {}, capacity: {}Kb", world.cull_data.len(), world.cull_data.buffer.size / 1000));
                ui.text(format!("Instance AABBs: {}, capacity: {}Kb", world.instance_aabbs.len(), world.instance_aabbs.buffer.size / 1000));
                ui.text(format!("Instance Bvh Root nodes: {}, capacity: {}Kb", world.instance_bvh_root_nodes.len(), world.instance_bvh_root_nodes.buffer.size / 1000));
                ui.text(format!("Instance Materials: {}, capacity: {}Kb", world.instance_materials.len(), world.instance_materials.buffer.size / 1000));
                ui.text(format!("Instance Transforms: {}, capacity: {}Kb", world.instance_transforms.len(), world.instance_transforms.buffer.size / 1000));
                ui.text(format!("Bvh Depth: {}", world.max_bvh_depth));
            window.end();
        }
    }
}

fn render(
    query: Query<&Camera>,
    world: Res<RenderWorld>,
    mut resources: Local<Option<RenderResources>>,
    mut staging_buffer: ResMut<StagingBuffer>,
    ui_resources: Res<UiResources>,
    time: Res<Time>,
    ui_state: Res<UiState>,
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

    Ctx::record_frame(&mut |cmd, swapchain_image| {
        // cmd.update_buffer_element(
        //     &resources.dispatch_params,
        //     0,
        //     &DispatchParams {
        //         node_head: 0,
        //         node_tail: 0,
        //         done: 0,
        //         meshlet_count: 0,
        //         indirect_draw: DrawIndirectCommand {
        //             vertex_count: 128 * 3,
        //             instance_count: 0,
        //             first_instance: 0,
        //             first_vertex: 0,
        //         },
        //         indirect_dispatch: DispatchIndirectCommand { x: 0, y: 1, z: 1 },
        //     },
        // );

        // cmd.fill_buffer(&resources.bvh_node_stack, 0, 0);
        // cmd.fill_buffer(&resources.cluster_buffer, 0, 0);
        if world.instance_bvh_root_nodes.len() > 0 {
            // cmd.compute::<InstanceCull>()
            //     .bind(InstanceCullBindings {
            //         num_instances: world.instance_bvh_root_nodes.len() as u64,
            //         aabbs: &world.instance_aabbs,
            //         instance_bvh_root_nodes: &world.instance_bvh_root_nodes,
            //         bvh_node_stack: &resources.bvh_node_stack,
            //         dp: &resources.dispatch_params,
            //         instance_transforms: &world.instance_transforms,
            //     })
            //     .dispatch(
            //         world.instance_bvh_root_nodes.len().div_ceil(64) as u32,
            //         1,
            //         1,
            //     );

            // cmd.compute::<BvhCull>()
            //     .bind(BvhCullBindings {
            //         bvh_node_stack: &resources.bvh_node_stack,
            //         bvh_nodes: &world.bvh_nodes,
            //         clusters: &resources.cluster_buffer,
            //         cull_data: &world.cull_data,
            //         dp: &resources.dispatch_params,
            //         instance_transforms: &world.instance_transforms,
            //     })
            //     .dispatch(4, 1, 1);

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
                .backface_culling(true)
                .draw_fullscreen(RasterVertexDispatch::Draw {
                    vertex_count: 128 * 3,
                    instance_count: world.meshlets.len() as u32,
                });



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
                window_size: Vec4::new(
                    Ctx::window_width() as f32,
                    Ctx::window_height() as f32,
                    0.0,
                    0.0,
                ),
            })
            .dispatch_fullscreen();

        if let Some(atlas) = &ui_resources.font_atlas {
            cmd.raster::<RasterUi>() 
                .bind(RasterUiBindings {
                    verticies: ui_resources.verticies.as_ref(),
                    font_atlas: atlas,
                })
                .color_attachment(&swapchain_image, None)
                .backface_culling(false)
                .wire_frame(false)
                .index_buffer(&ui_resources.indicies)
                .draw_fullscreen(RasterVertexDispatch::indexed(ui_resources.indicies.len() as u32 / 3, 1, 0));
        }

        cmd.present(swapchain_image);
        Ok(())
    })
    .unwrap();
}

#[derive(AppLabel, Hash, Debug, PartialEq, Eq, Clone)]
struct RenderApp;

#[allow(non_snake_case)]
pub fn CorePlugin(app: &mut App) {
    app
        .add_systems(Startup, (init, init_world.after(init)))
        .add_plugins((
            AssetPlugin {
                mode: AssetMode::Processed,
                file_path: format!("./assets"),
                processed_file_path: format!("./imported_assets"),
                ..Default::default()
            },
            WindowPlugin {
                primary_window: Some(Window {
                    resolution: WindowResolution::new(
                        INITIAL_WINDOW_SIZE.x as u32,
                        INITIAL_WINDOW_SIZE.y as u32,
                    ),
                    present_mode: bevy::window::PresentMode::AutoNoVsync,
                    title: "RayTracer".to_owned(),
                    resizable: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            PanicHandlerPlugin,
            LogPlugin::default(),
            TaskPoolPlugin::default(),
            FrameCountPlugin,
            TimePlugin,
            DiagnosticsPlugin,
            InputPlugin,
            AccessibilityPlugin,
            WinitPlugin::default(),
            CameraPlugin,
            MeshAssets, 
            UiPlugin,
        ))
        .add_systems(PreUpdate, (on_resize, Ctx::start_frame))
        .add_systems(
            Update,
            (
                ui,
                add_instance,
                load_assets.after(add_instance),
                transform_child_changed,
                transform_parent_changed,
            ),
        )
        .init_resource::<UiState>()
        .add_systems(PostUpdate, render);
    #[cfg(feature = "trace")]
    {
        app.add_systems(Last, tracy_client::frame_mark);
    }
}
