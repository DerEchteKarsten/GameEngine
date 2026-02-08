use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bytemuck::Pod;
use lava::{
    buffer::{AsBuffer, Buffer, BufferUsageFlags, GpuBuffer, Location, slice::BufferSlice},
    state::Ctx,
};
use std::sync::Arc;

use crate::render::world::UploadBuffer;

impl<T: Pod + Copy + Send + Sync> AsBuffer for StorageBuffer<T> {
    type DataType = T;
    type Location = GpuBuffer;
    fn get_ref(&self) -> &Buffer<Self::DataType, Self::Location> {
        &self.buffer
    }
    fn get_mut(&mut self) -> &mut Buffer<Self::DataType, Self::Location> {
        &mut self.buffer
    }
}

pub struct StorageBuffer<T: Copy + Pod + Send + Sync> {
    buffer: Buffer<T>,
    buffer_task: Option<(u64, Task<Option<Buffer<T>>>)>,
    wirtes: Vec<(u64, Arc<[T]>)>,
    pub size: u64,
    pub queue_size: u64,
}

impl<T: Pod + Copy + Send + Sync> Default for StorageBuffer<T> {
    fn default() -> Self {
        Self {
            buffer: Buffer::with_alignment(BufferUsageFlags::STORAGE, 1024 * 1024, None).unwrap(),
            buffer_task: None,
            wirtes: Vec::new(),
            size: 0,
            queue_size: 0,
        }
    }
}

impl<T: Copy + Pod + Send + Sync> StorageBuffer<T> {
    pub fn new() -> Self {
        Self {
            buffer: Buffer::with_alignment(BufferUsageFlags::STORAGE, 1024 * 1024, None).unwrap(),
            wirtes: Vec::new(),
            size: 0,
            buffer_task: None,
            queue_size: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.size as usize / size_of::<T>()
    }

    pub fn queue_wirte(&mut self, data: Arc<[T]>) -> u64 {
        let offset = self.queue_size;
        let data_size = (data.len() * std::mem::size_of::<T>()) as u64;
        self.wirtes.push((offset, data));
        self.queue_size += data_size;
        offset
    }

    pub fn resolve_write(&mut self, queue: &mut UploadBuffer) {
        if let Some((new_size, task)) = &mut self.buffer_task {
            if let Some(buffer) = check_ready(task) {
                self.size = *new_size;
                if let Some(buffer) = buffer {
                    Ctx::delay_deletion(std::mem::replace(&mut self.buffer, buffer));
                }
            }
        } else {
            if !self.wirtes.is_empty() {
                let queue_size = self.queue_size;
                self.buffer_task = Some((
                    queue_size,
                    AsyncComputeTaskPool::get().spawn({
                        let writes = std::mem::take(&mut self.wirtes);
                        let allocator = queue.allocator.clone();
                        let mut buffer = self.buffer.whole();
                        async move {
                            let new_buffer = if buffer.size < queue_size {
                                let new_buffer = Buffer::<T>::with_alignment(
                                    BufferUsageFlags::STORAGE,
                                    queue_size.next_power_of_two(),
                                    None,
                                )
                                .unwrap();
                                Ctx::transfer_queue().execute_command_wait(|cmd| {
                                    cmd.copy_buffer(buffer, new_buffer.whole());
                                });
                                buffer = new_buffer.whole();
                                Some(new_buffer)
                            } else {
                                None
                            };

                            for (dst_offset, data) in writes {
                                let data_size = (data.len() * std::mem::size_of::<T>()) as u64;
                                let mut staging_mem = allocator.allocate_blocking(data_size).await;
                                staging_mem.mem_copy_from(BufferSlice::from(&*data));
                                Ctx::transfer_queue()
                                    .execute_command_async(|cmd| {
                                        cmd.copy_buffer(
                                            staging_mem,
                                            buffer.byte_offset(dst_offset),
                                        );
                                    })
                                    .await;
                                allocator.deallocate(staging_mem).await;
                            }
                            new_buffer
                        }
                    }),
                ));
            }
        }
    }
}
