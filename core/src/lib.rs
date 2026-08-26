#![feature(f16)]
#![feature(random)]
#![feature(const_default)]
#![feature(lock_value_accessors)]
#![feature(integer_casts)]


#[cfg(feature = "bevy_window")]
use bevy::a11y::AccessibilityPlugin;
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

mod bindings;
pub mod physics;

use crate::{
    assets::MeshAssets,
    editor::EditorPlugin,
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
        AssetPlugin {
            mode: AssetMode::Processed,
            file_path: format!("/home/karsten/code/GameEngine/game/assets"),
            processed_file_path: format!("/home/karsten/code/GameEngine/game/imported_assets"),
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
        TransformPlugin::default(),
    ))
    .add_plugins((
        RenderPlugin::default(),
        PipelinedRenderingPlugin::default(),
        UiPlugin,
        PhysicsPlugin,
        EditorPlugin::default(),
        bevy::log::LogPlugin::default(),
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
