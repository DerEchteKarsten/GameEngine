//! Bindless slots of core's images and swapchain: an allocator per descriptor set that gets its slots back through lava's bindless drop hook. Textures take every sampled slot from `TEXTURE_SLOTS_BASE` up.
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

    /// An index below `end`.
    fn alloc(&self, end: u32) -> Result<u32> {
        if let Some(index) = self.free.lock().unwrap().pop() {
            return Ok(index);
        }
        self.next
            .try_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                (next < end).then_some(next + 1)
            })
            .map_err(|_| anyhow!("all {end} bindless slots are in use"))
    }

    fn release(&self, index: u32) {
        self.free.lock().unwrap().push(index);
    }
}

static SAMPLED: Slots = Slots::new();
static STORAGE: Slots = Slots::new();

/// A free sampled slot. A slot comes back when the image bound there is destroyed.
pub fn sampled_slot() -> Result<u32> {
    SAMPLED.alloc(TEXTURE_SLOTS_BASE)
}

/// A free storage slot. A slot comes back when the image or swapchain bound there is destroyed.
pub fn storage_slot() -> Result<u32> {
    STORAGE.alloc(max_storage_images())
}

/// Hooks the allocators into lava, call right after `lava::init`. Lava calls the hook when an
/// image or swapchain is destroyed, which is after every frame that may use it
/// (`lava::epoch`), so no slot is handed out while a pending frame reads it.
pub fn init() {
    lava::bindless::on_drop(release).unwrap();
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
        assert_eq!(slots.alloc(3).unwrap(), 0);
        assert_eq!(slots.alloc(3).unwrap(), 1);
        slots.release(0);
        assert_eq!(slots.alloc(3).unwrap(), 0);
        assert_eq!(slots.alloc(3).unwrap(), 2);
        assert!(slots.alloc(3).is_err());
        slots.release(1);
        assert_eq!(slots.alloc(3).unwrap(), 1);
    }
}
