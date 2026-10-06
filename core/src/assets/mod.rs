//! Asset plugin registering the loaders of the baked scene, mesh and texture files, plus binary read/write helpers.

use anyhow::{Ok, Result};
use bevy::{
    asset::{AssetLoader, AsyncReadExt, LoadContext, io::Reader},
    prelude::*,
};
use bytemuck::{Pod, Zeroable, bytes_of, bytes_of_mut};
use lava::{
    bindings::{BvhNode, Meshlet, Vertex},
    bindless::NULL_HANDLE,
    buffer::Buffer,
};

use crate::assets::{
    mesh::{
        GpuMesh, MaterialSet, MeshHeader, Scene, SceneFile, aabb_ptr_offset, aabb_ptr_set_offset,
        bvh_node_child_counts,
    },
    texture::{GpuTexture, PREVIEW_BYTES, TextureHeader, TextureKind, read_image},
    util::read_compressed,
};

pub mod bake;
pub mod mesh;
pub mod texture;
pub mod util;

pub struct MeshAssets;
impl Plugin for MeshAssets {
    fn build(&self, app: &mut App) {
        app.init_asset_loader::<SceneLoader>()
            .init_asset_loader::<GpuMeshLoader>()
            .init_asset_loader::<TextureLoader>()
            .init_asset::<Scene>()
            .init_asset::<GpuMesh>()
            .init_asset::<MaterialSet>()
            .init_asset::<GpuTexture>();
    }
}

pub const MESH_EXTENSION: &str = "mesh";
pub const SCENE_EXTENSION: &str = "scene";
pub const TEXTURE_EXTENSION: &str = "tex";
const ZSTD_LEVEL: i32 = 12;

#[derive(TypePath, Default)]
pub struct SceneLoader;
impl AssetLoader for SceneLoader {
    type Asset = Scene;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<Scene> {
        let mut scene = SceneFile::read(reader).await?;

        // The meshes and textures load on their own, and are shared with everything else
        // that loads the same files.
        let mut textures: Vec<Handle<GpuTexture>> = Vec::with_capacity(scene.textures.len());
        for path in &scene.textures {
            let path = load_context.path().resolve_embed_str(path)?;
            textures.push(load_context.load(path));
        }
        let mut meshes: Vec<Handle<GpuMesh>> = Vec::with_capacity(scene.meshes.len());
        for path in &scene.meshes {
            let path = load_context.path().resolve_embed_str(path)?;
            meshes.push(load_context.load(path));
        }

        // The materials of the file index its textures, those of the set get bindless indices.
        let material_textures = scene
            .materials
            .iter_mut()
            .map(|material| {
                [
                    &mut material.color_texture,
                    &mut material.metallic_roughness_texture,
                    &mut material.normal_texture,
                    &mut material.occlusion_texture,
                    &mut material.emissive_texture,
                ]
                .map(|index| {
                    let texture = std::mem::replace(index, NULL_HANDLE);
                    (texture != NULL_HANDLE).then(|| textures[texture as usize].clone())
                })
            })
            .collect();
        let materials = load_context.add_labeled_asset(
            "materials".to_string(),
            MaterialSet::new(&scene.materials, material_textures),
        );

        Ok(Scene {
            meshes,
            materials,
            instance_transforms: scene.instance_transforms,
            instance_materials: scene.instance_materials,
            instance_mesh: scene.instance_mesh,
            instance_names: scene.instance_names,
        })
    }

    fn extensions(&self) -> &[&str] {
        &[SCENE_EXTENSION]
    }
}

#[derive(TypePath, Default)]
pub struct GpuMeshLoader;
impl AssetLoader for GpuMeshLoader {
    type Asset = GpuMesh;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<GpuMesh> {
        let mut header = MeshHeader::zeroed();
        reader.read_exact(bytes_of_mut(&mut header)).await?;
        let mut data = read_compressed(reader).await?;

        let buffer = Buffer::new(data.len(), true)?;
        let address = buffer.address;

        let bvh_node_count = header.meshlet_offset as usize / size_of::<BvhNode>();
        for i in 0..bvh_node_count {
            // `data` is a byte buffer with no alignment guarantee, so patch a copy.
            let bytes = &mut data[i * size_of::<BvhNode>()..(i + 1) * size_of::<BvhNode>()];
            let mut node: BvhNode = bytemuck::pod_read_unaligned(bytes);
            for child_index in 0..node.aabb_and_offsets.len() {
                if bvh_node_child_counts(&node, child_index) == u8::MAX {
                    let aabb = &mut node.aabb_and_offsets[child_index];
                    let offset = aabb_ptr_offset(aabb);
                    aabb_ptr_set_offset(aabb, offset * size_of::<BvhNode>() as u64 + address);
                }
            }
            bytes.copy_from_slice(bytes_of(&node));
        }

        let meshlet_count =
            (header.cull_data_offset - header.meshlet_offset) as usize / size_of::<Meshlet>();
        for i in 0..meshlet_count {
            let offset = header.meshlet_offset as usize + i * size_of::<Meshlet>();
            let bytes = &mut data[offset..offset + size_of::<Meshlet>()];
            let mut meshlet: Meshlet = bytemuck::pod_read_unaligned(bytes);
            meshlet.triangle_index = meshlet.triangle_index + header.index_offset as u64 + address;
            meshlet.vertex_index = meshlet.vertex_index * size_of::<Vertex>() as u64
                + header.vertex_offset as u64
                + address;
            bytes.copy_from_slice(bytes_of(&meshlet));
        }
        buffer.range(..).copy_from(&data);

        Ok(GpuMesh {
            header,
            buffer,
            colission_bvh: read_compressed(reader).await?,
        })
    }

    fn extensions(&self) -> &[&str] {
        &[MESH_EXTENSION]
    }
}

#[derive(TypePath, Default)]
pub struct TextureLoader;
impl AssetLoader for TextureLoader {
    type Asset = GpuTexture;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<GpuTexture> {
        let mut header = TextureHeader::zeroed();
        reader.read_exact(bytes_of_mut(&mut header)).await?;
        // The preview is for the asset browser.
        reader.read_exact(&mut vec![0u8; PREVIEW_BYTES]).await?;
        Ok(match TextureKind::from_raw(header.kind)? {
            TextureKind::Color => GpuTexture::Color(read_image(&header, reader).await?),
            TextureKind::Data => GpuTexture::Data(read_image(&header, reader).await?),
            TextureKind::Normal => GpuTexture::Normal(read_image(&header, reader).await?),
        })
    }

    fn extensions(&self) -> &[&str] {
        &[TEXTURE_EXTENSION]
    }
}
