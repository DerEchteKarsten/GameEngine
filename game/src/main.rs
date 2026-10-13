//! Sample game binary: boots the engine and spawns an editor camera and the skybox
use core::{CorePlugin, editor::camera::EditorCamera, scene::Skybox, scene::camera::CameraBundle};

use bevy::{
    app::{App, Startup},
    asset::AssetServer,
    ecs::system::{Commands, Res},
    transform::components::Transform,
};
use glam::Vec3;

fn init(mut cmd: Commands, asset_server: Res<AssetServer>) {
    let camera = CameraBundle::new(
        Transform::from_translation(Vec3::new(0.0, 0.0, 0.0)),
        65.0_f32.to_radians(),
        0.01,
        100.0,
    );
    cmd.spawn((camera, EditorCamera));
    cmd.insert_resource(Skybox {
        image: asset_server.load("kloofendal_48d_partly_cloudy_puresky_4k.tex"),
    });
}

fn main() {
    // Before the app exists, so that the spans of every system record.
    core::register_tracing();

    let mut app = App::new();
    app.add_plugins(CorePlugin);
    #[cfg(feature = "profiling")]
    app.add_plugins(profiler_plugin());
    app.add_systems(Startup, init).run();
}

/// `GAME_PROFILE=DIR[:FRAMES]` saves a profile of the first FRAMES (default 300) frames.
#[cfg(feature = "profiling")]
fn profiler_plugin() -> core::profiler::ProfilerPlugin {
    let capture = std::env::var("GAME_PROFILE").ok().map(|value| {
        let split = value.rsplit_once(':');
        match split.and_then(|(dir, frames)| Some((frames.parse().ok()?, dir.into()))) {
            Some(capture) => capture,
            None => (core::profiler::MAX_FRAMES, std::path::PathBuf::from(value)),
        }
    });
    core::profiler::ProfilerPlugin {
        capture,
        ..Default::default()
    }
}
