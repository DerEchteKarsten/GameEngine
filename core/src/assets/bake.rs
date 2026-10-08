//! Bakes the scene of a glTF file into a `.scene` file (its node tree) and one `.mesh`/`.mat`/`.tex` file per mesh, material and texture, and an OpenEXR file into an HDR `.tex` file.
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fs::{self, File},
    io::BufWriter,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use bevy::{ecs::name::Name, tasks::AsyncComputeTaskPool, transform::components::Transform};
use glam::{Quat, Vec3, Vec4};
use tracing::warn;

use crate::{
    assets::{
        MESH_EXTENSION, TEXTURE_EXTENSION,
        material::{MATERIAL_EXTENSION, MaterialFile},
        mesh::MeshletMesh,
        texture::{TextureData, TextureKind},
    },
    scene::{
        Instance, SavedInstance,
        file::{Column, SceneData},
    },
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

/// Whether the engine can draw the primitive: triangles with positions.
fn drawable(primitive: &gltf::Primitive) -> bool {
    let count = match primitive.indices() {
        Some(indices) => indices.count(),
        None => primitive
            .get(&gltf::Semantic::Positions)
            .map_or(0, |positions| positions.count()),
    };
    primitive.mode() == gltf::mesh::Mode::Triangles
        && primitive.get(&gltf::Semantic::Positions).is_some()
        && count >= 3
}

struct MeshJob {
    mesh: usize,
    primitive: usize,
    path: String,
}

struct TextureJob {
    image: usize,
    kind: TextureKind,
    path: String,
}

/// The files a bake writes besides the scene, and what it had to leave out.
#[derive(Default)]
struct Bake {
    pieces: String,
    /// Paths relative to the scene file: the meshes and materials of the scene.
    assets: Vec<String>,
    mesh_jobs: Vec<MeshJob>,
    materials: Vec<(String, MaterialFile)>,
    texture_jobs: Vec<TextureJob>,
    /// Asset index of each glTF primitive and material.
    mesh_assets: HashMap<(usize, usize), u32>,
    material_assets: HashMap<Option<usize>, u32>,
    textures: HashMap<(usize, TextureKind), String>,
    names: HashSet<String>,
    unsupported: BTreeSet<&'static str>,
}

impl Bake {
    fn mesh(&mut self, mesh: &gltf::Mesh, primitive: &gltf::Primitive) -> u32 {
        let key = (mesh.index(), primitive.index());
        if let Some(asset) = self.mesh_assets.get(&key) {
            return *asset;
        }
        let name = primitive_name(mesh.name(), primitive.index(), mesh.primitives().len());
        let fallback = format!("mesh_{}", self.mesh_jobs.len());
        let file = make_unique_filename(&mut self.names, &name, &fallback, MESH_EXTENSION);
        let path = format!("{}/meshes/{file}", self.pieces);
        self.mesh_jobs.push(MeshJob {
            mesh: mesh.index(),
            primitive: primitive.index(),
            path: path.clone(),
        });
        let asset = self.assets.len() as u32;
        self.assets.push(path);
        self.mesh_assets.insert(key, asset);
        asset
    }

    /// The path of the texture relative to the material files.
    fn texture(&mut self, info: Option<(gltf::Texture, u32)>, kind: TextureKind) -> Option<String> {
        let (texture, tex_coord) = info?;
        if tex_coord != 0 {
            self.unsupported
                .insert("texture coordinate sets besides TEXCOORD_0");
        }
        let source = texture.source();
        let image = source.index();
        if let Some(path) = self.textures.get(&(image, kind)) {
            return Some(path.clone());
        }
        let name = source.name().or(texture.name()).unwrap_or_default();
        let fallback = format!("texture_{}", self.texture_jobs.len());
        let file = make_unique_filename(&mut self.names, name, &fallback, TEXTURE_EXTENSION);
        self.texture_jobs.push(TextureJob {
            image,
            kind,
            path: format!("{}/textures/{file}", self.pieces),
        });
        let path = format!("../textures/{file}");
        self.textures.insert((image, kind), path.clone());
        Some(path)
    }

    /// One material file per glTF material; `None` is glTF's default material.
    fn material(&mut self, material: gltf::Material) -> u32 {
        if let Some(asset) = self.material_assets.get(&material.index()) {
            return *asset;
        }
        let pbr = material.pbr_metallic_roughness();
        let normal = material.normal_texture();
        let occlusion = material.occlusion_texture();
        let file = MaterialFile {
            color: Vec4::from_array(pbr.base_color_factor()),
            emissive: Vec3::from_array(material.emissive_factor()),
            metalic_factor: pbr.metallic_factor(),
            roughness_factor: pbr.roughness_factor(),
            normal_scale: normal.as_ref().map_or(1.0, |n| n.scale()),
            occlusion_strength: occlusion.as_ref().map_or(1.0, |o| o.strength()),
            alpha_cutoff: alpha_cutoff(material.alpha_mode(), material.alpha_cutoff()),
            color_texture: self.texture(
                pbr.base_color_texture()
                    .map(|i| (i.texture(), i.tex_coord())),
                TextureKind::Color,
            ),
            metallic_roughness_texture: self.texture(
                pbr.metallic_roughness_texture()
                    .map(|i| (i.texture(), i.tex_coord())),
                TextureKind::Data,
            ),
            normal_texture: self.texture(
                normal.map(|n| (n.texture(), n.tex_coord())),
                TextureKind::Normal,
            ),
            occlusion_texture: self.texture(
                occlusion.map(|o| (o.texture(), o.tex_coord())),
                TextureKind::Data,
            ),
            emissive_texture: self.texture(
                material
                    .emissive_texture()
                    .map(|i| (i.texture(), i.tex_coord())),
                TextureKind::Color,
            ),
        };
        let name = match material.index() {
            Some(index) => material
                .name()
                .map_or(format!("material_{index}"), str::to_string),
            None => "default".to_string(),
        };
        let file_name =
            make_unique_filename(&mut self.names, &name, "material", MATERIAL_EXTENSION);
        let path = format!("{}/materials/{file_name}", self.pieces);
        self.materials.push((path.clone(), file));
        let asset = self.assets.len() as u32;
        self.assets.push(path);
        self.material_assets.insert(material.index(), asset);
        asset
    }
}

/// Bakes the default scene (or the first) of the glTF file `source` into the scene file
/// `scene_path` and, in the directory next to it with the same name, a file per mesh,
/// material and texture. With `ron`, the scene and materials are also written as RON.
/// Returns how many meshes, materials and textures there are.
pub fn bake_gltf(source: &Path, scene_path: &Path, ron: bool) -> Result<(usize, usize, usize)> {
    let base = source.parent();
    let scene_dir = scene_path.parent().unwrap_or(Path::new(""));
    let pieces = scene_path
        .file_stem()
        .context("the scene file has no name")?
        .to_string_lossy()
        .into_owned();
    // Whatever an earlier bake left there may no longer be part of the scene.
    let _ = fs::remove_dir_all(scene_dir.join(&pieces));
    for dir in ["meshes", "materials", "textures"] {
        fs::create_dir_all(scene_dir.join(&pieces).join(dir))?;
    }

    let gltf::Gltf { document, blob } = gltf::Gltf::from_slice(&fs::read(source)?)?;
    let buffers = gltf::import_buffers(&document, base, blob)?;

    let mut bake = Bake {
        pieces,
        ..Default::default()
    };
    for extension in document.extensions_used() {
        match extension {
            "KHR_lights_punctual" => bake.unsupported.insert("lights"),
            "KHR_texture_transform" => bake.unsupported.insert("texture transforms"),
            "KHR_mesh_gpu_instancing" => bake.unsupported.insert("GPU instancing"),
            _ => false,
        };
    }
    if document.animations().next().is_some() {
        bake.unsupported.insert("animations");
    }
    if document.scenes().len() > 1 {
        bake.unsupported.insert("scenes besides the default one");
    }
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next())
        .context("the file has no scene")?;

    // Every node is an entity, parents before their children.
    let mut scene_data = SceneData::default();
    let mut transforms = Vec::new();
    let mut names = (Vec::new(), Vec::new());
    let mut instances = (Vec::new(), Vec::new());
    let mut stack: Vec<(gltf::Node, u32)> = scene.nodes().map(|node| (node, u32::MAX)).collect();
    stack.reverse();
    while let Some((node, parent)) = stack.pop() {
        let entity = scene_data.parents.len() as u32;
        scene_data.parents.push(parent);
        let (translation, rotation, scale) = node.transform().decomposed();
        transforms.push(Transform {
            translation: Vec3::from_array(translation),
            rotation: Quat::from_array(rotation),
            scale: Vec3::from_array(scale),
        });
        if let Some(name) = node.name() {
            names.0.push(entity);
            names.1.push(Name::new(name.to_string()));
        }
        if node.camera().is_some() {
            bake.unsupported.insert("cameras");
        }
        if node.skin().is_some() {
            bake.unsupported.insert("skins (drawn in bind pose)");
        }
        let children: Vec<_> = node.children().collect();
        stack.extend(children.into_iter().rev().map(|child| (child, entity)));

        let Some(mesh) = node.mesh() else {
            continue;
        };
        let primitives: Vec<_> = mesh
            .primitives()
            .filter(|primitive| {
                if primitive.morph_targets().next().is_some() {
                    bake.unsupported
                        .insert("morph targets (drawn in the base pose)");
                }
                let drawable = drawable(primitive);
                if !drawable {
                    bake.unsupported.insert("primitives that aren't triangles");
                }
                drawable
            })
            .collect();
        let several = primitives.len() > 1;
        for primitive in primitives {
            let saved = SavedInstance {
                mesh: bake.mesh(&mesh, &primitive),
                material: bake.material(primitive.material()),
            };
            // Several primitives become children with an identity transform.
            let instance = if several {
                let child = scene_data.parents.len() as u32;
                scene_data.parents.push(entity);
                transforms.push(Transform::IDENTITY);
                if let Some(name) = mesh.name() {
                    names.0.push(child);
                    names
                        .1
                        .push(Name::new(format!("{name}.{}", primitive.index())));
                }
                child
            } else {
                entity
            };
            instances.0.push(instance);
            instances.1.push(saved);
        }
    }
    for kind in &bake.unsupported {
        warn!(
            "{}: {kind} are not supported and left out",
            source.display()
        );
    }

    // The instances first, so their meshes and materials start loading early.
    scene_data.columns = vec![
        Column::new::<Instance>(instances.0, instances.1),
        Column::new::<Transform>((0..transforms.len() as u32).collect(), transforms),
        Column::new::<Name>(names.0, names.1),
    ];
    scene_data.assets = std::mem::take(&mut bake.assets);

    // Each job holds its mesh or texture only until its file is written.
    let (document, buffers) = (&document, &buffers[..]);
    let pool = AsyncComputeTaskPool::get();
    let textures = pool.scope(|scope| {
        for job in &bake.texture_jobs {
            scope.spawn(async move {
                bake_texture(document, buffers, base, job, &scene_dir.join(&job.path))
                    .with_context(|| format!("texture {}", job.path))
            });
        }
    });
    let meshes = pool.scope(|scope| {
        for job in &bake.mesh_jobs {
            scope.spawn(async move {
                bake_mesh(document, buffers, job, &scene_dir.join(&job.path))
                    .with_context(|| format!("mesh {}", job.path))
            });
        }
    });
    for result in textures.into_iter().chain(meshes) {
        result?;
    }
    for (path, material) in &bake.materials {
        let path = scene_dir.join(path);
        fs::write(&path, material.write(false)?)?;
        if ron {
            fs::write(path.with_extension("mat.ron"), material.write(true)?)?;
        }
    }

    // Last, so the scene never names a file that isn't there yet.
    scene_data.write(&mut BufWriter::new(File::create(scene_path)?), false)?;
    if ron {
        let ron_path = scene_path.with_extension("scene.ron");
        scene_data.write(&mut BufWriter::new(File::create(ron_path)?), true)?;
    }
    Ok((
        bake.mesh_jobs.len(),
        bake.materials.len(),
        bake.texture_jobs.len(),
    ))
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

/// Bakes an OpenEXR file into an HDR texture at `path`.
pub fn bake_exr(source: &Path, path: &Path) -> Result<()> {
    let texture = TextureData::from_exr(source)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    texture.write(&mut BufWriter::new(File::create(path)?))
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
    let mut positions: Vec<Vec3> = reader
        .read_positions()
        .context("no positions")?
        .map(Vec3::from)
        .collect();
    // Missing UVs stay zero.
    let mut uvs: Vec<[f32; 2]> = match reader.read_tex_coords(0) {
        Some(uvs) => uvs.into_f32().collect(),
        None => vec![[0.0; 2]; positions.len()],
    };
    let mut indices: Vec<u32> = match reader.read_indices() {
        Some(indices) => indices.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    indices.truncate(indices.len() / 3 * 3);
    ensure!(
        indices.iter().all(|&i| (i as usize) < positions.len()),
        "an index is out of range"
    );
    ensure!(uvs.len() == positions.len(), "not one UV per position");
    let normals: Vec<Vec3> = match reader.read_normals() {
        Some(normals) => normals.map(Vec3::from).collect(),
        // Flat normals: every triangle gets vertices of its own.
        None => {
            positions = indices.iter().map(|&i| positions[i as usize]).collect();
            uvs = indices.iter().map(|&i| uvs[i as usize]).collect();
            indices = (0..positions.len() as u32).collect();
            positions
                .chunks_exact(3)
                .flat_map(|t| [(t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero(); 3])
                .collect()
        }
    };
    ensure!(
        normals.len() == positions.len(),
        "not one normal per position"
    );

    MeshletMesh::new(
        &indices,
        &positions,
        bytemuck::cast_slice(&normals),
        uvs.as_flattened(),
    )
    .write(&mut BufWriter::new(File::create(path)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::file::RonScene;
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

    /// Writes a glTF file with a parent node, a child that draws a triangle, a node that
    /// draws a two-primitive mesh, and a non-indexed triangle without normals.
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
            "scenes": [{"nodes": [0, 2, 3]}],
            "nodes": [
                {"name": "parent", "translation": [0.0, 5.0, 0.0], "children": [1]},
                {"mesh": 0, "name": "left", "translation": [2.0, 0.0, 0.0]},
                {"mesh": 1, "name": "pair"},
                {"mesh": 2}
            ],
            "materials": [{"name": "red", "pbrMetallicRoughness": {"baseColorFactor": [1, 0, 0, 1]}}],
            "meshes": [
                {"name": "Tri/angle", "primitives": [
                    {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2, "material": 0}
                ]},
                {"name": "Two", "primitives": [
                    {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2, "material": 0},
                    {"attributes": {"POSITION": 0, "NORMAL": 1}, "indices": 2}
                ]},
                {"primitives": [{"attributes": {"POSITION": 0}}]}
            ],
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
    fn baking_keeps_the_node_tree() {
        AsyncComputeTaskPool::get_or_init(TaskPool::new);
        let dir = std::env::temp_dir().join(format!("core-bake-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("baked/tri/meshes")).unwrap();
        let source = triangle_gltf(&dir);
        let scene_path = dir.join("baked/tri.scene");
        // A file of an earlier bake that the scene no longer has.
        let stray = dir.join("baked/tri/meshes/old.mesh");
        fs::write(&stray, b"old").unwrap();

        assert_eq!(bake_gltf(&source, &scene_path, true).unwrap(), (4, 2, 0));
        assert!(!stray.exists());
        assert!(scene_path.exists());
        let scene: RonScene =
            ron::de::from_bytes(&fs::read(dir.join("baked/tri.scene.ron")).unwrap()).unwrap();
        for asset in &scene.assets {
            assert!(dir.join("baked").join(asset).exists(), "{asset}");
        }

        let name = |entity: usize| -> Option<String> {
            Some(
                scene.entities[entity]
                    .components
                    .get("Name")?
                    .into_rust()
                    .unwrap(),
            )
        };
        let transform = |entity: usize| -> Transform {
            scene.entities[entity].components["Transform"]
                .into_rust()
                .unwrap()
        };
        let instance = |entity: usize| -> Option<SavedInstance> {
            Some(
                scene.entities[entity]
                    .components
                    .get("Instance")?
                    .into_rust()
                    .unwrap(),
            )
        };
        let parents: Vec<_> = scene.entities.iter().map(|e| e.parent).collect();
        // parent, left, pair, pair's two primitives, the unnamed node.
        assert_eq!(parents, [None, Some(0), None, Some(2), Some(2), None]);
        assert_eq!(name(0).as_deref(), Some("parent"));
        assert_eq!(name(3).as_deref(), Some("Two.0"));
        assert_eq!(name(4).as_deref(), Some("Two.1"));
        assert_eq!(name(5), None);
        // Local transforms, not world ones.
        assert_eq!(transform(0).translation, Vec3::new(0.0, 5.0, 0.0));
        assert_eq!(transform(1).translation, Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(transform(3), Transform::IDENTITY);

        assert!(instance(0).is_none() && instance(2).is_none());
        let [left, first, second, flat] = [1, 3, 4, 5].map(|e| instance(e).unwrap());
        assert_eq!(
            scene.assets[left.mesh as usize],
            "tri/meshes/Tri_angle.mesh"
        );
        // The two primitives are meshes of their own, one with glTF's default material.
        assert_ne!(first.mesh, second.mesh);
        assert_eq!(left.material, first.material);
        assert_eq!(second.material, flat.material);
        assert_eq!(
            scene.assets[first.material as usize],
            "tri/materials/red.mat"
        );
        assert_eq!(
            scene.assets[second.material as usize],
            "tri/materials/default.mat"
        );

        let red = fs::read(dir.join("baked/tri/materials/red.mat")).unwrap();
        let red = MaterialFile::read(&red, false).unwrap();
        assert_eq!(red.color, Vec4::new(1.0, 0.0, 0.0, 1.0));
        assert_eq!(red.color_texture, None);
        let default = fs::read(dir.join("baked/tri/materials/default.mat.ron")).unwrap();
        let default = MaterialFile::read(&default, true).unwrap();
        assert_eq!((default.color, default.metalic_factor), (Vec4::ONE, 1.0));
        fs::remove_dir_all(dir).unwrap();
    }
}
