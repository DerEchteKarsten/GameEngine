//! PBR material texture slots: mapping between materials and bindless texture indices.
use bevy::asset::Handle;
pub use lava::bindings::Material;

use crate::assets::texture::GpuTexture;

pub const NO_TEXTURE: u32 = lava::bindless::NULL_HANDLE;

pub const TEXTURE_SLOTS: usize = 5;

pub fn texture_indices(material: &Material) -> [u32; TEXTURE_SLOTS] {
    [
        material.color_texture,
        material.metallic_roughness_texture,
        material.normal_texture,
        material.occlusion_texture,
        material.emissive_texture,
    ]
}

pub fn set_texture_indices(material: &mut Material, indices: [u32; TEXTURE_SLOTS]) {
    [
        material.color_texture,
        material.metallic_roughness_texture,
        material.normal_texture,
        material.occlusion_texture,
        material.emissive_texture,
    ] = indices;
}

/// The textures of one material, in the order of [`texture_indices`].
#[derive(Clone, Default, PartialEq)]
pub struct MaterialTextures {
    pub color: Option<Handle<GpuTexture>>,
    pub metallic_roughness: Option<Handle<GpuTexture>>,
    pub normal: Option<Handle<GpuTexture>>,
    pub occlusion: Option<Handle<GpuTexture>>,
    pub emissive: Option<Handle<GpuTexture>>,
}

impl MaterialTextures {
    pub fn handles(&self) -> [Option<&Handle<GpuTexture>>; TEXTURE_SLOTS] {
        [
            self.color.as_ref(),
            self.metallic_roughness.as_ref(),
            self.normal.as_ref(),
            self.occlusion.as_ref(),
            self.emissive.as_ref(),
        ]
    }
}
