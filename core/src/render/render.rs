use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::mem::offset_of;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use bevy::app::App;
use bevy::app::PreUpdate;
use bevy::app::Startup;
use bevy::app::Update;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::With;
use bevy::ecs::resource::Resource;
use bevy::ecs::schedule::IntoScheduleConfigs;
use bevy::ecs::system::Commands;
use bevy::ecs::system::If;
use bevy::ecs::system::Local;
use bevy::ecs::system::Query;
use bevy::ecs::system::Res;
use bevy::ecs::system::ResMut;
use bevy::ecs::system::Single;
use bevy::ecs::system::SystemState;
use bevy::tasks::AsyncComputeTaskPool;
use bevy::tasks::Task;
use bevy::time::Time;
use bevy::window::PrimaryWindow;
use bevy::window::Window;

use bevy::window::WindowResized;
use glam::Mat4;
use glam::UVec2;
use glam::Vec4;
use imgui::TreeNodeFlags;
use itertools::Itertools;
use itertools::traits::IteratorIndex;
use lava::command_buffer::Filter;
use lava::command_buffer::RasterVertexDispatch;
use lava::command_buffer::ResourceHandle;
use lava::command_buffer::ResourceState;
use lava::image::Image;
use lava::image::format;
use lava::image::format::D32Sfloat;
use lava::image::format::R32G32B32A32Sfloat;
use lava::image::slice::AsImage;
use lava::image::slice::ImageSlice;
use lava::image::slice::ImageView;
use lava::image::usage::ColorAttachmentSampled;
use lava::image::usage::ColorAttachmentStorage;
use lava::image::usage::DepthAttachmentSampled;
use lava::state::Ctx;
use lava::vkobjects;
use lava::vkobjects::queue::Binary;
use lava::vkobjects::queue::CommandBufferMemory;
use lava::vkobjects::queue::CommandPool;
use lava::vkobjects::queue::Event;
use lava::vkobjects::queue::Gfx;
use lava::vkobjects::queue::Present;
use lava::vkobjects::queue::Queue;
use lava::vkobjects::queue::Semaphore;
use lava::vkobjects::queue::Timeline;

use lava::buffer::Buffer;
use lava::vkobjects::queue::Fence;
use tracing_log::log;

use crate::INITIAL_WINDOW_SIZE;
use crate::bindings;
use crate::bindings::BvhCull;
use crate::bindings::BvhCullBindings;
use crate::bindings::DrawAabbsBindings;
use crate::bindings::DrawArrowsBindings;
use crate::bindings::DrawGizzmosBindings;
use crate::bindings::DrawSpheresBindings;
use crate::bindings::InstanceBvhRoot;
use crate::bindings::InstanceCull;
use crate::bindings::InstanceCullBindings;
use crate::bindings::InstancedMeshlet;
use crate::bindings::Meshlet;
use crate::bindings::Raster;
use crate::bindings::RasterBindings;
use crate::bindings::RasterUi;
use crate::bindings::RasterUiBindings;
use crate::bindings::Skybox;
use crate::bindings::SkyboxBindings;
use crate::bindings::TraversalVariables;
use crate::bindings::UIVertex;
use crate::render::ExtractSchedule;
use crate::render::MainWorld;
use crate::render::Render;
use crate::render::RenderApp;
use crate::render::RenderStartup;
use crate::render::RenderSystems;
use crate::render::extract_param::Extract;
use crate::render::world::Gizzmos;
use crate::render::world::InstanceManager;
use crate::render::world::MAX_INSTANCES;
use crate::render::world::UploadQueue;
use crate::ui::UiBuilder;
use crate::ui::UiContext;
use crate::ui::UiResources;
use crate::{components::camera::Camera, render::FRAMES_IN_FLIGHT};
use tracing::info;

#[derive(Resource)]
pub struct CommandPools {
    pub command_buffers: [CommandBufferMemory; FRAMES_IN_FLIGHT],
    pub pools: [CommandPool; FRAMES_IN_FLIGHT],
}

#[derive(Resource, Default)]
pub struct SynchronizationResources {
    pub fences: [Fence; FRAMES_IN_FLIGHT],
    pub image_available: [Semaphore<Binary>; FRAMES_IN_FLIGHT],
    pub render_finished: Vec<Semaphore<Binary>>,
}

pub enum QueueStrategie {
    Single(Arc<Mutex<Queue<Gfx>>>),
    Multiple(Queue<Gfx>),
}

impl QueueStrategie {
    fn with<R, F: FnOnce(&Queue<Gfx>) -> R>(&self, f: F) -> R {
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
    pub swpachain: lava::vkobjects::swapchain::Swapchain<'static>,
    pub image_index: u32,
}

impl Swapchain {
    fn image(&self) -> ImageView<format::Swapchain, ColorAttachmentStorage> {
        self.images[self.image_index as usize]
    }
}

impl Deref for Swapchain {
    type Target = lava::vkobjects::swapchain::Swapchain<'static>;
    fn deref(&self) -> &Self::Target {
        &self.swpachain
    }
}

pub fn wait_frames_in_flight(
    sync: Res<SynchronizationResources>,
    command_pools: Res<CommandPools>,
    mut frame: ResMut<FrameCount>,
) {
    frame.0 += 1;
    if frame.0 > FRAMES_IN_FLIGHT as u64 {
        sync.fences[frame.frame_in_flight()].wait();
    }
    sync.fences[frame.frame_in_flight()].reset();
    command_pools.pools[frame.frame_in_flight()].reset();
}

pub fn aquire_swapchain_image(
    sync: Res<SynchronizationResources>,
    frame: Res<FrameCount>,
    mut swapchain: ResMut<Swapchain>,
) {
    swapchain.image_index =
        swapchain.aquire_image(&sync.image_available[frame.frame_in_flight()], None);
}

pub fn resize_swapchain(
    mut swapchain: ResMut<Swapchain>,
    window: Extract<Single<&Window, With<PrimaryWindow>>>,
) {
    let size = window.physical_size().to_array();
    if size != swapchain.size {
        info!("Resized Swapchain");
        swapchain.swpachain.recreate(size);
    }
}

pub fn extract_camera(mut cmd: Commands, camera: Extract<Single<&Camera>>) {
    let cam = camera.clone();
    cmd.insert_resource(cam);
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
    let num_images = swapchain.images.len();
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
    let pools: [CommandPool; FRAMES_IN_FLIGHT] = (0..FRAMES_IN_FLIGHT)
        .map(|_| queues.graphics.with(|queue| queue.create_pool()))
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    let command_buffers: [CommandBufferMemory; FRAMES_IN_FLIGHT] = pools
        .iter()
        .map(|p| p.create_command_buffer())
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    UploadQueue::init(&queues);
    cmd.insert_resource(queues);
    cmd.insert_resource(CommandPools {
        pools,
        command_buffers,
    });
    cmd.insert_resource(ResourceStates {
        resource_states: Some(HashMap::new()),
    });
    cmd.insert_resource(SynchronizationResources {
        fences: Default::default(),
        image_available: Default::default(),
        render_finished: (0..num_images).map(|_| Semaphore::new()).collect(),
    });
}

#[derive(Resource)]
pub struct ResourceStates {
    resource_states: Option<HashMap<ResourceHandle, ResourceState>>,
}

pub struct RenderResources {
    depth_attachment: Image<D32Sfloat, DepthAttachmentSampled>,
    // color_attachment: Image<R32G32B32A32Sfloat, ColorAttachmentSampled>,
    meshlets: Buffer<InstancedMeshlet>,
    bvh_node_stack: Buffer<InstanceBvhRoot>,
    meshlet_batches: Buffer<u32>,
    candidate_meshlets: Buffer<bindings::InstanceMeshletIndex>,
    variables: Buffer<TraversalVariables>,
}

#[derive(Resource, Default, Clone)]
pub struct RenderValues {
    meshlet_count: u32,
    instance_count: u32,
}

#[derive(Resource, Default, Clone)]
pub struct RenderSettings {
    pub freez_proj: Option<Mat4>,
    pub freez_view: Option<Mat4>,
    pub freez_pos: Option<Vec4>,
    pub draw_scene_bvh: bool,
}

pub fn settings_ui(
    mut ui: ResMut<UiBuilder>,
    res: Res<RenderValues>,
    mut settings: ResMut<RenderSettings>,
    cam: Single<&Camera>,
    window: Single<&Window>,
) {
    let Some(ui) = ui.ui() else {
        return;
    };
    ui.window("Render Settings")
        .build(|| {
            ui.text(format!("Num Meshlets: {}", res.meshlet_count));
            ui.text(format!("Num Instances: {}", res.instance_count));
            if ui.small_button("Freez Cam") {
                settings.freez_proj =
                    Some(cam.projection_matrix(glam::Vec2::new(window.width(), window.height())));
                settings.freez_view = Some(cam.view_matrix());
                settings.freez_pos = Some(cam.position.extend(0.0))
            }
            if ui.small_button("Unfreeze Cam") {
                settings.freez_proj = None;
                settings.freez_view = None;
                settings.freez_pos = None;
            }
            if ui.small_button("Draw Scene Bvh") {
                settings.draw_scene_bvh = !settings.draw_scene_bvh;
            }
        })
        .unwrap();
}

pub fn extract_ui(mut cmd: Commands, mut world: ResMut<MainWorld>, values: Res<RenderValues>) {
    world.insert_resource(values.clone());
    cmd.insert_resource(world.get_resource::<RenderSettings>().unwrap().clone());
}

pub fn render(
    camera: Res<Camera>,
    instances: Res<InstanceManager>,
    ui_resources: Res<UiResources>,
    queues: Res<Queues>,
    gizzmos: Res<Gizzmos>,
    mut resources: Local<Option<RenderResources>>,
    mut resource_states: ResMut<ResourceStates>,
    cmds: Res<CommandPools>,
    frame: Res<FrameCount>,
    sync: Res<SynchronizationResources>,
    swapchain: Res<Swapchain>,
    mut values: ResMut<RenderValues>,
    setting: Res<RenderSettings>,
) {
    let frame_in_flight = frame.frame_in_flight();
    let resources = resources.get_or_insert_with(|| RenderResources {
        depth_attachment: Image::new(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32)
            .unwrap(),
        // color_attachment: Image::new(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32)
        //     .unwrap(),
        meshlets: Buffer::new(2 * 1024 * 1024, false).unwrap(),
        bvh_node_stack: Buffer::new(2 * 1024 * 1024, false).unwrap(),
        variables: Buffer::new(1, false).unwrap(),
        meshlet_batches: Buffer::new(2 * 1024 * 1024, false).unwrap(),
        candidate_meshlets: Buffer::new(2 * 1024 * 1024, false).unwrap(),
    });

    values.instance_count = instances.instance_count as u32;
    values.meshlet_count = resources.variables[0].visible_meshlet_count;

    let states = queues.graphics.with(|queue| {
        queue
            .execute_command(
                resource_states.resource_states.take(),
                &cmds.command_buffers[frame.frame_in_flight()],
                Some(&sync.fences[frame.frame_in_flight()]),
                &[sync.image_available[frame.frame_in_flight()].info()],
                &[sync.render_finished[swapchain.image_index as usize].info()],
                |cmd| {
                    let proj =
                        camera.projection_matrix(UVec2::from_array(swapchain.size).as_vec2());
                    let view = camera.view_matrix();

                    cmd.compute::<Skybox>()
                        .bind(SkyboxBindings {
                            out: swapchain.image().as_storage(),
                            inverse_proj: proj.inverse(),
                            inverse_view: view.inverse(),
                            window_size: Vec4::new(
                                swapchain.size[0] as f32,
                                swapchain.size[1] as f32,
                                0.0,
                                0.0,
                            ),
                        })
                        .dispatch(
                            swapchain.size[0].div_ceil(8),
                            swapchain.size[1].div_ceil(8),
                            1,
                        );
                    // for i in resources.meshlets.range(0..10) {
                    //     log::info!("{:#?}", i);
                    // }

                    if instances.instance_count > 0 {
                        cmd.fill_buffer(resources.bvh_node_stack.range(..), !0);
                        cmd.fill_buffer(resources.candidate_meshlets.range(..), !0);
                        cmd.fill_buffer(resources.meshlet_batches.range(..), 0);
                        let cull_proj = setting.freez_proj.unwrap_or(proj.clone());
                        let cull_view = setting.freez_view.unwrap_or(view.clone());

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
                        cmd.compute::<InstanceCull>()
                            .bind(InstanceCullBindings {
                                clip_from_world,
                                instance_transforms: instances
                                    .transforms
                                    .range(MAX_INSTANCES * frame_in_flight..),
                                instance_aabbs: instances
                                    .aabbs
                                    .range(MAX_INSTANCES * frame_in_flight..),
                                bvh_node_stack: resources.bvh_node_stack.range(..),
                                instance_bvh_root_nodes: instances
                                    .bvh_root_nodes
                                    .range(MAX_INSTANCES * frame_in_flight..),
                                num_instances: instances.instance_count as u64,
                                variables: resources.variables.range(..),
                            })
                            .dispatch(instances.instance_count.div_ceil(64) as u32, 1, 1);
                        cmd.compute::<BvhCull>()
                            .bind(BvhCullBindings {
                                instance_headers: instances
                                    .instance_headers
                                    .range(MAX_INSTANCES * frame_in_flight..),
                                visible_meshlets: resources.meshlets.range(..),
                                queue: resources.bvh_node_stack.range(..),
                                queue_state: resources.variables.range(..),
                                camera_pos: setting
                                    .freez_pos
                                    .unwrap_or(camera.position.extend(0.0)),
                                proj: cull_proj,
                                canidate_meshlets: resources.candidate_meshlets.range(..),
                                clip_from_world,
                                window_height: swapchain.size[1] as f32,
                                instance_transforms: instances
                                    .transforms
                                    .range(MAX_INSTANCES * frame_in_flight..),
                                meshlet_batch_buffer: resources.meshlet_batches.range(..),
                            })
                            .dispatch(64, 1, 1);
                        cmd.raster::<Raster>()
                            .bind(RasterBindings {
                                proj: proj.clone(),
                                view: view.clone(),
                                instance_transforms: instances
                                    .transforms
                                    .range(MAX_INSTANCES * frame_in_flight..),
                                meshlets: resources.meshlets.range(..),
                            })
                            .color_attachment(swapchain.image(), None)
                            .depth_attachment(resources.depth_attachment.view(), true)
                            .backface_culling(true)
                            .draw(
                                swapchain.size[0],
                                swapchain.size[1],
                                RasterVertexDispatch::DrawIndirect {
                                    buffer: resources
                                        .variables
                                        .byte_range(offset_of!(TraversalVariables, vertex_count)..)
                                        .cast(),
                                },
                            );
                        if gizzmos.aabb_range.len() > 0 {
                            cmd.raster::<bindings::DrawAabbs>()
                                .bind(DrawAabbsBindings {
                                    world_to_clip: proj * view,
                                    gizzmos: gizzmos.gizzmos.range(
                                        (MAX_INSTANCES * frame_in_flight
                                            + gizzmos.aabb_range.start)..,
                                    ),
                                })
                                .color_attachment(swapchain.image(), None)
                                // .depth_attachment(resources.depth_attachment.view(), false)
                                .backface_culling(false)
                                .wire_frame(false)
                                .draw(
                                    swapchain.size[0],
                                    swapchain.size[1],
                                    RasterVertexDispatch::Draw {
                                        vertex_count: 36,
                                        instance_count: gizzmos.aabb_range.len() as u32,
                                    },
                                );
                        }
                        if gizzmos.sphere_range.len() > 0 {
                            cmd.raster::<bindings::DrawSpheres>()
                                .bind(DrawSpheresBindings {
                                    world_to_clip: proj * view,
                                    gizzmos: gizzmos.gizzmos.range(
                                        (MAX_INSTANCES * frame_in_flight
                                            + gizzmos.sphere_range.start)..,
                                    ),
                                })
                                .color_attachment(swapchain.image(), None)
                                // .depth_attachment(resources.depth_attachment.view(), false)
                                .backface_culling(false)
                                .wire_frame(false)
                                .draw(
                                    swapchain.size[0],
                                    swapchain.size[1],
                                    RasterVertexDispatch::Draw {
                                        vertex_count: 576,
                                        instance_count: gizzmos.sphere_range.len() as u32,
                                    },
                                );
                        }
                        if gizzmos.arrow_range.len() > 0 {
                            cmd.raster::<bindings::DrawArrows>()
                                .bind(DrawArrowsBindings {
                                    world_to_clip: proj * view,
                                    gizzmos: gizzmos.gizzmos.range(
                                        (MAX_INSTANCES * frame_in_flight
                                            + gizzmos.arrow_range.start)..,
                                    ),
                                })
                                .color_attachment(swapchain.image(), None)
                                // .depth_attachment(resources.depth_attachment.view(), false)
                                .backface_culling(false)
                                .wire_frame(false)
                                .draw(
                                    swapchain.size[0],
                                    swapchain.size[1],
                                    RasterVertexDispatch::Draw {
                                        vertex_count: 216,
                                        instance_count: gizzmos.arrow_range.len() as u32,
                                    },
                                );
                        }
                    }

                    if let Some(font_atlas) = &ui_resources.font_atlas {
                        for list in &ui_resources.draw_lists[frame.frame_in_flight()] {
                            cmd.raster::<RasterUi>()
                                .bind(RasterUiBindings {
                                    font_atlas: font_atlas.as_sampled(),
                                    indicies: ui_resources.indicies[frame.frame_in_flight()]
                                        .range(list.start_index as usize..),
                                    verticies: ui_resources.verticies[frame.frame_in_flight()]
                                        .range(list.start_vertex as usize..),
                                })
                                .backface_culling(false)
                                .color_attachment(swapchain.image(), None)
                                .draw_scissored(
                                    RasterVertexDispatch::Draw {
                                        vertex_count: list.count,
                                        instance_count: 1,
                                    },
                                    swapchain.size[0],
                                    swapchain.size[1],
                                    &[list.clip_rect],
                                );
                        }
                    }
                    cmd.present(swapchain.image());
                },
            )
            .unwrap()
    });

    resource_states.resource_states = Some(states);
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

pub fn RenderPassesPlugin(app: &mut App) {
    app.add_systems(
        Render,
        (
            wait_frames_in_flight.in_set(RenderSystems::WaitFences),
            aquire_swapchain_image.in_set(RenderSystems::AquireSwapchainImage),
            render.in_set(RenderSystems::Render),
        ),
    )
    .add_systems(RenderStartup, init_render);
}

pub fn RenderDebugUi(app: &mut App) {
    app.insert_resource(RenderValues::default())
        .insert_resource(RenderSettings::default())
        .add_systems(Update, settings_ui);

    let render_app = app.get_sub_app_mut(RenderApp).unwrap();
    render_app
        .add_systems(ExtractSchedule, extract_ui)
        .insert_resource(RenderSettings::default())
        .insert_resource(RenderValues::default());
}
