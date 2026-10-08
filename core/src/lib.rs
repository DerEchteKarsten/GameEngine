//! Engine core: assembles bevy, rendering, assets, scene, UI, editor and physics plugins, and the shared asset setup.
// Needed to call `RasterBuilder`'s draw methods, whose return type is `{ N + 1 }`.
#![allow(incomplete_features)]
#![feature(generic_const_exprs)]
#![feature(integer_casts)]

use bevy::{
    a11y::AccessibilityPlugin,
    app::{App, PanicHandlerPlugin, TaskPoolPlugin},
    asset::{AssetMetaCheck, AssetMode, AssetPlugin},
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
pub mod bindless;
pub mod editor;
pub mod profiler;
pub mod render;
pub mod scene;
pub mod ui;

pub use editor::console::register_tracing;

/// Root of the baked assets: the asset server's source and the asset browser's root.
pub const ASSET_DIR: &str = "/home/karsten/code/GameEngine/game/assets";
/// The source files (glTF) that the `bake` tool turns into the files in `ASSET_DIR`.
pub const UNBAKED_ASSET_DIR: &str = "/home/karsten/code/GameEngine/game/unbaked_assets";
pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(2000.0, 2000.0 * 9.0 / 16.0);

/// The asset server setup shared by the game and the headless tools: the baked files in
/// `ASSET_DIR` are loaded as they are. Nothing has a `.meta` file.
pub fn asset_plugin() -> AssetPlugin {
    AssetPlugin {
        mode: AssetMode::Unprocessed,
        file_path: ASSET_DIR.to_string(),
        meta_check: AssetMetaCheck::Never,
        ..Default::default()
    }
}

#[allow(non_snake_case)]
pub fn CorePlugin(app: &mut App) {
    app.add_plugins((
        ConsolePlugin,
        asset_plugin(),
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
        RenderPlugin::default(),
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
