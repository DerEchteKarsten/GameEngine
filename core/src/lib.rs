#![feature(f16)]
#![feature(random)]
#![feature(const_default)]
#![feature(lock_value_accessors)]
#![feature(integer_casts)]

use std::{
    any::type_name,
    mem::offset_of,
    ops::{Deref, DerefMut},
    time::{Duration, Instant},
};

#[cfg(feature = "bevy_window")]
use bevy::a11y::AccessibilityPlugin;
use bevy::{
    a11y::AccessibilityPlugin,
    app::{App, AppLabel, PanicHandlerPlugin, TaskPoolPlugin},
    asset::{AssetMode, AssetPlugin},
    diagnostic::{DiagnosticsPlugin, FrameCountPlugin},
    ecs::{
        schedule::{ScheduleBuildSettings, ScheduleLabel},
        system::NonSendMarker,
    },
    input::InputPlugin,
    log::{Level, LogPlugin},
    time::TimePlugin,
    transform::TransformPlugin,
    window::{PrimaryWindow, Window, WindowPlugin, WindowResized, WindowResolution},
    winit::{WinitPlugin, WinitWindows},
};
use bevy::{
    app::Startup,
    ecs::{
        entity::Entity,
        query::With,
        system::{Commands, Single},
    },
    window::{CursorIcon, RawHandleWrapperHolder, SystemCursorIcon, WindowTheme},
};
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec4};
use lava::{command_buffer::RasterVertexDispatch, state::Ctx};

mod bindings;
pub mod physics;

use crate::{
    assets::MeshAssets,
    editor::EditorPlugin,
    physics::PhysicsPlugin,
    render::{PipelinedRenderingPlugin, RenderPlugin, render::RenderDebugUi, world::InstanceFlags},
    scene::{Instance, ScenePlugin, SpawnScene, camera::Camera},
    ui::{UiPlugin, UiResources, builder::UiBuilder},
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
