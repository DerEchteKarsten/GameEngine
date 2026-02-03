use std::ops::{DerefMut, Deref};
use ash::vk::{self, Format};
use async_channel::{Receiver, Sender};
use bevy::{app::{App, AppExit, AppLabel, Plugin, SubApp}, asset::AssetServer, ecs::{change_detection::Mut, query::With, resource::Resource, schedule::{IntoScheduleConfigs, MainThreadExecutor, Schedule, ScheduleBuildSettings, ScheduleLabel, Schedules, SystemSet}, system::{Local, Query, Res, ResMut}, world::World}, tasks::ComputeTaskPool, time::Time, utils::default, window::{PrimaryWindow, RawHandleWrapperHolder}};
use glam::Vec4;
use lava::{FRAMES_IN_FLIGHT, command_buffer::RasterVertexDispatch, state::Ctx, vkobjects::{buffer::*, image::*}};

use crate::{INITIAL_WINDOW_SIZE, RenderResources, UiState, bindings::{DispatchIndirectCommand, DispatchParams, DrawIndirectCommand, Post, PostBindings, Raster, RasterBindings, RasterUi, RasterUiBindings}, components::camera::Camera, render::world::{RenderWorld, StagingBuffer, WorldPlugin, init_world}, ui::UiResources};

mod world;
mod extract_param;

fn render(
    query: Query<&Camera>,
    world: Res<RenderWorld>,
    mut resources: Local<Option<RenderResources>>,
    mut staging_buffer: ResMut<StagingBuffer>,
    ui_resources: Res<UiResources>,
    time: Res<Time>,
    ui_state: Res<UiState>,
) {
    let camera = query.single().unwrap();

    let resources = resources.get_or_insert_with(|| RenderResources {
        depth_attachment: Image::new_2d(
            vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            Format::D32_SFLOAT,
            ImageSize::XY(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32),
        )
        .unwrap(),
        color_attachment: Image::new_2d(
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED,
            Format::R32G32B32A32_SFLOAT,
            ImageSize::XY(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32),
        )
        .unwrap(),
        cluster_buffer: Buffer::new(BufferUsageFlags::STORAGE, 1 << 14).unwrap(),
        dispatch_params: Buffer::<_, GpuBuffer>::from_data(
            BufferUsageFlags::INDIRECT_COMMAND | BufferUsageFlags::STORAGE,
            &mut staging_buffer.0,
            &[DispatchParams {
                node_head: 0,
                node_tail: 0,
                done: 0,
                meshlet_count: 0,
                indirect_draw: DrawIndirectCommand {
                    vertex_count: 128 * 3,
                    instance_count: 0,
                    first_instance: 0,
                    first_vertex: 0,
                },
                indirect_dispatch: DispatchIndirectCommand { x: 0, y: 1, z: 1 },
            }],
        )
        .unwrap(),
        bvh_node_stack: Buffer::new(BufferUsageFlags::STORAGE, 10000).unwrap(),
    });

    Ctx::record_frame(&mut |cmd, swapchain_image| {
        // cmd.update_buffer_element(
        //     &resources.dispatch_params,
        //     0,
        //     &DispatchParams {
        //         node_head: 0,
        //         node_tail: 0,
        //         done: 0,
        //         meshlet_count: 0,
        //         indirect_draw: DrawIndirectCommand {
        //             vertex_count: 128 * 3,
        //             instance_count: 0,
        //             first_instance: 0,
        //             first_vertex: 0,
        //         },
        //         indirect_dispatch: DispatchIndirectCommand { x: 0, y: 1, z: 1 },
        //     },
        // );

        // cmd.fill_buffer(&resources.bvh_node_stack, 0, 0);
        // cmd.fill_buffer(&resources.cluster_buffer, 0, 0);
        if world.instance_bvh_root_nodes.len() > 0 {
            // cmd.compute::<InstanceCull>()
            //     .bind(InstanceCullBindings {
            //         num_instances: world.instance_bvh_root_nodes.len() as u64,
            //         aabbs: &world.instance_aabbs,
            //         instance_bvh_root_nodes: &world.instance_bvh_root_nodes,
            //         bvh_node_stack: &resources.bvh_node_stack,
            //         dp: &resources.dispatch_params,
            //         instance_transforms: &world.instance_transforms,
            //     })
            //     .dispatch(
            //         world.instance_bvh_root_nodes.len().div_ceil(64) as u32,
            //         1,
            //         1,
            //     );

            // cmd.compute::<BvhCull>()
            //     .bind(BvhCullBindings {
            //         bvh_node_stack: &resources.bvh_node_stack,
            //         bvh_nodes: &world.bvh_nodes,
            //         clusters: &resources.cluster_buffer,
            //         cull_data: &world.cull_data,
            //         dp: &resources.dispatch_params,
            //         instance_transforms: &world.instance_transforms,
            //     })
            //     .dispatch(4, 1, 1);

            // let params =
            //     cmd.read_buffer(&resources.dispatch_params, &(**staging_buffer).cast(), 1, 0);

            cmd.raster::<Raster>()
                .bind(RasterBindings {
                    indicies: &world.indecies,
                    instance_offsets: &resources.cluster_buffer,
                    instance_transforms: &world.instance_transforms,
                    meshlets: &world.meshlets,
                    verticies: &world.vertices,
                    proj: camera.projection_matrix(),
                    view: camera.view_matrix(),
                })
                .color_attachment(&resources.color_attachment, Some([0.2, 0.2, 0.4, 1.0]))
                .depth_attachment(&resources.depth_attachment)
                .backface_culling(true)
                .draw_fullscreen(RasterVertexDispatch::Draw {
                    vertex_count: 128 * 3,
                    instance_count: world.meshlets.len() as u32,
                });



            // cmd.raster()
            //     .mesh("meshshader", "mesh")
            //     .fragment("meshshader", "fragment")
            //     .constants(c!(
            //         camera.projection_matrix(),
            //         camera.view_matrix(),
            //         Mat4::from_scale_rotation_translation(Vec3::splat(2.0), Quat::from_euler(glam::EulerRot::XYZ, PI/2.0, 0.0, 0.0), Vec3::ZERO),
            //     ))
            //     .read(&world.vertices)
            //     .read(&world.indecies)
            //     .read(&world.meshlets)
            //     .color_attachment(&resources.color_attachment, Some([0.2, 0.2, 0.4, 1.0]))
            //     .depth_attachment(&resources.depth_attachment)
            //     .backface_culling(false)
            //     .draw_fullscreen(RasterDispatch::launch_mesh(world.meshlets.len() as u32, 1, 1));
        }

        cmd.compute::<Post>()
            .bind(PostBindings {
                color: &resources.color_attachment,
                depth: &resources.depth_attachment,
                out: &swapchain_image,
                inverse_proj: camera.projection_matrix().inverse(),
                inverse_view: camera.view_matrix().inverse(),
                window_size: Vec4::new(
                    Ctx::window_width() as f32,
                    Ctx::window_height() as f32,
                    0.0,
                    0.0,
                ),
            })
            .dispatch_fullscreen();

        let frame = (Ctx::current_frame() + 1) as usize % FRAMES_IN_FLIGHT;

        if let Some(atlas) = &ui_resources.font_atlas {
            cmd.raster::<RasterUi>() 
                .bind(RasterUiBindings {
                    verticies: ui_resources.verticies[frame].as_ref(),
                    font_atlas: atlas,
                })
                .color_attachment(&swapchain_image, None)
                .backface_culling(false)
                .wire_frame(false)
                .index_buffer(&ui_resources.indicies[frame])
                .draw_fullscreen(RasterVertexDispatch::indexed(ui_resources.indicies[frame].len() as u32 / 3, 1, 0));
        }

        cmd.present(swapchain_image);
        Ok(())
    })
    .unwrap();
}


#[derive(SystemSet, Hash, Debug, PartialEq, Eq, Clone)]
enum RenderSystems {
    ApplyExtractCommands,
    Upload,
    WaitFences,
    AquireSwapchainImage,
    Render,
    Submit,
}

#[derive(ScheduleLabel, PartialEq, Eq, Debug, Clone, Hash, Default)]
pub struct ExtractSchedule;

#[derive(AppLabel, Hash, Debug, PartialEq, Eq, Clone)]
struct RenderApp;

#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct Render;

#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct RenderStartup;

impl Render {
    pub fn base_schedule() -> Schedule {
        let mut schedule = Schedule::new(Self);

        schedule.configure_sets(
            (
                RenderSystems::ApplyExtractCommands,
                RenderSystems::Upload,
                RenderSystems::WaitFences,
                RenderSystems::AquireSwapchainImage,
                RenderSystems::Render,
                RenderSystems::Submit,
            )
            .chain(),
        );
        schedule
    }
}

fn apply_extract_commands(render_world: &mut World) {
    render_world.resource_scope(|render_world, mut schedules: Mut<Schedules>| {
        schedules
            .get_mut(ExtractSchedule)
            .unwrap()
            .apply_deferred(render_world);
    });
}
#[derive(Resource, Default)]
struct ScratchMainWorld(World);


#[derive(Resource, Default)]
pub struct MainWorld(World);

impl Deref for MainWorld {
    type Target = World;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for MainWorld {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

fn extract(main_world: &mut World, render_world: &mut World) {
    let scratch_world = main_world.remove_resource::<ScratchMainWorld>().unwrap();
    let inserted_world = core::mem::replace(main_world, scratch_world.0);
    render_world.insert_resource(MainWorld(inserted_world));
    render_world.run_schedule(ExtractSchedule);

    let inserted_world = render_world.remove_resource::<MainWorld>().unwrap();
    let scratch_world = core::mem::replace(main_world, inserted_world.0);
    main_world.insert_resource(ScratchMainWorld(scratch_world));
}


/// A Label for the sub app that runs the parts of pipelined rendering that need to run on the main thread.
///
/// The Main schedule of this app can be used to run logic after the render schedule starts, but
/// before I/O processing. This can be useful for something like frame pacing.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, AppLabel)]
pub struct RenderExtractApp;

/// Channels used by the main app to send and receive the render app.
#[derive(Resource)]
pub struct RenderAppChannels {
    app_to_render_sender: Sender<SubApp>,
    render_to_app_receiver: Receiver<SubApp>,
    render_app_in_render_thread: bool,
}

impl RenderAppChannels {
    /// Create a `RenderAppChannels` from a [`async_channel::Receiver`] and [`async_channel::Sender`]
    pub fn new(
        app_to_render_sender: Sender<SubApp>,
        render_to_app_receiver: Receiver<SubApp>,
    ) -> Self {
        Self {
            app_to_render_sender,
            render_to_app_receiver,
            render_app_in_render_thread: false,
        }
    }

    /// Send the `render_app` to the rendering thread.
    pub fn send_blocking(&mut self, render_app: SubApp) {
        self.app_to_render_sender.send_blocking(render_app).unwrap();
        self.render_app_in_render_thread = true;
    }

    /// Receive the `render_app` from the rendering thread.
    /// Return `None` if the render thread has panicked.
    pub async fn recv(&mut self) -> Option<SubApp> {
        let render_app = self.render_to_app_receiver.recv().await.ok()?;
        self.render_app_in_render_thread = false;
        Some(render_app)
    }
}

impl Drop for RenderAppChannels {
    fn drop(&mut self) {
        if self.render_app_in_render_thread {
            // Any non-send data in the render world was initialized on the main thread.
            // So on dropping the main world and ending the app, we block and wait for
            // the render world to return to drop it. Which allows the non-send data
            // drop methods to run on the correct thread.
            self.render_to_app_receiver.recv_blocking().ok();
        }
    }
}

#[derive(Default)]
pub struct PipelinedRenderingPlugin;

impl Plugin for PipelinedRenderingPlugin {
    fn build(&self, app: &mut App) {
        // Don't add RenderExtractApp if RenderApp isn't initialized.
        if app.get_sub_app(RenderApp).is_none() {
            return;
        }
        app.insert_resource(MainThreadExecutor::new());

        let mut sub_app = SubApp::new();
        sub_app.set_extract(renderer_extract);
        app.insert_sub_app(RenderExtractApp, sub_app);
    }

    // Sets up the render thread and inserts resources into the main app used for controlling the render thread.
    fn cleanup(&self, app: &mut App) {
        // skip setting up when headless
        if app.get_sub_app(RenderExtractApp).is_none() {
            return;
        }

        let (app_to_render_sender, app_to_render_receiver) = async_channel::bounded::<SubApp>(1);
        let (render_to_app_sender, render_to_app_receiver) = async_channel::bounded::<SubApp>(1);

        let mut render_app = app
            .remove_sub_app(RenderApp)
            .expect("Unable to get RenderApp. Another plugin may have removed the RenderApp before PipelinedRenderingPlugin");

        // clone main thread executor to render world
        let executor = app.world().get_resource::<MainThreadExecutor>().unwrap();
        render_app.world_mut().insert_resource(executor.clone());

        render_to_app_sender.send_blocking(render_app).unwrap();

        app.insert_resource(RenderAppChannels::new(
            app_to_render_sender,
            render_to_app_receiver,
        ));

        std::thread::spawn(move || {
            #[cfg(feature = "trace")]
            let _span = bevy_log::info_span!("render thread").entered();

            let compute_task_pool = ComputeTaskPool::get();
            loop {
                // run a scope here to allow main world to use this thread while it's waiting for the render app
                let sent_app = compute_task_pool
                    .scope(|s| {
                        s.spawn(async { app_to_render_receiver.recv().await });
                    })
                    .pop();
                let Some(Ok(mut render_app)) = sent_app else {
                    break;
                };

                {
                    #[cfg(feature = "trace")]
                    let _sub_app_span =
                        bevy_log::info_span!("sub app", name = ?RenderApp).entered();
                    render_app.update();
                }

                if render_to_app_sender.send_blocking(render_app).is_err() {
                    break;
                }
            }

            log::debug!("exiting pipelined rendering thread");
        });
    }
}

fn renderer_extract(app_world: &mut World, _world: &mut World) {
    app_world.resource_scope(|world, main_thread_executor: Mut<MainThreadExecutor>| {
        world.resource_scope(|world, mut render_channels: Mut<RenderAppChannels>| {
            if let Some(mut render_app) = ComputeTaskPool::get()
                .scope_with_executor(true, Some(&*main_thread_executor.0), |s| {
                    s.spawn(async { render_channels.recv().await });
                })
                .pop()
                .unwrap()
            {
                render_app.extract(world);

                render_channels.send_blocking(render_app);
            } else {
                // Renderer thread panicked
                world.write_message(AppExit::error());
            }
        });
    });
}

#[derive(Default, Debug)]
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        let primary_window = app
            .world_mut()
            .query_filtered::<&RawHandleWrapperHolder, With<PrimaryWindow>>()
            .single(app.world())
            .ok()
            .cloned()
            .unwrap()
            .0;
        let mutex = primary_window.lock();
        let window = mutex.as_ref().unwrap().as_ref().unwrap();
        #[cfg(debug_assertions)]
        let validation = true;

        #[cfg(not(debug_assertions))]
        let validation = false;

        lava::init(&window.get_display_handle(), &window.get_window_handle(), validation, false).unwrap();

        let mut render_app = SubApp::new();
        render_app.update_schedule = Some(Render.intern());
        let mut extract_schedule = Schedule::new(ExtractSchedule);
        extract_schedule.set_build_settings(ScheduleBuildSettings {
            auto_insert_apply_deferred: false,
            ..default()
        });
        extract_schedule.set_apply_final_deferred(false);
        
        let mut should_run_startup = true;

        render_app
            .add_systems(RenderStartup, init_world)
            .add_schedule(extract_schedule)
            .add_schedule(Render::base_schedule())
            .add_systems(
                Render,
                (
                    apply_extract_commands.in_set(RenderSystems::ApplyExtractCommands),
                    Ctx::start_frame.in_set(RenderSystems::WaitFences),
                    render
                        .in_set(RenderSystems::Render),
                ),
            )
            .set_extract(move |main_world, render_world| {
                if should_run_startup {
                    render_world.run_schedule(RenderStartup);
                    should_run_startup = false;
                }

                extract(main_world, render_world);
            })
            .add_plugins(WorldPlugin);

        app.insert_sub_app(RenderApp, render_app);
        app.add_plugins(PipelinedRenderingPlugin);
    }
    fn ready(&self, _app: &App) -> bool {
        lava::is_init()
    }
}