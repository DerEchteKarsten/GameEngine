//! Headless asset import: bakes everything in the asset directory without a window or GPU
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

use bevy::{
    app::{App, TaskPoolPlugin},
    asset::processor::{AssetProcessor, ProcessorState},
    tasks::block_on,
};
use core::{
    ASSET_DIR, IMPORTED_ASSET_DIR, asset_plugin, assets::MeshAssets, editor::console::ConsolePlugin,
};

const USAGE: &str = "usage: import [--force [FILE...]]

Bakes the assets in the asset directory that are new or changed.
  --force          reimport everything
  --force FILE...  reimport these files (paths relative to the asset directory)";

/// Files below `dir`, relative to it.
fn files(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn meta_of(file: &Path) -> PathBuf {
    let mut meta = Path::new(IMPORTED_ASSET_DIR).join(file).into_os_string();
    meta.push(".meta");
    meta.into()
}

/// Peak resident memory of this process in MiB.
fn peak_memory() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let sources = files(Path::new(ASSET_DIR));
    match args.next().as_deref() {
        None => {}
        Some("--force") => {
            // Without its meta file the processor considers an asset unprocessed.
            let named: Vec<PathBuf> = args.map(PathBuf::from).collect();
            if let Some(unknown) = named.iter().find(|file| !sources.contains(file)) {
                eprintln!("no such asset: {}", unknown.display());
                return ExitCode::FAILURE;
            }
            for file in if named.is_empty() { &sources } else { &named } {
                let _ = fs::remove_file(meta_of(file));
            }
        }
        Some(_) => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    }

    let mut app = App::new();
    app.add_plugins((
        ConsolePlugin {
            level: tracing::Level::INFO,
            also_log_to_stderr: true,
            filter: String::new(),
        },
        TaskPoolPlugin::default(),
        bevy::asset::AssetPlugin {
            watch_for_changes_override: Some(false),
            ..asset_plugin()
        },
        MeshAssets,
    ));
    app.finish();
    app.cleanup();

    let start = Instant::now();
    let processor = app.world().resource::<AssetProcessor>().clone();
    loop {
        app.update();
        if block_on(processor.get_state()) == ProcessorState::Finished {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let mut failed = false;
    for file in &sources {
        let baked = Path::new(IMPORTED_ASSET_DIR).join(file);
        let size = fs::metadata(&baked).map(|meta| meta.len()).unwrap_or(0);
        if size > 0 && meta_of(file).exists() {
            println!("{:>14} bytes  {}", size, file.display());
        } else {
            println!("        FAILED        {}", file.display());
            failed = true;
        }
    }
    print!("finished in {:.1} s", start.elapsed().as_secs_f32());
    match peak_memory() {
        Some(peak) => println!(", peak memory {peak} MiB"),
        None => println!(),
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
