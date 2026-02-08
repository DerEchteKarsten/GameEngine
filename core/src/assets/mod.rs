use core::slice;
use std::{collections::HashMap, mem::MaybeUninit};

use anyhow::{Ok, Result};
use bevy::{
    asset::{
        AssetLoader, AsyncReadExt, AsyncWriteExt, LoadContext, processor::LoadTransformAndSave,
        saver::AssetSaver, transformer::AssetTransformer,
    },
    prelude::*,
};
use bytemuck::Pod;
use glam::{Mat4, Vec3};
use std::sync::Arc;

use crate::{
    assets::{material::Material, mesh::MeshletMesh},
    bindings::Aabb,
};

pub mod material;
pub mod mesh;

pub struct MeshAssets;
impl Plugin for MeshAssets {
    fn build(&self, app: &mut App) {
        app
            .register_asset_processor::<LoadTransformAndSave<GltfMeshLoader, MeshTransformer, MeshSaver>>(
                LoadTransformAndSave::new(MeshTransformer, MeshSaver),
            )
            .set_default_asset_processor::<LoadTransformAndSave<GltfMeshLoader, MeshTransformer, MeshSaver>>("glb")
            .register_asset_loader(GltfMeshLoader)
            .register_asset_loader(MeshLoader)
            .init_asset::<Mesh>()
            .init_asset::<GltfMesh>();
    }
}

#[derive(Asset, TypePath)]
pub struct Mesh {
    pub meshes: Arc<[MeshletMesh]>,
    pub instance_transforms: Arc<[Mat4]>,
    pub materials: Arc<[Material]>,
    pub instance_materials: Arc<[u32]>,
    pub instance_mesh: Arc<[u32]>,
}

#[derive(Asset, TypePath)]
pub struct GltfMesh {
    document: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
    _images: Vec<gltf::image::Data>,
}

#[derive(TypePath)]
pub struct GltfMeshLoader;
impl AssetLoader for GltfMeshLoader {
    type Asset = GltfMesh;
    type Error = gltf::Error;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn bevy::asset::io::Reader,
        _settings: &(),
        _load_context: &mut bevy::asset::LoadContext<'_>,
    ) -> gltf::Result<Self::Asset> {
        let mut file_buf = Vec::new();
        reader.read_to_end(&mut file_buf).await?;
        let (document, buffers, images) = gltf::import_slice(file_buf)?;
        gltf::Result::Ok(GltfMesh {
            document,
            buffers,
            _images: images,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["glb"]
    }
}

#[inline(always)]
pub fn typed_to_bytes<T: Sized>(typed: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(typed.as_ptr().cast(), std::mem::size_of_val(typed)) }
}
#[derive(TypePath)]
struct MeshTransformer;
impl AssetTransformer for MeshTransformer {
    type AssetInput = GltfMesh;
    type AssetOutput = Mesh;
    type Error = anyhow::Error;
    type Settings = ();
    async fn transform<'a>(
        &'a self,
        asset: bevy::asset::transformer::TransformedAsset<Self::AssetInput>,
        _settings: &'a Self::Settings,
    ) -> std::result::Result<
        bevy::asset::transformer::TransformedAsset<Self::AssetOutput>,
        Self::Error,
    > {
        let mut remap = HashMap::new();
        let mut meshes = Vec::new();
        for mesh in asset.document.meshes() {
            for primitive in mesh.primitives() {
                let index = (mesh.index(), primitive.index());
                if remap.get(&index).is_some() {
                    continue;
                }

                let reader = primitive.reader(|buffer| Some(&asset.buffers[buffer.index()]));
                let Some(normals) = reader.read_normals() else {
                    continue;
                };
                let normals = normals.flatten().collect::<Vec<_>>();

                let verticies = reader
                    .read_positions()
                    .unwrap()
                    .map(|e| Vec3::from(e))
                    .collect::<Vec<_>>();

                let uvs = reader
                    .read_tex_coords(0)
                    .map(|e| e.into_f32().flatten().collect::<Vec<_>>());
                let uvs = uvs.unwrap_or(vec![0.0; verticies.len() * 2]);
                let indicies = reader
                    .read_indices()
                    .unwrap()
                    .into_u32()
                    .collect::<Vec<_>>();

                remap.insert(index, meshes.len() as u32);

                let mesh = MeshletMesh::new(&indicies, &verticies, &normals, &uvs);
                meshes.push(mesh);
            }
        }

        let mut instance_transforms = vec![];
        let mut materials = vec![];
        let mut instance_materials = vec![];
        let mut instance_mesh = vec![];
        for node in asset.document.nodes().filter(|n| n.mesh().is_some()) {
            let transform = node.transform().matrix();
            let gltf_mesh = node.mesh().unwrap();

            for primitive in gltf_mesh.primitives() {
                let material = materials.len();
                let pmaterial = primitive.material();
                let pbr = pmaterial.pbr_metallic_roughness();
                materials.push(Material {
                    color: [
                        pbr.base_color_factor()[0],
                        pbr.base_color_factor()[1],
                        pbr.base_color_factor()[2],
                    ],
                    metalic_factor: pbr.metallic_factor(),
                    roughness_factor: pbr.roughness_factor(),
                    texture_offset: pbr
                        .base_color_texture()
                        .map(|v| v.texture().index() as u32)
                        .unwrap_or(!0u32),
                });

                let Some(mesh) = remap.get(&(gltf_mesh.index(), primitive.index())) else {
                    continue;
                };

                instance_materials.push(material as u32);
                instance_mesh.push(mesh.clone());
                instance_transforms.push(Mat4::from_cols_array_2d(&transform));
            }
        }

        let mesh = Mesh {
            instance_transforms: instance_transforms.into(),
            instance_materials: instance_materials.into(),
            instance_mesh: instance_mesh.into(),
            materials: materials.into(),
            meshes: meshes.into(),
        };

        let asset = asset.replace_asset(mesh);
        return Ok(asset);
    }
}

async fn write_slice<T: Pod>(field: &[T], writer: &mut bevy::asset::io::Writer) -> Result<()> {
    writer
        .write_all(&(field.len() as u64).to_le_bytes())
        .await?;
    writer.write_all(bytemuck::cast_slice(field)).await?;
    Ok(())
}
async fn read_u64(reader: &mut dyn bevy::asset::io::Reader) -> Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes).await?;
    Ok(u64::from_le_bytes(bytes))
}

async fn read_slice<T: Pod>(reader: &mut dyn bevy::asset::io::Reader) -> Result<Arc<[T]>> {
    let len = read_u64(reader).await? as usize;
    let mut data: Arc<[MaybeUninit<T>]> = Arc::new_uninit_slice(len);

    let buf = Arc::get_mut(&mut data).ok_or_else(|| anyhow::anyhow!("Arc unexpectedly shared"))?;
    let byte_buf: &mut [u8] = unsafe {
        slice::from_raw_parts_mut(
            buf.as_mut_ptr() as *mut u8,
            buf.len() * std::mem::size_of::<T>(),
        )
    };
    reader.read_exact(byte_buf).await?;

    Ok(unsafe { data.assume_init() })
}

#[derive(TypePath)]
struct MeshSaver;
impl AssetSaver for MeshSaver {
    type Asset = Mesh;
    type Error = anyhow::Error;
    type Settings = ();
    type OutputLoader = MeshLoader;
    async fn save(
        &self,
        writer: &mut bevy::asset::io::Writer,
        asset: bevy::asset::saver::SavedAsset<'_, Self::Asset>,
        _settings: &Self::Settings,
    ) -> std::result::Result<<Self::OutputLoader as AssetLoader>::Settings, Self::Error> {
        let mesh = asset.get();

        write_slice(&mesh.instance_transforms, writer).await?;
        write_slice(&mesh.materials, writer).await?;
        write_slice(&mesh.instance_mesh, writer).await?;
        write_slice(&mesh.instance_materials, writer).await?;

        writer
            .write_all(&(mesh.meshes.len() as u64).to_le_bytes())
            .await?;

        for mesh in mesh.meshes.iter() {
            writer.write_all(&(mesh.bvh_depth.to_le_bytes())).await?;
            writer
                .write_all(&bytemuck::cast_slice(&[mesh.aabb]))
                .await?;

            write_slice(&mesh.vertices, writer).await?;
            write_slice(&mesh.indices, writer).await?;
            write_slice(&mesh.meshlets, writer).await?;
            write_slice(&mesh.cull_data, writer).await?;
            write_slice(&mesh.bvh, writer).await?;
        }

        return Ok(());
    }
}

#[derive(TypePath)]
pub struct MeshLoader;
impl AssetLoader for MeshLoader {
    type Asset = Mesh;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn bevy::asset::io::Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> std::result::Result<Self::Asset, Self::Error> {
        let instance_transforms = read_slice(reader).await?;
        let materials = read_slice(reader).await?;
        let instance_materials = read_slice(reader).await?;
        let instance_meshlet_offsets = read_slice(reader).await?;
        let num_meshes = read_u64(reader).await?;

        let mut meshes = Vec::new();
        for _ in 0..num_meshes {
            let mut buffer = [0u8; size_of::<Aabb>()];
            reader.read(&mut buffer).await?;
            let aabb = bytemuck::cast_slice(&buffer)[0];
            reader.read(&mut buffer[0..4]).await?;
            let bvh_depth = u32::from_le_bytes(buffer[0..4].try_into().unwrap());
            let mesh = MeshletMesh {
                bvh_root_node_index: !0u32,
                aabb,
                bvh_depth,
                vertices: read_slice(reader).await.unwrap(),
                indices: read_slice(reader).await.unwrap(),
                meshlets: read_slice(reader).await.unwrap(),
                cull_data: read_slice(reader).await.unwrap(),
                bvh: read_slice(reader).await.unwrap(),
            };
            meshes.push(mesh);
        }

        Ok(Mesh {
            instance_transforms,
            instance_materials,
            instance_mesh: instance_meshlet_offsets,
            materials,
            meshes: meshes.into(),
        })
    }
    fn extensions(&self) -> &[&str] {
        &["mesh"]
    }
}
