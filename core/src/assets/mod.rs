use core::slice;
use std::{
    collections::HashMap, f16, fmt::Debug, future::IntoFuture, mem::offset_of,
    sync::atomic::AtomicU32,
};

use anyhow::{Ok, Result};
use ash::{Instance, vk::QCOM_FILTER_CUBIC_CLAMP_NAME};
use bevy_app::Plugin;
use bevy_asset::{
    AssetApp, AssetLoader, AsyncReadExt, AsyncWriteExt, LoadContext,
    processor::LoadTransformAndSave,
    saver::AssetSaver,
    transformer::{AssetTransformer, TransformedAsset},
};
use bevy_reflect::TypePath;
use bincode::{config::Configuration, de::read::Reader, enc::write::Writer};
use dgfsdk_rs::wrappers::bake_default;
use glam::vec3;

use crate::world::DrawTask;

pub struct MeshAssets;
impl Plugin for MeshAssets {
    fn build(&self, app: &mut bevy_app::App) {
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

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Material {
    metalic_factor: f16,
    roughness_factor: f16,
    color: [f16; 3],
    texture_offset: u16,
}

impl bincode::Encode for Material {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> std::result::Result<(), bincode::error::EncodeError> {
        encoder.writer().write(&self.metalic_factor.to_be_bytes())?;
        encoder
            .writer()
            .write(&self.roughness_factor.to_be_bytes())?;
        for i in 0..3 {
            encoder.writer().write(&self.color[i].to_be_bytes())?;
        }
        bincode::Encode::encode(&self.texture_offset, encoder)?;
        std::result::Result::Ok(())
    }
}

impl<Context> bincode::Decode<Context> for Material {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let mut metalic_factor_buf = [0u8; 2];
        decoder.reader().read(&mut metalic_factor_buf)?;
        let mut roughness_buf = [0u8; 2];
        decoder.reader().read(&mut roughness_buf)?;
        let mut color = [0f16; 3];
        let mut color_buf = [0u8; 2];
        for i in 0..3 {
            decoder.reader().read(&mut color_buf)?;
            color[i] = f16::from_be_bytes(color_buf);
        }
        let texture_offset = bincode::Decode::decode(decoder)?;
        std::result::Result::Ok(Self {
            metalic_factor: f16::from_be_bytes(metalic_factor_buf),
            roughness_factor: f16::from_be_bytes(metalic_factor_buf),
            color,
            texture_offset,
        })
    }
}

impl<'a, Context> bincode::BorrowDecode<'a, Context> for Material {
    fn borrow_decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let mut metalic_factor_buf = [0u8; 2];
        decoder.reader().read(&mut metalic_factor_buf)?;
        let mut roughness_buf = [0u8; 2];
        decoder.reader().read(&mut roughness_buf)?;
        let mut color = [0f16; 3];
        let mut color_buf = [0u8; 2];
        for i in 0..3 {
            decoder.reader().read(&mut color_buf)?;
            color[i] = f16::from_be_bytes(color_buf);
        }
        let texture_offset = bincode::Decode::decode(decoder)?;
        std::result::Result::Ok(Self {
            metalic_factor: f16::from_be_bytes(metalic_factor_buf),
            roughness_factor: f16::from_be_bytes(metalic_factor_buf),
            color,
            texture_offset,
        })
    }
}

#[derive(bevy_asset::Asset, TypePath)]
pub struct Mesh {
    pub mesh: SavedMesh,
    pub uploaded: bool,
}

#[derive(bincode::Encode, bincode::Decode, Clone)]
pub struct SavedMesh {
    pub draw_tasks: Vec<DrawTask>,
    pub dgf_blocks: Vec<u8>,
    pub materials: Vec<Material>,
    pub instances: Vec<[f32; 16]>,
    pub num_geometries: u32,
}

const CONFIG: Configuration<bincode::config::BigEndian> = bincode::config::standard()
    .with_variable_int_encoding()
    .with_big_endian()
    .with_no_limit();

pub struct MeshLoader;
impl AssetLoader for MeshLoader {
    type Asset = Mesh;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn bevy_asset::io::Reader,
        settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> std::result::Result<Self::Asset, Self::Error> {
        // let mesh: SavedMesh = bincode::decode_from_reader(MReader(reader), CONFIG)?;
        let mut slice = vec![];
        reader.read_to_end(&mut slice).await.unwrap();
        let (mesh, _): (SavedMesh, usize) = bincode::decode_from_slice(&slice, CONFIG).unwrap();
        Ok(Mesh {
            mesh,
            uploaded: false,
        })
    }
    fn extensions(&self) -> &[&str] {
        &["mesh"]
    }
}

#[derive(bevy_asset::Asset, TypePath)]
pub struct GltfMesh {
    document: gltf::Document,
    buffers: Vec<gltf::buffer::Data>,
    images: Vec<gltf::image::Data>,
}

pub struct GltfMeshLoader;
impl AssetLoader for GltfMeshLoader {
    type Asset = GltfMesh;
    type Error = gltf::Error;
    type Settings = ();

    async fn load(
        &self,
        reader: &mut dyn bevy_asset::io::Reader,
        settings: &(),
        load_context: &mut bevy_asset::LoadContext<'_>,
    ) -> gltf::Result<Self::Asset> {
        let mut file_buf = Vec::new();
        reader.read_to_end(&mut file_buf).await?;
        let (document, buffers, images) = gltf::import_slice(file_buf)?;
        gltf::Result::Ok(GltfMesh {
            document,
            buffers,
            images,
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

struct MeshTransformer;
impl AssetTransformer for MeshTransformer {
    type AssetInput = GltfMesh;
    type AssetOutput = Mesh;
    type Error = anyhow::Error;
    type Settings = ();
    async fn transform<'a>(
        &'a self,
        asset: bevy_asset::transformer::TransformedAsset<Self::AssetInput>,
        settings: &'a Self::Settings,
    ) -> std::result::Result<
        bevy_asset::transformer::TransformedAsset<Self::AssetOutput>,
        Self::Error,
    > {
        let mut materials = vec![];
        let mut mesh_primitve_index_to_dgf_index = HashMap::new();
        let mut dgf_blocks = vec![];
        let mut geom_id = 0;
        for mesh in asset.document.meshes() {
            for primitive in mesh.primitives() {
                let index = (mesh.index(), primitive.index());
                let reader = primitive.reader(|buffer| Some(&asset.buffers[buffer.index()]));

                // let normals = reader.read_normals().unwrap().collect::<Vec<_>>();

                // let uvs = reader
                //     .read_tex_coords(0)
                //     .map(|reader| reader.into_f32().collect::<Vec<_>>());
                let vertex_positions = reader
                    .read_positions()
                    .unwrap()
                    .flat_map(|v| v)
                    .collect::<Vec<f32>>();

                let indices = reader
                    .read_indices()
                    .unwrap()
                    .into_u32()
                    .collect::<Vec<u32>>();

                let material = primitive.material();
                let pbr = material.pbr_metallic_roughness();
                materials.push(Material {
                    color: [
                        pbr.base_color_factor()[0] as f16,
                        pbr.base_color_factor()[1] as f16,
                        pbr.base_color_factor()[2] as f16,
                    ],
                    metalic_factor: pbr.metallic_factor() as f16,
                    roughness_factor: pbr.roughness_factor() as f16,
                    texture_offset: pbr
                        .base_color_texture()
                        .map(|v| v.texture().index() as u16)
                        .unwrap_or(!0u16),
                });
                geom_id += 1;
                mesh_primitve_index_to_dgf_index.insert(index, (dgf_blocks.len(), geom_id));

                let mut baked = bake_default(&vertex_positions, &indices).unwrap();
                dgf_blocks.append(&mut baked.dgf_blocks);
            }
        }

        let mut instance_infos = vec![];
        let mut instances = vec![];
        for node in asset.document.nodes().filter(|n| n.mesh().is_some()) {
            let transform = node.transform().matrix();
            let gltf_mesh = node.mesh().unwrap();
            for primitive in gltf_mesh.primitives() {
                let index = (gltf_mesh.index(), primitive.index());
                let (dgf_offset, geom_id) = mesh_primitve_index_to_dgf_index
                    .get(&index)
                    .unwrap()
                    .clone();
                let material = primitive.material().index().unwrap_or(0);

                instance_infos.push(DrawTask {
                    block_id: dgf_offset as u32,
                    material_id: material as u32,
                    instance_id: instances.len() as u32,
                    geometry_id: geom_id,
                });
                instances.push([
                    transform[0][0],
                    transform[1][0],
                    transform[2][0],
                    transform[3][0],
                    transform[0][1],
                    transform[1][1],
                    transform[2][1],
                    transform[3][1],
                    transform[0][2],
                    transform[1][2],
                    transform[2][2],
                    transform[3][2],
                    transform[0][3],
                    transform[1][3],
                    transform[2][3],
                    transform[3][3],
                ]);
            }
        }

        let mesh = Mesh {
            mesh: SavedMesh {
                draw_tasks: instance_infos,
                dgf_blocks,
                materials,
                num_geometries: geom_id,
                instances,
            },
            uploaded: false,
        };

        let asset = asset.replace_asset(mesh);
        return Ok(asset);
    }
}

struct MeshSaver;
impl AssetSaver for MeshSaver {
    type Asset = Mesh;
    type Error = anyhow::Error;
    type Settings = ();
    type OutputLoader = MeshLoader;
    async fn save(
        &self,
        writer: &mut bevy_asset::io::Writer,
        asset: bevy_asset::saver::SavedAsset<'_, Self::Asset>,
        settings: &Self::Settings,
    ) -> std::result::Result<<Self::OutputLoader as AssetLoader>::Settings, Self::Error> {
        let mesh = asset.mesh.clone();
        let mut slice = vec![
            0u8;
            size_of_val(&mesh)
                + mesh.dgf_blocks.len() * size_of::<u8>()
                + mesh.draw_tasks.len() * size_of::<DrawTask>()
                + mesh.materials.len() * size_of::<Material>()
                + mesh.instances.len() * size_of::<[f32; 16]>()
        ];
        bincode::encode_into_slice(&mesh, &mut slice, CONFIG)?;
        writer.write_all(&slice).await?;
        return Ok(());
    }
}
