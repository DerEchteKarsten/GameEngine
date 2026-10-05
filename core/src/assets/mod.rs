//! Asset plugin registering the loaders of the baked scene, mesh and texture files, plus binary read/write helpers.
use std::io::Write;

use anyhow::{Ok, Result};
use bevy::{asset::AsyncReadExt, prelude::*};
use bytemuck::Pod;
use futures::AsyncRead;

use crate::assets::{
    mesh::{GpuMesh, GpuMeshLoader, MaterialSet, Scene, SceneLoader},
    texture::{GpuTexture, TextureLoader},
};

pub mod bake;
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

async fn read_slice<T: Pod>(reader: &mut (impl AsyncRead + Unpin + ?Sized)) -> Result<Vec<T>> {
    let mut slice = vec![T::zeroed(); read_u64(reader).await? as usize];
    reader.read_exact(bytemuck::cast_slice_mut(&mut slice)).await?;
    Ok(slice)
}

/// Reads what `write_compressed` wrote.
async fn read_compressed(reader: &mut (impl AsyncRead + Unpin + ?Sized)) -> Result<Vec<u8>> {
    let len = read_u64(reader).await? as usize;
    let compressed: Vec<u8> = read_slice(reader).await?;
    Ok(zstd::bulk::decompress(&compressed, len)?)
}

/// Writes how many names there are, then each as a slice of its UTF-8 bytes.
fn write_names(names: &[String], writer: &mut impl Write) -> Result<()> {
    writer.write_all(&(names.len() as u64).to_le_bytes())?;
    for name in names {
        write_slice(name.as_bytes(), writer)?;
    }
    Ok(())
}

async fn read_names(reader: &mut (impl AsyncRead + Unpin + ?Sized)) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for _ in 0..read_u64(reader).await? {
        names.push(String::from_utf8(read_slice(reader).await?)?);
    }
    Ok(names)
}
