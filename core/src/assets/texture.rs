use bevy::{asset::Asset, reflect::TypePath};
use bytemuck::{Pod, Zeroable};
use lava::{
    bindless::NULL_HANDLE,
    image::{Image, format::R8G8B8A8Srgb, usage::Sampled},
};

/// A material texture on the GPU: RGBA8 sRGB with a full mip chain, registered in the bindless
/// sampled image heap. Textures are labeled sub assets of a `Scene` (`texture_{i}`).
#[derive(Asset, TypePath)]
pub struct GpuTexture {
    pub image: Image<1, R8G8B8A8Srgb, Sampled>,
}

impl GpuTexture {
    /// Index into the bindless sampled image heap, what `Material::texture` expects on the GPU.
    pub fn descriptor_index(&self) -> u32 {
        self.image
            .handle
            .map(|handle| handle.descriptor_index_set0)
            .unwrap_or(NULL_HANDLE)
    }
}

#[repr(C)]
#[derive(Pod, Zeroable, Clone, Copy, Debug)]
pub struct TextureHeader {
    pub width: u32,
    pub height: u32,
    pub mip_levels: u32,
    pub _pad: u32,
}

/// CPU side texture as stored in the processed scene file: RGBA8 sRGB, `mips[0]` is the base level.
pub struct TextureData {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
}

impl TextureData {
    /// Converts a decoded glTF image to RGBA8 and generates the full mip chain down to 1x1.
    pub fn from_gltf(image: &gltf::image::Data) -> Self {
        let mut mips = vec![to_rgba8(image)];
        let (mut width, mut height) = (image.width, image.height);
        while width > 1 || height > 1 {
            let (next, next_width, next_height) = downsample(mips.last().unwrap(), width, height);
            mips.push(next);
            width = next_width;
            height = next_height;
        }
        Self {
            width: image.width,
            height: image.height,
            mips,
        }
    }

    pub fn header(&self) -> TextureHeader {
        TextureHeader {
            width: self.width,
            height: self.height,
            mip_levels: self.mips.len() as u32,
            _pad: 0,
        }
    }
}

/// 2x2 box filter in linear space (color channels are sRGB, alpha is linear). Odd edges clamp.
fn downsample(src: &[u8], width: u32, height: u32) -> (Vec<u8>, u32, u32) {
    let srgb_to_linear = srgb_to_linear_lut();
    let (next_width, next_height) = ((width / 2).max(1), (height / 2).max(1));
    let mut dst = vec![0u8; (next_width * next_height * 4) as usize];
    for y in 0..next_height {
        for x in 0..next_width {
            let mut sum = [0.0f32; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let sx = (x * 2 + dx).min(width - 1);
                let sy = (y * 2 + dy).min(height - 1);
                let p = ((sy * width + sx) * 4) as usize;
                sum[0] += srgb_to_linear[src[p] as usize];
                sum[1] += srgb_to_linear[src[p + 1] as usize];
                sum[2] += srgb_to_linear[src[p + 2] as usize];
                sum[3] += src[p + 3] as f32 / 255.0;
            }
            let o = ((y * next_width + x) * 4) as usize;
            dst[o] = linear_to_srgb(sum[0] * 0.25);
            dst[o + 1] = linear_to_srgb(sum[1] * 0.25);
            dst[o + 2] = linear_to_srgb(sum[2] * 0.25);
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

/// Expands any glTF pixel format to 8 bit RGBA. Missing channels become 0 (alpha 255),
/// single channel images are broadcast to gray.
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
