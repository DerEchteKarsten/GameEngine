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
use glam::{vec3, Mat4, Vec3};
use meshopt::VertexDataAdapter;

use crate::assets::material::Material;

pub mod material;

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


#[derive(Clone, Copy)]
#[repr(C)]
pub struct SavedMat4(pub glam::Mat4);

impl bincode::Encode for SavedMat4 {
    fn encode<E: bincode::enc::Encoder>(
        &self,
        encoder: &mut E,
    ) -> std::result::Result<(), bincode::error::EncodeError> {
        bincode::Encode::encode(&self.0.to_cols_array(), encoder)?;
        std::result::Result::Ok(())
    }
}


impl<Context> bincode::Decode<Context> for SavedMat4 {
    fn decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let array = bincode::Decode::decode(decoder)?;
        std::result::Result::Ok(Self(glam::Mat4::from_cols_array(&array)))
    }
}

impl<'a, Context> bincode::BorrowDecode<'a, Context> for SavedMat4 {
    fn borrow_decode<D: bincode::de::Decoder<Context = Context>>(
        decoder: &mut D,
    ) -> std::result::Result<Self, bincode::error::DecodeError> {
        let array = bincode::Decode::decode(decoder)?;
        std::result::Result::Ok(Self(glam::Mat4::from_cols_array(&array)))
    }
}

#[derive(Copy, Clone, bincode::Encode, bincode::Decode)]
#[repr(C)]
pub struct Vertex {
    position: [f32; 3],
    pad: f32,
}

#[derive(Copy, Clone, bincode::Encode, bincode::Decode)]
#[repr(C)]
pub struct MeshletBoundingSpheres {
    pub self_culling: MeshletBoundingSphere,
    pub self_lod: MeshletBoundingSphere,
    pub parent_lod: MeshletBoundingSphere,
}

#[derive(Copy, Clone, bincode::Encode, bincode::Decode)]
#[repr(C)]
pub struct MeshletBoundingSphere {
    pub center: [f32; 3],
    pub radius: f32,
}

#[derive(Copy, Clone, bincode::Encode, bincode::Decode)]
#[repr(C)]
pub struct Meshlet {
    vertex_count: u32,
    vertex_index: u32,
    triangle_count: u32,
    triangle_index: u32,
}

#[derive(bincode::Encode, bincode::Decode, Clone, bevy_asset::Asset, TypePath)]
pub struct Mesh {
    pub uploaded: bool,
    pub vertices: Vec<Vertex>,
    pub indicies: Vec<u8>,
    pub meshlets: Vec<Meshlet>,
    pub materials: Vec<Material>,

    pub cull_data: Vec<MeshletBoundingSpheres>,
    pub aabb: Aabb,
    pub bvh_depth
}

#[derive(bincode::Encode, bincode::Decode, Clone)]
struct InstanceRange {
    meshlet_index: u32,
    meshlet_count: u32,
}

pub const CONFIG: Configuration<bincode::config::BigEndian> = bincode::config::standard()
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
        let mut buff = vec![];
        reader.read_to_end(&mut buff).await.unwrap();
        let (mut mesh, size) : (Mesh, usize) = bincode::decode_from_slice(&buff, CONFIG).unwrap();
        mesh.uploaded = false;
        Ok(mesh)
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


fn mes

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
        let mut vertices = vec![];
        let mut indicies = vec![];
        let mut materials = vec![];
        let mut meshlets = vec![];

        let mut remap = HashMap::new();
        for mesh in asset.document.meshes() {
            for primitive in mesh.primitives() {
                let index = (mesh.index(), primitive.index());

                if remap.get(&index).is_some() {
                    continue;
                }

                let reader = primitive.reader(|buffer| Some(&asset.buffers[buffer.index()]));

                let normals = reader
                    .read_normals()
                    .unwrap()
                    .collect::<Vec<_>>();

                let uvs = reader
                    .read_tex_coords(0)
                    .map(|reader| reader.into_f32().collect::<Vec<_>>());
                let mut pverticies = vec![];
                reader.read_positions().unwrap().enumerate().for_each(|(index, p)| {
                    let n = normals[index];    
                    let t = uvs.as_ref().map_or([0.0, 0.0], |uvs| uvs[index]);

                    pverticies.push(Vertex { position: p, pad: 0.0 });
                });

                let mut pindicies = reader.read_indices().unwrap().into_u32().collect::<Vec<_>>();
                let material = materials.len();
                let pmaterial = primitive.material();
                let pbr = pmaterial.pbr_metallic_roughness();
                materials.push(Material {
                    color: [pbr.base_color_factor()[0] as f16, pbr.base_color_factor()[1] as f16, pbr.base_color_factor()[2] as f16],
                    metalic_factor: pbr.metallic_factor() as f16,
                    roughness_factor: pbr.roughness_factor() as f16,
                    texture_offset: pbr.base_color_texture().map(|v| { v.texture().index() as u16 }).unwrap_or(!0u16),
                });

                let vertex_reader = VertexDataAdapter::new(
                    typed_to_bytes(&pverticies),
                    std::mem::size_of::<Vertex>(),
                    offset_of!(Vertex, position),
                ).unwrap();
                let mut pmeshlets = meshopt::build_meshlets(&pindicies, &vertex_reader, 64, 124, 0.0);
                remap.insert(index, InstanceRange {
                    meshlet_index: meshlets.len() as u32,
                    meshlet_count: pmeshlets.len() as u32
                });
                meshlets.extend(pmeshlets.meshlets.iter().map(|m| Meshlet {
                    triangle_count: m.triangle_count,
                    triangle_index: m.triangle_offset + indicies.len() as u32,
                    vertex_count: m.vertex_count,
                    vertex_index: m.vertex_offset + vertices.len() as u32,
                }));
                let mut meshlet_vertecies = pmeshlets.vertices.iter().map(|i| {pverticies[*i as usize].clone()}).collect::<Vec<Vertex>>(); 
                
                indicies.append(&mut pmeshlets.triangles);
                vertices.append(&mut meshlet_vertecies);
            }         
        }
        
        let mut instance_transforms = vec![];
        let mut instance_ranges = vec![];
        for node in asset.document.nodes().filter(|n| n.mesh().is_some()) {
            let transform = node.transform().matrix();
            let gltf_mesh = node.mesh().unwrap();

            for primitive in gltf_mesh.primitives() {
                let index = (gltf_mesh.index(), primitive.index());
                instance_ranges.push(remap.get(&index).unwrap().clone());
                instance_transforms.push(SavedMat4(Mat4::from_cols_array_2d(&transform)));
            }
        }

        let mesh = Mesh {
            indicies,
            materials,
            meshlets,
            vertices,
            instance_ranges,
            instance_transforms,
            cull_data: vec![],
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
        let mesh = asset.get();
        let bytes = bincode::encode_to_vec(mesh, CONFIG)?;

        writer.write_all(&bytes).await?;
        return Ok(());
    }
}
