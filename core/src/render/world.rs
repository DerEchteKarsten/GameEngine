use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::fmt::Debug;
use std::num::NonZeroU64;
use std::ops::{Deref, DerefMut, Range};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::thread::{JoinHandle, Thread};
use std::time::Duration;

use bevy::asset::{AsAssetId, LoadState};
use bevy::ecs::entity::Entities;
use bevy::ecs::system::SystemState;
use bevy::prelude::*;
use futures::channel::oneshot;
use lava::image::slice::AsImage;
use std::sync::Mutex;

use bevy::tasks::futures::check_ready;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, Scope, Task, TaskPool, block_on};
use bytemuck::Pod;
use futures::join;
use glam::Mat4;
use gpu_allocator::vulkan::Allocation;
use lava::buffer::Buffer;

use lava::buffer::slice::{BufferSlice, BufferView};
use lava::buffer::{AsBuffer, BufferUsageFlags, CpuBuffer, GpuBuffer, Location};
use lava::command_buffer::CommandBuffer;
use lava::image::format::R8Uint;
use lava::image::slice::{ImageSlice, TypeLessImageView};
use lava::image::usage::UsageSet;
use lava::state::{Ctx, Functions, raw_vulkan};
use lava::vkobjects::acceleration_structure::AccelerationStructure;
use lava::vkobjects::queue::{CommandBufferMemory, CommandPool, Fence, Gfx, Queue, Transfer};
use lava::{AccessFlags2, ImageLayout, PipelineStageFlags2, vkobjects};
use rand::random;
use smallvec::SmallVec;

use crate::assets::mesh::MeshletMesh;
use crate::assets::{Mesh, material::Material};
use crate::bindings::{AabbError, BvhNode, CullData, Meshlet, Vertex};
use crate::render::extract_param::Extract;
use crate::render::render::{
    CommandPools, FrameCount, QueueStrategie, Queues, Swapchain, SynchronizationResources,
    extract_camera,
};
use crate::render::{
    ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, Render, RenderStartup, RenderSystems,
};
use crate::ui::UiContext;

#[derive(Component, Clone)]
pub struct Model {
    pub model: Handle<Mesh>,
}

impl AsAssetId for Model {
    type Asset = Mesh;
    fn as_asset_id(&self) -> AssetId<Self::Asset> {
        self.model.id()
    }
}

#[derive(Resource)]
pub struct InstanceManager {
    pub transforms: Buffer<Mat4, CpuBuffer>,
    pub materials: Buffer<u32, CpuBuffer>,
    pub bvh_root_nodes: Buffer<u64, CpuBuffer>,
    pub aabbs: Buffer<AabbError, CpuBuffer>,
    pub instance_count: usize,
}

#[derive(Clone, Copy)]
struct Instance {
    transform: Mat4,
    material: u32,
    bvh_root: u64,
    aabb: AabbError,
}

impl InstanceManager {
    fn add_instance(&mut self, instance: Instance) {
        let slot = self.instance_count;
        self.instance_count += 1;
        self.transforms
            .element_offset(slot)
            .mem_copy_from(BufferSlice::from(&[instance.transform]));
        self.materials
            .element_offset(slot)
            .mem_copy_from(BufferSlice::from(&[instance.material]));
        self.bvh_root_nodes
            .element_offset(slot)
            .mem_copy_from(BufferSlice::from(&[instance.bvh_root]));
        self.aabbs
            .element_offset(slot)
            .mem_copy_from(BufferSlice::from(&[instance.aabb]));
    }
    fn clear(&mut self) {
        self.instance_count = 0;
    }
}

const STAGING_BUFFER_SIZE: usize = 16 * 1024 * 1024;

#[derive(Clone)]
enum Dst {
    Buffer(BufferSlice),
    Image(ImageSlice),
}
struct CopyRegion {
    completed: Option<oneshot::Sender<()>>,
    src: Vec<u8>,
    dst: Dst,
}

#[derive(Debug)]
enum TransferQueueStrategie {
    SingleQueue(Arc<Mutex<Queue<Gfx>>>),
    MultipleGfx(Queue<Gfx>),
    Transfer(Queue<Transfer>),
}

impl TransferQueueStrategie {
    fn with<R, F: FnOnce(&Queue<Transfer>) -> R>(&self, f: F) -> R {
        match &self {
            TransferQueueStrategie::SingleQueue(queue) => {
                let q = queue.lock().unwrap();
                let queue = &*q;
                f(unsafe { std::mem::transmute(queue) })
            }
            TransferQueueStrategie::MultipleGfx(queue) => f(unsafe { std::mem::transmute(queue) }),
            TransferQueueStrategie::Transfer(queue) => f(queue),
        }
    }
}

struct NonRebarResources {
    pool: CommandPool,
    fence: Fence,
    cmd: CommandBufferMemory,
    staging: Buffer<u8, CpuBuffer>,
    queue: TransferQueueStrategie,
}

#[derive(Debug)]
pub struct UploadQueue {
    copy_queue: Sender<CopyRegion>,
    thread: JoinHandle<()>,
}

static UPLOAD_QUEUE: OnceLock<UploadQueue> = OnceLock::new();

impl UploadQueue {
    fn flush(res: &NonRebarResources, regions: &Vec<(BufferSlice<u8, CpuBuffer>, Dst)>) {
        res.pool.reset();
        res.fence.reset();
        res.queue.with(|queue| {
            queue
                .execute_command(None, &res.cmd, Some(&res.fence), &[], &[], |cmd| {
                    for entry in regions {
                        match entry.1 {
                            Dst::Buffer(buff) => {
                                cmd.copy_buffer(entry.0, buff);
                                if Ctx::transfer_queue_index() != Ctx::gfx_queue_index() {
                                    cmd.buffer_barrier(
                                        buff,
                                        AccessFlags2::TRANSFER_WRITE,
                                        AccessFlags2::NONE,
                                        PipelineStageFlags2::TRANSFER,
                                        PipelineStageFlags2::NONE,
                                        Ctx::transfer_queue_index(),
                                        Ctx::gfx_queue_index(),
                                    );
                                }
                            }
                            Dst::Image(img) => {
                                cmd.copy_buffer_to_image(entry.0, img);
                                if Ctx::transfer_queue_index() != Ctx::gfx_queue_index() {
                                    cmd.image_barrier(
                                        img.view,
                                        AccessFlags2::TRANSFER_WRITE,
                                        AccessFlags2::NONE,
                                        PipelineStageFlags2::TRANSFER,
                                        PipelineStageFlags2::NONE,
                                        ImageLayout::UNDEFINED,
                                        ImageLayout::UNDEFINED,
                                        Ctx::transfer_queue_index(),
                                        Ctx::gfx_queue_index(),
                                    );
                                }
                            }
                        }
                    }
                })
                .unwrap();
            res.fence.wait();
        });
    }
    pub fn init(queues: &Queues) {
        let (sender, receiver) = std::sync::mpsc::channel::<CopyRegion>();
        let thread = if !Ctx::features().rebar {
            let pool;
            let queue = match (
                &queues.graphics,
                Ctx::gfx_queue_index() == Ctx::transfer_queue_index(),
            ) {
                (QueueStrategie::Multiple(_), true) => {
                    let queue = Queue::new().unwrap();
                    pool = queue.create_pool();
                    TransferQueueStrategie::MultipleGfx(queue)
                }
                (QueueStrategie::Single(queue), true) => {
                    pool = queue.lock().unwrap().create_pool();
                    TransferQueueStrategie::SingleQueue(queue.clone())
                }
                (_, false) => {
                    let queue = Queue::new().unwrap();
                    pool = queue.create_pool();
                    TransferQueueStrategie::Transfer(queue)
                }
            };
            let res = NonRebarResources {
                queue,
                cmd: pool.create_command_buffer(),
                fence: Fence::new(),
                pool,
                staging: Buffer::new(STAGING_BUFFER_SIZE).unwrap(),
            };
            std::thread::spawn(move || {
                #[cfg(feature = "trace")]
                let _span = log::info_span!("Upload Thread").entered();
                let mut staging_slice = res.staging.whole().clone();
                let mut regions = Vec::new();
                loop {
                    let mut item = if let Ok(item) = receiver.recv_timeout(Duration::from_millis(1))
                    {
                        item
                    } else {
                        if !regions.is_empty() {
                            Self::flush(&res, &regions);
                        }
                        receiver.recv().unwrap()
                    };
                    let mut src_remaining_bytes = item.src.len() as u64;
                    while {
                        let staging_remaining_bytes = staging_slice.size;
                        unsafe {
                            staging_slice.cpu_ptr().copy_from(
                                item.src.as_ptr(),
                                src_remaining_bytes.min(staging_remaining_bytes) as usize,
                            )
                        };
                        regions.push((
                            staging_slice
                                .num_bytes(src_remaining_bytes.min(staging_remaining_bytes)),
                            item.dst.clone(),
                        ));
                        staging_slice = staging_slice.add_byte_offset(src_remaining_bytes);
                        src_remaining_bytes =
                            src_remaining_bytes.saturating_sub(staging_remaining_bytes);
                        staging_slice.size == 0
                    } {
                        Self::flush(&res, &regions);
                        regions.clear();
                        staging_slice = res.staging.whole();
                    }
                    let _ = item.completed.take().unwrap().send(());
                }
            })
        } else {
            std::thread::spawn(move || {
                #[cfg(feature = "trace")]
                let _span = log::info_span!("Upload Thread").entered();
                for mut item in receiver.iter() {
                    match &item.dst {
                        Dst::Buffer(buff) => {
                            buff.mem_copy_from(BufferSlice::from(item.src.as_slice()));
                        }
                        Dst::Image(image) => {
                            let regions = [raw_vulkan::MemoryToImageCopyEXT::default()
                                .host_pointer(item.src.as_ptr().cast())
                                .image_extent(image.extend)
                                .image_offset(image.offset)
                                .image_subresource(image.view.subresource_layers())
                                .memory_image_height(image.extend.height as u32)
                                .memory_row_length(image.extend.width as u32)];
                            let copy_memory_to_image_info =
                                raw_vulkan::CopyMemoryToImageInfoEXT::default()
                                    .dst_image(image.view.image)
                                    .dst_image_layout(raw_vulkan::ImageLayout::TRANSFER_DST_OPTIMAL)
                                    .regions(&regions);
                            unsafe {
                                Functions::host_image_copy()
                                    .copy_memory_to_image(&copy_memory_to_image_info)
                                    .unwrap()
                            };
                        }
                    }
                    let _ = item.completed.take().unwrap().send(());
                }
            })
        };

        UPLOAD_QUEUE
            .set(Self {
                copy_queue: sender,
                thread,
            })
            .unwrap();
    }

    pub fn push_buffer<T: Copy + Pod + Send + Sync + Debug>(
        src: Vec<T>,
        buffer: BufferSlice<T, GpuBuffer>,
    ) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        UPLOAD_QUEUE
            .wait()
            .copy_queue
            .send(CopyRegion {
                completed: Some(sender),
                src: bytemuck::try_cast_vec(src).unwrap(),
                dst: Dst::Buffer(buffer.cast()),
            })
            .unwrap();
        receiver
    }
    pub fn push_image<F: lava::image::format::Format, U: UsageSet>(
        src: Vec<u8>,
        image: ImageSlice<F, U>,
    ) -> oneshot::Receiver<()> {
        let (sender, receiver) = oneshot::channel();
        UPLOAD_QUEUE
            .wait()
            .copy_queue
            .send(CopyRegion {
                completed: Some(sender),
                src,
                dst: Dst::Image(image.cast()),
            })
            .unwrap();
        receiver
    }
}

pub(super) fn init_world(mut cmd: Commands) {
    cmd.insert_resource(InstanceManager {
        aabbs: Buffer::new(1024 * 10).unwrap(),
        transforms: Buffer::new(1024 * 10).unwrap(),
        bvh_root_nodes: Buffer::new(1024 * 10).unwrap(),
        materials: Buffer::new(1024 * 10).unwrap(),
        instance_count: 0,
    });
    cmd.init_resource::<FrameCount>();
}

fn extract_meshlet_instances(
    mut instance_manager: ResMut<InstanceManager>,
    mut main_world: ResMut<MainWorld>,
    mut system_state: Local<
        Option<
            SystemState<(
                Query<(&Model, &GlobalTransform)>,
                Res<AssetServer>,
                ResMut<Assets<Mesh>>,
                MessageReader<AssetEvent<Mesh>>,
            )>,
        >,
    >,
) {
    instance_manager.clear();
    if system_state.is_none() {
        *system_state = Some(SystemState::new(&mut main_world));
    }
    let system_state = system_state.as_mut().unwrap();
    let (instances_query, asset_server, mut assets, mut asset_events) =
        system_state.get_mut(&mut main_world);

    for asset_event in asset_events.read() {
        if let AssetEvent::Unused { id } | AssetEvent::Modified { id } = asset_event {
            todo!();
        }
    }

    for (instance, transform) in &instances_query {
        if let Some(mesh) = assets.get(&instance.model) {
            let transform = transform.affine();
            for (i, mesh_index) in mesh.instance_mesh.iter().enumerate() {
                instance_manager.add_instance(Instance {
                    aabb: mesh.meshes[*mesh_index as usize].aabb,
                    bvh_root: mesh.meshes[*mesh_index as usize].buffer.address,
                    material: mesh.instance_materials[i],
                    transform: mesh.instance_transforms[i],
                });
            }
        }
    }
}

pub fn WorldPlugin(app: &mut App) {
    app.add_systems(RenderStartup, init_world)
        .add_systems(ExtractSchedule, (extract_meshlet_instances, extract_camera));
}
