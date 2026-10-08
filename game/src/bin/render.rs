//! Headless render: loads a scene file, renders frames of it without a window, saves the last as a PNG and optionally profiles them
use std::{
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
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
    profiler::{Profiler, ProfilerPlugin, capture::ProfileLayer},
    render::{
        headless::{HeadlessRenderPlugin, render_frame},
        render::RenderSettings,
    },
    scene::{Instance, ScenePlugin, Skybox, SpawnScene, camera::CameraBundle, file::Scene},
};
use glam::{Quat, UVec2};
use tracing_subscriber::{
    Layer, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

const USAGE: &str = "usage: render [--frames N] [--profile DIR] SCENE OUTPUT.png

Renders the scene file SCENE (.scene or .scene.ron, inside the asset directory)
and writes the image to OUTPUT.png.

  --frames N     render N frames once the scene is loaded (default 1); the image is the last
  --profile DIR  profile those frames: write DIR/profile.txt (summary, also printed) and
                 DIR/profile.json (Chrome trace for Perfetto or chrome://tracing)";

struct Args {
    scene: PathBuf,
    output: PathBuf,
    frames: usize,
    profile: Option<PathBuf>,
}

fn parse_args() -> Option<Args> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let (mut frames, mut profile) = (1, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next()?.parse().ok().filter(|n| *n > 0)?,
            "--profile" => profile = Some(PathBuf::from(args.next()?)),
            _ if arg.starts_with("--") => return None,
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    let [scene, output] = <[PathBuf; 2]>::try_from(positional).ok()?;
    Some(Args {
        scene,
        output,
        frames,
        profile,
    })
}

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

fn run(args: &Args) -> Result<(), String> {
    let (scene, output) = (args.scene.as_path(), args.output.as_path());
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
    if args.profile.is_some() {
        app.add_plugins(ProfilerPlugin::default());
    }

    app.world_mut().spawn(CameraBundle::new(
        // Looks along +X, tilted up to aim at (10, 0, 3).
        Transform::from_xyz(0.0, 0.0, 2.0).with_rotation(Quat::from_rotation_y(-0.1f32.atan())),
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
    app.world_mut()
        .insert_resource(Skybox { image: sky.clone() });
    let assets: [(&str, UntypedHandle); 2] =
        [(&scene_path, scene.untyped()), (SKYBOX, sky.untyped())];
    app.finish();
    app.cleanup();
    if let Some(mut profiler) = app.world_mut().get_resource_mut::<Profiler>() {
        profiler.set_paused(true);
    }

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

    let loaded = start.elapsed();
    if let Some(mut profiler) = app.world_mut().get_resource_mut::<Profiler>() {
        profiler.set_paused(false);
    }
    let mut frame = None;
    for _ in 0..args.frames {
        frame = Some(render_frame(app.world_mut(), &RenderSettings::default()));
        // Ends the profiler's frame.
        app.update();
    }
    let frame = frame.expect("at least one frame is rendered");
    write_png(output, frame.size, &frame.pixels)
        .map_err(|err| format!("{}: {err}", output.display()))?;
    if let Some(dir) = &args.profile {
        let profiler = app.world().resource::<Profiler>();
        profiler
            .save(dir)
            .map_err(|err| format!("{}: {err}", dir.display()))?;
        print!(
            "{}",
            fs::read_to_string(dir.join("profile.txt")).unwrap_or_default()
        );
    }

    let world = app.world_mut();
    let instances = world.query::<&Instance>().iter(world).count();
    println!(
        "{}: {}x{}, {instances} instances, {} meshlets drawn, loaded in {:.1} s, {} frames in {:.2} s",
        output.display(),
        frame.size.x,
        frame.size.y,
        frame.visible_meshlets,
        loaded.as_secs_f32(),
        args.frames,
        (start.elapsed() - loaded).as_secs_f32()
    );
    Ok(())
}

fn main() -> ExitCode {
    let Some(args) = parse_args() else {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    };
    // Lava reports validation errors through tracing; the profiler records spans.
    let log = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_filter(LevelFilter::WARN);
    let profile = args
        .profile
        .is_some()
        .then(|| ProfileLayer.with_filter(LevelFilter::INFO));
    tracing_subscriber::registry()
        .with(log)
        .with(profile)
        .init();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
