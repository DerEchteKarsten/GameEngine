//! GPU texture asset: the baked `.tex` file (BC7/BC5/BC6H mips plus a browser preview), its loader, its bindless slot, and mip baking and block compression of glTF images and OpenEXR files.
use std::{io::Write, path::Path};

use anyhow::{Result, bail};
use bevy::{
    asset::{Asset, AssetEvent, AssetId, Assets, io::Reader},
    ecs::{
        message::MessageReader,
        system::{Local, Res},
    },
    log::warn_once,
    reflect::TypePath,
};
use bytemuck::{Pod, Zeroable, bytes_of, bytes_of_mut};
use futures::AsyncReadExt;
use half::f16;
use lava::{
    bindless::{BindlessWrites, NULL_HANDLE, max_sampled_images},
    image::{
        Image,
        format::{
            BC5UnormBlock, BC6HUfloatBlock, BC7SrgbBlock, BC7UnormBlock, Format, R8G8B8A8Unorm,
        },
        slice::AsImage,
        usage::Sampled,
    },
};

use crate::{
    assets::util::{read_compressed, write_compressed},
    bindless::TEXTURE_SLOTS_BASE,
};

/// What a texture holds, which decides how it is compressed.
#[repr(u32)]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextureKind {
    /// Colours, stored as sRGB: BC7.
    Color = 0,
    /// Up to four channels of linear values, like roughness or occlusion: BC7.
    Data = 1,
    /// A tangent space normal map: x and y in BC5, the shader works out z.
    Normal = 2,
    /// Linear HDR colours from an OpenEXR file, like a sky: BC6H, without alpha.
    Hdr = 3,
}

impl TextureKind {
    pub fn from_raw(raw: u32) -> Result<Self> {
        Ok(match raw {
            0 => Self::Color,
            1 => Self::Data,
            2 => Self::Normal,
            3 => Self::Hdr,
            _ => bail!("unknown texture kind {raw}"),
        })
    }

    fn srgb(self) -> bool {
        self == Self::Color
    }
}

#[derive(Asset, TypePath)]
pub enum GpuTexture {
    Color(Image<BC7SrgbBlock, Sampled>),
    Data(Image<BC7UnormBlock, Sampled>),
    Normal(Image<BC5UnormBlock, Sampled>),
    Hdr(Image<BC6HUfloatBlock, Sampled>),
}

/// The sampled slot of a texture asset, known as soon as its handle exists.
fn texture_slot(id: AssetId<GpuTexture>) -> Option<u32> {
    match id {
        // Bevy keeps the index field private; the low 32 bits of `to_bits` are the index.
        AssetId::Index { index, .. } => {
            Some(TEXTURE_SLOTS_BASE.saturating_add(index.to_bits() as u32))
        }
        AssetId::Uuid { .. } => None,
    }
}

/// The bindless index of a texture, `NULL_HANDLE` if it has none. Bevy reuses the indices of
/// freed assets, so the slots only need room for the textures alive at once.
pub fn texture_index(id: AssetId<GpuTexture>) -> u32 {
    match texture_slot(id) {
        Some(slot) if slot < max_sampled_images() => slot,
        _ => {
            warn_once!("texture {id:?} has no bindless slot and is not drawn");
            NULL_HANDLE
        }
    }
}

/// Binds every texture at its slot, and a white placeholder at the slots of textures that
/// are loading or gone. Asset events come in order, so a slot freed by a removal is only
/// taken by a texture added after it.
pub(crate) fn write_texture_slots(
    mut events: MessageReader<AssetEvent<GpuTexture>>,
    textures: Res<Assets<GpuTexture>>,
    mut placeholder: Local<Option<Image<R8G8B8A8Unorm, Sampled>>>,
) {
    let mut writes = BindlessWrites::default();
    let placeholder = placeholder.get_or_insert_with(|| {
        let mut image = Image::new(1, 1).unwrap();
        image.copy_from(&[u8::MAX; 4], 0).unwrap();
        for slot in TEXTURE_SLOTS_BASE..max_sampled_images() {
            writes.sampled(image.whole_view(), slot);
        }
        image
    });
    for event in events.read() {
        match event {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                let index = texture_index(*id);
                match textures.get(*id) {
                    Some(_) if index == NULL_HANDLE => {}
                    Some(GpuTexture::Color(image)) => writes.sampled(image.whole_view(), index),
                    Some(GpuTexture::Data(image)) => writes.sampled(image.whole_view(), index),
                    Some(GpuTexture::Normal(image)) => writes.sampled(image.whole_view(), index),
                    Some(GpuTexture::Hdr(image)) => writes.sampled(image.whole_view(), index),
                    None => {}
                }
            }
            AssetEvent::Removed { id } => {
                if let Some(slot) = texture_slot(*id).filter(|slot| *slot < max_sampled_images()) {
                    writes.sampled(placeholder.whole_view(), slot);
                }
            }
            _ => {}
        }
    }
    writes.submit();
}

/// Each compressed mip is uploaded before the next is read.
pub async fn read_image<F: Format>(
    header: &TextureHeader,
    reader: &mut dyn Reader,
) -> Result<Image<F, Sampled>> {
    let mut image = Image::with_mip_levels(header.width, header.height, header.mip_levels)?;
    for level in 0..header.mip_levels {
        image.copy_from(&read_compressed(reader).await?, level)?;
    }
    Ok(image)
}

#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy, Debug)]
pub struct TextureHeader {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    /// A [`TextureKind`].
    pub kind: u32,
}

/// Side length of the square preview every texture gets baked for the asset browser.
pub const PREVIEW_SIZE: u32 = 128;
/// Bytes of one preview: `PREVIEW_SIZE`² RGBA8 texels.
pub const PREVIEW_BYTES: usize = (PREVIEW_SIZE * PREVIEW_SIZE * 4) as usize;

/// Reads only the preview of a `.tex` file: `PREVIEW_SIZE`² RGBA8 texels.
pub fn read_preview(mut reader: impl std::io::Read) -> Result<Vec<u8>> {
    let mut header = TextureHeader::zeroed();
    reader.read_exact(bytes_of_mut(&mut header))?;
    let mut preview = vec![0u8; PREVIEW_BYTES];
    reader.read_exact(&mut preview)?;
    Ok(preview)
}

pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub kind: TextureKind,
    /// The compressed blocks of each mip level.
    pub mips: Vec<Vec<u8>>,
    /// The texture fitted to a `PREVIEW_SIZE` square, `PREVIEW_BYTES` long.
    pub preview: Vec<u8>,
}

impl TextureData {
    pub fn from_gltf(image: &gltf::image::Data, kind: TextureKind) -> Self {
        let srgb = kind.srgb();
        let srgb_to_linear = srgb_to_linear_lut();
        let decode = |value: u8| {
            if srgb {
                srgb_to_linear[value as usize]
            } else {
                value as f32 / 255.0
            }
        };
        let encode = |value: f32| {
            if srgb {
                linear_to_srgb(value)
            } else {
                (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            }
        };
        // Colour is averaged in linear space.
        let mips = mip_chain(to_rgba8(image), image.width, image.height, |texels| {
            let mut average = [0; 4];
            for channel in 0..3 {
                let sum: f32 = texels.iter().map(|texel| decode(texel[channel])).sum();
                average[channel] = encode(sum * 0.25);
            }
            let alpha: u32 = texels.iter().map(|texel| texel[3] as u32).sum();
            average[3] = ((alpha + 2) / 4) as u8;
            average
        });
        let preview = preview(&mips, image.width, image.height, |texel| texel);
        // One fully opaque texel less and the alpha channel has to be kept.
        let opaque = mips[0].iter().all(|texel| texel[3] == u8::MAX);
        let mips = mips
            .iter()
            .enumerate()
            .map(|(level, mip)| {
                let size = mip_size(image.width, image.height, level);
                compress(mip, size.x, size.y, kind, opaque)
            })
            .collect();
        Self {
            preview,
            width: image.width,
            height: image.height,
            kind,
            mips,
        }
    }

    /// Reads the colour of the first layer of an OpenEXR file as an `Hdr` texture.
    pub fn from_exr(path: &Path) -> Result<Self> {
        let image = exr::prelude::read_first_rgba_layer_from_file(
            path,
            |size, _| (size.width(), vec![[0.0; 4]; size.area()]),
            |(width, texels): &mut (usize, Vec<[f32; 4]>),
             position,
             (r, g, b, _): (f32, f32, f32, f32)| {
                texels[position.y() * *width + position.x()] = [r, g, b, 1.0];
            },
        )?;
        let size = image.layer_data.size;
        let (_, texels) = image.layer_data.channel_data.pixels;
        Ok(Self::from_hdr(
            texels,
            size.width() as u32,
            size.height() as u32,
        ))
    }

    /// Linear RGB texels, alpha is ignored.
    pub fn from_hdr(mut texels: Vec<[f32; 4]>, width: u32, height: u32) -> Self {
        // BC6H stores unsigned half floats; `max` also turns NaN into 0.
        for texel in &mut texels {
            *texel = texel.map(|value| value.max(0.0).min(f16::MAX.to_f32()));
        }
        let mips = mip_chain(texels, width, height, |texels| {
            std::array::from_fn(|channel| {
                texels.iter().map(|texel| texel[channel]).sum::<f32>() * 0.25
            })
        });
        let preview = preview(&mips, width, height, |[r, g, b, _]| {
            [
                linear_to_srgb(r),
                linear_to_srgb(g),
                linear_to_srgb(b),
                u8::MAX,
            ]
        });
        let mips = mips
            .iter()
            .enumerate()
            .map(|(level, mip)| {
                let size = mip_size(width, height, level);
                let half: Vec<[f16; 4]> =
                    mip.iter().map(|texel| texel.map(f16::from_f32)).collect();
                let (half, width, height) = pad_to_blocks(&half, size.x, size.y);
                let surface = intel_tex_2::RgbaSurface {
                    data: bytemuck::cast_slice(&half),
                    width,
                    height,
                    stride: width * size_of::<[f16; 4]>() as u32,
                };
                intel_tex_2::bc6h::compress_blocks(&intel_tex_2::bc6h::basic_settings(), &surface)
            })
            .collect();
        Self {
            preview,
            width,
            height,
            kind: TextureKind::Hdr,
            mips,
        }
    }

    /// Writes the `.tex` file of the texture.
    pub fn write(&self, writer: &mut impl Write) -> Result<()> {
        assert_eq!(self.preview.len(), PREVIEW_BYTES);
        writer.write_all(bytes_of(&TextureHeader {
            width: self.width,
            height: self.height,
            mip_levels: self.mips.len() as u32,
            kind: self.kind as u32,
        }))?;
        writer.write_all(&self.preview)?;
        for mip in &self.mips {
            write_compressed(mip, writer)?;
        }
        Ok(())
    }
}

/// Side length of the blocks of the BC formats.
const BLOCK_SIZE: u32 = 4;

/// Compresses `width` x `height` RGBA8 texels into the blocks of the format of `kind`.
fn compress(rgba: &[[u8; 4]], width: u32, height: u32, kind: TextureKind, opaque: bool) -> Vec<u8> {
    let (rgba, width, height) = pad_to_blocks(rgba, width, height);
    match kind {
        TextureKind::Color | TextureKind::Data => {
            let settings = if opaque {
                intel_tex_2::bc7::opaque_basic_settings()
            } else {
                intel_tex_2::bc7::alpha_basic_settings()
            };
            let surface = intel_tex_2::RgbaSurface {
                data: bytemuck::cast_slice(&rgba),
                width,
                height,
                stride: width * 4,
            };
            intel_tex_2::bc7::compress_blocks(&settings, &surface)
        }
        TextureKind::Normal => {
            let rg: Vec<u8> = rgba.iter().flat_map(|texel| [texel[0], texel[1]]).collect();
            let surface = intel_tex_2::RgSurface {
                data: &rg,
                width,
                height,
                stride: width * 2,
            };
            intel_tex_2::bc5::compress_blocks(&surface)
        }
        TextureKind::Hdr => unreachable!("HDR textures are compressed by `from_hdr`"),
    }
}

/// Grows the texels to whole blocks by repeating the last column and row, which is what the
/// part of a block outside of a mip level should look like to the texels inside.
fn pad_to_blocks<T: Copy>(texels: &[T], width: u32, height: u32) -> (Vec<T>, u32, u32) {
    let padded_width = width.next_multiple_of(BLOCK_SIZE);
    let padded_height = height.next_multiple_of(BLOCK_SIZE);
    let mut padded = Vec::with_capacity((padded_width * padded_height) as usize);
    for y in 0..padded_height {
        for x in 0..padded_width {
            padded.push(texels[(y.min(height - 1) * width + x.min(width - 1)) as usize]);
        }
    }
    (padded, padded_width, padded_height)
}

/// Every mip level down to 1x1, each texel the `average` of 2x2 texels of the level above.
fn mip_chain<T: Copy>(
    texels: Vec<T>,
    width: u32,
    height: u32,
    average: impl Fn([T; 4]) -> T,
) -> Vec<Vec<T>> {
    let mut mips = vec![texels];
    let (mut width, mut height) = (width, height);
    while width > 1 || height > 1 {
        let src = mips.last().unwrap();
        let (next_width, next_height) = ((width / 2).max(1), (height / 2).max(1));
        let mut next = Vec::with_capacity((next_width * next_height) as usize);
        for y in 0..next_height {
            for x in 0..next_width {
                next.push(average([(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| {
                    let sx = (x * 2 + dx).min(width - 1);
                    let sy = (y * 2 + dy).min(height - 1);
                    src[(sy * width + sx) as usize]
                })));
            }
        }
        mips.push(next);
        width = next_width;
        height = next_height;
    }
    mips
}

fn mip_size(width: u32, height: u32, level: usize) -> glam::UVec2 {
    lava::image::mip_extent(glam::UVec2::new(width, height), level as u32)
}

/// Fits the texture to a `PREVIEW_SIZE` square with the nearest texels of the smallest mip
/// that still covers it (level 0 for smaller textures).
fn preview<T: Copy>(
    mips: &[Vec<T>],
    width: u32,
    height: u32,
    to_rgba8: impl Fn(T) -> [u8; 4],
) -> Vec<u8> {
    let level = (0..mips.len())
        .rev()
        .find(|level| mip_size(width, height, *level).min_element() >= PREVIEW_SIZE)
        .unwrap_or(0);
    let size = mip_size(width, height, level);
    let mut preview = Vec::with_capacity(PREVIEW_BYTES);
    for y in 0..PREVIEW_SIZE {
        for x in 0..PREVIEW_SIZE {
            let source = y * size.y / PREVIEW_SIZE * size.x + x * size.x / PREVIEW_SIZE;
            preview.extend_from_slice(&to_rgba8(mips[level][source as usize]));
        }
    }
    preview
}

fn srgb_to_linear_lut() -> [f32; 256] {
    let mut lut = [0.0f32; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        let c = i as f32 / 255.0;
        *v = if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        };
    }
    lut
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let c = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0 + 0.5) as u8
}

fn to_rgba8(image: &gltf::image::Data) -> Vec<[u8; 4]> {
    use gltf::image::Format::*;
    let mut out = Vec::with_capacity((image.width * image.height) as usize);
    let push = |out: &mut Vec<[u8; 4]>, channels: &[u8]| match channels {
        [r] => out.push([*r, *r, *r, 255]),
        [r, g] => out.push([*r, *g, 0, 255]),
        [r, g, b] => out.push([*r, *g, *b, 255]),
        [r, g, b, a] => out.push([*r, *g, *b, *a]),
        _ => unreachable!(),
    };
    let channels = match image.format {
        R8 | R16 => 1,
        R8G8 | R16G16 => 2,
        R8G8B8 | R16G16B16 | R32G32B32FLOAT => 3,
        R8G8B8A8 | R16G16B16A16 | R32G32B32A32FLOAT => 4,
    };
    match image.format {
        R8 | R8G8 | R8G8B8 | R8G8B8A8 => {
            for texel in image.pixels.chunks_exact(channels) {
                push(&mut out, texel);
            }
        }
        R16 | R16G16 | R16G16B16 | R16G16B16A16 => {
            let mut texel = [0u8; 4];
            for (i, value) in image.pixels.chunks_exact(2).enumerate() {
                texel[i % channels] = (u16::from_ne_bytes([value[0], value[1]]) >> 8) as u8;
                if i % channels == channels - 1 {
                    push(&mut out, &texel[..channels]);
                }
            }
        }
        R32G32B32FLOAT | R32G32B32A32FLOAT => {
            let mut texel = [0u8; 4];
            for (i, value) in image.pixels.chunks_exact(4).enumerate() {
                let f = f32::from_ne_bytes([value[0], value[1], value[2], value[3]]);
                texel[i % channels] = (f.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                if i % channels == channels - 1 {
                    push(&mut out, &texel[..channels]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the layout of `AssetIndex::to_bits`, which bevy calls opaque.
    #[test]
    fn texture_slots_follow_the_asset_index() {
        let assets = Assets::<GpuTexture>::default();
        let first = assets.reserve_handle();
        let second = assets.reserve_handle();
        assert_eq!(texture_slot(first.id()), Some(TEXTURE_SLOTS_BASE));
        assert_eq!(texture_slot(second.id()), Some(TEXTURE_SLOTS_BASE + 1));
        assert_eq!(texture_slot(AssetId::invalid()), None);
    }

    #[test]
    fn mips_are_padded_to_whole_blocks() {
        let texels: Vec<u8> = (0..2 * 3).collect();
        let (padded, width, height) = pad_to_blocks(&texels, 2, 3);
        assert_eq!((width, height), (4, 4));
        // Rows 0 1 / 2 3 / 4 5, the last column and row repeated.
        assert_eq!(padded, [0, 1, 1, 1, 2, 3, 3, 3, 4, 5, 5, 5, 4, 5, 5, 5]);
    }

    #[test]
    fn the_file_holds_the_preview_and_every_mip() {
        use bevy::asset::io::VecReader;

        let image = gltf::image::Data {
            pixels: (0..4 * 2).flat_map(|i| [i, 0, 7, 255]).collect(),
            format: gltf::image::Format::R8G8B8A8,
            width: 4,
            height: 2,
        };
        let texture = TextureData::from_gltf(&image, TextureKind::Normal);
        let mut file = Vec::new();
        texture.write(&mut file).unwrap();

        // Magnified: the corners of the preview are the corners of the image.
        let preview = read_preview(file.as_slice()).unwrap();
        assert_eq!(preview[..4], [0, 0, 7, 255]);
        assert_eq!(preview[PREVIEW_BYTES - 4..], [7, 0, 7, 255]);

        bevy::tasks::block_on(async {
            let mut reader = VecReader::new(file);
            let mut header = TextureHeader::zeroed();
            reader.read_exact(bytes_of_mut(&mut header)).await.unwrap();
            assert_eq!((header.width, header.height, header.mip_levels), (4, 2, 3));
            assert_eq!(
                TextureKind::from_raw(header.kind).unwrap(),
                TextureKind::Normal
            );
            reader
                .read_exact(&mut vec![0; PREVIEW_BYTES])
                .await
                .unwrap();
            for mip in &texture.mips {
                assert_eq!(&read_compressed(&mut reader).await.unwrap(), mip);
            }
        });
    }

    #[test]
    fn hdr_textures_are_averaged_in_linear_and_clamped_to_half_floats() {
        // Columns of 4 and 0, a negative texel and one too large for a half float.
        let mut texels = vec![[4.0, 0.0, 0.0, 1.0]; 4 * 2];
        texels[1] = [0.0, -1.0, f32::NAN, 1.0];
        texels[3] = [0.0, 1e9, 0.0, 1.0];
        let texture = TextureData::from_hdr(texels, 4, 2);
        assert_eq!(texture.kind, TextureKind::Hdr);
        // 4x2, 2x1, 1x1: one 16-byte block each.
        assert_eq!(texture.mips.len(), 3);
        assert!(texture.mips.iter().all(|mip| mip.len() == 16));
        // Above 1 is white in the preview, the clamped negative texel black.
        assert_eq!(texture.preview[..4], [255, 0, 0, 255]);
        let second_texel = (PREVIEW_SIZE / 4 * 4) as usize;
        assert_eq!(
            texture.preview[second_texel..second_texel + 4],
            [0, 0, 0, 255]
        );
    }
}
