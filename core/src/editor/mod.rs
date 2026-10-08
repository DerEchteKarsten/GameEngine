//! Editor plugin wiring up editor tools, panels and the frame-time histogram.
use bevy::{
    app::{Plugin, PreUpdate, Update},
    asset::Handle,
    ecs::{
        entity::Entity,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Local, Res},
    },
    math::Rect,
    time::Time,
};
use bevy::{ecs::reflect::ReflectResource, reflect::Reflect};
use glam::Vec2;
use lava::bindless::BindlessHandle;

use crate::{
    INITIAL_WINDOW_SIZE,
    assets::{material::GpuMaterial, mesh::GpuMesh, texture::GpuTexture},
    editor::{
        asset_browser::{asset_browser, drop_in_viewport},
        camera::{CameraSettings, update_camera},
        console::ConsolePlugin,
        gizzmos::{Gizzmos, extract_gizzmos, init_gizzmos, write_gizzmos},
        picking::{hierarchy_ui, picking},
        selected::{ReflectEditorView, selected_ui},
        viewport::{ViewPort, viewport_ui},
    },
    physics::bvh::debug_draw_scene_bvh,
    render::{ExtractSchedule, RenderApp, RenderStartup, RenderSystems, render::RenderDebugUi},
    scene::file::Scene,
    ui::builder::UiBuilder,
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

#[derive(Resource, Reflect)]
#[reflect(Resource)]
struct UiState {
    delta_time_histogram: [f32; 300],
    cursor: usize,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            delta_time_histogram: [0.0; 300],
            cursor: 0,
        }
    }
}

fn frame_histogram(mut ui: UiBuilder, mut state: Local<UiState>, time: Res<Time>) {
    let cursor = state.cursor;
    state.delta_time_histogram[cursor] = time.delta_secs() * 1000.0;
    state.cursor = (state.cursor + 1) % state.delta_time_histogram.len();
    let average =
        state.delta_time_histogram.iter().sum::<f32>() / state.delta_time_histogram.len() as f32;
    ui.build("Frame Histogram", |ui| {
        ui.text(format!(
            "Average: {:.3}ms / {:.3}fps",
            average,
            (1.0 / average) * 1000.0
        ));
        let len = state.delta_time_histogram.len();
        let (before, after) = state.delta_time_histogram.split_at(state.cursor);
        ui.histogram(
            ui.clip_rect.width() - 20.0,
            50.0,
            32.0,
            0.0,
            after.iter().chain(before.iter()),
            len,
        )
    });
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
        .add_plugins(RenderDebugUi)
        .insert_resource(self.camera_settings)
        .init_resource::<UiState>()
        .insert_resource(ViewPort {
            focused: false,
            hovered: false,
            image: BindlessHandle::default(),
            image_size: Vec2::ZERO,
            rect: Rect::from_corners(Vec2::ZERO, INITIAL_WINDOW_SIZE),
        })
        .add_systems(
            Update,
            (
                debug_draw_scene_bvh,
                update_camera,
                selected_ui,
                hierarchy_ui,
                frame_histogram,
                asset_browser,
                viewport_ui,
                drop_in_viewport,
                picking
                    .after(update_camera)
                    .after(selected_ui)
                    .after(hierarchy_ui)
                    .after(frame_histogram),
            ),
        );
        app.get_sub_app_mut(RenderApp)
            .unwrap()
            .add_systems(ExtractSchedule, extract_gizzmos)
            .add_systems(RenderSystems::PreRender, write_gizzmos)
            .add_systems(RenderStartup, init_gizzmos);

        // `MaterialSettings` registers these too, but `ScenePlugin` may be added after us.
        app.register_type::<Handle<GpuTexture>>()
            .register_type::<Option<Handle<GpuTexture>>>();
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
            Handle<GpuMaterial>,
            Handle<Scene>,
            Handle<GpuTexture>,
            Option<Handle<GpuTexture>>
        );
    }
}
