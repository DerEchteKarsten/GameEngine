//! PBR material texture slots: mapping between materials and bindless texture indices.
pub use lava::bindings::Material;

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
