//! Bindless slots of core's images: an allocator per descriptor set that gets its slots back through lava's image drop hook. Textures take every sampled slot from `TEXTURE_SLOTS_BASE` up.
use std::sync::{
    Mutex,
    atomic::{AtomicU32, Ordering},
};

use anyhow::{Result, anyhow};
use lava::bindless::{BindlessHandle, NULL_HANDLE, max_storage_images};

/// `GpuTexture` assets: `TEXTURE_SLOTS_BASE + asset index`, up to `max_sampled_images()`.
pub const TEXTURE_SLOTS_BASE: u32 = 1024;

/// Hands out the indices of one set: freed ones first, then never-used ones.
struct Slots {
    next: AtomicU32,
    free: Mutex<Vec<u32>>,
}

impl Slots {
    const fn new() -> Self {
        Self {
            next: AtomicU32::new(0),
            free: Mutex::new(Vec::new()),
        }
    }

    /// `count` consecutive indices below `end`; only single ones come from the free list.
    fn alloc(&self, count: u32, end: u32) -> Result<u32> {
        if count == 1
            && let Some(index) = self.free.lock().unwrap().pop()
        {
            return Ok(index);
        }
        self.next
            .try_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                (next + count <= end).then_some(next + count)
            })
            .map_err(|_| anyhow!("all {end} bindless slots are in use"))
    }

    fn release(&self, index: u32) {
        self.free.lock().unwrap().push(index);
    }
}

static SAMPLED: Slots = Slots::new();
static STORAGE: Slots = Slots::new();

/// A free sampled slot. A slot comes back when the `Image` bound there is dropped.
pub fn sampled_slot() -> Result<u32> {
    SAMPLED.alloc(1, TEXTURE_SLOTS_BASE)
}

/// `count` consecutive free storage slots (more than one for the swapchain).
pub fn storage_slots(count: u32) -> Result<u32> {
    STORAGE.alloc(count, max_storage_images())
}

/// Hooks the allocators into lava, call right after `lava::init`. Retired images (`Frame::retire`)
/// drop only after their frame, so no slot is handed out while a pending frame reads it.
pub fn init() {
    lava::image::on_drop(release).unwrap();
}

fn release(handle: BindlessHandle) {
    // Textures are written at their slots without a handle, but never hand those out here.
    if handle.descriptor_index_set0 < TEXTURE_SLOTS_BASE {
        SAMPLED.release(handle.descriptor_index_set0);
    }
    if handle.descriptor_index_set1 != NULL_HANDLE {
        STORAGE.release(handle.descriptor_index_set1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freed_slots_are_handed_out_first() {
        let slots = Slots::new();
        assert_eq!(slots.alloc(1, 4).unwrap(), 0);
        assert_eq!(slots.alloc(1, 4).unwrap(), 1);
        slots.release(0);
        assert_eq!(slots.alloc(1, 4).unwrap(), 0);
        // Blocks only come from never-used slots.
        slots.release(1);
        assert_eq!(slots.alloc(2, 4).unwrap(), 2);
        assert!(slots.alloc(1, 4).is_ok());
        assert!(slots.alloc(1, 4).is_err());
    }
}
