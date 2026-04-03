use crate::{
    render::{
        render::{
            CommandPools, RenderDebugUi, RenderPassesPlugin, Swapchain, SynchronizationResources,
            aquire_swapchain_image, init_render, render, resize_swapchain, wait_frames_in_flight,
        },
        world::{InstanceManager, UploadQueue, WorldPlugin, init_world},
    },
    scene::camera::Camera,
    ui::UiResources,
};
use async_std::channel::{Receiver, Sender};
use bevy::{
    app::{App, AppExit, AppLabel, Plugin, PreStartup, SubApp},
    asset::AssetServer,
    ecs::{
        change_detection::Mut,
        query::With,
        resource::Resource,
        schedule::{
            IntoScheduleConfigs, MainThreadExecutor, Schedule, ScheduleBuildSettings,
            ScheduleLabel, Schedules, SystemSet,
        },
        system::{Commands, Local, Query, Res, ResMut, Single},
        world::World,
    },
    log,
    tasks::ComputeTaskPool,
    time::Time,
    utils::default,
    window::{PrimaryWindow, RawHandleWrapperHolder},
};
use glam::Vec4;
use lava::{
    buffer::Buffer,
    command_buffer::RasterVertexDispatch,
    image::{
        Image,
        format::{D32Sfloat, R32G32B32A32Sfloat},
        slice::{AsImage, ImageSlice},
        usage::{ColorAttachmentSampled, DepthAttachmentSampled},
    },
    state::Ctx,
    vkobjects::{
        self,
        queue::{Binary, CommandBufferMemory, CommandPool, Semaphore, Timeline},
    },
};
use std::ops::{Deref, DerefMut};

pub mod extract_param;
pub mod render;
pub mod world;

pub const FRAMES_IN_FLIGHT: usize = 2;

#[derive(SystemSet, Hash, Debug, PartialEq, Eq, Clone)]
pub enum RenderSystems {
    ApplyExtractCommands,
    WaitFences,
    AquireSwapchainImage,
    PreRender,
    Render,
    AfterFences,
}

#[derive(ScheduleLabel, PartialEq, Eq, Debug, Clone, Hash, Default)]
pub struct ExtractSchedule;

#[derive(AppLabel, Hash, Debug, PartialEq, Eq, Clone)]
pub struct RenderApp;

#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct Render;

#[derive(ScheduleLabel, Debug, Hash, PartialEq, Eq, Clone, Default)]
pub struct RenderStartup;

impl Render {
    pub fn base_schedule() -> Schedule {
        let mut schedule = Schedule::new(Self);

        schedule.configure_sets((
            (
                RenderSystems::ApplyExtractCommands,
                RenderSystems::WaitFences,
                RenderSystems::AquireSwapchainImage,
                RenderSystems::Render,
            )
                .chain(),
            RenderSystems::PreRender
                .after(RenderSystems::WaitFences)
                .before(RenderSystems::Render),
            RenderSystems::AfterFences.after(RenderSystems::WaitFences),
        ));
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
        let (app_to_render_sender, app_to_render_receiver) =
            async_std::channel::bounded::<SubApp>(1);
        let (render_to_app_sender, render_to_app_receiver) =
            async_std::channel::bounded::<SubApp>(1);

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
            let _span = log::info_span!("render thread").entered();

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
                    let _sub_app_span = log::info_span!("sub app", name = ?RenderApp).entered();
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

fn init(window: Single<&RawHandleWrapperHolder, With<PrimaryWindow>>) {
    #[cfg(debug_assertions)]
    let validation = true;

    #[cfg(not(debug_assertions))]
    let validation = false;

    let mutex = window.0.lock().unwrap();
    let handle = mutex.as_ref().unwrap();
    lava::init(
        &handle.get_display_handle(),
        &handle.get_window_handle(),
        validation,
        false,
    )
    .unwrap();
}

#[derive(Default, Debug)]
pub struct RenderPlugin;

impl Plugin for RenderPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PreStartup, init);
        app.init_resource::<ScratchMainWorld>();

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
            .add_schedule(extract_schedule)
            .add_schedule(Render::base_schedule())
            .add_systems(ExtractSchedule, resize_swapchain)
            .add_systems(
                Render,
                apply_extract_commands.in_set(RenderSystems::ApplyExtractCommands),
            )
            .set_extract(move |main_world, render_world| {
                if should_run_startup {
                    render_world.run_schedule(RenderStartup);
                    should_run_startup = false;
                }

                extract(main_world, render_world);
            })
            .add_plugins(WorldPlugin)
            .add_plugins(RenderPassesPlugin);

        app.insert_sub_app(RenderApp, render_app);
    }
}
