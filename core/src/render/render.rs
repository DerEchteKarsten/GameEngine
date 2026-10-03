//! Per-frame rendering: swapchain, sync, render resources/settings and frame recording.
use std::{
    collections::HashMap,
    mem::offset_of,
    sync::{Arc, Mutex},
};

use bevy::{
    app::{App, Update},
    ecs::{
        query::With,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Local, Res, ResMut, Single, SystemState},
        world::{Mut, World},
    },
    log::*,
    transform::components::Transform,
    window::{PrimaryWindow, Window},
};
use glam::{Mat4, Vec3, Vec4, Vec4Swizzles};
use lava::{
    buffer::{
        Buffer,
        usage::{Index, Indirect, StorageIndirect},
    },
    command_buffer::{DrawIndirectCommand, Scissor, Viewport},
    image::{
        Image,
        format::{self, D32Sfloat},
        slice::{AsImage, ImageView},
        usage::{ColorAttachmentStorage, DepthAttachmentSampled},
    },
    state::Ctx,
    vkobjects::{
        self,
        queue::{Binary, Frame, FrameSlot, Gfx, PendingAccesses, Present, Queue, Semaphore},
    },
};

use crate::{
    INITIAL_WINDOW_SIZE,
    editor::{gizzmos::GizzmoResources, viewport::ViewPort},
    id,
    render::{
        ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, Render, RenderApp, RenderStartup,
        RenderSystems,
        extract_param::Extract,
        world::{InstanceManager, MAX_INSTANCES},
    },
    scene::camera::Camera,
    ui::{UiResources, builder::UiBuilder},
};
use lava::bindings::{
    BvhCull, DrawOutline, InstanceBvhRoot, InstanceCull, InstanceMeshletIndex, InstancedMeshlet,
    Raster, RasterOutline, RasterUi, Skybox, TraversalVariables,
};

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
    pub swpachain: lava::vkobjects::swapchain::Swapchain,
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
    window: Extract<Single<&Window, With<PrimaryWindow>>>,
) {
    let size = window.physical_size();
    if size.to_array() != swapchain.swpachain.size {
        info!("Resized Swapchain");
        swapchain.swpachain.recreate(size.to_array()).unwrap();
    }
}

#[derive(Resource)]
pub(crate) struct RenderCamera {
    pub(crate) camera: Camera,
    pub(crate) transform: Transform,
}

pub fn extract_camera(mut cmd: Commands, camera: Extract<Single<(&Camera, &Transform)>>) {
    cmd.insert_resource(RenderCamera {
        camera: *camera.0,
        transform: *camera.1,
    });
}

pub fn init_render(mut cmd: Commands) {
    let swapchain = Swapchain {
        image_index: 0,
        swpachain: vkobjects::swapchain::Swapchain::new(
            None,
            Some(INITIAL_WINDOW_SIZE.as_uvec2().to_array()),
        )
        .unwrap(),
    };
    let num_images = swapchain.num_images();
    let queues = Queues {
        graphics: if Ctx::num_gfx_queues() == 1 {
            QueueStrategie::Single(Arc::new(Mutex::new(Queue::new().unwrap())))
        } else {
            QueueStrategie::Multiple(Queue::new().unwrap())
        },
        present: if Ctx::gfx_queue_index() == Ctx::present_queue_index() {
            None
        } else {
            Some(Queue::new().unwrap())
        },
    };
    cmd.insert_resource(swapchain);
    let slots =
        std::array::from_fn(|_| queues.graphics.with(|queue| FrameSlot::new(queue).unwrap()));

    cmd.insert_resource(queues);
    cmd.insert_resource(FrameSlots { slots });
    cmd.insert_resource(ResourceStates {
        pending: Some(PendingAccesses::default()),
    });
    cmd.insert_resource(SynchronizationResources {
        image_available: Default::default(),
        render_finished: (0..num_images).map(|_| Semaphore::new().unwrap()).collect(),
    });
}

#[derive(Resource)]
pub struct ResourceStates {
    pending: Option<PendingAccesses>,
}

pub struct RenderResources {
    depth_attachment: Image<D32Sfloat, DepthAttachmentSampled>,
    meshlets: Buffer<InstancedMeshlet>,
    bvh_node_stack: Buffer<InstanceBvhRoot>,
    meshlet_batches: Buffer<u32>,
    candidate_meshlets: Buffer<InstanceMeshletIndex>,
    variables: Buffer<TraversalVariables, StorageIndirect>,
}

#[derive(Resource, Default, Clone)]
pub struct RenderValues {
    meshlet_count: u32,
    instance_count: u32,
}

#[derive(Resource, Clone)]
pub struct RenderSettings {
    pub freez_proj: Option<Mat4>,
    pub freez_view: Option<Mat4>,
    pub freez_pos: Option<Vec4>,
    pub draw_scene_leaf_nodes: bool,
    pub draw_scene_blas_nodes: bool,
    pub draw_scene_nodes: bool,
    pub outline_color: Vec3,
    pub outline_radius: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            draw_scene_blas_nodes: false,
            draw_scene_leaf_nodes: false,
            draw_scene_nodes: false,
            freez_pos: None,
            freez_proj: None,
            freez_view: None,
            outline_color: Vec3::new(0.920, 0.640, 0.118),
            outline_radius: 2.0,
        }
    }
}

pub(crate) fn settings_ui(
    mut ui: UiBuilder,
    res: Res<RenderValues>,
    mut settings: ResMut<RenderSettings>,
    cam: Single<(&Camera, &Transform)>,
) {
    ui.build("Render Settings", |ui| {
        ui.text(format!("Num Meshlets: {}", res.meshlet_count));
        ui.text(format!("Num Instances: {}", res.instance_count));
        if ui.button("Freez Cam") {
            settings.freez_proj = Some(cam.0.proj);
            settings.freez_view = Some(cam.0.view);
            settings.freez_pos = Some(cam.1.translation.extend(0.0))
        }
        if ui.button("Unfreeze Cam") {
            settings.freez_proj = None;
            settings.freez_view = None;
            settings.freez_pos = None;
        }

        ui.horizontal();
        ui.text("Draw Scene Blas Nodes");
        settings.draw_scene_blas_nodes = ui.checkbox(settings.draw_scene_blas_nodes);
        ui.vertical();

        ui.horizontal();
        ui.text("Draw Scene Leaf Nodes");
        settings.draw_scene_leaf_nodes = ui.checkbox(settings.draw_scene_leaf_nodes);
        ui.vertical();

        ui.horizontal();
        ui.text("Draw Scene Nodes");
        settings.draw_scene_nodes = ui.checkbox(settings.draw_scene_nodes);
        ui.vertical();

        ui.text("Outline Color");
        settings.outline_color = ui
            .color_picker(id!(), settings.outline_color.extend(1.0))
            .xyz();

        ui.text("Outline Thickness");
        settings.outline_radius = ui.slider(id!(), 0.0, 6.0, 300.0, settings.outline_radius);
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
    Res<'w, ViewPort>,
    Option<ResMut<'w, RenderValues>>,
    Res<'w, RenderSettings>,
    Res<'w, UiResources>,
);

pub(super) fn render(
    world: &mut World,
    params: &mut SystemState<RenderParams<'static, 'static>>,
) {
    world.resource_scope(|world, mut slots: Mut<FrameSlots>| {
        let frame_in_flight = {
            let mut frame = world.resource_mut::<FrameCount>();
            frame.0 += 1;
            frame.frame_in_flight()
        };
        let frame = slots.slots[frame_in_flight].begin().unwrap();
        world.run_schedule(RenderSystems::AquireSwapchainImage);
        world.run_schedule(RenderSystems::PreRender);
        record_frame(frame, frame_in_flight, params.get_mut(world));
    });
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
        viewport,
        mut values,
        setting,
        ui_resources,
    ): RenderParams,
) {
    let resources = resources.get_or_insert_with(|| RenderResources {
        depth_attachment: Image::new(swapchain.size[0], swapchain.size[1]).unwrap(),
        meshlets: Buffer::new(2 * 1024 * 1024, false).unwrap(),
        bvh_node_stack: Buffer::new(2 * 1024 * 1024, false).unwrap(),
        variables: Buffer::new(1, false).unwrap(),
        meshlet_batches: Buffer::new(2 * 1024 * 1024, false).unwrap(),
        candidate_meshlets: Buffer::new(2 * 1024 * 1024, false).unwrap(),
    });

    if resources
        .depth_attachment
        .extent
        .cmplt(swapchain.size.into())
        .any()
    {
        tracing::info!("Recreating Depth");
        let old = std::mem::replace(
            &mut resources.depth_attachment,
            Image::new(swapchain.size[0], swapchain.size[1]).unwrap(),
        );
        frame.retire(old);
    }

    if let Some(values) = &mut values {
        values.instance_count = instances.instance_count as u32;
        values.meshlet_count = resources.variables[0].visible_meshlet_count;
    }

    let states = queues.graphics.with(|queue| {
        frame.execute(
            queue,
            resource_states.pending.take().unwrap(),
            &[sync.image_available[frame_in_flight].info()],
            &[sync.render_finished[swapchain.image_index as usize].info()],
            |cmd| {
                cmd.clear_image(swapchain.image(), [0.0; 4]);
                cmd.compute(
                    Skybox::new(
                        camera.camera.proj_inv(),
                        camera.camera.view_inv(),
                        swapchain.image(),
                        viewport.rect.min.as_ivec2(),
                        viewport.rect.size().as_uvec2(),
                        swapchain.size.into(),
                    ),
                    [
                        (viewport.visible_rect.width() as u32).div_ceil(8),
                        (viewport.visible_rect.height() as u32).div_ceil(8),
                        1,
                    ],
                );

                // for i in resources.meshlets.range(0..10) {
                //     log::info!("{:#?}", i);
                // }

                if instances.instance_count > 0 {
                    cmd.fill_buffer(resources.bvh_node_stack.range(..), !0);
                    cmd.fill_buffer(resources.candidate_meshlets.range(..), !0);
                    cmd.fill_buffer(resources.meshlet_batches.range(..), 0);
                    let cull_proj = setting.freez_proj.unwrap_or(camera.camera.proj);
                    let cull_view = setting.freez_view.unwrap_or(camera.camera.view);
                    let scissors = [Scissor {
                        extent: viewport.visible_rect.size().as_uvec2(),
                        offset: viewport.visible_rect.min.as_ivec2(),
                    }];
                    let vp = Viewport {
                        extent: viewport.rect.size().as_uvec2(),
                        offset: viewport.rect.min.as_ivec2(),
                    };
                    let dic = resources
                        .variables
                        .byte_range(offset_of!(TraversalVariables, vertex_count)..)
                        .cast::<DrawIndirectCommand>();
                    cmd.update_buffer(
                        resources.variables.range(..),
                        &TraversalVariables {
                            node_count: 0,
                            node_batch_read_offset: 0,
                            node_write_offset: 0,
                            visible_meshlet_count: 0,
                            first_instance: 0,
                            first_vertex: 0,
                            vertex_count: 128 * 3,
                            candidate_meshlet_write_offset: 0,
                            meshlet_batch_read_offset: 0,
                            total_meshlets: 0,
                        },
                    );
                    let clip_from_world = (cull_proj * cull_view).transpose();
                    cmd.compute(
                        InstanceCull::new(
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
                        BvhCull::new(
                            resources.bvh_node_stack.range(..),
                            resources.variables.range(..),
                            resources.meshlets.range(..),
                            resources.candidate_meshlets.range(..),
                            resources.meshlet_batches.range(..),
                            instances
                                .transforms
                                .range(MAX_INSTANCES * frame_in_flight..),
                            instances.headers.range(MAX_INSTANCES * frame_in_flight..),
                            setting
                                .freez_pos
                                .unwrap_or(camera.transform.translation.extend(0.0)),
                            cull_proj,
                            clip_from_world,
                            viewport.rect.height(),
                        ),
                        [64, 1, 1],
                    );
                    cmd.raster()
                        .color_attachment(swapchain.image(), None)
                        .depth_attachment(
                            resources.depth_attachment.whole_view(),
                            Some([0.0]),
                            true,
                        )
                        .backface_culling(true)
                        .draw_indirect_with_dynstates(
                            Raster::new(
                                camera.camera.view,
                                camera.camera.proj,
                                camera.transform.translation.extend(1.0),
                                instances
                                    .transforms
                                    .range(MAX_INSTANCES * frame_in_flight..),
                                resources.meshlets.range(..),
                                instances.materials.range(MAX_INSTANCES * frame_in_flight..),
                            ),
                            swapchain.size.into(),
                            dic,
                            &scissors,
                            vp,
                        );

                    if instances.any_outlined {
                        cmd.raster()
                            .backface_culling(true)
                            .depth_attachment(
                                resources.depth_attachment.whole_view(),
                                Some([0.0]),
                                true,
                            )
                            .draw_indirect_with_dynstates(
                                RasterOutline::new(
                                    camera.camera.view,
                                    camera.camera.proj,
                                    instances
                                        .transforms
                                        .range(MAX_INSTANCES * frame_in_flight..),
                                    resources.meshlets.range(..),
                                    instances.flags.range(MAX_INSTANCES * frame_in_flight..),
                                ),
                                swapchain.size.into(),
                                dic,
                                &scissors,
                                vp,
                            );

                        cmd.compute(
                            DrawOutline::new(
                                resources.depth_attachment.whole_view(),
                                swapchain.image(),
                                setting.outline_color.extend(setting.outline_radius),
                                viewport.visible_rect.min.as_ivec2(),
                                viewport.visible_rect.size().as_uvec2(),
                                swapchain.size.into(),
                            ),
                            [
                                (viewport.visible_rect.width() as u32).div_ceil(8),
                                (viewport.visible_rect.height() as u32).div_ceil(8),
                                1,
                            ],
                        );
                    }
                    if let Some(gizzmos) = gizzmos {
                        gizzmos.draw(cmd, &swapchain, &camera, &viewport, frame_in_flight);
                    }
                }

                cmd.raster()
                    .backface_culling(false)
                    .color_attachment(swapchain.image(), None)
                    .draw_indexed(
                        RasterUi::new(
                            ui_resources.verticies[frame_in_flight].range(..),
                            ui_resources.font_atlas.whole_view(),
                        ),
                        swapchain.size.into(),
                        ui_resources.indicies[frame_in_flight].range(..ui_resources.num_indicies),
                        1,
                    );
                // cmd.blit_image(
                //     nui_resources.font_atlas.whole(),
                //     swapchain.image().region(UVec2::new(
                //         nui_resources.font_atlas.extent.width,
                //         nui_resources.font_atlas.extent.height,
                //     )),
                //     Filter::Nearest,
                // );
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

#[allow(non_snake_case)]
pub fn RenderPassesPlugin(app: &mut App) {
    app.add_systems(Render, render.in_set(RenderSystems::Render))
        .add_systems(RenderSystems::AquireSwapchainImage, aquire_swapchain_image)
    .insert_resource(RenderSettings::default())
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
