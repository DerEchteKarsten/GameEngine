//! Headless rendering: frames of the scene passes drawn into an offscreen image and read back, without a window, swapchain, UI or render sub-app.
use bevy::{
    app::{App, Plugin},
    ecs::{resource::Resource, system::RunSystemOnce, world::World},
    log::info_span,
};
use bevy::{ecs::reflect::ReflectResource, reflect::Reflect};
use glam::UVec2;
use lava::{
    buffer::Buffer,
    image::{Image, format::R8G8B8A8Unorm, slice::AsImage, usage::ColorAttachmentStorage},
    vkobjects::queue::{FrameSlot, Gfx, PendingAccesses, Queue},
};

use crate::bindless;
use crate::profiler::{GpuFrame, Profiler, capture::now_ns};
use crate::render::{
    MainWorld,
    render::{
        RenderCamera, RenderResources, RenderSettings, RenderSkybox, extract_camera,
        extract_skybox, record_scene,
    },
    world::{InstanceManager, extract_meshlet_instances, init_world, wirte_instances},
};

/// Size of the headless render target. In the main world it stands in for the window size.
#[derive(Resource, Clone, Copy, Reflect)]
#[reflect(Resource)]
pub struct HeadlessSize(pub UVec2);

/// Initialises Vulkan without a display. Frames are drawn on demand by [`render_frame`].
pub struct HeadlessRenderPlugin {
    pub size: UVec2,
}

impl Plugin for HeadlessRenderPlugin {
    fn build(&self, app: &mut App) {
        lava::init(None, cfg!(debug_assertions), false).unwrap();
        bindless::init();
        app.insert_resource(HeadlessSize(self.size));
    }
}

pub struct HeadlessFrame {
    pub size: UVec2,
    /// Meshlets the frame drew.
    pub visible_meshlets: u32,
}

/// What [`render_frame`] keeps from one frame to the next.
#[derive(Resource)]
struct HeadlessRenderer {
    render_world: World,
    image: Image<R8G8B8A8Unorm, ColorAttachmentStorage>,
    readback: Buffer<u8>,
    resources: Option<RenderResources>,
    slot: FrameSlot,
    queue: Queue<Gfx>,
    /// What the frames left for the next one to synchronise with.
    pending: PendingAccesses,
}

/// The last frame [`render_frame`] drew, tightly packed RGBA8. Reading the host-visible
/// buffer is slow (about 10 ms for 1280x720), so it isn't part of every frame.
pub fn read_pixels(main_world: &World) -> Vec<u8> {
    let renderer = main_world.resource::<HeadlessRenderer>();
    renderer.readback.range(..).as_slice().to_vec()
}

/// Extracts the main world into the render world, draws it once and waits for the result.
/// With a [`Profiler`] in the main world, the frame's GPU timings are handed to it.
pub fn render_frame(main_world: &mut World, settings: &RenderSettings) -> HeadlessFrame {
    let _span = info_span!("render_frame").entered();
    let size = main_world.resource::<HeadlessSize>().0;
    let mut renderer = main_world
        .remove_resource::<HeadlessRenderer>()
        .unwrap_or_else(|| {
            let mut render_world = World::new();
            render_world.run_system_once(init_world).unwrap();
            let slot = bindless::storage_slots(1).unwrap();
            let queue = Queue::<Gfx>::new().unwrap();
            HeadlessRenderer {
                render_world,
                image: Image::new_storage(size.x, size.y, 1, slot).unwrap(),
                readback: Buffer::new((size.x * size.y * 4) as usize, true).unwrap(),
                resources: None,
                slot: FrameSlot::new(&queue).unwrap(),
                queue,
                pending: PendingAccesses::default(),
            }
        });
    let profiling = main_world.get_resource::<Profiler>().is_some_and(|p| p.enabled);

    let render_world = &mut renderer.render_world;
    render_world.insert_resource(MainWorld(std::mem::take(main_world)));
    let instances = render_world.run_system_once(extract_meshlet_instances);
    let camera = render_world.run_system_once(extract_camera);
    let sky = render_world.run_system_once(extract_skybox);
    *main_world = render_world.remove_resource::<MainWorld>().unwrap().0;
    instances.unwrap();
    sky.unwrap();
    camera.expect("the scene needs exactly one camera");
    render_world.run_system_once(wirte_instances).unwrap();

    let mut camera = render_world.remove_resource::<RenderCamera>().unwrap();
    let sky = render_world.resource::<RenderSkybox>();
    let instances = render_world.resource::<InstanceManager>();

    renderer.slot.set_profiling(profiling);
    let mut frame = renderer.slot.begin().unwrap();
    RenderResources::fit(&mut renderer.resources, size, size, &mut frame);
    let resources = renderer.resources.as_mut().unwrap();
    let (image, readback) = (&renderer.image, &renderer.readback);
    let submit_ns = now_ns();
    renderer.pending = frame
        .execute(
            &renderer.queue,
            std::mem::take(&mut renderer.pending),
            &[],
            &[],
            |cmd| {
                record_scene(
                    cmd,
                    image.whole_view(),
                    size,
                    &mut camera,
                    sky,
                    instances,
                    resources,
                    settings,
                    None,
                    0,
                );
                cmd.copy_image_to_buffer(image.whole(), readback.range(..));
            },
        )
        .unwrap();
    // Beginning a frame waits for the one the slot submitted before.
    let frame = renderer.slot.begin().unwrap();
    if let Some(timings) = frame.last_timings()
        && let Some(mut profiler) = main_world.get_resource_mut::<Profiler>()
    {
        let timings = timings.clone();
        profiler.add_gpu(GpuFrame { submit_ns, timings });
    }
    drop(frame);

    let frame = HeadlessFrame {
        size,
        visible_meshlets: renderer.resources.as_ref().unwrap().visible_meshlets(),
    };
    main_world.insert_resource(renderer);
    frame
}
