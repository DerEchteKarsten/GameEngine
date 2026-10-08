//! Asset baking tool: turns the glTF and OpenEXR files in the unbaked asset directory into the scene, mesh, material and texture files the engine loads
use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Instant,
};

use bevy::tasks::{AsyncComputeTaskPool, TaskPool};
use core::{
    ASSET_DIR, UNBAKED_ASSET_DIR,
    assets::{
        SCENE_EXTENSION, TEXTURE_EXTENSION,
        bake::{bake_exr, bake_gltf},
    },
};

/// The glTF and OpenEXR files below `dir`, relative to it.
fn sources(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path
                .extension()
                .is_some_and(|e| e == "glb" || e == "gltf" || e == "exr")
            {
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
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();
    let mut force = false;
    // Also write the scene and materials as RON, for reading.
    let mut ron = false;
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--force" => force = true,
            "--ron" => ron = true,
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
        let exr = file.extension().is_some_and(|e| e == "exr");
        let baked = Path::new(ASSET_DIR).join(file.with_extension(if exr {
            TEXTURE_EXTENSION
        } else {
            SCENE_EXTENSION
        }));
        let modified = |path: &Path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
        // A source that isn't there is left to fail in the bake.
        if !force && modified(&source).is_some_and(|source| modified(&baked) > Some(source)) {
            println!("{}: up to date", file.display());
            continue;
        }
        let file_start = Instant::now();
        let result = if exr {
            bake_exr(&source, &baked).map(|()| "HDR texture".to_string())
        } else {
            bake_gltf(&source, &baked, ron).map(|(meshes, materials, textures)| {
                format!("{meshes} meshes, {materials} materials, {textures} textures")
            })
        };
        match result {
            Ok(summary) => println!(
                "{}: {summary}, {:.1} s",
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
