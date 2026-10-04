//! Headless render: loads a scene description, renders one frame without a window and saves it as a PNG
use std::{
    fs,
    io::BufWriter,
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

use bevy::{
    app::{App, TaskPoolPlugin},
    asset::{AssetPlugin, AssetServer, Handle, LoadState},
    ecs::query::With,
    transform::{TransformPlugin, components::Transform},
};
use core::{
    asset_plugin,
    assets::{MeshAssets, mesh::Scene},
    editor::console::ConsolePlugin,
    render::{
        RenderApp, RenderPlugin,
        headless::{HeadlessTarget, read_frame},
        render::RenderSettings,
    },
    scene::{Instance, ScenePlugin, SpawnScene, camera::CameraBundle},
};
use glam::{EulerRot, Quat, UVec2, Vec3};
use serde::Deserialize;

const USAGE: &str = "usage: render SCENE.ron OUTPUT.png

Renders the scene described by SCENE.ron and writes the image to OUTPUT.png.
See game/scenes/sponza.ron for the format.";

/// Frames rendered after everything is loaded, so transforms and instance buffers settle.
const SETTLE_FRAMES: usize = 4;
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Deserialize)]
struct SceneDescription {
    /// Width and height of the image in pixels.
    size: (u32, u32),
    camera: CameraDescription,
    #[serde(default)]
    scenes: Vec<SceneInstance>,
    /// LOD error threshold in pixels, the "LOD Bias" of the editor.
    pixel_error: Option<f32>,
}

#[derive(Deserialize)]
struct CameraDescription {
    position: Vec3,
    look_at: Vec3,
    /// Vertical field of view in degrees.
    #[serde(default = "default_fov")]
    fov: f32,
    #[serde(default = "default_near")]
    near: f32,
    #[serde(default = "default_far")]
    far: f32,
}

/// A scene file placed in the world.
#[derive(Deserialize)]
struct SceneInstance {
    /// Path relative to the asset directory.
    path: String,
    #[serde(default)]
    position: Vec3,
    /// Euler angles in degrees, applied in XYZ order.
    #[serde(default)]
    rotation: Vec3,
    #[serde(default = "default_scale")]
    scale: f32,
}

fn default_fov() -> f32 {
    65.0
}
fn default_near() -> f32 {
    0.01
}
fn default_far() -> f32 {
    100.0
}
fn default_scale() -> f32 {
    1.0
}

impl SceneInstance {
    fn transform(&self) -> Transform {
        let rotation = Quat::from_euler(
            EulerRot::XYZ,
            self.rotation.x.to_radians(),
            self.rotation.y.to_radians(),
            self.rotation.z.to_radians(),
        );
        Transform {
            translation: self.position,
            rotation,
            scale: Vec3::splat(self.scale),
        }
    }
}

fn write_png(path: &Path, size: UVec2, rgba: &[u8]) -> Result<(), String> {
    // The alpha channel is whatever the passes left there, so only the colour is kept.
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|texel| [texel[0], texel[1], texel[2]])
        .collect();
    let file = fs::File::create(path).map_err(|err| err.to_string())?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), size.x, size.y);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|err| err.to_string())?;
    writer.write_image_data(&rgb).map_err(|err| err.to_string())
}

fn run(description: &Path, output: &Path) -> Result<(), String> {
    let text = fs::read_to_string(description)
        .map_err(|err| format!("{}: {err}", description.display()))?;
    let description: SceneDescription =
        ron::from_str(&text).map_err(|err| format!("{}: {err}", description.display()))?;
    let size = UVec2::from(description.size);
    if size.cmpeq(UVec2::ZERO).any() {
        return Err("the image size must not be zero".into());
    }

    let mut app = App::new();
    app.add_plugins((
        ConsolePlugin {
            level: tracing::Level::WARN,
            also_log_to_stderr: true,
            filter: String::new(),
        },
        TaskPoolPlugin::default(),
        AssetPlugin {
            watch_for_changes_override: Some(false),
            ..asset_plugin()
        },
        MeshAssets,
        TransformPlugin,
        ScenePlugin,
        // Not pipelined: every `update` extracts and renders, and the frame can be read back.
        RenderPlugin {
            headless: Some(size),
        },
    ));

    let camera = &description.camera;
    app.world_mut().spawn(CameraBundle::new(
        Transform::from_translation(camera.position).looking_at(camera.look_at, Vec3::Y),
        camera.fov.to_radians(),
        camera.near,
        camera.far,
    ));
    let scenes: Vec<(String, Handle<Scene>)> = description
        .scenes
        .iter()
        .map(|instance| {
            let scene = app
                .world()
                .resource::<AssetServer>()
                .load::<Scene>(instance.path.clone());
            app.world_mut().spawn((
                instance.transform(),
                SpawnScene {
                    scene: scene.clone(),
                },
            ));
            (instance.path.clone(), scene)
        })
        .collect();
    if let Some(pixel_error) = description.pixel_error {
        app.sub_app_mut(RenderApp)
            .world_mut()
            .resource_mut::<RenderSettings>()
            .pixel_error = pixel_error;
    }
    app.finish();
    app.cleanup();

    // A scene is in the world once its `SpawnScene` was replaced by its instances.
    let start = Instant::now();
    loop {
        app.update();
        let server = app.world().resource::<AssetServer>();
        for (path, scene) in &scenes {
            if let LoadState::Failed(err) = server.load_state(scene) {
                return Err(format!("failed to load {path}: {err}"));
            }
        }
        let world = app.world_mut();
        if world
            .query_filtered::<(), With<SpawnScene>>()
            .iter(world)
            .next()
            .is_none()
        {
            break;
        }
        if start.elapsed() > LOAD_TIMEOUT {
            return Err("timed out waiting for the scenes to load".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    for _ in 0..SETTLE_FRAMES {
        app.update();
    }

    let (size, pixels) = read_frame(&mut app);
    write_png(output, size, &pixels).map_err(|err| format!("{}: {err}", output.display()))?;

    let world = app.world_mut();
    let instances = world.query::<&Instance>().iter(world).count();
    let meshlets = app
        .sub_app(RenderApp)
        .world()
        .resource::<HeadlessTarget>()
        .visible_meshlets;
    println!(
        "{}: {}x{}, {instances} instances, {meshlets} meshlets drawn, {:.1} s",
        output.display(),
        size.x,
        size.y,
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [description, output] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    match run(Path::new(description), Path::new(output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
