use core::slice;
use std::{
    alloc::Layout,
    collections::HashMap,
    mem::{ManuallyDrop, MaybeUninit},
};

use anyhow::{Ok, Result};
use bevy::{
    asset::{
        AssetLoader, AsyncReadExt, AsyncWriteExt, LoadContext, processor::LoadTransformAndSave,
        saver::AssetSaver, transformer::AssetTransformer,
    },
    prelude::*,
};
use bytemuck::{Pod, Zeroable, bytes_of, bytes_of_mut, try_cast_vec};
use futures::future::join_all;
use glam::{Mat4, Vec3};
use std::sync::Arc;
use tracing_log::log;

use crate::{
    assets::{material::Material, mesh::MeshletMesh},
    bindings::{AabbError, BvhNode, CullData, Meshlet, Vertex},
    physics,
    render::world::UploadQueue,
};

use lava::{
    buffer::{Buffer, slice::BufferSlice},
    state::Ctx,
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
            .init_asset::<Scene>()
            .init_asset::<GltfMesh>()
            .init_asset::<GpuMeshletMesh>();
    }
}

#[derive(Pod, Zeroable, Clone, Copy, Debug)]
#[repr(C)]
pub struct MeshHeader {
    pub meshlet_offset: u32,
    pub vertex_offset: u32,
    pub index_offset: u32,
    pub cull_data_offset: u32,
    pub colission_bvh_root_node: u32,
    pub aabb: Aabb,
}

#[derive(Pod, Zeroable, Clone, Copy, Debug)]
#[repr(C)]
pub struct Aabb {
    pub center: [f32; 3],
    pub half_extend: [f32; 3],
}

#[derive(TypePath, Asset)]
pub struct GpuMeshletMesh {
    pub header: MeshHeader,
    pub buffer: Buffer<u8>,
    pub colission_bvh: Vec<u8>,
}

#[derive(Asset, TypePath)]
pub struct Scene {
    pub meshes: Vec<Handle<GpuMeshletMesh>>,
    pub instance_transforms: Vec<Mat4>,
    pub materials: Vec<Material>,
    pub instance_materials: Vec<u32>,
    pub instance_mesh: Vec<u32>,
}

#[derive(Asset, TypePath)]
pub struct FileScene {
    pub meshes: Vec<MeshletMesh>,
    pub instance_transforms: Vec<Mat4>,
    pub materials: Vec<Material>,
    pub instance_materials: Vec<u32>,
    pub instance_mesh: Vec<u32>,
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

#[derive(TypePath)]
struct MeshTransformer;
impl AssetTransformer for MeshTransformer {
    type AssetInput = GltfMesh;
    type AssetOutput = FileScene;
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

                assert_eq!(verticies.len() * 3, normals.len());

                let uvs = reader
                    .read_tex_coords(0)
                    .map(|e| e.into_f32().flatten().collect::<Vec<_>>());
                let uvs = uvs.unwrap_or(vec![0.0; verticies.len() * 2]);
                assert_eq!(verticies.len() * 2, uvs.len());

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

        let mesh = FileScene {
            instance_transforms: instance_transforms,
            instance_materials: instance_materials,
            instance_mesh: instance_mesh,
            materials: materials,
            meshes: meshes,
        };

        let asset = asset.replace_asset(mesh);
        return Ok(asset);
    }
}

async fn write_slice<T: Pod>(field: &[T], writer: &mut bevy::asset::io::Writer) -> Result<()> {
    let len = field.len() as u64;
    writer.write_all(&len.to_le_bytes()).await?;
    let byte_slice = bytemuck::cast_slice(field);
    writer.write_all(&byte_slice).await?;
    Ok(())
}
async fn read_u64(reader: &mut dyn bevy::asset::io::Reader) -> Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes).await?;
    Ok(u64::from_le_bytes(bytes))
}

async fn read_slice<T: Pod>(
    reader: &mut dyn bevy::asset::io::Reader,
    alignment: Option<usize>,
) -> Result<Vec<T>> {
    let len = read_u64(reader).await?;
    read_len_slice(reader, len as usize, alignment).await
}

async fn read_len_slice<T: Pod>(
    reader: &mut dyn bevy::asset::io::Reader,
    len: usize,
    alignment: Option<usize>,
) -> Result<Vec<T>> {
    let slice = unsafe {
        let size = len * size_of::<T>();
        let align = alignment.unwrap_or(align_of::<T>());
        let layout = Layout::from_size_align(size, align).unwrap();
        let mem = std::alloc::alloc(layout);
        slice::from_raw_parts_mut(mem, size)
    };
    reader.read_exact(slice).await?;
    Ok(unsafe { Vec::from_raw_parts(slice.as_mut_ptr().cast::<T>(), len, len) })
}

#[derive(TypePath)]
struct MeshSaver;
impl AssetSaver for MeshSaver {
    type Asset = FileScene;
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
        let num_meshes = mesh.meshes.len() as u64;
        writer.write_all(&num_meshes.to_le_bytes()).await?;
        for mesh in mesh.meshes.iter() {
            writer.write_all(bytes_of(&mesh.header)).await?;
            write_slice(&mesh.data, writer).await?;
            write_slice(&mesh.colission_bvh, writer).await?;
        }

        return Ok(());
    }
}

fn check_vertex(v: Vertex) -> bool {
    v.position_and_uv1.xyz().length() < 10000.0
}

// fn verify_bvh(buffer: BufferSlice<u8>, node: BvhNode, num_meshlets: &mut usize) -> bool {
//     for i in 0..8 {
//         let offset = node.aabb_and_offsets[i].offset();
//         let offset = offset - buffer.base_address;
//         let child_count = (node.child_counts >> i * 8) & 0xff;
//         if child_count == 255 {
//             if offset > buffer.size {
//                 std::intrinsics::breakpoint();
//                 return false;
//             }
//             if node.lod_bounds[i].length() > 10000.0 {
//                 std::intrinsics::breakpoint();
//                 return false;
//             }
//             let child_node = buffer.range(offset as usize..).cast()[0];
//             if !verify_bvh(buffer, child_node, num_meshlets) {
//                 std::intrinsics::breakpoint();
//                 return false;
//             }
//         } else {
//             *num_meshlets -= child_count as usize;
//             for m in 0..child_count as usize {
//                 if (offset + (m * size_of::<Meshlet>()) as u64) > buffer.size {
//                     std::intrinsics::breakpoint();
//                     return false;
//                 }
//                 let meshlet = buffer
//                     .range(offset as usize + m * size_of::<Meshlet>()..)
//                     .cast::<Meshlet>()[0];
//                 let vertex_offset = meshlet.vertex_index - buffer.base_address;
//                 if vertex_offset > buffer.size {
//                     std::intrinsics::breakpoint();
//                     return false;
//                 }
//                 for v in 0..meshlet.vertex_count {
//                     let v_offset = vertex_offset + v as u64 * size_of::<Vertex>() as u64;
//                     if v_offset > buffer.size {
//                         std::intrinsics::breakpoint();
//                         return false;
//                     }
//                     let vertex = buffer.range(v_offset as usize..).cast::<Vertex>()[0];
//                     if !check_vertex(vertex) {
//                         std::intrinsics::breakpoint();
//                         return false;
//                     }
//                 }
//             }
//         }
//     }
//     true
// }

async fn read_range<'a>(
    reader: &mut dyn bevy::asset::io::Reader,
    data: &mut Option<Vec<u8>>,
    slice: BufferSlice<'a, u8>,
    start_offset: u32,
    end_offset: u32,
) -> Result<()> {
    let len = (end_offset - start_offset) as usize;
    if let Some(data) = data {
        let mut slice =
            unsafe { slice::from_raw_parts_mut(data.as_mut_ptr().add(start_offset as usize), len) };
        reader.read_exact(&mut slice).await?;
        unsafe { data.set_len(len) };
    } else {
        let mut mem_slice = unsafe { slice::from_raw_parts_mut(slice.ptr(), len) };
        reader.read_exact(&mut mem_slice).await?;
    }
    Ok(())
}

#[derive(TypePath)]
pub struct MeshLoader;
impl AssetLoader for MeshLoader {
    type Asset = Scene;
    type Error = anyhow::Error;
    type Settings = ();
    async fn load(
        &self,
        reader: &mut dyn bevy::asset::io::Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> std::result::Result<Self::Asset, Self::Error> {
        let instance_transforms = read_slice(reader, None).await?;
        let materials = read_slice(reader, None).await?;
        let instance_mesh = read_slice(reader, None).await?;
        let instance_materials = read_slice(reader, None).await?;

        let num_meshes = read_u64(reader).await?;
        let mut futures = Vec::with_capacity(num_meshes as usize);
        let mut meshes = Vec::with_capacity(num_meshes as usize);
        for i in 0..num_meshes {
            let mut header = MeshHeader::zeroed();
            reader.read_exact(bytes_of_mut(&mut header)).await?;
            let len = read_u64(reader).await? as usize;

            let buffer = Buffer::new(len, false).unwrap();
            let mut slice = buffer.range(..);
            let address = buffer.address;

            fn push_data<T: Pod>(v: T, data: &mut Option<Vec<u8>>, buffer: &mut BufferSlice<u8>) {
                let bytes = bytes_of(&v);
                if let Some(data) = data {
                    data.extend_from_slice(bytes);
                } else {
                    buffer.copy_from(&bytes);
                    *buffer = buffer.range(bytes.len()..);
                }
            }

            let mut data = if Ctx::features().rebar {
                None
            } else {
                Some(Vec::with_capacity(len))
            };

            for i in 0..(header.meshlet_offset as usize / size_of::<BvhNode>()) {
                let mut node = BvhNode::zeroed();
                reader.read_exact(bytes_of_mut(&mut node)).await?;
                for (child_index, aabb) in node.aabb_and_offsets.iter_mut().enumerate() {
                    let offset = aabb.offset();
                    aabb.set_offset(
                        if ((node.child_counts >> (child_index * 8)) & 0xFF) as u8 == 255 {
                            offset * size_of::<BvhNode>() as u64 + address
                        } else {
                            offset
                        },
                    );
                }
                push_data(node, &mut data, &mut slice);
            }

            let meshlet_count =
                (header.cull_data_offset - header.meshlet_offset) as usize / size_of::<Meshlet>();

            for _ in 0..meshlet_count {
                let mut meshlet = Meshlet::zeroed();
                reader.read_exact(bytes_of_mut(&mut meshlet)).await?;
                meshlet.triangle_index =
                    meshlet.triangle_index + header.index_offset as u64 + address;
                meshlet.vertex_index = meshlet.vertex_index * size_of::<Vertex>() as u64
                    + header.vertex_offset as u64
                    + address;
                push_data(meshlet, &mut data, &mut slice);
            }

            read_range(
                reader,
                &mut data,
                slice,
                header.cull_data_offset,
                len as u32,
            )
            .await?;

            let colission_bvh = read_slice(reader, Some(8)).await?;

            if let Some(data) = data {
                futures.push((async move || {
                    Ok((
                        GpuMeshletMesh {
                            buffer: UploadQueue::push_buffer(data, buffer).await?,
                            colission_bvh,
                            header,
                        },
                        i,
                    ))
                })());
            } else {
                let handle = load_context.add_labeled_asset(
                    format!("mesh_{}", i),
                    GpuMeshletMesh {
                        buffer,
                        colission_bvh,
                        header,
                    },
                );
                meshes.push(handle);
            }
        }
        if futures.len() > 0 {
            let iter = futures::future::join_all(futures.into_iter()).await;
            for res in iter {
                let (mesh, i) = res?;
                let handle = load_context.add_labeled_asset(format!("mesh_{}", i), mesh);
                meshes.push(handle);
            }
        }

        Ok(Scene {
            instance_transforms,
            instance_materials,
            instance_mesh,
            materials,
            meshes,
        })
    }
    fn extensions(&self) -> &[&str] {
        &["mesh"]
    }
}
