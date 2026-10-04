//! GPU texture asset: sRGB/linear mipmapped sampled images, their serialized header and baked browser previews.
use anyhow::Result;
use bevy::{asset::Asset, reflect::TypePath};
use bytemuck::{Pod, Zeroable};
use lava::{
    bindless::{BindlessHandle, NULL_HANDLE},
    image::{
        Image,
        format::{R8G8B8A8Srgb, R8G8B8A8Unorm},
        slice::AsImage,
        usage::Sampled,
    },
};

#[derive(Asset, TypePath)]
pub struct GpuTexture {
    image: TextureImage,
}

enum TextureImage {
    Srgb(Image<R8G8B8A8Srgb, Sampled>),
    Linear(Image<R8G8B8A8Unorm, Sampled>),
}

impl GpuTexture {
    pub fn new(header: &TextureHeader) -> Result<Self> {
        let image = if header.srgb != 0 {
            let image = Image::with_mip_levels(header.width, header.height, header.mip_levels)?;
            TextureImage::Srgb(image)
        } else {
            let image = Image::with_mip_levels(header.width, header.height, header.mip_levels)?;
            TextureImage::Linear(image)
        };
        Ok(Self { image })
    }

    pub fn upload_mip(&mut self, level: u32, pixels: &[u8]) -> Result<()> {
        match &mut self.image {
            TextureImage::Srgb(image) => image.copy_from(pixels, level)?,
            TextureImage::Linear(image) => image.copy_from(pixels, level)?,
        }
        Ok(())
    }

    /// Bindless handle of the texture, e.g. for drawing it in the UI.
    pub fn handle(&self) -> BindlessHandle {
        match &self.image {
            TextureImage::Srgb(image) => image.handle,
            TextureImage::Linear(image) => image.handle,
        }
    }

    pub fn descriptor_index(&self) -> u32 {
        match &self.image {
            TextureImage::Srgb(image) => image.handle.descriptor_index_set0,
            TextureImage::Linear(image) => image.handle.descriptor_index_set0,
        }
    }
}

#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy, Debug)]
pub struct TextureHeader {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub srgb: u32,
}

/// Side length of the square preview every texture gets baked for the asset browser.
pub const PREVIEW_SIZE: u32 = 128;
/// Bytes of one preview: `PREVIEW_SIZE`² RGBA8 texels.
pub const PREVIEW_BYTES: usize = (PREVIEW_SIZE * PREVIEW_SIZE * 4) as usize;

pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub srgb: bool,
    pub mips: Vec<Vec<u8>>,
    /// The texture fitted to a `PREVIEW_SIZE` square, `PREVIEW_BYTES` long.
    pub preview: Vec<u8>,
}

impl TextureData {
    pub fn from_gltf(image: &gltf::image::Data, srgb: bool) -> Self {
        let mut mips = vec![to_rgba8(image)];
        let (mut width, mut height) = (image.width, image.height);
        while width > 1 || height > 1 {
            let (next, next_width, next_height) =
                downsample(mips.last().unwrap(), width, height, srgb);
            mips.push(next);
            width = next_width;
            height = next_height;
        }
        Self {
            preview: preview(&mips, image.width, image.height, srgb),
            width: image.width,
            height: image.height,
            srgb,
            mips,
        }
    }

    pub fn header(&self) -> TextureHeader {
        TextureHeader {
            width: self.width,
            height: self.height,
            mip_levels: self.mips.len() as u32,
            srgb: self.srgb as u32,
        }
    }
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
            false,
        )
    }

    fn preview_texel(preview: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * PREVIEW_SIZE + x) * 4) as usize;
        preview[i..i + 4].try_into().unwrap()
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
