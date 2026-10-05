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
        format::{BC5UnormBlock, BC7SrgbBlock, BC7UnormBlock},
        usage::Sampled,
    },
};

use crate::assets::{read_compressed, write_compressed};

pub const TEXTURE_EXTENSION: &str = "tex";

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
    fn from_raw(raw: u32) -> Result<Self> {
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
pub struct GpuTexture {
    image: TextureImage,
}

enum TextureImage {
    Color(Image<BC7SrgbBlock, Sampled>),
    Data(Image<BC7UnormBlock, Sampled>),
    Normal(Image<BC5UnormBlock, Sampled>),
}

impl GpuTexture {
    pub fn new(header: &TextureHeader) -> Result<Self> {
        let (width, height, mips) = (header.width, header.height, header.mip_levels);
        let image = match TextureKind::from_raw(header.kind)? {
            TextureKind::Color => TextureImage::Color(Image::with_mip_levels(width, height, mips)?),
            TextureKind::Data => TextureImage::Data(Image::with_mip_levels(width, height, mips)?),
            TextureKind::Normal => {
                TextureImage::Normal(Image::with_mip_levels(width, height, mips)?)
            }
        };
        Ok(Self { image })
    }

    /// `blocks` are the compressed 4x4 blocks of the mip level.
    pub fn upload_mip(&mut self, level: u32, blocks: &[u8]) -> Result<()> {
        match &mut self.image {
            TextureImage::Color(image) => image.copy_from(blocks, level)?,
            TextureImage::Data(image) => image.copy_from(blocks, level)?,
            TextureImage::Normal(image) => image.copy_from(blocks, level)?,
        }
        Ok(())
    }

    /// Bindless handle of the texture, e.g. for drawing it in the UI.
    pub fn handle(&self) -> BindlessHandle {
        match &self.image {
            TextureImage::Color(image) => image.handle,
            TextureImage::Data(image) => image.handle,
            TextureImage::Normal(image) => image.handle,
        }
    }

    pub fn descriptor_index(&self) -> u32 {
        self.handle().descriptor_index_set0
    }
}

/// Loads a `.tex` file: the header, the preview (skipped), then one compressed mip after
/// the other, each uploaded before the next is read.
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
        let mut texture = GpuTexture::new(&header)?;
        for level in 0..header.mip_levels {
            let blocks = read_compressed(reader, None).await?;
            texture.upload_mip(level, &blocks)?;
        }
        Ok(texture)
    }

    fn extensions(&self) -> &[&str] {
        &[TEXTURE_EXTENSION]
    }
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
        let preview = preview(&mips, image.width, image.height, srgb);
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

    pub fn header(&self) -> TextureHeader {
        TextureHeader {
            width: self.width,
            height: self.height,
            mip_levels: self.mips.len() as u32,
            kind: self.kind as u32,
        }
    }

    /// Writes the `.tex` file of the texture.
    pub fn write(&self, writer: &mut impl Write) -> Result<()> {
        assert_eq!(self.preview.len(), PREVIEW_BYTES);
        writer.write_all(bytes_of(&self.header()))?;
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

/// Colour channel conversion to and from the linear values that filtering averages.
fn codec(srgb: bool) -> (impl Fn(u8) -> f32, impl Fn(f32) -> u8) {
    let srgb_to_linear = srgb_to_linear_lut();
    let decode = move |value: u8| {
        if srgb {
            srgb_to_linear[value as usize]
        } else {
            value as f32 / 255.0
        }
    };
    let encode = move |value: f32| {
        if srgb {
            linear_to_srgb(value)
        } else {
            (value.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        }
    };
    (decode, encode)
}

/// The mip level a preview is resampled from: the smallest one that still covers
/// `PREVIEW_SIZE` in both dimensions, or level 0 for textures smaller than that.
fn preview_level(width: u32, height: u32, levels: usize) -> usize {
    (0..levels)
        .rev()
        .find(|level| mip_size(width, height, *level).min_element() >= PREVIEW_SIZE)
        .unwrap_or(0)
}

fn mip_size(width: u32, height: u32, level: usize) -> glam::UVec2 {
    lava::image::mip_extent(glam::UVec2::new(width, height), level as u32)
}

/// Fits the texture to a `PREVIEW_SIZE` square by averaging the source texels under each
/// preview texel. Non-square textures are stretched, smaller ones magnified.
fn preview(mips: &[Vec<u8>], width: u32, height: u32, srgb: bool) -> Vec<u8> {
    let (decode, encode) = codec(srgb);
    let level = preview_level(width, height, mips.len());
    let src = &mips[level];
    let size = mip_size(width, height, level);
    // Source texels covered by preview texel `i` along an axis of `len` texels.
    let span = |i: u32, len: u32| {
        let start = (i as u64 * len as u64 / PREVIEW_SIZE as u64) as u32;
        let end = ((i as u64 + 1) * len as u64 / PREVIEW_SIZE as u64) as u32;
        start..end.max(start + 1)
    };
    let mut dst = Vec::with_capacity(PREVIEW_BYTES);
    for y in 0..PREVIEW_SIZE {
        for x in 0..PREVIEW_SIZE {
            let mut sum = [0.0f32; 4];
            let mut count = 0.0;
            for sy in span(y, size.y) {
                for sx in span(x, size.x) {
                    let p = ((sy * size.x + sx) * 4) as usize;
                    sum[0] += decode(src[p]);
                    sum[1] += decode(src[p + 1]);
                    sum[2] += decode(src[p + 2]);
                    sum[3] += src[p + 3] as f32 / 255.0;
                    count += 1.0;
                }
            }
            dst.extend_from_slice(&[
                encode(sum[0] / count),
                encode(sum[1] / count),
                encode(sum[2] / count),
                (sum[3] / count * 255.0 + 0.5) as u8,
            ]);
        }
    }
    dst
}

fn downsample(src: &[u8], width: u32, height: u32, srgb: bool) -> (Vec<u8>, u32, u32) {
    let (decode, encode) = codec(srgb);
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

    fn texture(width: u32, height: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> TextureData {
        let pixels = (0..height)
            .flat_map(|y| (0..width).map(move |x| (x, y)))
            .flat_map(|(x, y)| texel(x, y))
            .collect();
        TextureData::from_gltf(
            &gltf::image::Data {
                pixels,
                format: gltf::image::Format::R8G8B8A8,
                width,
                height,
            },
            TextureKind::Data,
        )
    }

    #[test]
    fn mips_are_padded_to_whole_blocks() {
        let rgba: Vec<u8> = (0..2 * 3).flat_map(|i| [i, i, i, 255]).collect();
        let (padded, width, height) = pad_to_blocks(&rgba, 2, 3);
        assert_eq!((width, height), (4, 4));
        let reds: Vec<u8> = padded.chunks_exact(4).map(|texel| texel[0]).collect();
        // Rows 0 1 / 2 3 / 4 5, the last column and row repeated.
        assert_eq!(reds, [0, 1, 1, 1, 2, 3, 3, 3, 4, 5, 5, 5, 4, 5, 5, 5]);

        let (same, width, height) = pad_to_blocks(&padded, 4, 4);
        assert_eq!((width, height), (4, 4));
        assert_eq!(same, padded);
    }

    #[test]
    fn every_mip_is_compressed_into_whole_blocks() {
        // 10x6 texels: 3x2, 2x1 and then single blocks.
        let image = gltf::image::Data {
            pixels: vec![200; 10 * 6 * 4],
            format: gltf::image::Format::R8G8B8A8,
            width: 10,
            height: 6,
        };
        for kind in [TextureKind::Color, TextureKind::Data, TextureKind::Normal] {
            let texture = TextureData::from_gltf(&image, kind);
            let blocks: Vec<usize> = texture.mips.iter().map(|mip| mip.len() / 16).collect();
            assert_eq!(blocks, [6, 2, 1, 1], "{kind:?}");
            assert_eq!(texture.header().kind, kind as u32);
            assert_eq!(TextureKind::from_raw(kind as u32).unwrap(), kind);
        }
        assert!(TextureKind::from_raw(3).is_err());
    }

    fn preview_texel(preview: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * PREVIEW_SIZE + x) * 4) as usize;
        preview[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn the_file_holds_the_preview_and_every_mip() {
        use bevy::asset::io::VecReader;

        let texture = texture(4, 2, |x, y| [x as u8, y as u8, 7, 255]);
        let mut file = Vec::new();
        texture.write(&mut file).unwrap();

        assert!(read_preview(file.as_slice()).unwrap() == texture.preview);

        bevy::tasks::block_on(async {
            let mut reader = VecReader::new(file);
            let mut header = TextureHeader::zeroed();
            reader.read_exact(bytes_of_mut(&mut header)).await.unwrap();
            assert_eq!((header.width, header.height, header.mip_levels), (4, 2, 3));
            reader.read_exact(&mut vec![0; PREVIEW_BYTES]).await.unwrap();
            for mip in &texture.mips {
                assert_eq!(&read_compressed(&mut reader, None).await.unwrap(), mip);
            }
        });
    }

    #[test]
    fn previews_come_from_the_smallest_covering_mip() {
        assert_eq!(preview_level(1024, 1024, 11), 3);
        assert_eq!(preview_level(128, 128, 8), 0);
        assert_eq!(preview_level(64, 64, 7), 0);
        // The short side decides, so the long one is never magnified.
        assert_eq!(preview_level(2048, 256, 12), 1);
    }

    #[test]
    fn previews_are_always_the_same_size() {
        for (width, height) in [(1, 1), (5, 3), (128, 128), (512, 64), (300, 700)] {
            let texture = texture(width, height, |_, _| [10, 20, 30, 255]);
            assert_eq!(texture.preview.len(), PREVIEW_BYTES, "{width}x{height}");
            assert!(
                texture
                    .preview
                    .chunks_exact(4)
                    .all(|texel| texel == [10, 20, 30, 255]),
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn previews_stretch_to_the_square() {
        // Left half red, right half green, on a wide and on a tiny texture.
        let halves = |width: u32| {
            move |x: u32, _| {
                if x < width / 2 {
                    [255, 0, 0, 255]
                } else {
                    [0, 255, 0, 255]
                }
            }
        };
        for (width, height) in [(512, 128), (2, 1)] {
            let texture = texture(width, height, halves(width));
            for y in [0, PREVIEW_SIZE - 1] {
                assert_eq!(preview_texel(&texture.preview, 0, y), [255, 0, 0, 255]);
                assert_eq!(
                    preview_texel(&texture.preview, PREVIEW_SIZE - 1, y),
                    [0, 255, 0, 255]
                );
            }
        }
    }
}
