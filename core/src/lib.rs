#![feature(integer_casts)]

use bevy::{
    a11y::AccessibilityPlugin,
    app::{App, PanicHandlerPlugin, TaskPoolPlugin},
    asset::{AssetMode, AssetPlugin},
    diagnostic::DiagnosticsPlugin,
    input::InputPlugin,
    time::TimePlugin,
    transform::TransformPlugin,
    window::{PrimaryWindow, Window, WindowPlugin, WindowResolution},
    winit::WinitPlugin,
};
use bevy::{
    app::Startup,
    ecs::{
        entity::Entity,
        query::With,
        system::{Commands, Single},
    },
    window::{CursorIcon, SystemCursorIcon},
};
use glam::Vec2;
use tracing::Level;

mod bindings;
pub mod physics;

use crate::{
    assets::MeshAssets,
    editor::{EditorPlugin, console::ConsolePlugin},
    physics::PhysicsPlugin,
    render::{PipelinedRenderingPlugin, RenderPlugin},
    scene::ScenePlugin,
    ui::UiPlugin,
};

pub mod assets;
pub mod editor;
pub mod render;
pub mod scene;
pub mod ui;

pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(2000.0, 2000.0 * 9.0 / 16.0);

#[allow(non_snake_case)]
pub fn CorePlugin(app: &mut App) {
    app.add_plugins((
        ConsolePlugin {
            also_log_to_stderr: false,
            level: Level::DEBUG,
            ..Default::default()
        },
        AssetPlugin {
            mode: AssetMode::Processed,
            file_path: "/home/karsten/code/GameEngine/game/assets".to_string(),
            processed_file_path: "/home/karsten/code/GameEngine/game/imported_assets".to_string(),
            ..Default::default()
        },
        WinitPlugin::default(),
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
        TaskPoolPlugin::default(),
        TimePlugin,
        DiagnosticsPlugin,
        InputPlugin,
        AccessibilityPlugin,
        MeshAssets,
        TransformPlugin,
    ))
    .add_plugins((
        RenderPlugin,
        PipelinedRenderingPlugin,
        UiPlugin,
        PhysicsPlugin,
        EditorPlugin::default(),
        ScenePlugin,
    ))
    .add_systems(
        Startup,
        |mut cmd: Commands, window: Single<Entity, With<PrimaryWindow>>| {
            cmd.entity(*window)
                .insert(CursorIcon::System(SystemCursorIcon::Default));
        },
    );
}
