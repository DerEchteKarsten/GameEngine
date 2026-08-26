use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Material {
    pub metalic_factor: f32,
    pub roughness_factor: f32,
    pub color: [f32; 3],
    pub texture_offset: u32,
}
