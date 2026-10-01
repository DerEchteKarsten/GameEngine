pub use lava::bindings::Material;

/// Value of a `Material` texture field for an absent texture.
/// Equal to the bindless null handle so the same value works in the scene file and on the GPU.
pub const NO_TEXTURE: u32 = lava::bindless::NULL_HANDLE;

/// Number of texture slots in `Material`: base color, metallic-roughness, normal, occlusion,
/// emissive, in that order.
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
