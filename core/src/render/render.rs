//! Per-frame rendering: swapchain, sync, render resources/settings, the shared scene passes and windowed frame recording.
use bevy::ecs::change_detection::{DetectChanges, DetectChangesMut};
use bevy::{ecs::reflect::ReflectResource, reflect::Reflect, time::Time};
use smallvec::SmallVec;
use std::{
    collections::HashMap,
    mem::offset_of,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bevy::{
    app::{App, Update},
    asset::Assets,
    ecs::{
        query::With,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Local, Res, ResMut, Single, SystemState},
        world::{Mut, World},
    },
    log::*,
    math::Rect,
    transform::components::{GlobalTransform, Transform},
    window::{PrimaryWindow, Window},
};
use glam::{Mat4, UVec2, Vec2, Vec3, Vec4, Vec4Swizzles};
use lava::{
    bindless::{BindlessHandle, BindlessWrites, NULL_HANDLE},
    buffer::{
        Buffer,
        usage::{Index, Indirect, StorageIndirect},
    },
    command_buffer::{Blend, CommandBuffer, RasterState},
    image::{
        Image,
        format::{
            self, ColorAspect, D32Sfloat, Format, R16G16B16A16Sfloat, R16Sfloat, R32G32B32A32Sfloat,
        },
        slice::{AsImage, ImageView},
        usage::{
            ColorAttachmentSampled, ColorAttachmentSampledStorage, ColorAttachmentStorage,
            DepthAttachmentSampled,
        },
    },
    state::Ctx,
    vkobjects::{
        self,
        queue::{Binary, Frame, FrameSlot, Gfx, PendingAccesses, Present, Queue, Semaphore},
    },
};

#[cfg(feature = "profiling")]
use crate::profiler::{GpuTimings, capture};
use crate::{
    INITIAL_WINDOW_SIZE,
    assets::texture::{GpuTexture, texture_index},
    bindless,
    editor::{gizzmos::GizzmoResources, viewport::ViewPort},
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
use lava::bindings::{
    BvhCull, DrawOutline, InstanceBvhRoot, InstanceCull, InstanceMeshletIndex, InstancedMeshlet,
    MeshletDraw, Raster, RasterBlended, RasterOutline, RasterUi, Tonemap, TraversalVariables,
};

/// `MESHLET_STRIDE` in `datatypes.slang`: `groups_y` of every `MeshletDraw`.
const MESHLET_STRIDE: u32 = 64;

#[derive(Resource)]
pub struct FrameSlots {
    pub slots: [FrameSlot; FRAMES_IN_FLIGHT],
}

#[derive(Resource, Default)]
pub struct SynchronizationResources {
    pub image_available: [Semaphore<Binary>; FRAMES_IN_FLIGHT],
    pub render_finished: Vec<Semaphore<Binary>>,
}

pub enum QueueStrategie {
    Single(Arc<Mutex<Queue<Gfx>>>),
    Multiple(Queue<Gfx>),
}

impl QueueStrategie {
    pub fn with<R, F: FnOnce(&Queue<Gfx>) -> R>(&self, f: F) -> R {
        match &self {
            QueueStrategie::Single(queue) => {
                let q = queue.lock().unwrap();
                f(&q)
            }
            QueueStrategie::Multiple(queue) => f(queue),
        }
    }
}

#[derive(Resource)]
pub struct Queues {
    pub graphics: QueueStrategie,
    pub present: Option<Queue<Present>>,
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
}

impl Swapchain {
    pub fn image(&self) -> ImageView<'_, format::Swapchain, ColorAttachmentStorage> {
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
    sync: Res<SynchronizationResources>,
    frame: Res<FrameCount>,
    mut swapchain: ResMut<Swapchain>,
) {
    swapchain.image_index = swapchain
        .aquire_image(&sync.image_available[frame.frame_in_flight()], None)
        .unwrap();
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
    if size.to_array() != swapchain.swpachain.size {
        info!("Resized Swapchain");
        recreate = true;
    }
    if recreate {
        let old_images = swapchain.num_images() as u32;
        let old_first = swapchain.swpachain.image(0).handle.descriptor_index_set1;
        swapchain
            .swpachain
            .recreate(&swapchain.surface, size.to_array())
            .unwrap();
        // The new images are unbound, and their count can change with the present mode.
        let num_images = swapchain.num_images();
        let first = if num_images as u32 == old_images {
            old_first
        } else {
            bindless::release_storage_slots(old_first, old_images);
            while sync.render_finished.len() < num_images {
                sync.render_finished.push(Semaphore::new().unwrap());
            }
            bindless::storage_slots(num_images as u32).unwrap()
        };
        let mut writes = BindlessWrites::default();
        swapchain.swpachain.bind_storage(first, &mut writes);
        writes.submit();
    }
}

#[derive(Resource)]
pub(crate) struct RenderCamera {
    pub(crate) camera: Camera,
    pub(crate) transform: Transform,
}

pub fn extract_camera(mut cmd: Commands, camera: Extract<Single<(&Camera, &GlobalTransform)>>) {
    cmd.insert_resource(RenderCamera {
        camera: *camera.0,
        transform: camera.1.compute_transform(),
    });
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

/// The graphics queue with its frame slots.
pub(super) fn init_queues(cmd: &mut Commands, present: Option<Queue<Present>>) {
    let queues = Queues {
        graphics: if Ctx::num_gfx_queues() == 1 {
            QueueStrategie::Single(Arc::new(Mutex::new(Queue::new().unwrap())))
        } else {
            QueueStrategie::Multiple(Queue::new().unwrap())
        },
        present,
    };
    let slots =
        std::array::from_fn(|_| queues.graphics.with(|queue| FrameSlot::new(queue).unwrap()));
    cmd.insert_resource(queues);
    cmd.insert_resource(FrameSlots { slots });
    cmd.insert_resource(ResourceStates {
        pending: Some(PendingAccesses::default()),
    });
}

pub fn init_render(mut cmd: Commands, mut surface: ResMut<PrimarySurface>) {
    let surface = surface
        .0
        .take()
        .expect("the primary surface is created at startup");
    let mut swpachain = vkobjects::swapchain::Swapchain::new(
        &surface,
        None,
        Some(INITIAL_WINDOW_SIZE.as_uvec2().to_array()),
    )
    .unwrap();
    let first = bindless::storage_slots(swpachain.num_images() as u32).unwrap();
    let mut writes = BindlessWrites::default();
    swpachain.bind_storage(first, &mut writes);
    writes.submit();
    let swapchain = Swapchain {
        image_index: 0,
        swpachain,
        surface,
    };
    let num_images = swapchain.num_images();
    let present =
        (Ctx::gfx_queue_index() != Ctx::present_queue_index()).then(|| Queue::new().unwrap());
    cmd.insert_resource(swapchain);
    init_queues(&mut cmd, present);
    cmd.insert_resource(SynchronizationResources {
        image_available: Default::default(),
        render_finished: (0..num_images).map(|_| Semaphore::new().unwrap()).collect(),
    });
}

#[derive(Resource)]
pub struct ResourceStates {
    pub(super) pending: Option<PendingAccesses>,
}

pub struct RenderResources {
    depth_attachment: Image<D32Sfloat, DepthAttachmentSampled>,
    /// The scene in linear HDR colour, the same size as the depth; tonemapped into the target.
    hdr: Image<R16G16B16A16Sfloat, ColorAttachmentSampled>,
    /// Weighted blended OIT of the blended meshlets, composited over `hdr` by the tonemap: the
    /// weighted sum of their premultiplied colours (rgb) and of their weights (a). 32-bit,
    /// because specular highlights times the weight overflow half floats.
    accum: Image<R32G32B32A32Sfloat, ColorAttachmentStorage>,
    /// The product of `1 - alpha` of the blended fragments: how much of `hdr` shows through.
    /// The tonemap reads it through the sampler (a typed load of a storage image is several
    /// times slower on Intel) and resets it as storage.
    revealage: Image<R16Sfloat, ColorAttachmentSampledStorage>,
    meshlets: Buffer<InstancedMeshlet>,
    /// Visible meshlets of materials with a negative alpha cutoff, drawn after `meshlets`.
    blended_meshlets: Buffer<InstancedMeshlet>,
    /// Visible meshlets of `OUTLINE` instances, also in one of the lists above.
    outlined_meshlets: Buffer<InstancedMeshlet>,
    bvh_node_stack: Buffer<InstanceBvhRoot>,
    meshlet_batches: Buffer<u32>,
    candidate_meshlets: Buffer<InstanceMeshletIndex>,
    variables: Buffer<TraversalVariables, StorageIndirect>,
    /// `bvh_node_stack` and `meshlet_batches` still have to be filled, and `accum` and `revealage`
    /// cleared: after that, the passes that use them leave them that way.
    fresh: bool,
}

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
    /// Index into the swapchain's `present_modes` (0: lowest latency).
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
            ui.text("Present Mode");
            s.present_mode = ui.dropdown(id!(), s.present_mode, &res.present_modes);
        }

        ui.horizontal();
        ui.text("Draw Scene Blas Nodes");
        s.draw_scene_blas_nodes = ui.checkbox(s.draw_scene_blas_nodes);
        ui.vertical();

        ui.horizontal();
        ui.text("Draw Scene Leaf Nodes");
        s.draw_scene_leaf_nodes = ui.checkbox(s.draw_scene_leaf_nodes);
        ui.vertical();

        ui.horizontal();
        ui.text("Draw Scene Nodes");
        s.draw_scene_nodes = ui.checkbox(s.draw_scene_nodes);
        ui.vertical();

        ui.text("Outline Color");
        s.outline_color = ui.color_picker(id!(), s.outline_color.extend(1.0)).xyz();

        ui.text("Outline Thickness");
        s.outline_radius = ui.slider(id!(), 0.0, 6.0, 300.0, s.outline_radius);

        ui.text("LOD Bias");
        s.pixel_error = ui.slider(id!(), 0.0, 5.0, 300.0, s.pixel_error);

        ui.text("Exposure");
        s.exposure = ui.slider(id!(), 0.0, 8.0, 300.0, s.exposure);
    });
}

pub fn extract_ui(mut cmd: Commands, mut world: ResMut<MainWorld>, values: Res<RenderValues>) {
    world.insert_resource(values.clone());
    cmd.insert_resource(world.get_resource::<RenderSettings>().unwrap().clone());
}

type RenderParams<'w, 's> = (
    ResMut<'w, RenderCamera>,
    Res<'w, InstanceManager>,
    Res<'w, Queues>,
    Option<Res<'w, GizzmoResources>>,
    Local<'s, Option<RenderResources>>,
    ResMut<'w, ResourceStates>,
    Res<'w, SynchronizationResources>,
    Res<'w, Swapchain>,
    Option<ResMut<'w, RenderValues>>,
    Res<'w, RenderSettings>,
    Option<Res<'w, UiResources>>,
    ResMut<'w, ViewPortTarget>,
    Res<'w, RenderSkybox>,
);

pub(super) fn render(world: &mut World, params: &mut SystemState<RenderParams<'static, 'static>>) {
    world.resource_scope(|world, mut slots: Mut<FrameSlots>| {
        let frame_in_flight = {
            let mut frame = world.resource_mut::<FrameCount>();
            frame.0 += 1;
            frame.frame_in_flight()
        };
        let slot = &mut slots.slots[frame_in_flight];
        #[cfg(feature = "profiling")]
        slot.set_profiling(
            world
                .get_resource::<GpuTimings>()
                .is_some_and(|t| t.enabled),
        );
        let frame = slot.begin().unwrap();
        #[cfg(feature = "profiling")]
        if let Some(last) = frame.last_timings() {
            world.resource_mut::<GpuTimings>().slots[frame_in_flight].read_back(last);
        }
        world.run_schedule(RenderSystems::AquireSwapchainImage);
        world.run_schedule(RenderSystems::PreRender);
        record_frame(frame, frame_in_flight, params.get_mut(world).unwrap());
        #[cfg(feature = "profiling")]
        if let Some(mut timings) = world.get_resource_mut::<GpuTimings>() {
            timings.slots[frame_in_flight].submitted = (capture::frame(), capture::now_ns());
        }
    });
}

impl RenderResources {
    pub(super) fn fit<'a>(
        resources: &'a mut Option<Self>,
        target_size: UVec2,
        min_size: UVec2,
        frame: &mut Frame,
    ) {
        let resources = resources.get_or_insert_with(|| {
            let depth_size = target_size.max(INITIAL_WINDOW_SIZE.as_uvec2());
            RenderResources {
                depth_attachment: Image::new_sampled(
                    depth_size.x,
                    depth_size.y,
                    1,
                    bindless::sampled_slot().unwrap(),
                )
                .unwrap(),
                hdr: Image::new_sampled(
                    depth_size.x,
                    depth_size.y,
                    1,
                    bindless::sampled_slot().unwrap(),
                )
                .unwrap(),
                accum: Image::new_storage(
                    depth_size.x,
                    depth_size.y,
                    1,
                    bindless::storage_slots(1).unwrap(),
                )
                .unwrap(),
                revealage: Image::new_unified(
                    depth_size.x,
                    depth_size.y,
                    1,
                    bindless::storage_slots(1).unwrap(),
                    bindless::sampled_slot().unwrap(),
                )
                .unwrap(),
                meshlets: Buffer::new(2 * 1024 * 1024, false).unwrap(),
                blended_meshlets: Buffer::new(256 * 1024, false).unwrap(),
                outlined_meshlets: Buffer::new(256 * 1024, false).unwrap(),
                bvh_node_stack: Buffer::new(2 * 1024 * 1024, false).unwrap(),
                variables: Buffer::new(1, false).unwrap(),
                meshlet_batches: Buffer::new(2 * 1024 * 1024, false).unwrap(),
                candidate_meshlets: Buffer::new(2 * 1024 * 1024, false).unwrap(),
                fresh: true,
            }
        });

        let depth_extent = resources.depth_attachment.extent;
        if depth_extent.cmplt(target_size).any() {
            let size = depth_extent.max(target_size).max(min_size);
            let old = std::mem::replace(
                &mut resources.depth_attachment,
                Image::new_sampled(size.x, size.y, 1, bindless::sampled_slot().unwrap()).unwrap(),
            );
            frame.retire(old);
            let old = std::mem::replace(
                &mut resources.hdr,
                Image::new_sampled(size.x, size.y, 1, bindless::sampled_slot().unwrap()).unwrap(),
            );
            frame.retire(old);
            let old = std::mem::replace(
                &mut resources.accum,
                Image::new_storage(size.x, size.y, 1, bindless::storage_slots(1).unwrap()).unwrap(),
            );
            frame.retire(old);
            let old = std::mem::replace(
                &mut resources.revealage,
                Image::new_unified(
                    size.x,
                    size.y,
                    1,
                    bindless::storage_slots(1).unwrap(),
                    bindless::sampled_slot().unwrap(),
                )
                .unwrap(),
            );
            frame.retire(old);
            resources.fresh = true;
        }
    }

    pub(super) fn visible_meshlets(&self) -> u32 {
        let draws = &self.variables[0].draws;
        draws[0].meshlet_count + draws[1].meshlet_count
    }
}

pub(super) fn record_scene<F: Format<Texels = [f32; 4]> + ColorAspect>(
    cmd: &mut CommandBuffer,
    target_image: ImageView<'_, F, ColorAttachmentStorage>,
    target_size: UVec2,
    camera: &mut RenderCamera,
    sky: &RenderSkybox,
    instances: &InstanceManager,
    resources: &mut RenderResources,
    setting: &RenderSettings,
    gizzmos: Option<&GizzmoResources>,
    frame_in_flight: usize,
) {
    // for i in resources.meshlets.range(0..10) {
    //     log::info!("{:#?}", i);
    // }

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
    cmd.begin_scope("cull");
    // Only once: bvh_cull leaves every queue slot and batch counter it consumes as filled here,
    // and the tonemap resets every pixel of `accum` and `revealage` the blended pass drew to.
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
        },
    );
    if instances.instance_count > 0 {
        let cull_proj = setting.freez_proj.unwrap_or(camera.camera.proj);
        let cull_view = setting.freez_view.unwrap_or(camera.camera.view);
        let clip_from_world = (cull_proj * cull_view).transpose();
        cmd.compute(
            InstanceCull::push_bindings(
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
            BvhCull::push_bindings(
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
    // Also without instances: it clears the depth, and the tonemap draws the sky where it stays 0.
    cmd.raster(target_size)
        .color_attachment(resources.hdr.whole_view(), None)
        .color_attachment(resources.accum.whole_view(), None)
        .color_attachment(resources.revealage.whole_view(), None)
        .depth_attachment(resources.depth_attachment.whole_view(), Some([0.0]))
        .launch_indirect(
            Raster::push_bindings(
                view,
                proj,
                eye,
                transforms,
                resources.meshlets.range(..),
                opaque_draw,
                materials,
            ),
            opaque_draw.cast(),
            RasterState::default().blends([Blend::Replace, Blend::Skip, Blend::Skip]),
        )
        .launch_indirect(
            RasterBlended::push_bindings(
                view,
                proj,
                eye,
                transforms,
                resources.blended_meshlets.range(..),
                blended_draw,
                materials,
            ),
            blended_draw.cast(),
            RasterState::default()
                .blends([Blend::Skip, Blend::Add, Blend::Attenuate])
                .depth_write(false),
        )
        .record("Scene");

    cmd.begin_scope("post");
    // cmd.flush_all();
    cmd.compute(
        Tonemap::push_bindings(
            resources.hdr.whole_view(),
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
            .depth_attachment(resources.depth_attachment.whole_view(), Some([0.0]))
            .launch_indirect(
                RasterOutline::push_bindings(
                    view,
                    proj,
                    eye,
                    transforms,
                    resources.outlined_meshlets.range(..),
                    outlined_draw,
                    materials,
                ),
                outlined_draw.cast(),
                RasterState::default(),
            )
            .record("Outline");

        cmd.compute(
            DrawOutline::push_bindings(
                resources.depth_attachment.whole_view(),
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
    mut frame: Frame,
    frame_in_flight: usize,
    (
        mut camera,
        instances,
        queues,
        gizzmos,
        mut resources,
        mut resource_states,
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
    RenderResources::fit(
        &mut resources,
        target_size,
        UVec2::from(swapchain.size),
        &mut frame,
    );
    let Some(resources) = resources.as_mut() else {
        return;
    };
    if let Some(extent) = target.image.as_ref().map(|image| image.extent)
        && extent.cmplt(target_size).any()
    {
        let size = extent.max(target_size).max(UVec2::from(swapchain.size));
        let new =
            Image::new_storage(size.x, size.y, 1, bindless::storage_slots(1).unwrap()).unwrap();
        if let Some(old) = target.image.replace(new) {
            frame.retire(old);
        }
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

    let states = queues.graphics.with(|queue| {
        frame
            .execute(
                queue,
                resource_states.pending.take().unwrap(),
                &[sync.image_available[frame_in_flight].info()],
                &[sync.render_finished[swapchain.image_index as usize].info()],
                |cmd| {
                    if !target.paused {
                        record_scene(
                            cmd,
                            target_image,
                            target_size,
                            &mut camera,
                            &sky,
                            &instances,
                            resources,
                            &setting,
                            gizzmos.as_deref(),
                            frame_in_flight,
                        );
                    }

                    if let Some(ui_resources) = ui_resources.as_ref() {
                        let ui = RasterUi::push_bindings(
                            ui_resources.verticies[frame_in_flight].range(..),
                        )
                        .constant_bindings(ui_resources.font_atlas.whole_view());
                        // The viewport tab reads the scene through a handle in its vertices.
                        let ui = match &target.image {
                            Some(image) => ui.storage_read(image.whole_view()),
                            None => ui,
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
            .unwrap()
    });

    resource_states.pending = Some(states);
    if let Some(present) = &queues.present {
        present
            .present(
                &swapchain,
                swapchain.image_index,
                &[&sync.render_finished[swapchain.image_index as usize]],
            )
            .unwrap();
    } else {
        queues.graphics.with(|queue| {
            queue
                .present(
                    &swapchain,
                    swapchain.image_index,
                    &[&sync.render_finished[swapchain.image_index as usize]],
                )
                .unwrap()
        });
    }
}

#[derive(Resource, Default)]
pub struct ViewPortTarget {
    pub image: Option<Image<format::Swapchain, ColorAttachmentStorage>>,
    pub rect: Rect,
    /// Copied from `ViewPort::paused`: skip the scene passes.
    pub paused: bool,
}

#[allow(non_snake_case)]
pub fn RenderPassesPlugin(app: &mut App) {
    app.add_systems(Render, render.in_set(RenderSystems::Render))
        .add_systems(RenderSystems::AquireSwapchainImage, aquire_swapchain_image)
        .insert_resource(RenderSettings::default())
        .init_resource::<ViewPortTarget>()
        .add_systems(ExtractSchedule, extract_view_port)
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
