//! Headless rendering: one frame of the scene passes drawn into an offscreen image and read back, without a window, swapchain, UI or render sub-app.
use bevy::{
    app::{App, Plugin},
    ecs::{resource::Resource, system::RunSystemOnce, world::World},
};
use bevy::{ecs::reflect::ReflectResource, reflect::Reflect};
use glam::UVec2;
use lava::{
    buffer::Buffer,
    image::{Image, format::R8G8B8A8Unorm, slice::AsImage, usage::ColorAttachmentStorage},
    vkobjects::queue::{FrameSlot, Gfx, PendingAccesses, Queue},
};

use crate::bindless;
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
    /// Tightly packed RGBA8.
    pub pixels: Vec<u8>,
    /// Meshlets the frame drew.
    pub visible_meshlets: u32,
}

/// Extracts the main world into a throwaway render world, draws it once and waits for the result.
pub fn render_frame(main_world: &mut World, settings: &RenderSettings) -> HeadlessFrame {
    let size = main_world.resource::<HeadlessSize>().0;

    let mut render_world = World::new();
    render_world.run_system_once(init_world).unwrap();
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

    let slot = bindless::storage_slots(1).unwrap();
    let image =
        Image::<R8G8B8A8Unorm, ColorAttachmentStorage>::new_storage(size.x, size.y, 1, slot)
            .unwrap();
    let readback = Buffer::<u8>::new((size.x * size.y * 4) as usize, true).unwrap();
    let queue = Queue::<Gfx>::new().unwrap();
    let mut slot = FrameSlot::new(&queue).unwrap();
    let mut frame = slot.begin().unwrap();
    let mut resources = None;
    let resources = RenderResources::fit(&mut resources, size, size, &mut frame);
    frame
        .execute(&queue, PendingAccesses::default(), &[], &[], |cmd| {
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
        })
        .unwrap();
    // Beginning a frame waits for the one the slot submitted before.
    drop(slot.begin().unwrap());

    HeadlessFrame {
        size,
        pixels: readback.range(..).as_slice().to_vec(),
        visible_meshlets: resources.visible_meshlets(),
    }
}
