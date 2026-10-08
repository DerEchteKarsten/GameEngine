//! Asset plugin registering the loaders of the scene, material, mesh and texture files and the system binding textures to their slots.

use anyhow::{Ok, Result};
use bevy::{
    asset::{AssetEventSystems, AssetLoader, AsyncReadExt, LoadContext, io::Reader},
    prelude::*,
};
use bytemuck::{Zeroable, bytes_of, bytes_of_mut};
use lava::{
    bindings::{BvhNode, Meshlet, Vertex},
    buffer::Buffer,
};

use crate::{
    assets::{
        material::{GpuMaterial, MaterialLoader},
        mesh::{GpuMesh, MeshHeader, aabb_ptr_offset, aabb_ptr_set_offset, bvh_node_child_counts},
        texture::{
            GpuTexture, PREVIEW_BYTES, TextureHeader, TextureKind, read_image, write_texture_slots,
        },
        util::read_compressed,
    },
    scene::file::{Scene, SceneLoader},
};

pub mod bake;
pub mod material;
pub mod mesh;
pub mod texture;
pub mod util;

pub struct MeshAssets;
impl Plugin for MeshAssets {
    fn build(&self, app: &mut App) {
        app.init_asset_loader::<SceneLoader>()
            .init_asset_loader::<MaterialLoader>()
            .init_asset_loader::<GpuMeshLoader>()
            .init_asset_loader::<TextureLoader>()
            .init_asset::<Scene>()
            .init_asset::<GpuMesh>()
            .init_asset::<GpuMaterial>()
            .init_asset::<GpuTexture>()
            .add_systems(Last, write_texture_slots.after(AssetEventSystems));
    }
}

pub const MESH_EXTENSION: &str = "mesh";
pub const SCENE_EXTENSION: &str = "scene";
pub const TEXTURE_EXTENSION: &str = "tex";
const ZSTD_LEVEL: i32 = 12;

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
            TextureKind::Hdr => GpuTexture::Hdr(read_image(&header, reader).await?),
        })
    }

    fn extensions(&self) -> &[&str] {
        &[TEXTURE_EXTENSION]
    }
}
