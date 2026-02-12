#![feature(f16)]
#![feature(random)]
#![feature(arc_is_unique)]
use std::{
    any::type_name,
    mem::offset_of,
    ops::{Deref, DerefMut},
    time::{Duration, Instant},
};

#[cfg(feature = "bevy_window")]
use bevy::a11y::AccessibilityPlugin;
use bevy::prelude::*;
use bevy::{
    a11y::AccessibilityPlugin,
    app::{AppLabel, PanicHandlerPlugin},
    diagnostic::{DiagnosticsPlugin, FrameCountPlugin},
    ecs::{
        schedule::{ScheduleBuildSettings, ScheduleLabel},
        system::NonSendMarker,
    },
    input::InputPlugin,
    log::LogPlugin,
    prelude::*,
    time::TimePlugin,
    window::{PrimaryWindow, WindowResized, WindowResolution},
    winit::{WinitPlugin, WinitWindows},
};
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec4};
use lava::{command_buffer::RasterVertexDispatch, state::Ctx};

mod bindings;

use crate::{
    assets::MeshAssets,
    components::camera::{Camera, CameraPlugin},
    render::{PipelinedRenderingPlugin, RenderPlugin},
    ui::{UiContext, UiPlugin, UiResources},
};

pub mod assets;
pub mod components;
pub mod render;
pub mod ui;

pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(2000.0, 2000.0 * 9.0 / 16.0);

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

// fn ui(
//     mut ui: NonSendMut<UiContext>,
//     mut state: ResMut<UiState>,
//     world: Res<RenderWorld>,
//     time: Res<Time>,
// ) {
//     state.delta_time_histogram.rotate_left(1);
//     state.delta_time_histogram[299] = time.delta_secs() * 1000.0;
//     let average = state.delta_time_histogram.iter().cloned().reduce(|acc, e| acc + e).unwrap_or(0.0) / state.delta_time_histogram.len() as f32;
//     if let Some(ui) = ui.ui() {
//         if let Some(window) = ui.window("Frame Stats")
//             .size([100.0, 300.0], imgui::Condition::FirstUseEver)
//             .begin() {
//                 ui.plot_histogram(format!("Frame Time: {:?}", average), &state.delta_time_histogram)
//                     .scale_max(16.0)
//                     .scale_min(0.0)
//                     .build();
//                 ui.text(format!("Verticies: {}, capacity: {}Kb", world.vertices.len(), world.vertices.buffer.size / 1000));
//                 ui.text(format!("Indicies: {}, capacity: {}Kb", world.indecies.len(), world.indecies.buffer.size / 1000));
//                 ui.text(format!("Meshlets: {}, capacity: {}Kb", world.meshlets.len(), world.meshlets.buffer.size / 1000));
//                 ui.text(format!("Materials: {}, capacity: {}Kb", world.materials.len(), world.materials.buffer.size / 1000));
//                 ui.text(format!("Bvh Nodes: {}, capacity: {}Kb", world.bvh_nodes.len(), world.bvh_nodes.buffer.size / 1000));
//                 ui.text(format!("Cull Data: {}, capacity: {}Kb", world.cull_data.len(), world.cull_data.buffer.size / 1000));
//                 ui.text(format!("Instance AABBs: {}, capacity: {}Kb", world.instance_aabbs.len(), world.instance_aabbs.buffer.size / 1000));
//                 ui.text(format!("Instance Bvh Root nodes: {}, capacity: {}Kb", world.instance_bvh_root_nodes.len(), world.instance_bvh_root_nodes.buffer.size / 1000));
//                 ui.text(format!("Instance Materials: {}, capacity: {}Kb", world.instance_materials.len(), world.instance_materials.buffer.size / 1000));
//                 ui.text(format!("Instance Transforms: {}, capacity: {}Kb", world.instance_transforms.len(), world.instance_transforms.buffer.size / 1000));
//                 ui.text(format!("Bvh Depth: {}", world.max_bvh_depth));
//                 if let Some(_cb) = ui.begin_combo("Queue", "") {
//                     world.upload_queue.iter().for_each(|e| ui.text(format!("{:#?}", e)));
//                 }
//             window.end();
//         }
//     }
// }

#[allow(non_snake_case)]
pub fn CorePlugin(app: &mut App) {
    app.add_plugins((
        AssetPlugin {
            mode: AssetMode::Processed,
            file_path: format!("/home/karsten/code/GameEngine/game/assets"),
            processed_file_path: format!("/home/karsten/code/GameEngine/game/imported_assets"),
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
        TimePlugin,
        DiagnosticsPlugin,
        InputPlugin,
        AccessibilityPlugin,
        WinitPlugin::default(),
        CameraPlugin,
        MeshAssets,
        UiPlugin,
        TransformPlugin::default(),
    ))
    .add_plugins((RenderPlugin::default(), PipelinedRenderingPlugin::default()))
    // .add_systems(
    //     Update,
    //         ui
    // )
    .init_resource::<UiState>();
    #[cfg(feature = "trace")]
    {
        app.add_systems(Last, tracy_client::frame_mark);
    }
}
