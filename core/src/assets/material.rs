pub use lava::bindings::Material;

/// Value of `Material::texture` for materials without a base color texture.
/// Equal to the bindless null handle so the same value works in the scene file and on the GPU.
pub const NO_TEXTURE: u32 = lava::bindless::NULL_HANDLE;
