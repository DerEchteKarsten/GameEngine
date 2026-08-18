use bevy::{
    app::{Plugin, PreUpdate, Update},
    asset::Handle,
    ecs::{
        entity::Entity,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Res, ResMut},
    },
    time::Time,
};
use glam::{IVec2, UVec2};
use lava::buffer::Buffer;
use tracing::Level;

use crate::{
    INITIAL_WINDOW_SIZE,
    assets::mesh::{GpuMesh, Scene},
    editor::{
        asset_browser::{AssetDND, asset_browser},
        camera::{CameraSettings, update_camera},
        console::ConsolePlugin,
        gizzmos::{Gizzmos, extract_gizzmos, init_gizzmos, write_gizzmos},
        picking::{hierarchy_ui, picking},
        selected::{ReflectEditorView, selected_ui},
        viewport::{ViewPort, update_view_port},
    },
    physics::bvh::debug_draw_scene_bvh,
    render::{
        ExtractSchedule, FRAMES_IN_FLIGHT, Render, RenderApp, RenderStartup, RenderSystems,
        extract_param::Extract, render::RenderDebugUi, world::MAX_INSTANCES,
    },
    ui::{UiBuilder, OldUiContext},
};

pub mod asset_browser;
pub mod camera;
pub mod console;
pub mod gizzmos;
pub(crate) mod picking;
pub mod selected;
pub mod viewport;

pub struct EditorPlugin {
    pub camera_settings: CameraSettings,
}

impl Default for EditorPlugin {
    fn default() -> Self {
        Self {
            camera_settings: CameraSettings {
                move_speed: 1.0,
                sensitivity: 1.0,
                keyboard_sensitivity: 3.0,
            },
        }
    }
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

fn frame_histogram(mut ui: ResMut<UiBuilder>, mut state: ResMut<UiState>, time: Res<Time>) {
    state.delta_time_histogram.rotate_left(1);
    state.delta_time_histogram[299] = time.delta_secs() * 1000.0;
    let average = state
        .delta_time_histogram
        .iter()
        .cloned()
        .reduce(|acc, e| acc + e)
        .unwrap_or(0.0)
        / state.delta_time_histogram.len() as f32;
    if let Some(ui) = ui.ui() {
        if let Some(window) = ui
            .window("Entity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdl##frame_stats")
            .size([100.0, 300.0], imgui::Condition::FirstUseEver)
            .begin()
        {
            ui.plot_histogram(format!("{:?}", average), &state.delta_time_histogram)
                .scale_max(32.0)
                .scale_min(0.0)
                .build()
        }
    }
}

macro_rules! register_editor_views {
    ($app:expr, $($t:ty),*) => {
        $($app.register_type_data::<$t, ReflectEditorView>();)*
    };
}

impl Plugin for EditorPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.insert_resource(Gizzmos {
            gizzmos: Vec::new(),
        })
        .add_plugins((
            ConsolePlugin {
                also_log_to_stderr: true,
                level: Level::TRACE,
                ..Default::default()
            },
            RenderDebugUi,
        ))
        .insert_resource(self.camera_settings)
        .init_resource::<UiState>()
        .insert_resource(ViewPort {
            view_size: INITIAL_WINDOW_SIZE.as_uvec2(),
            scissor_size: INITIAL_WINDOW_SIZE.as_uvec2(),
            view_pos: IVec2::ZERO,
            scissor_pos: UVec2::ZERO,
            focused: true,
        })
        .insert_resource(AssetDND(None))
        .add_systems(
            Update,
            (
                debug_draw_scene_bvh,
                update_camera,
                selected_ui,
                hierarchy_ui,
                frame_histogram,
                asset_browser,
                picking
                    .after(update_camera)
                    .after(selected_ui)
                    .after(hierarchy_ui)
                    .after(frame_histogram),
            ),
        )
        .add_systems(PreUpdate, update_view_port);
        app.get_sub_app_mut(RenderApp)
            .unwrap()
            .add_systems(ExtractSchedule, extract_gizzmos)
            .add_systems(Render, write_gizzmos.in_set(RenderSystems::PreRender))
            .add_systems(RenderStartup, init_gizzmos);

        register_editor_views!(
            app,
            f32,
            f64,
            i32,
            u32,
            u64,
            bool,
            String,
            glam::Vec2,
            glam::Vec3,
            glam::Vec4,
            glam::Quat,
            glam::Mat4,
            glam::Affine3A,
            Entity,
            Handle<GpuMesh>,
            Handle<Scene>
        );
    }
}
