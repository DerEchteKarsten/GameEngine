//! GPU texture asset: the baked `.tex` file (BC7/BC5 mips plus a browser preview), its loader, and mip baking and block compression.
use std::io::Write;

use anyhow::{Result, bail};
use bevy::{
    asset::{Asset, AssetLoader, LoadContext, io::Reader},
    reflect::TypePath,
};
use bytemuck::{Pod, Zeroable, bytes_of, bytes_of_mut};
use futures::AsyncReadExt;
use lava::{
    bindless::BindlessHandle,
    image::{
        Image,
        format::{BC5UnormBlock, BC7SrgbBlock, BC7UnormBlock, Format},
        usage::Sampled,
    },
};

use crate::assets::util::{read_compressed, write_compressed};

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
}

impl TextureKind {
    pub fn from_raw(raw: u32) -> Result<Self> {
        Ok(match raw {
            0 => Self::Color,
            1 => Self::Data,
            2 => Self::Normal,
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
}

impl GpuTexture {
    pub fn handle(&self) -> BindlessHandle {
        match self {
            Self::Color(image) => image.handle,
            Self::Data(image) => image.handle,
            Self::Normal(image) => image.handle,
        }
    }

    pub fn descriptor_index(&self) -> u32 {
        self.handle().descriptor_index_set0
    }
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
        let mut mips = vec![to_rgba8(image)];
        let (mut width, mut height) = (image.width, image.height);
        while width > 1 || height > 1 {
            let (next, next_width, next_height) =
                downsample(mips.last().unwrap(), width, height, srgb);
            mips.push(next);
            width = next_width;
            height = next_height;
        }
        let preview = preview(&mips, image.width, image.height);
        // One fully opaque texel less and the alpha channel has to be kept.
        let opaque = mips[0].chunks_exact(4).all(|texel| texel[3] == u8::MAX);
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
fn compress(rgba: &[u8], width: u32, height: u32, kind: TextureKind, opaque: bool) -> Vec<u8> {
    let (rgba, width, height) = pad_to_blocks(rgba, width, height);
    match kind {
        TextureKind::Color | TextureKind::Data => {
            let settings = if opaque {
                intel_tex_2::bc7::opaque_basic_settings()
            } else {
                intel_tex_2::bc7::alpha_basic_settings()
            };
            let surface = intel_tex_2::RgbaSurface {
                data: &rgba,
                width,
                height,
                stride: width * 4,
            };
            intel_tex_2::bc7::compress_blocks(&settings, &surface)
        }
        TextureKind::Normal => {
            let rg: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|texel| [texel[0], texel[1]])
                .collect();
            let surface = intel_tex_2::RgSurface {
                data: &rg,
                width,
                height,
                stride: width * 2,
            };
            intel_tex_2::bc5::compress_blocks(&surface)
        }
    }
}

/// Grows the texels to whole blocks by repeating the last column and row, which is what the
/// part of a block outside of a mip level should look like to the texels inside.
fn pad_to_blocks(rgba: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let padded_width = width.next_multiple_of(BLOCK_SIZE);
    let padded_height = height.next_multiple_of(BLOCK_SIZE);
    let mut padded = Vec::with_capacity((padded_width * padded_height * 4) as usize);
    for y in 0..padded_height {
        for x in 0..padded_width {
            let texel = ((y.min(height - 1) * width + x.min(width - 1)) * 4) as usize;
            padded.extend_from_slice(&rgba[texel..texel + 4]);
        }
    }
    (padded, padded_width, padded_height)
}

fn mip_size(width: u32, height: u32, level: usize) -> glam::UVec2 {
    lava::image::mip_extent(glam::UVec2::new(width, height), level as u32)
}

/// Fits the texture to a `PREVIEW_SIZE` square with the nearest texels of the smallest mip
/// that still covers it (level 0 for smaller textures).
fn preview(mips: &[Vec<u8>], width: u32, height: u32) -> Vec<u8> {
    let level = (0..mips.len())
        .rev()
        .find(|level| mip_size(width, height, *level).min_element() >= PREVIEW_SIZE)
        .unwrap_or(0);
    let size = mip_size(width, height, level);
    let mut preview = Vec::with_capacity(PREVIEW_BYTES);
    for y in 0..PREVIEW_SIZE {
        for x in 0..PREVIEW_SIZE {
            let source = y * size.y / PREVIEW_SIZE * size.x + x * size.x / PREVIEW_SIZE;
            let texel = source as usize * 4;
            preview.extend_from_slice(&mips[level][texel..texel + 4]);
        }
    }
    preview
}

fn downsample(src: &[u8], width: u32, height: u32, srgb: bool) -> (Vec<u8>, u32, u32) {
    // Colour is averaged in linear space.
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
    let (next_width, next_height) = ((width / 2).max(1), (height / 2).max(1));
    let mut dst = vec![0u8; (next_width * next_height * 4) as usize];
    for y in 0..next_height {
        for x in 0..next_width {
            let mut sum = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (x * 2 + dx).min(width - 1);
                let sy = (y * 2 + dy).min(height - 1);
                let p = ((sy * width + sx) * 4) as usize;
                sum[0] += decode(src[p]);
                sum[1] += decode(src[p + 1]);
                sum[2] += decode(src[p + 2]);
                sum[3] += src[p + 3] as f32 / 255.0;
            }
            let o = ((y * next_width + x) * 4) as usize;
            dst[o] = encode(sum[0] * 0.25);
            dst[o + 1] = encode(sum[1] * 0.25);
            dst[o + 2] = encode(sum[2] * 0.25);
            dst[o + 3] = (sum[3] * 0.25 * 255.0 + 0.5) as u8;
        }
    }
    (dst, next_width, next_height)
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

fn to_rgba8(image: &gltf::image::Data) -> Vec<u8> {
    use gltf::image::Format::*;
    let texels = (image.width * image.height) as usize;
    let mut out = Vec::with_capacity(texels * 4);
    let push = |out: &mut Vec<u8>, channels: &[u8]| match channels {
        [r] => out.extend_from_slice(&[*r, *r, *r, 255]),
        [r, g] => out.extend_from_slice(&[*r, *g, 0, 255]),
        [r, g, b] => out.extend_from_slice(&[*r, *g, *b, 255]),
        [r, g, b, a] => out.extend_from_slice(&[*r, *g, *b, *a]),
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

    #[test]
    fn mips_are_padded_to_whole_blocks() {
        let rgba: Vec<u8> = (0..2 * 3).flat_map(|i| [i, i, i, 255]).collect();
        let (padded, width, height) = pad_to_blocks(&rgba, 2, 3);
        assert_eq!((width, height), (4, 4));
        let reds: Vec<u8> = padded.chunks_exact(4).map(|texel| texel[0]).collect();
        // Rows 0 1 / 2 3 / 4 5, the last column and row repeated.
        assert_eq!(reds, [0, 1, 1, 1, 2, 3, 3, 3, 4, 5, 5, 5, 4, 5, 5, 5]);
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
}
