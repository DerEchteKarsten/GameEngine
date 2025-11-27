#![cfg_attr(target_arch = "spirv", no_std)]

#![feature(asm_experimental_arch)]

use core::ops::{Index, IndexMut};
use spirv_std::arch::IndexUnchecked;
use spirv_std::spirv;
use spirv_std::glam::*;


#[repr(C)]
pub struct Image {
    pub index: u64,
}
#[repr(C)]
pub struct MutImage {
    pub index: u64,
}

#[repr(C)]
pub struct Ptr<T> {
    pub ptr: *const T,
}

#[repr(C)]
pub struct MutPtr<T> {
    pub ptr: *mut T,
}

pub mod post;
