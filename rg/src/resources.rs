use std::collections::HashMap;

use ash::vk::{self, Format};
use lava::bindless::{BindlessDescriptorHeap, DescriptorHandle};
use lava::vkobjects::image::ImageSize;
use lava::vkobjects::{buffer::Buffer, image::Image, image::ImageHandle};

pub type ResourceHandle = usize;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Event {
    // event: vk::Event,
    pub(super) pipeline_barrier_src_stages: vk::PipelineStageFlags2,
    pub(super) to_flush: vk::AccessFlags2,
    pub(super) invalidated_in_stage: [vk::AccessFlags2; 25],
    pub(super) layout: vk::ImageLayout,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ResourceDescription {
    pub(super) name: &'static str,
    pub(super) handle: ResourceHandle,
    pub(super) ty: ResourceDescriptionType,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub(super) enum ResourceDescriptionType {
    Image {
        size: ImageSize,
        usage: vk::ImageUsageFlags,
        format: Format,
    },
    Buffer {
        size: u64,
        usage: vk::BufferUsageFlags,
    },
}

#[derive(PartialEq, Clone, Debug)]
pub struct Resource {
    pub event: Event,
    pub descriptor: DescriptorHandle,
    pub ty: ResourceType,
}

impl Resource {
    pub(super) fn new(descriptor: DescriptorHandle, ty: ResourceType) -> Self {
        Self {
            descriptor,
            ty,
            event: Event {
                invalidated_in_stage: [vk::AccessFlags2::empty(); 25],
                pipeline_barrier_src_stages: vk::PipelineStageFlags2::empty(),
                to_flush: vk::AccessFlags2::default(),
                layout: vk::ImageLayout::UNDEFINED,
            },
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub(super) enum ResourceType {
    Image(Image),
    Buffer(Buffer),
    ExternalDescriptor,
    Uninitilized(usize),
}

impl ResourceType {
    pub(super) fn buffer(&self) -> &Buffer {
        match self {
            Self::Buffer(buffer) => buffer,
            _ => unreachable!(),
        }
    }
    pub(super) fn image(&self) -> &Image {
        match self {
            Self::Image(image) => image,
            _ => unreachable!(),
        }
    }
}

pub(super) trait Importable {
    fn resource(self) -> Resource;
}

impl Importable for Buffer {
    fn resource(self) -> Resource {
        let descriptor = BindlessDescriptorHeap::get().allocate_buffer_handle(&self);
        Resource::new(descriptor, ResourceType::Buffer(self))
    }
}

impl Importable for Image {
    fn resource(self) -> Resource {
        let descriptor = if self.usage.contains(vk::ImageUsageFlags::STORAGE) {
            Some(BindlessDescriptorHeap::get().allocate_image_handle(&self))
        } else if self.usage.contains(vk::ImageUsageFlags::SAMPLED) {
            Some(BindlessDescriptorHeap::get().allocate_texture_handle(&self))
        } else {
            None
        };
        Resource::new(
            descriptor.unwrap_or(DescriptorHandle(!0u32)),
            ResourceType::Image(self),
        )
    }
}
impl Importable for ImageHandle {
    fn resource(self) -> Resource {
        let descriptor = if self.usage.contains(vk::ImageUsageFlags::STORAGE) {
            BindlessDescriptorHeap::get().allocate_image_handle(&self)
        } else {
            BindlessDescriptorHeap::get().allocate_texture_handle(&self)
        };
        Resource::new(
            descriptor,
            ResourceType::Image(Image {
                allocation: None,
                size: self.size,
                format: self.format,
                image: self.image,
                usage: self.usage,
                view: self.view,
            }),
        )
    }
}
impl Importable for DescriptorHandle {
    fn resource(self) -> Resource {
        Resource::new(self, ResourceType::ExternalDescriptor)
    }
}
