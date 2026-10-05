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

fn main() -> ExitCode {
    let mut force = false;
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--force" => force = true,
            file => files.push(PathBuf::from(file)),
        }
    }
    if files.is_empty() {
        files = sources(Path::new(UNBAKED_ASSET_DIR));
    }

    // The meshlet builder and the bake run their jobs on this pool.
    AsyncComputeTaskPool::get_or_init(TaskPool::new);
    let start = Instant::now();
    let mut failed = false;
    for file in &files {
        let source = Path::new(UNBAKED_ASSET_DIR).join(file);
        let scene = Path::new(ASSET_DIR).join(file.with_extension(SCENE_EXTENSION));
        let modified = |path: &Path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
        // A source that isn't there is left to fail in the bake.
        if !force && modified(&source).is_some_and(|source| modified(&scene) > Some(source)) {
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

    println!("finished in {:.1} s", start.elapsed().as_secs_f32());
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
