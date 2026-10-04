//! Headless rendering: the scene passes drawn into an offscreen image that is read back, without a window, swapchain or UI.
use bevy::{
    app::{App, SubApp},
    ecs::{
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, Local, Res, ResMut, SystemState},
        world::{Mut, World},
    },
};
use glam::UVec2;
use lava::{
    buffer::Buffer,
    image::{Image, format::R8G8B8A8Unorm, slice::AsImage, usage::ColorAttachmentStorage},
    vkobjects::queue::Frame,
};

use crate::render::{
    Render, RenderApp, RenderStartup, RenderSystems,
    render::{
        FrameCount, FrameSlots, Queues, RenderCamera, RenderResources, RenderSettings,
        ResourceStates, init_queues, record_scene,
    },
    world::InstanceManager,
};

/// Size of the headless render target. In the main world it stands in for the window size.
#[derive(Resource, Clone, Copy)]
pub struct HeadlessSize(pub UVec2);

/// What the headless renderer draws into, and the host-visible copy of it.
#[derive(Resource)]
pub struct HeadlessTarget {
    image: Image<R8G8B8A8Unorm, ColorAttachmentStorage>,
    /// The last recorded frame as tightly packed RGBA8.
    readback: Buffer<u8>,
    /// Meshlets the last finished frame drew.
    pub visible_meshlets: u32,
}

/// Initialises Vulkan without a display.
pub(super) fn init() {
    lava::init(None, cfg!(debug_assertions), false).unwrap();
}

fn init_target(mut cmd: Commands, size: Res<HeadlessSize>) {
    let size = size.0;
    init_queues(&mut cmd, None);
    cmd.insert_resource(HeadlessTarget {
        image: Image::new(size.x, size.y).unwrap(),
        readback: Buffer::new((size.x * size.y * 4) as usize, true).unwrap(),
        visible_meshlets: 0,
    });
}

type HeadlessParams<'w, 's> = (
    Option<ResMut<'w, RenderCamera>>,
    Res<'w, InstanceManager>,
    Res<'w, Queues>,
    Local<'s, Option<RenderResources>>,
    ResMut<'w, ResourceStates>,
    Res<'w, RenderSettings>,
    ResMut<'w, HeadlessTarget>,
);

fn render(world: &mut World, params: &mut SystemState<HeadlessParams<'static, 'static>>) {
    world.resource_scope(|world, mut slots: Mut<FrameSlots>| {
        let frame_in_flight = {
            let mut frame = world.resource_mut::<FrameCount>();
            frame.0 += 1;
            frame.frame_in_flight()
        };
        let frame = slots.slots[frame_in_flight].begin().unwrap();
        world.run_schedule(RenderSystems::PreRender);
        record_frame(frame, frame_in_flight, params.get_mut(world));
    });
}

fn record_frame(
    mut frame: Frame,
    frame_in_flight: usize,
    (camera, instances, queues, mut resources, mut resource_states, setting, mut target): HeadlessParams,
) {
    let Some(mut camera) = camera else {
        return;
    };
    let size = target.image.extent;
    let resources = RenderResources::fit(&mut resources, size, size, &mut frame);
    target.visible_meshlets = resources.visible_meshlets();

    let states = queues.graphics.with(|queue| {
        frame
            .execute(
                queue,
                resource_states.pending.take().unwrap(),
                &[],
                &[],
                |cmd| {
                    record_scene(
                        cmd,
                        target.image.whole_view(),
                        size,
                        &mut camera,
                        &instances,
                        resources,
                        &setting,
                        None,
                        frame_in_flight,
                    );
                    cmd.copy_image_to_buffer(target.image.whole(), target.readback.range(..));
                },
            )
            .unwrap()
    });
    resource_states.pending = Some(states);
}

pub(super) fn build(render_app: &mut SubApp, size: UVec2) {
    render_app
        .insert_resource(HeadlessSize(size))
        .insert_resource(RenderSettings::default())
        .add_systems(RenderStartup, init_target)
        .add_systems(Render, render.in_set(RenderSystems::Render));
}

/// Waits for the submitted frames and returns the last one as tightly packed RGBA8 with its
/// size. The app must not use `PipelinedRenderingPlugin`: the render world has to be at home.
pub fn read_frame(app: &mut App) -> (UVec2, Vec<u8>) {
    let world = app.sub_app_mut(RenderApp).world_mut();
    for slot in &mut world.resource_mut::<FrameSlots>().slots {
        // Beginning a frame waits for the one the slot submitted before.
        drop(slot.begin().unwrap());
    }
    let target = world.resource::<HeadlessTarget>();
    (
        target.image.extent,
        target.readback.range(..).as_slice().to_vec(),
    )
}
