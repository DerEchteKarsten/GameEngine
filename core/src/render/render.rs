//! Per-frame rendering: swapchain, sync, render resources/settings, the shared scene passes and windowed frame recording.
use bevy::ecs::change_detection::{DetectChanges, DetectChangesMut};
use bevy::{ecs::reflect::ReflectResource, reflect::Reflect, time::Time};
use bytemuck::Pod;
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    mem::offset_of,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use bevy::{
    app::{App, Update},
    asset::Assets,
    ecs::{
        query::With,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Local, Query, Res, ResMut, Single, SystemState},
        world::{Mut, World},
    },
    log::*,
    math::Rect,
    transform::components::{GlobalTransform, Transform},
    window::{PrimaryWindow, Window},
};
use glam::{Mat4, UVec2, Vec2, Vec3, Vec4, Vec4Swizzles};
use lava::{
    bindless::{BindlessHandle, NULL_HANDLE},
    buffer::{
        Buffer,
        usage::{Index, Indirect, StorageIndirect},
    },
    command_buffer::{BindingOutput, Blend, CommandBuffer, RasterState},
    image::{
        Image,
        format::{
            self, ColorAspect, D32Sfloat, HasTexels, R8Unorm, R16G16B16A16Sfloat, R32G32B32A32Uint,
        },
        slice::ImageView,
        usage::{
            ColorAttachmentSampled, ColorAttachmentSampledStorageTransferDst,
            ColorAttachmentStorageTransferDst, DepthAttachmentSampled, IsColorAttachment,
            IsStorage,
        },
    },
    state::Ctx,
    epoch::Epoch,
    vkobjects::{
        self,
        queue::{Binary, Frame, FrameSlot, Gfx, Queue, Semaphore, Stage},
    },
};

#[cfg(feature = "profiling")]
use crate::profiler::{self, GpuTimings};
use crate::{
    INITIAL_WINDOW_SIZE,
    assets::texture::{GpuTexture, texture_index},
    bindless,
    editor::{
        gizzmos::{GizzmoResources, Gizzmos, MAX_GIZZMOS},
        viewport::ViewPort,
    },
    id,
    render::{
        ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, PrimarySurface, Render, RenderApp,
        RenderStartup, RenderSystems,
        extract_param::Extract,
        world::{FLAG_WORDS, InstanceManager, MAX_INSTANCES, extract_view_port},
    },
    scene::{self, camera::Camera},
    ui::{UiResources, builder::UiBuilder},
};
use crate::bindings::{
    BvhCull, Composite, DrawOutline, InstanceBvhRoot, InstanceCull, InstanceMeshletIndex,
    InstancedMeshlet, MeshletDraw, Raster, RasterBlended, RasterOutline, RasterUi,
    TraversalVariables,
};

/// `MESHLET_STRIDE` in `datatypes.slang`: `groups_y` of every `MeshletDraw`.
const MESHLET_STRIDE: u32 = 64;
/// The `OVERFLOW_*` bits of `TraversalVariables.overflow` in `datatypes.slang`: what
/// `bvh_cull` dropped for lack of space, with the buffer of `RenderResources` that
/// `RenderResources::fit` then doubles, up to `MAX_CULL_BUFFER` entries.
const OVERFLOWS: [(u32, &str); 5] = [
    (1, "bvh_node_stack"),
    (2, "candidate_meshlets"),
    (4, "meshlets"),
    (8, "blended_meshlets"),
    (16, "outlined_meshlets"),
];
/// Entries the cull buffers start with (sponza draws about 10k meshlets) and may grow to.
const CULL_BUFFER: usize = 64 * 1024;
const MAX_CULL_BUFFER: usize = 2 * 1024 * 1024;
/// `MAX_RADIUS` in `draw_outline.slang`.
pub const MAX_OUTLINE_RADIUS: f32 = 8.0;

/// The frame slots and the queue lease they are submitted through.
#[derive(Resource)]
pub struct FrameSlots {
    pub queue: Queue<Gfx>,
    pub slots: [FrameSlot; FRAMES_IN_FLIGHT],
}

/// The epoch of the frame being extracted, reserved before the extract reads anything the
/// frame's GPU work reaches (instance addresses, material slots, bindless textures), which
/// `render` hands to the frame slot. A frame that is skipped completes it at the next extract.
#[derive(Resource)]
pub struct FrameEpoch(pub Option<Epoch>);

#[derive(Resource, Default)]
pub struct SynchronizationResources {
    pub image_available: [Semaphore<Binary>; FRAMES_IN_FLIGHT],
    pub render_finished: Vec<Semaphore<Binary>>,
}

#[derive(Resource, Default)]
pub struct FrameCount(pub u64);

impl FrameCount {
    pub fn frame_in_flight(&self) -> usize {
        self.0 as usize % FRAMES_IN_FLIGHT
    }
}

#[derive(Resource)]
pub struct Swapchain {
    // Declared before the surface so it is destroyed first.
    pub swpachain: lava::vkobjects::swapchain::Swapchain,
    pub surface: vkobjects::surface::Surface,
    pub image_index: u32,
    /// The window has no extent to present to: `render` skips its frames.
    pub minimized: bool,
}

impl Drop for Swapchain {
    fn drop(&mut self) {
        // The last frames may still present it, which their frame slots don't wait for.
        if let Err(err) = lava::wait_idle() {
            error!("failed to wait for the device before dropping the swapchain: {err}");
        }
    }
}

/// The least time between two frames of the render world, slept before each: the frame
/// limit the `RenderPlugin` was given for an open and for a minimised window.
#[derive(Resource)]
pub struct FrameLimit {
    pub open: Option<Duration>,
    pub minimized: Option<Duration>,
    last: Instant,
}

impl FrameLimit {
    pub fn new(open: Option<Duration>, minimized: Option<Duration>) -> Self {
        Self {
            open,
            minimized,
            last: Instant::now(),
        }
    }

    /// Sleeps until the frame may start.
    fn wait(&mut self, minimized: bool) {
        let limit = if minimized { self.minimized } else { self.open };
        if let Some(limit) = limit
            && let Some(left) = limit.checked_sub(self.last.elapsed())
        {
            std::thread::sleep(left);
        }
        self.last = Instant::now();
    }
}

impl Swapchain {
    pub fn image(&self) -> ImageView<'_, format::Dynamic, ColorAttachmentStorageTransferDst> {
        self.swpachain.image(self.image_index)
    }
}

impl std::ops::Deref for Swapchain {
    type Target = lava::vkobjects::swapchain::Swapchain;
    fn deref(&self) -> &Self::Target {
        &self.swpachain
    }
}

pub fn aquire_swapchain_image(
    mut sync: ResMut<SynchronizationResources>,
    frame: Res<FrameCount>,
    mut swapchain: ResMut<Swapchain>,
) {
    let frame = frame.frame_in_flight();
    swapchain.image_index = match swapchain.aquire_image(&sync.image_available[frame], None) {
        Ok(index) => index,
        // The surface changed under the swapchain (a resize the window hasn't reported yet):
        // a swapchain of the surface's current extent can be acquired from, unless the
        // window was minimised since the extract, which skips this frame.
        Err(lava::Error::Vulkan(lava::VkResult::ERROR_OUT_OF_DATE_KHR)) => {
            let size = swapchain.swpachain.size;
            if !recreate_swapchain(&mut swapchain, &mut sync, size) {
                return;
            }
            swapchain
                .aquire_image(&sync.image_available[frame], None)
                .unwrap()
        }
        Err(err) => panic!("failed to acquire a swapchain image: {err}"),
    };
}

/// Recreates the swapchain at `size`, binds its images at new slots and gives every image a
/// new `render_finished` semaphore. The old swapchain and semaphores are dropped: they are
/// destroyed, and the old slots freed, once the frames and presents that may use them are done
/// (`lava::epoch`). Sets `minimized` instead, and returns false, when the
/// window has no extent to create one for.
fn recreate_swapchain(
    swapchain: &mut Swapchain,
    sync: &mut SynchronizationResources,
    size: UVec2,
) -> bool {
    let Swapchain {
        swpachain,
        surface,
        minimized,
        ..
    } = swapchain;
    match swpachain.recreate(surface, size) {
        Ok(_old) => {
            let num_images = swpachain.num_images();
            let semaphores = (0..num_images).map(|_| Semaphore::new().unwrap());
            sync.render_finished = semaphores.collect();
        }
        Err(_) if surface.is_minimised() => {
            *minimized = true;
            return false;
        }
        Err(err) => panic!("failed to recreate the swapchain: {err}"),
    }
    *minimized = false;
    let slots = (0..swpachain.num_images()).map(|_| bindless::storage_slot().unwrap());
    swpachain.bind_storage(slots);
    true
}

pub fn resize_swapchain(
    mut swapchain: ResMut<Swapchain>,
    mut sync: ResMut<SynchronizationResources>,
    window: Extract<Single<&Window, With<PrimaryWindow>>>,
    settings: Extract<Option<Res<RenderSettings>>>,
) {
    let swapchain = &mut *swapchain;
    let mut recreate = false;
    if let Some(settings) = settings.as_ref()
        && settings.present_mode != swapchain.swpachain.present_mode
    {
        swapchain.swpachain.present_mode = settings.present_mode;
        recreate = true;
    }
    let size = window.physical_size();
    if size.x == 0 || size.y == 0 {
        swapchain.minimized = true;
        return;
    }
    // A swapchain the window was minimised with has to be made again when it comes back.
    if size != swapchain.swpachain.size || swapchain.minimized {
        info!("Resized Swapchain");
        recreate = true;
    }
    if recreate {
        recreate_swapchain(swapchain, &mut sync, size);
    }
}

#[derive(Resource)]
pub(crate) struct RenderCamera {
    pub(crate) camera: Camera,
    pub(crate) transform: Transform,
}

/// The scene is drawn from the first camera; without one, only the UI is drawn.
pub fn extract_camera(mut cmd: Commands, cameras: Extract<Query<(&Camera, &GlobalTransform)>>) {
    let mut cameras = cameras.iter();
    match cameras.next() {
        Some((camera, transform)) => {
            if cameras.next().is_some() {
                warn_once!("several cameras: the scene is drawn from the first");
            }
            cmd.insert_resource(RenderCamera {
                camera: *camera,
                transform: transform.compute_transform(),
            });
        }
        None => cmd.remove_resource::<RenderCamera>(),
    }
}

/// Bindless index of the sky texture, `NULL_HANDLE` while it isn't loaded.
#[derive(Resource)]
pub(crate) struct RenderSkybox(pub(crate) u32);

pub fn extract_skybox(
    mut cmd: Commands,
    sky: Extract<(Res<scene::Skybox>, Res<Assets<GpuTexture>>)>,
) {
    let (skybox, textures) = &*sky;
    let id = skybox.image.id();
    cmd.insert_resource(RenderSkybox(if textures.contains(id) {
        texture_index(id)
    } else {
        NULL_HANDLE
    }));
}

pub fn init_render(mut cmd: Commands, mut surface: ResMut<PrimarySurface>) {
    let mut surface = surface
        .0
        .take()
        .expect("the primary surface is created at startup");
    let mut swpachain = vkobjects::swapchain::Swapchain::new(
        &mut surface,
        None,
        Some(INITIAL_WINDOW_SIZE.as_uvec2()),
    )
    .unwrap();
    let slots = (0..swpachain.num_images()).map(|_| bindless::storage_slot().unwrap());
    swpachain.bind_storage(slots);
    let swapchain = Swapchain {
        image_index: 0,
        swpachain,
        surface,
        minimized: false,
    };
    let num_images = swapchain.num_images();
    cmd.insert_resource(swapchain);
    let queue = Queue::new();
    let slots = std::array::from_fn(|_| FrameSlot::new(&queue).unwrap());
    cmd.insert_resource(FrameSlots { queue, slots });
    cmd.insert_resource(SynchronizationResources {
        image_available: Default::default(),
        render_finished: (0..num_images).map(|_| Semaphore::new().unwrap()).collect(),
    });
}

pub struct RenderResources {
    depth_attachment: Image<D32Sfloat, DepthAttachmentSampled>,
    /// The opaque surfaces (`pack_gbuffer` in `gbuffer.slang`), the same size as the depth; the
    /// composite pass lights them into the target.
    gbuffer: Image<R32G32B32A32Uint, ColorAttachmentSampled>,
    /// Weighted blended OIT of the blended meshlets, composited over the lit G-buffer: the
    /// weighted sum of their premultiplied colours (rgb) and of their weights (a). Half floats,
    /// so the blended pass clamps the weighted colour.
    accum: Image<R16G16B16A16Sfloat, ColorAttachmentStorageTransferDst>,
    /// The product of `1 - alpha` of the blended fragments: how much of the surface behind them
    /// shows through. The composite pass reads it through the sampler (a typed load of a storage image is several
    /// times slower on Intel) and resets it as storage.
    revealage: Image<R8Unorm, ColorAttachmentSampledStorageTransferDst>,
    /// Where the outlined meshlets are, cleared and drawn by the outline pass, so the scene
    /// depth stays as the scene left it.
    outline_mask: Image<R8Unorm, ColorAttachmentSampled>,
    meshlets: Buffer<InstancedMeshlet>,
    /// Visible meshlets of materials with a negative alpha cutoff, drawn after `meshlets`.
    blended_meshlets: Buffer<InstancedMeshlet>,
    /// Visible meshlets of `OUTLINE` instances, also in one of the lists above.
    outlined_meshlets: Buffer<InstancedMeshlet>,
    bvh_node_stack: Buffer<InstanceBvhRoot>,
    /// One counter per `MESHLET_BATCH` candidates: `candidate_meshlets.len() / MESHLET_BATCH`.
    meshlet_batches: Buffer<u32>,
    candidate_meshlets: Buffer<InstanceMeshletIndex>,
    variables: Buffer<TraversalVariables, StorageIndirect>,
    /// `bvh_node_stack` and `meshlet_batches` still have to be filled, and `accum` and `revealage`
    /// cleared: after that, the passes that use them leave them that way.
    fresh: bool,
    /// The `OVERFLOWS` bits of buffers that reached `MAX_CULL_BUFFER`, logged once.
    reported_overflow: u32,
}

/// `DAG_CULL_GROUP_SIZE` in `bvh_cull.slang`: candidates per batch counter.
const MESHLET_BATCH: usize = 64;

#[derive(Resource, Default, Clone, Reflect)]
#[reflect(Resource)]
pub struct RenderValues {
    meshlet_count: u32,
    instance_count: u32,
    #[reflect(ignore, clone)]
    present_modes: SmallVec<[&'static str; 4]>,
}

#[derive(Resource, Clone, PartialEq, Reflect)]
#[reflect(Resource)]
pub struct RenderSettings {
    pub freez_proj: Option<Mat4>,
    pub freez_view: Option<Mat4>,
    pub freez_pos: Option<Vec4>,
    pub draw_scene_leaf_nodes: bool,
    pub draw_scene_blas_nodes: bool,
    pub draw_scene_nodes: bool,
    pub outline_color: Vec3,
    pub outline_radius: f32,
    pub pixel_error: f32,
    /// Scales the HDR colour before tonemapping.
    pub exposure: f32,
    /// Index into the swapchain's `present_modes` (0: FIFO).
    pub present_mode: usize,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            present_mode: 0,
            draw_scene_blas_nodes: false,
            draw_scene_leaf_nodes: false,
            draw_scene_nodes: false,
            freez_pos: None,
            freez_proj: None,
            freez_view: None,
            outline_color: Vec3::new(0.920, 0.640, 0.118),
            outline_radius: 2.0,
            pixel_error: 1.0,
            exposure: 1.0,
        }
    }
}

pub(crate) fn settings_ui(
    mut ui: UiBuilder,
    res: Res<RenderValues>,
    time: Res<Time>,
    mut s: ResMut<RenderSettings>,
    cam: Single<(&Camera, &GlobalTransform)>,
    mut view_port: Option<ResMut<ViewPort>>,
    mut gizmos: Option<ResMut<Gizzmos>>,
    mut last_times: Local<([Duration; 32], u64)>,
) {
    // Edits a copy, so the settings only count as changed when a value does.
    ui.build("Render Settings", |ui| {
        last_times.1 = (last_times.1 + 1) % 32;

        let idx = last_times.1 as usize;
        last_times.0[idx] = time.delta();

        ui.text(format!(
            "{:#?}",
            last_times.0.iter().cloned().sum::<Duration>().div_f32(32.0)
        ));

        ui.text(format!("Num Meshlets: {}", res.meshlet_count));
        ui.text(format!("Num Instances: {}", res.instance_count));
        if ui.button("Freez Cam") {
            s.freez_proj = Some(cam.0.proj);
            s.freez_view = Some(cam.0.view);
            s.freez_pos = Some(cam.1.translation().extend(0.0))
        }
        if ui.button("Unfreeze Cam") {
            s.freez_proj = None;
            s.freez_view = None;
            s.freez_pos = None;
        }
        if let Some(vp) = view_port.as_deref_mut()
            && ui.button(if vp.paused {
                "Resume Viewport"
            } else {
                "Pause Viewport"
            })
        {
            vp.paused = !vp.paused;
        }

        if !res.present_modes.is_empty() {
            s.present_mode = ui.property("Present Mode", |ui, _| {
                ui.dropdown(id!(), s.present_mode, &res.present_modes)
            });
        }

        s.draw_scene_blas_nodes = ui.checkbox("Draw Scene Blas Nodes", s.draw_scene_blas_nodes);
        s.draw_scene_leaf_nodes = ui.checkbox("Draw Scene Leaf Nodes", s.draw_scene_leaf_nodes);
        s.draw_scene_nodes = ui.checkbox("Draw Scene Nodes", s.draw_scene_nodes);

        ui.text("Outline Color");
        s.outline_color = ui.color_picker(id!(), s.outline_color.extend(1.0)).xyz();

        s.outline_radius = ui.property("Outline Thickness", |ui, width| {
            ui.slider(id!(), 0.0, MAX_OUTLINE_RADIUS, width, s.outline_radius)
        });
        if let Some(gizmos) = gizmos.as_deref_mut()
            && gizmos.total > MAX_GIZZMOS
        {
            // Which `MAX_GIZZMOS` of them are drawn.
            let max = (gizmos.total - MAX_GIZZMOS) as f32;
            gizmos.first = ui.property("First Gizmo", |ui, width| {
                ui.slider(id!(), 0.0, max, width, gizmos.first as f32)
            }) as usize;
        }
        s.pixel_error = ui.property("LOD Bias", |ui, width| {
            ui.slider(id!(), 0.0, 5.0, width, s.pixel_error)
        });
        s.exposure = ui.property("Exposure", |ui, width| {
            ui.slider(id!(), 0.0, 8.0, width, s.exposure)
        });
    });
}

pub fn extract_ui(mut cmd: Commands, mut world: ResMut<MainWorld>, values: Res<RenderValues>) {
    world.insert_resource(values.clone());
    cmd.insert_resource(world.get_resource::<RenderSettings>().unwrap().clone());
}

type RenderParams<'w, 's> = (
    Option<ResMut<'w, RenderCamera>>,
    Res<'w, InstanceManager>,
    Option<Res<'w, GizzmoResources>>,
    Local<'s, Option<RenderResources>>,
    Res<'w, SynchronizationResources>,
    Res<'w, Swapchain>,
    Option<ResMut<'w, RenderValues>>,
    Res<'w, RenderSettings>,
    Option<Res<'w, UiResources>>,
    ResMut<'w, ViewPortTarget>,
    Res<'w, RenderSkybox>,
);

pub(super) fn render(world: &mut World, params: &mut SystemState<RenderParams<'static, 'static>>) {
    let minimized = world.resource::<Swapchain>().minimized;
    world.resource_mut::<FrameLimit>().wait(minimized);
    if minimized {
        return;
    }
    world.resource_scope(|world, mut slots: Mut<FrameSlots>| {
        let FrameSlots { queue, slots } = &mut *slots;
        let frame_in_flight = {
            let mut frame = world.resource_mut::<FrameCount>();
            frame.0 += 1;
            frame.frame_in_flight()
        };
        let slot = &mut slots[frame_in_flight];
        #[cfg(feature = "profiling")]
        if let Some(timings) = world.get_resource::<GpuTimings>() {
            slot.set_timestamps(timings.timestamps);
            slot.set_counters(timings.counters.clone());
        }
        let epoch = world.resource_mut::<FrameEpoch>().0.take();
        let frame = slot
            .begin(epoch.expect("the extract reserves an epoch"))
            .unwrap();
        #[cfg(feature = "profiling")]
        if let Some(mut timings) = world.get_resource_mut::<GpuTimings>() {
            timings.slots[frame_in_flight].read_back(&frame);
        }
        world.run_schedule(RenderSystems::AquireSwapchainImage);
        let swapchain = world.resource::<Swapchain>();
        if swapchain.minimized {
            // Minimised since the extract: the frame is dropped unrecorded.
            return;
        }
        world.run_schedule(RenderSystems::PreRender);
        record_frame(
            frame,
            queue,
            frame_in_flight,
            params.get_mut(world).unwrap(),
        );
        #[cfg(feature = "profiling")]
        if let Some(mut timings) = world.get_resource_mut::<GpuTimings>() {
            timings.slots[frame_in_flight].submitted = (profiler::frame(), profiler::now_ns());
        }
        let swapchain = world.resource::<Swapchain>();
        let sync = world.resource::<SynchronizationResources>();
        let wait = [&sync.render_finished[swapchain.image_index as usize]];
        queue
            .present(swapchain, swapchain.image_index, &wait)
            .unwrap();
    });
}

impl RenderResources {
    /// Makes the resources, or grows those that are too small. A replaced image or buffer is
    /// just dropped: it is destroyed once the frames in flight are done (`lava::epoch`).
    pub(super) fn fit<'a>(resources: &'a mut Option<Self>, target_size: UVec2, min_size: UVec2) {
        let resources = resources.get_or_insert_with(|| {
            let depth_size = target_size.max(INITIAL_WINDOW_SIZE.as_uvec2());
            RenderResources {
                depth_attachment: Image::new(depth_size)
                    .unwrap()
                    .bind_sampled(bindless::sampled_slot().unwrap()),
                gbuffer: Image::new(depth_size)
                    .unwrap()
                    .bind_sampled(bindless::sampled_slot().unwrap()),
                accum: Image::new(depth_size)
                    .unwrap()
                    .bind_storage(bindless::storage_slot().unwrap()),
                revealage: Image::new(depth_size)
                    .unwrap()
                    .bind_sampled(bindless::sampled_slot().unwrap())
                    .bind_storage(bindless::storage_slot().unwrap()),
                outline_mask: Image::new(depth_size)
                    .unwrap()
                    .bind_sampled(bindless::sampled_slot().unwrap()),
                meshlets: Buffer::new(CULL_BUFFER, false).unwrap(),
                blended_meshlets: Buffer::new(CULL_BUFFER, false).unwrap(),
                outlined_meshlets: Buffer::new(CULL_BUFFER, false).unwrap(),
                bvh_node_stack: Buffer::new(CULL_BUFFER, false).unwrap(),
                variables: Buffer::new(1, false).unwrap(),
                meshlet_batches: Buffer::new(CULL_BUFFER / MESHLET_BATCH, false).unwrap(),
                candidate_meshlets: Buffer::new(CULL_BUFFER, false).unwrap(),
                fresh: true,
                reported_overflow: 0,
            }
        });
        resources.grow_overflowed();

        let depth_extent = resources.depth_attachment.extent();
        if depth_extent.cmplt(target_size).any() {
            let size = depth_extent.max(target_size).max(min_size);
            resources.depth_attachment = Image::new(size)
                .unwrap()
                .bind_sampled(bindless::sampled_slot().unwrap());
            resources.gbuffer = Image::new(size)
                .unwrap()
                .bind_sampled(bindless::sampled_slot().unwrap());
            resources.accum = Image::new(size)
                .unwrap()
                .bind_storage(bindless::storage_slot().unwrap());
            resources.revealage = Image::new(size)
                .unwrap()
                .bind_sampled(bindless::sampled_slot().unwrap())
                .bind_storage(bindless::storage_slot().unwrap());
            resources.outline_mask = Image::new(size)
                .unwrap()
                .bind_sampled(bindless::sampled_slot().unwrap());
            resources.fresh = true;
        }
    }

    pub(super) fn visible_meshlets(&self) -> u32 {
        let draws = &self.variables[0].draws;
        draws[0].meshlet_count + draws[1].meshlet_count
    }

    /// Doubles every buffer `bvh_cull` ran out of (the old one lives until the frames in flight
    /// are done), and
    /// warns once per buffer that is at `MAX_CULL_BUFFER`. Like `visible_meshlets` it reads
    /// what the last frames left in `variables`, so an overflow may show up a frame late,
    /// but never falsely; the frame that overflowed drew without what didn't fit.
    fn grow_overflowed(&mut self) {
        let overflow = self.variables[0].overflow;
        if overflow == 0 {
            return;
        }
        fn grow<T: Pod>(buffer: &mut Buffer<T>, len: usize) {
            *buffer = Buffer::new(len, false).unwrap();
        }
        for (bit, name) in OVERFLOWS {
            if overflow & bit == 0 {
                continue;
            }
            let len = match bit {
                1 => self.bvh_node_stack.len(),
                2 => self.candidate_meshlets.len(),
                4 => self.meshlets.len(),
                8 => self.blended_meshlets.len(),
                _ => self.outlined_meshlets.len(),
            };
            if len >= MAX_CULL_BUFFER {
                if self.reported_overflow & bit == 0 {
                    self.reported_overflow |= bit;
                    warn!(
                        "the BVH cull ran out of space in RenderResources.{name} at its maximum of {MAX_CULL_BUFFER} entries"
                    );
                }
                continue;
            }
            let len = (len * 2).min(MAX_CULL_BUFFER);
            info!(
                "the BVH cull ran out of space in RenderResources.{name}: growing it to {len} entries"
            );
            match bit {
                1 => grow(&mut self.bvh_node_stack, len),
                2 => {
                    grow(&mut self.candidate_meshlets, len);
                    grow(&mut self.meshlet_batches, len / MESHLET_BATCH);
                }
                4 => grow(&mut self.meshlets, len),
                8 => grow(&mut self.blended_meshlets, len),
                _ => grow(&mut self.outlined_meshlets, len),
            }
            // The queue and the batch counters have to be filled again.
            self.fresh = true;
        }
    }
}

pub(super) fn record_scene<
    F: HasTexels<[f32; 4]> + ColorAspect,
    U: IsColorAttachment + IsStorage,
>(
    cmd: &mut CommandBuffer,
    target_image: ImageView<'_, F, U>,
    target_size: UVec2,
    camera: &mut RenderCamera,
    sky: &RenderSkybox,
    instances: &InstanceManager,
    resources: &mut RenderResources,
    setting: &RenderSettings,
    gizzmos: Option<&GizzmoResources>,
    frame_in_flight: usize,
) {
    let draws = resources
        .variables
        .byte_range(
            offset_of!(TraversalVariables, draws)
                ..offset_of!(TraversalVariables, candidate_meshlet_write_offset),
        )
        .cast::<MeshletDraw>();
    let (opaque_draw, blended_draw, outlined_draw) =
        (draws.range(0..1), draws.range(1..2), draws.range(2..3));
    let transforms = instances
        .transforms
        .range(MAX_INSTANCES * frame_in_flight..);
    let materials = instances
        .instance_materials
        .range(MAX_INSTANCES * frame_in_flight..);
    let eye = camera.transform.translation.extend(1.0);
    let (view, proj) = (camera.camera.view, camera.camera.proj);
    // `mesh_threads` in `scene_raster.slang`: what the driver prefers, but no more than a meshlet
    // has vertices or triangles (128, see `build_meshlets`).
    let mesh_threads = Ctx::mesh_shader_properties()
        .max_preferred_mesh_work_group_invocations
        .min(128);
    cmd.begin_scope("cull");
    // Only once: bvh_cull leaves every queue slot and batch counter it consumes as filled here,
    // and the composite pass resets every pixel of `accum` and `revealage` the blended pass drew to.
    if resources.fresh {
        resources.fresh = false;
        cmd.fill_buffer(resources.bvh_node_stack.range(..), !0);
        cmd.fill_buffer(resources.meshlet_batches.range(..), 0);
        cmd.clear_image(resources.accum.whole_view(), [0.0; 4]);
        cmd.clear_image(resources.revealage.whole_view(), [1.0]);
    }
    // Also without instances: the blended pass always runs and reads its draw from here.
    cmd.update_buffer(
        resources.variables.range(..),
        &TraversalVariables {
            node_count: 0,
            node_batch_read_offset: 0,
            node_write_offset: 0,
            draws: [MeshletDraw {
                groups_x: 0,
                groups_y: MESHLET_STRIDE,
                groups_z: 1,
                meshlet_count: 0,
            }; 3],
            candidate_meshlet_write_offset: 0,
            meshlet_batch_read_offset: 0,
            total_meshlets: 0,
            overflow: 0,
        },
    );
    if instances.instance_count > 0 {
        let cull_proj = setting.freez_proj.unwrap_or(camera.camera.proj);
        let cull_view = setting.freez_view.unwrap_or(camera.camera.view);
        let clip_from_world = (cull_proj * cull_view).transpose();
        cmd.compute(
            &InstanceCull::push_bindings(
                instances.instance_count as u64,
                instances
                    .bvh_root_nodes
                    .range(MAX_INSTANCES * frame_in_flight..),
                instances.aabbs.range(MAX_INSTANCES * frame_in_flight..),
                instances
                    .transforms
                    .range(MAX_INSTANCES * frame_in_flight..),
                resources.bvh_node_stack.range(..),
                resources.variables.range(..),
                clip_from_world,
            ),
            [instances.instance_count.div_ceil(64) as u32, 1, 1],
        );
        cmd.compute(
            &BvhCull::push_bindings(
                resources.bvh_node_stack.range(..),
                resources.variables.range(..),
                resources.meshlets.range(..),
                resources.blended_meshlets.range(..),
                resources.outlined_meshlets.range(..),
                resources.candidate_meshlets.range(..),
                resources.meshlet_batches.range(..),
                instances
                    .transforms
                    .range(MAX_INSTANCES * frame_in_flight..),
                instances.headers.range(MAX_INSTANCES * frame_in_flight..),
                instances.flags.range(FLAG_WORDS * frame_in_flight..),
                setting
                    .freez_pos
                    .unwrap_or(camera.transform.translation.extend(0.0)),
                cull_proj,
                clip_from_world,
                target_size.y as f32,
                setting.pixel_error,
            )
            .constant_bindings(
                resources.bvh_node_stack.len() as u32,
                resources.candidate_meshlets.len() as u32,
                resources.meshlets.len() as u32,
                resources.blended_meshlets.len() as u32,
                resources.outlined_meshlets.len() as u32,
            ),
            [64, 1, 1],
        );
    }
    cmd.end_scope();
    // Also without instances: it clears the depth, and the composite pass draws the sky where it
    // stays 0.
    cmd.raster(target_size)
        .color_attachment(resources.gbuffer.whole_view(), None)
        .color_attachment(resources.accum.whole_view(), None)
        .color_attachment(resources.revealage.whole_view(), None)
        .depth_attachment(resources.depth_attachment.whole_view(), Some([0.0]))
        .launch_indirect(
            &Raster::push_bindings(
                view,
                proj,
                eye,
                transforms,
                resources.meshlets.range(..),
                opaque_draw,
                materials,
            )
            .constant_bindings(mesh_threads),
            opaque_draw.cast(),
            RasterState::default().blends([Blend::Replace, Blend::Skip, Blend::Skip]),
        )
        .launch_indirect(
            &RasterBlended::push_bindings(
                view,
                proj,
                eye,
                transforms,
                resources.blended_meshlets.range(..),
                blended_draw,
                materials,
            )
            .constant_bindings(mesh_threads),
            blended_draw.cast(),
            RasterState::default()
                .blends([Blend::Skip, Blend::Add, Blend::Attenuate])
                .depth_write(false),
        )
        .record("Scene");

    cmd.begin_scope("post");
    // cmd.flush_all();
    cmd.compute(
        &Composite::push_bindings(
            resources.gbuffer.whole_view(),
            resources.depth_attachment.whole_view(),
            resources.accum.whole_view(),
            resources.revealage.whole_view(),
            target_image,
            camera.camera.proj_inv(),
            camera.camera.view_inv(),
            target_size,
            setting.exposure,
        )
        .constant_bindings(sky.0),
        [target_size.x.div_ceil(8), target_size.y.div_ceil(8), 1],
    );

    if instances.any_outlined {
        cmd.raster(target_size)
            .color_attachment(resources.outline_mask.whole_view(), Some([0.0]))
            .launch_indirect(
                &RasterOutline::push_bindings(
                    view,
                    proj,
                    eye,
                    transforms,
                    resources.outlined_meshlets.range(..),
                    outlined_draw,
                    materials,
                )
                .constant_bindings(mesh_threads),
                outlined_draw.cast(),
                RasterState::default(),
            )
            .record("Outline");

        cmd.compute(
            &DrawOutline::push_bindings(
                resources.outline_mask.whole_view(),
                target_image,
                setting.outline_color.extend(setting.outline_radius),
                target_size,
            ),
            [target_size.x.div_ceil(8), target_size.y.div_ceil(8), 1],
        );
    }
    if let Some(gizzmos) = gizzmos {
        gizzmos.draw(cmd, target_image, target_size, camera, frame_in_flight);
    }

    cmd.end_scope();
}

fn record_frame(
    frame: Frame,
    queue: &Queue<Gfx>,
    frame_in_flight: usize,
    (
        mut camera,
        instances,
        gizzmos,
        mut resources,
        sync,
        swapchain,
        mut values,
        setting,
        ui_resources,
        mut target,
        sky,
    ): RenderParams,
) {
    let target_size = target.rect.size().as_uvec2().max(UVec2::ONE);
    RenderResources::fit(&mut resources, target_size, swapchain.size);
    let Some(resources) = resources.as_mut() else {
        return;
    };
    if let Some(extent) = target.image.as_ref().map(|image| image.extent())
        && extent.cmplt(target_size).any()
    {
        let size = extent.max(target_size).max(UVec2::from(swapchain.size));
        target.image = Some(ViewPortTarget::new_image(size, swapchain.format));
    }

    let target_image = target
        .image
        .as_ref()
        .map(|i| i.whole_view())
        .unwrap_or(swapchain.image());

    if let Some(values) = &mut values {
        values.instance_count = instances.instance_count as u32;
        values.meshlet_count = resources.visible_meshlets();
        values.present_modes.clear();
        values
            .present_modes
            .extend(swapchain.present_modes.iter().map(|(_, name)| *name));
    }

    // The frame waits for the acquired image only from its first use on, so it culls and
    // shades before that: the composite pass when the scene is drawn straight into it, else
    // the UI rendering (or just the present, which chains with any stage).
    let scene_into_swapchain = target.image.is_none() && !target.paused && camera.is_some();
    let acquired = sync.image_available[frame_in_flight].info().at(if scene_into_swapchain {
        Stage::Compute
    } else {
        Stage::Raster
    });
    frame
        .execute(
            queue,
            &[acquired],
            &[sync.render_finished[swapchain.image_index as usize].info()],
            |cmd| {
                if !target.paused
                    && let Some(camera) = camera.as_deref_mut()
                {
                    record_scene(
                        cmd,
                        target_image,
                        target_size,
                        camera,
                        &sky,
                        &instances,
                        resources,
                        &setting,
                        gizzmos.as_deref(),
                        frame_in_flight,
                    );
                }

                if let Some(ui_resources) = ui_resources.as_ref() {
                    let ui =
                        RasterUi::push_bindings(ui_resources.verticies[frame_in_flight].range(..))
                            .constant_bindings(ui_resources.font_atlas.whole_view());
                    // The viewport tab reads the scene through a handle in its vertices. The
                    // registration makes a larger binding, so each arm borrows its own.
                    let with_viewport;
                    let ui: &BindingOutput<_> = match &target.image {
                        Some(image) => {
                            with_viewport = ui.storage_read(image.whole_view());
                            &with_viewport
                        }
                        None => &ui,
                    };
                    cmd.raster(swapchain.size.into())
                        .color_attachment(swapchain.image(), None)
                        .draw_indexed(
                            ui,
                            ui_resources.indicies[frame_in_flight]
                                .range(..ui_resources.num_indicies),
                            1,
                            RasterState::default()
                                .blends([Blend::Alpha])
                                .backface_culling(false),
                        )
                        .record("UI");
                }
                cmd.present(swapchain.image());
            },
        )
        .unwrap();
}

#[derive(Resource, Default)]
pub struct ViewPortTarget {
    /// The swapchain image's usage, since it stands in for it.
    pub image: Option<Image<format::Dynamic, ColorAttachmentStorageTransferDst>>,
    pub rect: Rect,
    /// Copied from `ViewPort::paused`: skip the scene passes.
    pub paused: bool,
}

impl ViewPortTarget {
    /// An image of `size` in the swapchain's `format`, bound in the storage set.
    pub(super) fn new_image(
        size: UVec2,
        format: format::Dynamic,
    ) -> Image<format::Dynamic, ColorAttachmentStorageTransferDst> {
        let usage = ColorAttachmentStorageTransferDst;
        Image::with_params(format, usage, size, 1)
            .unwrap()
            .bind_storage(bindless::storage_slot().unwrap())
    }
}

#[allow(non_snake_case)]
pub fn RenderPassesPlugin(app: &mut App) {
    app.add_systems(Render, render.in_set(RenderSystems::Render))
        .add_systems(RenderSystems::AquireSwapchainImage, aquire_swapchain_image)
        .insert_resource(RenderSettings::default())
        .init_resource::<ViewPortTarget>()
        .add_systems(ExtractSchedule, (resize_swapchain, extract_view_port).chain())
        .add_systems(RenderStartup, init_render);
}

#[allow(non_snake_case)]
pub fn RenderDebugUi(app: &mut App) {
    app.insert_resource(RenderValues::default())
        .add_systems(Update, settings_ui)
        .insert_resource(RenderSettings::default());

    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    render_app
        .add_systems(ExtractSchedule, extract_ui)
        .insert_resource(RenderValues::default());
}
