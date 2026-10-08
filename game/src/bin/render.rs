//! Headless render: loads a scene file, renders frames of it from a given camera without a window, saves the last as a PNG and optionally profiles them
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
#[cfg(feature = "profiling")]
use core::profiler::{Profiler, ProfilerPlugin, capture::ProfileLayer};
use core::{
    ASSET_DIR, asset_plugin,
    assets::{MeshAssets, texture::GpuTexture},
    render::{
        headless::{HeadlessRenderPlugin, read_pixels, render_frame},
        render::RenderSettings,
    },
    scene::{Instance, ScenePlugin, Skybox, SpawnScene, camera::CameraBundle, file::Scene},
};
use glam::{Quat, UVec2, Vec3};
use tracing_subscriber::{
    Layer, filter::LevelFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

const USAGE: &str = "usage: render [--frames N] [--profile DIR] [--eye X,Y,Z] [--target X,Y,Z]
              [--fov DEG] SCENE OUTPUT.png

Renders the scene file SCENE (.scene or .scene.ron, inside the asset directory)
and writes the image to OUTPUT.png.

  --eye X,Y,Z    camera position in world space (+Z up, default 0,0,2)
  --target X,Y,Z point the camera looks at (default 10,0,3)
  --fov DEG      vertical field of view in degrees (default 65)
  --frames N     render N frames once the scene is loaded (default 1); the image is the last
  --profile DIR  profile those frames: write DIR/profile.txt (summary, also printed) and
                 DIR/profile.json (Chrome trace for Perfetto or chrome://tracing);
                 needs the `profiling` feature";

struct Args {
    scene: PathBuf,
    output: PathBuf,
    frames: usize,
    profile: Option<PathBuf>,
    eye: Vec3,
    target: Vec3,
    fov: f32,
}

fn parse_vec3(arg: &str) -> Option<Vec3> {
    let mut parts = arg.split(',').map(|part| part.trim().parse().ok());
    let v = Vec3::new(parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

fn parse_args() -> Option<Args> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let (mut frames, mut profile) = (1, None);
    let (mut eye, mut target, mut fov) =
        (Vec3::new(0.0, 0.0, 2.0), Vec3::new(10.0, 0.0, 3.0), 65.0);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--frames" => frames = args.next()?.parse().ok().filter(|n| *n > 0)?,
            "--profile" => profile = Some(PathBuf::from(args.next()?)),
            "--eye" => eye = parse_vec3(&args.next()?)?,
            "--target" => target = parse_vec3(&args.next()?)?,
            "--fov" => {
                fov = args
                    .next()?
                    .parse()
                    .ok()
                    .filter(|f| *f > 0.0 && *f < 180.0)?
            }
            _ if arg.starts_with("--") => return None,
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    let [scene, output] = <[PathBuf; 2]>::try_from(positional).ok()?;
    if (target - eye).length_squared() == 0.0 {
        return None;
    }
    Some(Args {
        scene,
        output,
        frames,
        profile,
        eye,
        target,
        fov,
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
    #[cfg(feature = "profiling")]
    if args.profile.is_some() {
        app.add_plugins(ProfilerPlugin::default());
    }
    #[cfg(not(feature = "profiling"))]
    if args.profile.is_some() {
        return Err("--profile needs the `profiling` feature".into());
    }

    // The camera looks along its local +X: yaw turns it around +Z, a positive pitch looks up.
    let dir = args.target - args.eye;
    let (yaw, pitch) = (dir.y.atan2(dir.x), dir.z.atan2(dir.truncate().length()));
    app.world_mut().spawn(CameraBundle::new(
        Transform::from_translation(args.eye)
            .with_rotation(Quat::from_rotation_z(yaw) * Quat::from_rotation_y(-pitch)),
        args.fov.to_radians(),
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
    #[cfg(feature = "profiling")]
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
    #[cfg(feature = "profiling")]
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
    write_png(output, frame.size, &read_pixels(app.world()))
        .map_err(|err| format!("{}: {err}", output.display()))?;
    #[cfg(feature = "profiling")]
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
    let subscriber = tracing_subscriber::registry().with(log);
    #[cfg(feature = "profiling")]
    let subscriber = subscriber.with(
        args.profile
            .is_some()
            .then(|| ProfileLayer.with_filter(LevelFilter::INFO)),
    );
    subscriber.init();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
