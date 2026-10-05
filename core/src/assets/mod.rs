//! Asset plugin registering the loaders of the baked scene, mesh and texture files, plus binary read/write helpers.
use core::slice;
use std::{alloc::Layout, io::Write};

use anyhow::{Ok, Result};
use bevy::{asset::AsyncReadExt, prelude::*};
use bytemuck::Pod;
use futures::AsyncRead;

use crate::assets::{
    mesh::{GpuMesh, GpuMeshLoader, MaterialSet, Scene, SceneLoader},
    texture::{GpuTexture, TextureLoader},
};

pub mod bake;
pub mod material;
pub mod mesh;
pub mod texture;

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

/// Decoding is as fast at any level, so this only trades bake time for file size.
const ZSTD_LEVEL: i32 = 12;

fn write_slice<T: Pod>(field: &[T], writer: &mut impl Write) -> Result<()> {
    let len = field.len() as u64;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(bytemuck::cast_slice(field))?;
    Ok(())
}

/// Writes the length of `bytes`, then `bytes` zstd-compressed as a slice.
fn write_compressed(bytes: &[u8], writer: &mut impl Write) -> Result<()> {
    writer.write_all(&(bytes.len() as u64).to_le_bytes())?;
    write_slice(&zstd::bulk::compress(bytes, ZSTD_LEVEL)?, writer)
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

/// Reads what `write_compressed` wrote.
async fn read_compressed(
    reader: &mut (impl AsyncRead + Unpin + ?Sized),
    alignment: Option<usize>,
) -> Result<Vec<u8>> {
    let len = read_u64(reader).await? as usize;
    let compressed: Vec<u8> = read_slice(reader, None).await?;
    let bytes = zstd::bulk::decompress(&compressed, len)?;
    match alignment {
        None => Ok(bytes),
        Some(_) => read_len_slice(&mut bytes.as_slice(), len, alignment).await,
    }
}

/// Writes a name as its byte length followed by its UTF-8 bytes.
fn write_name(name: &str, writer: &mut impl Write) -> Result<()> {
    write_slice(name.as_bytes(), writer)
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
