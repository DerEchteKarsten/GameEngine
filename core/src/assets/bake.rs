//! Bakes a glTF file into a `.scene` file and one `.mesh`/`.tex` file per mesh and texture.
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::BufWriter,
    path::Path,
};

use anyhow::{Context, Result};
use bevy::tasks::AsyncComputeTaskPool;
use glam::{Mat4, Vec3, Vec4};
use lava::{bindings::Material, bindless::NULL_HANDLE};

use crate::assets::{
    mesh::{MESH_EXTENSION, MeshletMesh, SceneFile},
    texture::{TEXTURE_EXTENSION, TextureData, TextureKind},
};

/// glTF's default `alphaCutoff`.
const DEFAULT_ALPHA_CUTOFF: f32 = 0.5;

/// The alpha below which fragments of a material are discarded; 0 draws everything. There
/// is no blending, so blended materials are cut out at the default cutoff instead of being
/// drawn opaque. So are masked ones with a cutoff of 0: a mask that cuts nothing is how some
/// exporters write blended decals.
fn alpha_cutoff(mode: gltf::material::AlphaMode, cutoff: Option<f32>) -> f32 {
    use gltf::material::AlphaMode;
    match (mode, cutoff) {
        (AlphaMode::Opaque, _) => 0.0,
        (AlphaMode::Mask, Some(cutoff)) if cutoff > 0.0 => cutoff,
        (AlphaMode::Mask | AlphaMode::Blend, _) => DEFAULT_ALPHA_CUTOFF,
    }
}

/// `name`, told apart by primitive when the glTF mesh has several: primitives have no names.
fn primitive_name(name: Option<&str>, primitive: usize, primitives: usize) -> String {
    match name {
        Some(name) if primitives > 1 => format!("{name}.{primitive}"),
        Some(name) => name.to_string(),
        None => String::new(),
    }
}

fn make_unique_filename(
    names: &mut HashSet<String>,
    name: &str,
    fallback: &str,
    extension: &str,
) -> String {
    let stem: String = name
        .chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '_',
        })
        .collect();
    let stem = stem.trim_matches('.');
    let stem = if stem.is_empty() { fallback } else { stem };
    let mut file = format!("{stem}.{extension}");
    let mut number = 2;
    // File systems that ignore case must not see two names as one.
    while !names.insert(file.to_lowercase()) {
        file = format!("{stem}_{number}.{extension}");
        number += 1;
    }
    file
}

struct MeshJob {
    mesh: usize,
    primitive: usize,
    /// Relative to the directory of the scene file.
    path: String,
}

struct TextureJob {
    image: usize,
    kind: TextureKind,
    path: String,
}

/// Bakes the glTF file `source` into the scene file `scene_path` and, in the directory next
/// to it with the same name, a file per mesh and texture. Returns how many of each.
pub fn bake_gltf(source: &Path, scene_path: &Path) -> Result<(usize, usize)> {
    let base = source.parent();
    let scene_dir = scene_path.parent().unwrap_or(Path::new(""));
    let pieces = scene_path
        .file_stem()
        .context("the scene file has no name")?
        .to_string_lossy()
        .into_owned();
    // Whatever an earlier bake left there may no longer be part of the scene.
    let _ = fs::remove_dir_all(scene_dir.join(&pieces));
    for dir in ["meshes", "textures"] {
        fs::create_dir_all(scene_dir.join(&pieces).join(dir))?;
    }

    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice(&fs::read(source)?)?;
    let buffers = gltf::import_buffers(&document, base, blob)?;

    // Every primitive that can be drawn is a mesh.
    let mut mesh_jobs = Vec::new();
    let mut mesh_remap = HashMap::new();
    let mut mesh_names = HashSet::new();
    for mesh in document.meshes() {
        let primitives = mesh.primitives().len();
        for primitive in mesh.primitives() {
            let drawable = primitive.get(&gltf::Semantic::Positions).is_some()
                && primitive.get(&gltf::Semantic::Normals).is_some()
                && primitive.indices().is_some_and(|i| i.count() >= 3);
            if !drawable {
                continue;
            }
            let index = mesh_jobs.len();
            mesh_remap.insert((mesh.index(), primitive.index()), index as u32);
            let name = primitive_name(mesh.name(), primitive.index(), primitives);
            let file = make_unique_filename(
                &mut mesh_names,
                &name,
                &format!("mesh_{index}"),
                MESH_EXTENSION,
            );
            mesh_jobs.push(MeshJob {
                mesh: mesh.index(),
                primitive: primitive.index(),
                path: format!("{pieces}/meshes/{file}"),
            });
        }
    }

    let mut scene = SceneFile::default();
    let mut material_remap: HashMap<Option<usize>, u32> = HashMap::new();
    let mut texture_jobs: Vec<TextureJob> = Vec::new();
    let mut texture_remap: HashMap<(usize, TextureKind), u32> = HashMap::new();
    let mut texture_names = HashSet::new();
    // The same image is baked once per kind of texture it is used as.
    let mut import_texture = |texture: Option<gltf::Texture>, kind: TextureKind| -> u32 {
        let Some(texture) = texture else {
            return NULL_HANDLE;
        };
        let source = texture.source();
        let image = source.index();
        *texture_remap.entry((image, kind)).or_insert_with(|| {
            let index = texture_jobs.len();
            let name = source.name().or(texture.name()).unwrap_or_default();
            let file = make_unique_filename(
                &mut texture_names,
                name,
                &format!("texture_{index}"),
                TEXTURE_EXTENSION,
            );
            texture_jobs.push(TextureJob {
                image,
                kind,
                path: format!("{pieces}/textures/{file}"),
            });
            index as u32
        })
    };
    let mut world_nodes = Vec::new();
    let mut stack: Vec<(gltf::Node, Mat4)> = document
        .scenes()
        .flat_map(|scene| scene.nodes())
        .map(|node| (node, Mat4::IDENTITY))
        .collect();
    while let Some((node, parent)) = stack.pop() {
        let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
        stack.extend(node.children().map(|child| (child, world)));
        world_nodes.push((node, world));
    }
    for (node, transform) in &world_nodes {
        let Some(gltf_mesh) = node.mesh() else {
            continue;
        };

        let primitives = gltf_mesh.primitives().len();
        for primitive in gltf_mesh.primitives() {
            let Some(mesh) = mesh_remap.get(&(gltf_mesh.index(), primitive.index())) else {
                continue;
            };

            // One material per glTF material in use; `None` is glTF's default material.
            let pmaterial = primitive.material();
            let material = *material_remap.entry(pmaterial.index()).or_insert_with(|| {
                let pbr = pmaterial.pbr_metallic_roughness();
                let normal = pmaterial.normal_texture();
                let occlusion = pmaterial.occlusion_texture();
                scene.materials.push(Material {
                    color: Vec4::from_array(pbr.base_color_factor()),
                    emissive: Vec3::from_array(pmaterial.emissive_factor()),
                    metalic_factor: pbr.metallic_factor(),
                    roughness_factor: pbr.roughness_factor(),
                    normal_scale: normal.as_ref().map(|n| n.scale()).unwrap_or(1.0),
                    occlusion_strength: occlusion.as_ref().map(|o| o.strength()).unwrap_or(1.0),
                    alpha_cutoff: alpha_cutoff(pmaterial.alpha_mode(), pmaterial.alpha_cutoff()),
                    color_texture: import_texture(
                        pbr.base_color_texture().map(|i| i.texture()),
                        TextureKind::Color,
                    ),
                    metallic_roughness_texture: import_texture(
                        pbr.metallic_roughness_texture().map(|i| i.texture()),
                        TextureKind::Data,
                    ),
                    normal_texture: import_texture(
                        normal.map(|n| n.texture()),
                        TextureKind::Normal,
                    ),
                    occlusion_texture: import_texture(
                        occlusion.map(|o| o.texture()),
                        TextureKind::Data,
                    ),
                    emissive_texture: import_texture(
                        pmaterial.emissive_texture().map(|i| i.texture()),
                        TextureKind::Color,
                    ),
                    pad: Vec3::ZERO,
                });
                (scene.materials.len() - 1) as u32
            });

            scene.instance_materials.push(material);
            scene.instance_mesh.push(*mesh);
            scene.instance_transforms.push(*transform);
            scene
                .instance_names
                .push(primitive_name(node.name(), primitive.index(), primitives));
        }
    }
    scene.meshes = mesh_jobs.iter().map(|job| job.path.clone()).collect();
    scene.textures = texture_jobs.iter().map(|job| job.path.clone()).collect();

    // Each job holds its mesh or texture only until its file is written.
    let (document, buffers) = (&document, &buffers[..]);
    let pool = AsyncComputeTaskPool::get();
    let textures = pool.scope(|scope| {
        for job in &texture_jobs {
            scope.spawn(async move {
                bake_texture(document, buffers, base, job, &scene_dir.join(&job.path))
                    .with_context(|| format!("texture {}", job.path))
            });
        }
    });
    let meshes = pool.scope(|scope| {
        for job in &mesh_jobs {
            scope.spawn(async move {
                bake_mesh(document, buffers, job, &scene_dir.join(&job.path))
                    .with_context(|| format!("mesh {}", job.path))
            });
        }
    });
    for result in textures.into_iter().chain(meshes) {
        result?;
    }

    // Last, so the scene never names a file that isn't there yet.
    scene.write(&mut BufWriter::new(File::create(scene_path)?))?;
    Ok((mesh_jobs.len(), texture_jobs.len()))
}

fn bake_texture(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    base: Option<&Path>,
    job: &TextureJob,
    path: &Path,
) -> Result<()> {
    let image = document
        .images()
        .nth(job.image)
        .context("the texture has no image")?;
    let image = gltf::image::Data::from_source(image.source(), base, buffers)?;
    TextureData::from_gltf(&image, job.kind).write(&mut BufWriter::new(File::create(path)?))
}

fn bake_mesh(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    job: &MeshJob,
    path: &Path,
) -> Result<()> {
    let primitive = document
        .meshes()
        .nth(job.mesh)
        .and_then(|mesh| mesh.primitives().nth(job.primitive))
        .context("the mesh is gone")?;
    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
    let positions: Vec<Vec3> = reader
        .read_positions()
        .context("no positions")?
        .map(Vec3::from)
        .collect();
    let normals: Vec<f32> = reader
        .read_normals()
        .context("no normals")?
        .flatten()
        .collect();
    let uvs: Vec<f32> = match reader.read_tex_coords(0) {
        Some(uvs) => uvs.into_f32().flatten().collect(),
        None => vec![0.0; positions.len() * 2],
    };
    let indices: Vec<u32> = reader
        .read_indices()
        .context("no indices")?
        .into_u32()
        .collect();
    assert_eq!(positions.len() * 3, normals.len());
    assert_eq!(positions.len() * 2, uvs.len());

    MeshletMesh::new(&indices, &positions, &normals, &uvs)
        .write(&mut BufWriter::new(File::create(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::tasks::TaskPool;

    #[test]
    fn alpha_cutoff_follows_the_alpha_mode() {
        use gltf::material::AlphaMode;
        assert_eq!(alpha_cutoff(AlphaMode::Opaque, Some(0.7)), 0.0);
        assert_eq!(alpha_cutoff(AlphaMode::Mask, Some(0.7)), 0.7);
        assert_eq!(alpha_cutoff(AlphaMode::Mask, None), 0.5);
        assert_eq!(alpha_cutoff(AlphaMode::Mask, Some(0.0)), 0.5);
        assert_eq!(alpha_cutoff(AlphaMode::Blend, None), 0.5);
    }

    #[test]
    fn file_names_are_safe_and_unique() {
        let mut names = HashSet::new();
        let mut name = |name| make_unique_filename(&mut names, name, "mesh_0", "mesh");
        assert_eq!(name("a#b/c d"), "a_b_c_d.mesh");
        assert_eq!(name(".."), "mesh_0.mesh");
        assert_eq!(name("Cube.1"), "Cube.1.mesh");
        assert_eq!(name("cube.1"), "cube.1_2.mesh");
    }

    /// Writes a glTF file with one triangle that two nodes draw.
    fn triangle_gltf(dir: &Path) -> std::path::PathBuf {
        let mut bin = Vec::new();
        for position in [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0f32]] {
            bin.extend_from_slice(bytemuck::cast_slice(&position));
        }
        for _ in 0..3 {
            bin.extend_from_slice(bytemuck::cast_slice(&[0.0, 0.0, 1.0f32]));
        }
        bin.extend_from_slice(bytemuck::cast_slice(&[0u32, 1, 2]));
        fs::write(dir.join("triangle.bin"), &bin).unwrap();
        let json = r#"{
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0, 1]}],
            "nodes": [
                {"mesh": 0, "name": "left"},
                {"mesh": 0, "name": "right", "translation": [2.0, 0.0, 0.0]}
            ],
            "meshes": [{"name": "Tri/angle", "primitives": [
                {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2}
            ]}],
            "buffers": [{"uri": "triangle.bin", "byteLength": 84}],
            "bufferViews": [
                {"buffer": 0, "byteOffset": 0, "byteLength": 36},
                {"buffer": 0, "byteOffset": 36, "byteLength": 36},
                {"buffer": 0, "byteOffset": 72, "byteLength": 12}
            ],
            "accessors": [
                {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                 "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
                {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
                {"bufferView": 2, "componentType": 5125, "count": 3, "type": "SCALAR"}
            ]
        }"#;
        let path = dir.join("tri.gltf");
        fs::write(&path, json).unwrap();
        path
    }

    #[test]
    fn baking_writes_a_scene_and_its_meshes() {
        use bevy::asset::io::VecReader;

        AsyncComputeTaskPool::get_or_init(TaskPool::new);
        let dir = std::env::temp_dir().join(format!("core-bake-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("baked/tri/meshes")).unwrap();
        let source = triangle_gltf(&dir);
        let scene_path = dir.join("baked/tri.scene");
        // A file of an earlier bake that the scene no longer has.
        let stray = dir.join("baked/tri/meshes/old.mesh");
        fs::write(&stray, b"old").unwrap();

        assert_eq!(bake_gltf(&source, &scene_path).unwrap(), (1, 0));
        let mut reader = VecReader::new(fs::read(&scene_path).unwrap());
        let scene = bevy::tasks::block_on(SceneFile::read(&mut reader)).unwrap();
        assert_eq!(scene.meshes, ["tri/meshes/Tri_angle.mesh"]);
        assert_eq!(scene.instance_names, ["right", "left"]);
        assert_eq!(scene.instance_mesh, [0, 0]);
        assert_eq!(scene.materials[0].color_texture, NULL_HANDLE);
        assert!(dir.join("baked").join(&scene.meshes[0]).exists());
        assert!(!stray.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
