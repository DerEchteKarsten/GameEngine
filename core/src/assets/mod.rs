use core::slice;
use std::alloc::Layout;

use anyhow::{Ok, Result};
use bevy::{
    asset::{
        AsyncReadExt, AsyncWriteExt, processor::LoadTransformAndSave,
    },
    prelude::*,
};
use bytemuck::Pod;

use crate::assets::mesh::{
            GltfMesh, GltfMeshLoader, GpuMesh, MeshLoader, MeshSaver, MeshTransformer,
            Scene,
        };

use lava::buffer::slice::BufferSlice;

pub mod material;
pub mod mesh;
pub mod texture;

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
            .init_asset::<GpuMesh>();
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

async fn read_slice_to_buffer<'a>(
    reader: &mut dyn bevy::asset::io::Reader,
    slice: BufferSlice<'a, u8>,
) -> Result<()> {
    let mut mem_slice = unsafe { slice::from_raw_parts_mut(slice.ptr(), slice.len()) };
    reader.read_exact(&mut mem_slice).await?;
    Ok(())
}
