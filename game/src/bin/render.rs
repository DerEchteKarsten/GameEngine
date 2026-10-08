//! Headless render: loads a scene file, renders one frame of it without a window and saves it as a PNG
use std::{
    fs,
    io::BufWriter,
    path::Path,
    process::ExitCode,
    time::{Duration, Instant},
};

use bevy::{
    app::{App, TaskPoolPlugin},
    asset::{AssetPlugin, AssetServer, LoadState, RecursiveDependencyLoadState, UntypedHandle},
    ecs::query::With,
    transform::{TransformPlugin, components::Transform},
};
use core::{
    ASSET_DIR, asset_plugin,
    assets::{MeshAssets, texture::GpuTexture},
    render::{
        headless::{HeadlessRenderPlugin, render_frame},
        render::RenderSettings,
    },
    scene::{Instance, ScenePlugin, Skybox, SpawnScene, camera::CameraBundle, file::Scene},
};
use glam::{UVec2, Vec3};

const USAGE: &str = "usage: render SCENE OUTPUT.png

Renders the scene file SCENE (.scene or .scene.ron, inside the asset directory)
and writes the image to OUTPUT.png.";

const SIZE: UVec2 = UVec2::new(1280, 720);
const SKYBOX: &str = "kloofendal_48d_partly_cloudy_puresky_4k.tex";
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);

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

fn run(scene: &Path, output: &Path) -> Result<(), String> {
    let absolute = fs::canonicalize(scene).map_err(|err| format!("{}: {err}", scene.display()))?;
    let scene_path = absolute
        .strip_prefix(ASSET_DIR)
        .map_err(|_| format!("{} is not inside {ASSET_DIR}", scene.display()))?
        .to_string_lossy()
        .into_owned();

    let mut app = App::new();
    app.add_plugins((
        TaskPoolPlugin::default(),
        AssetPlugin {
            watch_for_changes_override: Some(false),
            ..asset_plugin()
        },
        MeshAssets,
        TransformPlugin,
        ScenePlugin,
        HeadlessRenderPlugin { size: SIZE },
    ));

    app.world_mut().spawn(CameraBundle::new(
        Transform::from_xyz(0.0, 2.0, 0.0).looking_at(Vec3::new(10.0, 3.0, 0.0), Vec3::Y),
        65.0_f32.to_radians(),
        0.01,
        100.0,
    ));
    let server = app.world().resource::<AssetServer>();
    let scene = server.load::<Scene>(scene_path.clone());
    let sky = server.load::<GpuTexture>(SKYBOX);
    app.world_mut().spawn(SpawnScene {
        scene: scene.clone(),
    });
    app.world_mut().insert_resource(Skybox { image: sky.clone() });
    let assets: [(&str, UntypedHandle); 2] =
        [(&scene_path, scene.untyped()), (SKYBOX, sky.untyped())];
    app.finish();
    app.cleanup();

    // The scene is in the world once its `SpawnScene` was replaced by its entities (including
    // those of the scenes it places), and complete once the files it references are loaded too.
    let start = Instant::now();
    loop {
        app.update();
        let server = app.world().resource::<AssetServer>();
        let mut loaded = true;
        for (path, handle) in &assets {
            if let LoadState::Failed(err) = server.load_state(handle) {
                return Err(format!("failed to load {path}: {err}"));
            }
            if let RecursiveDependencyLoadState::Failed(err) =
                server.recursive_dependency_load_state(handle)
            {
                return Err(format!("failed to load a file of {path}: {err}"));
            }
            loaded &= server.is_loaded_with_dependencies(handle);
        }
        let world = app.world_mut();
        let spawned = world
            .query_filtered::<(), With<SpawnScene>>()
            .iter(world)
            .next()
            .is_none();
        if loaded && spawned {
            break;
        }
        if start.elapsed() > LOAD_TIMEOUT {
            return Err("timed out waiting for the scene to load".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // One more update, so the transforms and camera matrices of the new instances settle.
    app.update();

    let frame = render_frame(app.world_mut(), &RenderSettings::default());
    write_png(output, frame.size, &frame.pixels)
        .map_err(|err| format!("{}: {err}", output.display()))?;

    let world = app.world_mut();
    let instances = world.query::<&Instance>().iter(world).count();
    println!(
        "{}: {}x{}, {instances} instances, {} meshlets drawn, {:.1} s",
        output.display(),
        frame.size.x,
        frame.size.y,
        frame.visible_meshlets,
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [scene, output] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    // Lava reports validation errors through tracing.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_max_level(tracing::Level::WARN)
        .init();
    match run(Path::new(scene), Path::new(output)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
