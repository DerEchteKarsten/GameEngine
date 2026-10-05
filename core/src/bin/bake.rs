//! Asset baking tool: turns the glTF files in the unbaked asset directory into the scene, mesh and texture files the engine loads
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use bevy::tasks::{AsyncComputeTaskPool, TaskPool};
use core::{
    ASSET_DIR, UNBAKED_ASSET_DIR,
    assets::{bake::bake_gltf, mesh::SCENE_EXTENSION},
};

const USAGE: &str = "usage: bake [--force] [FILE...]

Bakes the glTF files (.glb, .gltf) in the unbaked asset directory that are newer
than their baked scene into the asset directory.
  FILE...  bake only these files (paths relative to the unbaked asset directory)
  --force  bake them even if they are not newer";

/// The glTF files below `dir`, relative to it.
fn sources(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|e| e == "glb" || e == "gltf") {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Peak resident memory of this process in MiB.
fn peak_memory() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmHWM:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib / 1024)
}

fn main() -> ExitCode {
    let mut force = false;
    let mut named = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--force" => force = true,
            flag if flag.starts_with('-') => {
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            file => named.push(PathBuf::from(file)),
        }
    }
    let sources = sources(Path::new(UNBAKED_ASSET_DIR));
    if let Some(unknown) = named.iter().find(|file| !sources.contains(file)) {
        eprintln!("no such asset: {}", unknown.display());
        return ExitCode::FAILURE;
    }

    // The meshlet builder and the bake run their jobs on this pool.
    AsyncComputeTaskPool::get_or_init(TaskPool::new);
    let start = Instant::now();
    let mut failed = false;
    for file in if named.is_empty() { &sources } else { &named } {
        let source = Path::new(UNBAKED_ASSET_DIR).join(file);
        let scene = Path::new(ASSET_DIR).join(file.with_extension(SCENE_EXTENSION));
        let modified = |path: &Path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
        if !force && modified(&scene) > modified(&source) {
            println!("{}: up to date", file.display());
            continue;
        }
        let file_start = Instant::now();
        match bake_gltf(&source, &scene) {
            Ok((meshes, textures)) => println!(
                "{}: {meshes} meshes, {textures} textures, {:.1} s",
                file.display(),
                file_start.elapsed().as_secs_f32()
            ),
            Err(err) => {
                println!("{}: FAILED: {err:#}", file.display());
                failed = true;
            }
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
