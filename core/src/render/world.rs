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

use bevy::app::App;
use bevy::asset::{AsAssetId, AssetId, Assets, Handle, LoadState};
use bevy::ecs::component::Component;
use bevy::ecs::entity::{Entities, Entity};
use bevy::ecs::hierarchy::ChildOf;
use bevy::ecs::query::{Has, With};
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::{Commands, If, Local, Query, Res, ResMut, Single, SystemState};
use bevy::ecs::world::EntityMut;
use bevy::log;
use bevy::reflect::Reflect;
use bevy::transform::components::GlobalTransform;
use bevy::window::Window;
use bitflags::bitflags;
use futures::channel::oneshot;
use lava::image::Image;
use lava::image::slice::AsImage;
use std::sync::Mutex;

use bevy::tasks::futures::check_ready;
use bevy::tasks::futures_lite::future;
use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, Scope, Task, TaskPool, block_on};
use bytemuck::Pod;
use futures::join;
use glam::{IVec2, Mat4, Quat, UVec2, Vec2, Vec3, Vec4};
use gpu_allocator::vulkan::Allocation;
use lava::buffer::Buffer;

use lava::buffer::slice::BufferSlice;
use lava::command_buffer::CommandBuffer;
use lava::image::format::R8Uint;
use lava::image::slice::ImageSlice;
use lava::image::usage::UsageSet;
use lava::state::{Ctx, Functions, raw_vulkan};
use lava::vkobjects::acceleration_structure::AccelerationStructure;
use lava::vkobjects::queue::{CommandBufferMemory, CommandPool, Fence, Gfx, Queue, Transfer};
use lava::{AccessFlags2, ImageLayout, PipelineStageFlags2, vkobjects};
use rand::random;
use smallvec::SmallVec;
use tracing::error;

use crate::assets::mesh::MeshletMesh;
use crate::assets::{GpuMeshletMesh, MeshHeader};
use crate::assets::{Scene, material::Material};
use crate::bindings::{
    self, AabbError, BvhNode, CullData, Gizzmo, InstanceBvhRoot, Meshlet, Vertex,
};
use crate::editor::picking::Selected;
use crate::editor::viewport::ViewPort;
use crate::render::extract_param::Extract;
use crate::render::render::{
    CommandPools, FrameCount, QueueStrategie, Queues, Swapchain, SynchronizationResources,
    extract_camera,
};
use crate::render::{
    ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, Render, RenderStartup, RenderSystems,
};
use crate::scene::Instance;
use crate::ui::UiContext;

#[derive(Resource)]
pub struct InstanceManager {
    pub transforms: Buffer<Mat4>,
    pub bvh_root_nodes: Buffer<u64>,
    pub headers: Buffer<bindings::InstanceHeader>,
    pub aabbs: Buffer<AabbError>,
    pub flags: Buffer<u32>,
    pub instance_count: usize,
    pub any_outlined: bool,
    pending_instances: Vec<TempInstance>,
}

bitflags! {
    #[derive(Copy, Clone)]
    pub struct InstanceFlags: u32 {
        const OUTLINE = 0b00000001;
        // const B = 0b00000010;
        // const C = 0b00000100;
    }
}

#[derive(Clone, Copy)]
struct TempInstance {
    flags: InstanceFlags,
    transform: Mat4,
    bvh_root: u64,
    header: MeshHeader,
}

pub const MAX_INSTANCES: usize = 8 * 1024;

const STAGING_BUFFER_SIZE: usize = 16 * 1024 * 1024;

enum Dst {
    Buffer(Buffer<u8>),
    Image(Image),
}

enum DstRef<'a> {
    Buffer(BufferSlice<'a, u8>),
    Image(ImageSlice<'a>),
}

enum SendBack {
    Buffer(oneshot::Sender<Buffer<u8>>),
    Image(oneshot::Sender<Image>),
}

struct CopyRegion {
    src: Vec<u8>,
    dst: Option<Dst>,
    send_back: Option<SendBack>,
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
    staging: Buffer<u8>,
    queue: TransferQueueStrategie,
}

#[derive(Debug)]
pub struct UploadQueue {
    copy_queue: Sender<CopyRegion>,
    thread: JoinHandle<()>,
}

static UPLOAD_QUEUE: OnceLock<UploadQueue> = OnceLock::new();

impl UploadQueue {
    fn send_back(mut item: CopyRegion) {
        match item.send_back.take().unwrap() {
            SendBack::Buffer(buff) => {
                let Dst::Buffer(buffer) = item.dst.take().unwrap() else {
                    unreachable!()
                };
                if let Err(_) = buff.send(buffer) {
                    error!(
                        "Receiver was dropped, buffer could not be sent back and will be dropped"
                    );
                }
            }
            SendBack::Image(imag) => {
                let Dst::Image(image) = item.dst.take().unwrap() else {
                    unreachable!()
                };
                if let Err(_) = imag.send(image) {
                    error!(
                        "Receiver was dropped, image could not be sent back and will be dropped"
                    );
                }
            }
        }
    }
    fn flush(res: &NonRebarResources, mut regions: Vec<(BufferSlice<u8>, DstRef)>) {
        res.pool.reset();
        res.fence.reset();
        res.queue.with(move |queue| {
            queue
                .execute_command(None, &res.cmd, Some(&res.fence), &[], &[], |cmd| {
                    for entry in regions.iter_mut() {
                        match &entry.1 {
                            DstRef::Buffer(buff) => {
                                cmd.copy_buffer(entry.0, *buff);
                                if Ctx::transfer_queue_index() != Ctx::gfx_queue_index() {
                                    cmd.buffer_barrier(
                                        *buff,
                                        AccessFlags2::TRANSFER_WRITE,
                                        AccessFlags2::NONE,
                                        PipelineStageFlags2::TRANSFER,
                                        PipelineStageFlags2::NONE,
                                        Ctx::transfer_queue_index(),
                                        Ctx::gfx_queue_index(),
                                    );
                                }
                            }
                            DstRef::Image(view) => {
                                cmd.copy_buffer_to_image(entry.0, *view);
                                if Ctx::transfer_queue_index() != Ctx::gfx_queue_index() {
                                    cmd.image_barrier(
                                        view.view,
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
                staging: Buffer::new(STAGING_BUFFER_SIZE, true).unwrap(),
            };

            std::thread::spawn(move || {
                #[cfg(feature = "trace")]
                let _span = log::info_span!("Upload Thread").entered();
                let mut staging_slice = res.staging.range(..).clone();
                let mut regions = Vec::new();
                let mut need_send_back = Vec::new();
                loop {
                    let item = if let Ok(item) = receiver.recv_timeout(Duration::from_millis(1)) {
                        item
                    } else {
                        if !regions.is_empty() {
                            let len = regions.capacity();
                            Self::flush(
                                &res,
                                std::mem::replace(&mut regions, Vec::with_capacity(len)),
                            );
                            for i in need_send_back.drain(..) {
                                Self::send_back(i);
                            }
                        }
                        receiver.recv().unwrap()
                    };
                    let mut src_remaining_bytes = item.src.len() as u64;

                    let mut flushed = false;
                    while {
                        let staging_remaining_bytes = staging_slice.size;
                        let copy_size = src_remaining_bytes.min(staging_remaining_bytes) as usize;
                        unsafe { staging_slice.ptr().copy_from(item.src.as_ptr(), copy_size) };
                        regions.push((
                            staging_slice.byte_range(
                                (staging_slice.size - staging_remaining_bytes) as usize
                                    ..(staging_slice.size
                                        - (staging_remaining_bytes
                                            .saturating_sub(copy_size as u64)))
                                        as usize,
                            ),
                            match item.dst.as_ref().unwrap() {
                                Dst::Buffer(buffer) => {
                                    let slice = buffer.range(
                                        (buffer.size() - src_remaining_bytes) as usize
                                            ..(buffer.size()
                                                - (src_remaining_bytes
                                                    .saturating_sub(copy_size as u64)))
                                                as usize,
                                    );
                                    DstRef::Buffer(unsafe { std::mem::transmute(slice) })
                                }
                                Dst::Image(image) => {
                                    if item.src.len() > STAGING_BUFFER_SIZE {
                                        todo!("KOPFSCHMERZEN")
                                    }
                                    DstRef::Image(unsafe { std::mem::transmute(image.whole()) })
                                }
                            },
                        ));
                        staging_slice = staging_slice.byte_range(copy_size..);
                        src_remaining_bytes =
                            src_remaining_bytes.saturating_sub(staging_remaining_bytes);
                        staging_slice.size == 0
                    } {
                        let len = regions.capacity();
                        flushed = true;
                        Self::flush(
                            &res,
                            std::mem::replace(&mut regions, Vec::with_capacity(len)),
                        );
                        staging_slice = res.staging.range(..);
                    }
                    if flushed {
                        Self::send_back(item);
                        for i in need_send_back.drain(..) {
                            Self::send_back(i);
                        }
                    } else {
                        need_send_back.push(item);
                    }
                }
            })
        } else {
            std::thread::spawn(move || {
                #[cfg(feature = "trace")]
                let _span = log::info_span!("Upload Thread").entered();
                for item in receiver.iter() {
                    match item.dst.as_ref().unwrap() {
                        Dst::Buffer(buff) => {
                            buff.range(..).copy_from(item.src.as_slice());
                        }
                        Dst::Image(image) => {
                            let regions = [raw_vulkan::MemoryToImageCopyEXT::default()
                                .host_pointer(item.src.as_ptr().cast())
                                .image_extent(image.extent)
                                .image_offset(raw_vulkan::Offset3D { x: 0, y: 0, z: 0 })
                                .image_subresource(image.view().subresource_layers())
                                .memory_image_height(image.extent.height as u32)
                                .memory_row_length(image.extent.width as u32)];
                            let copy_memory_to_image_info =
                                raw_vulkan::CopyMemoryToImageInfoEXT::default()
                                    .dst_image(image.image)
                                    .dst_image_layout(raw_vulkan::ImageLayout::TRANSFER_DST_OPTIMAL)
                                    .regions(&regions);
                            unsafe {
                                Functions::host_image_copy()
                                    .copy_memory_to_image(&copy_memory_to_image_info)
                                    .unwrap()
                            };
                        }
                    }
                    Self::send_back(item);
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
        buffer: Buffer<T>,
    ) -> oneshot::Receiver<Buffer<T>> {
        let (sender, receiver) = oneshot::channel();
        UPLOAD_QUEUE
            .wait()
            .copy_queue
            .send(CopyRegion {
                src: bytemuck::try_cast_vec(src).unwrap(),
                dst: Some(Dst::Buffer(buffer.cast())),
                send_back: Some(SendBack::Buffer(sender)),
            })
            .unwrap();
        unsafe { std::mem::transmute(receiver) }
    }
    pub fn push_image<F: lava::image::format::Format, U: UsageSet>(
        src: Vec<u8>,
        image: Image<F, U>,
    ) -> oneshot::Receiver<Image<F, U>> {
        let (sender, receiver) = oneshot::channel();
        UPLOAD_QUEUE
            .wait()
            .copy_queue
            .send(CopyRegion {
                src,
                dst: Some(Dst::Image(image.cast())),
                send_back: Some(SendBack::Image(sender)),
            })
            .unwrap();
        unsafe { std::mem::transmute(receiver) }
    }
}

pub(super) fn init_world(mut cmd: Commands) {
    cmd.insert_resource(InstanceManager {
        headers: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        aabbs: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        transforms: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        bvh_root_nodes: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        // materials: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        instance_count: 0,
        any_outlined: false,
        flags: Buffer::new(MAX_INSTANCES * FRAMES_IN_FLIGHT, true).unwrap(),
        pending_instances: Vec::with_capacity(MAX_INSTANCES),
    });
    cmd.init_resource::<FrameCount>();
}

fn extract_meshlet_instances(
    mut instance_manager: ResMut<InstanceManager>,
    instances: Extract<Query<(&Instance, &GlobalTransform, Has<Selected>)>>,
    meshes: Extract<Res<Assets<GpuMeshletMesh>>>,
) {
    instance_manager.any_outlined = false;
    for (instance, transform, selected) in &instances {
        if let Some(mesh) = meshes.get(&instance.mesh) {
            let mat = transform.to_matrix();
            let flags = if selected {
                instance.flags | InstanceFlags::OUTLINE
            } else {
                instance.flags
            };
            instance_manager.any_outlined |= flags.contains(InstanceFlags::OUTLINE);
            instance_manager.pending_instances.push(TempInstance {
                bvh_root: mesh.buffer.address,
                header: mesh.header,
                transform: mat,
                flags,
            });
        }
    }
}

fn wirte_instances(mut instances: ResMut<InstanceManager>, frame: Res<FrameCount>) {
    let frame_in_flight = frame.frame_in_flight();
    for slot in 0..instances.pending_instances.len() {
        let instance = instances.pending_instances[slot];
        instances.transforms[slot + frame_in_flight * MAX_INSTANCES] = instance.transform;
        instances.bvh_root_nodes[slot + frame_in_flight * MAX_INSTANCES] = instance.bvh_root;
        instances.aabbs[slot + frame_in_flight * MAX_INSTANCES] = AabbError {
            center_and_error: Vec3::from_array(instance.header.aabb.center).extend(0.0),
            half_extent: Vec3::from_array(instance.header.aabb.half_extend).extend(0.0),
        };
        instances.headers[slot + frame_in_flight * MAX_INSTANCES] = bindings::InstanceHeader {
            meshlet_offset: instance.header.meshlet_offset as u64 + instance.bvh_root,
            cull_data_offset: instance.header.cull_data_offset as u64 + instance.bvh_root,
        };
        instances.flags[slot + frame_in_flight * MAX_INSTANCES] = instance.flags.bits();
    }
    instances.instance_count = instances.pending_instances.len();
    instances.pending_instances.clear();
}

fn extract_view_port(
    mut cmd: Commands,
    view_port: Extract<Option<Res<ViewPort>>>,
    window: Extract<Single<&Window>>,
) {
    cmd.insert_resource(view_port.as_deref().cloned().unwrap_or(ViewPort {
        view_pos: IVec2::ZERO,
        view_size: window.physical_size(),
        scissor_size: window.physical_size(),
        scissor_pos: UVec2::ZERO,
        focused: true,
    }));
}

pub fn WorldPlugin(app: &mut App) {
    app.add_systems(RenderStartup, init_world)
        .add_systems(
            ExtractSchedule,
            (extract_meshlet_instances, extract_camera, extract_view_port),
        )
        .add_systems(Render, (wirte_instances).in_set(RenderSystems::PreRender));
}
