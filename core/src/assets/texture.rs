//! GPU texture asset: sRGB/linear mipmapped sampled images and their serialized header.
use anyhow::Result;
use bevy::{asset::Asset, reflect::TypePath};
use bytemuck::{Pod, Zeroable};
use lava::{
    bindless::NULL_HANDLE,
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

pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub srgb: bool,
    pub mips: Vec<Vec<u8>>,
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

fn downsample(src: &[u8], width: u32, height: u32, srgb: bool) -> (Vec<u8>, u32, u32) {
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
