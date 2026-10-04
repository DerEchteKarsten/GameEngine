//! Asset plugin registering the glTF mesh processor/loaders, plus binary read/write helpers.
use core::slice;
use std::alloc::Layout;

use anyhow::{Ok, Result};
use bevy::{
    asset::{AsyncReadExt, AsyncWriteExt, processor::LoadTransformAndSave},
    prelude::*,
};
use bytemuck::Pod;
use futures::AsyncRead;

use crate::assets::{
    mesh::{GltfMesh, GltfMeshLoader, GpuMesh, MeshLoader, MeshSaver, MeshTransformer, Scene},
    texture::GpuTexture,
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
            .init_asset_loader::<MeshLoader>()
            .init_asset::<Scene>()
            .init_asset::<GltfMesh>()
            .init_asset::<GpuMesh>()
            .init_asset::<GpuTexture>();
    }
}

async fn write_slice<T: Pod>(field: &[T], writer: &mut bevy::asset::io::Writer) -> Result<()> {
    let len = field.len() as u64;
    writer.write_all(&len.to_le_bytes()).await?;
    let byte_slice = bytemuck::cast_slice(field);
    writer.write_all(byte_slice).await?;
    Ok(())
}
/// Bytes `write_slice` writes for `field`.
fn slice_bytes<T>(field: &[T]) -> u64 {
    (size_of::<u64>() + size_of_val(field)) as u64
}

async fn read_u64(reader: &mut (impl AsyncRead + Unpin + ?Sized)) -> Result<u64> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes).await?;
    Ok(u64::from_le_bytes(bytes))
}

async fn read_slice<T: Pod>(
    reader: &mut (impl AsyncRead + Unpin + ?Sized),
    alignment: Option<usize>,
) -> Result<Vec<T>> {
    let len = read_u64(reader).await?;
    read_len_slice(reader, len as usize, alignment).await
}

async fn read_len_slice<T: Pod>(
    reader: &mut (impl AsyncRead + Unpin + ?Sized),
    len: usize,
    alignment: Option<usize>,
) -> Result<Vec<T>> {
    // Nothing to read, and a zero-sized allocation is not allowed.
    if len == 0 {
        return Ok(Vec::new());
    }
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

/// Writes a name as its byte length followed by its UTF-8 bytes.
async fn write_name(name: &str, writer: &mut bevy::asset::io::Writer) -> Result<()> {
    write_slice(name.as_bytes(), writer).await
}

async fn read_name(reader: &mut (impl AsyncRead + Unpin + ?Sized)) -> Result<String> {
    Ok(String::from_utf8(read_slice(reader, None).await?)?)
}

async fn read_names(
    reader: &mut (impl AsyncRead + Unpin + ?Sized),
    count: usize,
) -> Result<Vec<String>> {
    let mut names = Vec::with_capacity(count);
    for _ in 0..count {
        names.push(read_name(reader).await?);
    }
    Ok(names)
}

async fn read_slice_to_buffer<'a>(
    reader: &mut (impl AsyncRead + Unpin + ?Sized),
    slice: BufferSlice<'a, u8>,
) -> Result<()> {
    let mem_slice = unsafe { slice::from_raw_parts_mut(slice.ptr(), slice.len()) };
    reader.read_exact(mem_slice).await?;
    Ok(())
}
