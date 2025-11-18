#![cfg_attr(target_arch = "spirv", no_std)]
#![feature(asm_experimental_arch)]

use spirv_std::glam::*;

#[repr(C)]
pub struct TextureHandle {
    pub index: u64,
}
#[repr(C)]
pub struct ImageHandle {
    pub index: u64,
}

pub mod post;
