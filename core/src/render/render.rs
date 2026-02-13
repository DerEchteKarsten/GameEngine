use std::ops::Deref;

use bevy::ecs::resource::Resource;
use bevy::ecs::system::Commands;
use bevy::ecs::system::Local;
use bevy::ecs::system::Query;
use bevy::ecs::system::Res;
use bevy::ecs::system::ResMut;
use bevy::ecs::system::Single;
use bevy::time::Time;
use glam::UVec2;
use glam::Vec4;
use lava::command_buffer::RasterVertexDispatch;
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
use lava::vkobjects::queue::Semaphore;
use lava::vkobjects::queue::Timeline;

use lava::buffer::AsBuffer;
use lava::buffer::Buffer;
use lava::vkobjects::queue::Fence;

use crate::INITIAL_WINDOW_SIZE;
use crate::bindings::InstancedOffset;
use crate::bindings::Post;
use crate::bindings::PostBindings;
use crate::bindings::Raster;
use crate::bindings::RasterBindings;
use crate::render::extract_param::Extract;
use crate::render::world::InstanceManager;
use crate::render::world::MeshletManager;
use crate::render::world::UploadQueue;
use crate::ui::UiResources;
use crate::{components::camera::Camera, render::FRAMES_IN_FLIGHT};

#[derive(Resource)]
pub struct CommandPools {
    pub command_buffers: [CommandBufferMemory; FRAMES_IN_FLIGHT],
    pub pools: [CommandPool; FRAMES_IN_FLIGHT],
}

#[derive(Resource, Default)]
pub struct SynchronizationResources {
    pub fences: [Fence; FRAMES_IN_FLIGHT],
    pub image_available: [Semaphore<Binary>; FRAMES_IN_FLIGHT],
    pub render_finished: [Semaphore<Binary>; FRAMES_IN_FLIGHT],
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
    fn image(&self) -> ImageView<format::Swapchain, ColorAttachmentStorage> {
        self.images[self.image_index as usize]
    }
}

impl Deref for Swapchain {
    type Target = lava::vkobjects::swapchain::Swapchain;
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
    swapchain.image_index = swapchain.aquire_image(&sync.image_available[frame.frame_in_flight()]);
}

pub struct RenderResources {
    depth_attachment: Image<D32Sfloat, DepthAttachmentSampled>,
    color_attachment: Image<R32G32B32A32Sfloat, ColorAttachmentSampled>,
    cluster_buffer: Buffer<InstancedOffset>,
    bvh_node_stack: Buffer<InstancedOffset>,
}

pub fn extract_camera(mut cmd: Commands, camera: Extract<Single<&Camera>>) {
    let cam = camera.clone();
    cmd.insert_resource(cam);
}

pub fn init_render(mut cmd: Commands) {
    cmd.insert_resource(Swapchain {
        image_index: 0,
        swpachain: vkobjects::swapchain::Swapchain::new(None, None).unwrap()
    });
    let pools: [CommandPool; 2] = (0..FRAMES_IN_FLIGHT).map(|_| {
        Ctx::gfx_queue().create_pool()
    }).collect::<Vec<_>>().try_into().unwrap();
    let command_buffers: [CommandBufferMemory; 2] = pools.iter().map(|p| {
        p.create_command_buffer()
    }).collect::<Vec<_>>().try_into().unwrap();
    cmd.insert_resource(UploadQueue::new());
    cmd.insert_resource(CommandPools {
        pools,
        command_buffers
    });
    cmd.init_resource::<SynchronizationResources>();
    cmd.init_resource::<MeshletManager>();
}

pub fn render(
    camera: Res<Camera>,
    instances: Res<InstanceManager>,
    meshlets: Res<MeshletManager>,
    mut resources: Local<Option<RenderResources>>,
    cmds: ResMut<CommandPools>,
    frame: Res<FrameCount>,
    sync: Res<SynchronizationResources>,
    swapchain: Res<Swapchain>,
) {
    let resources = resources.get_or_insert_with(|| RenderResources {
        depth_attachment: Image::new(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32)
            .unwrap(),
        color_attachment: Image::new(INITIAL_WINDOW_SIZE.x as u32, INITIAL_WINDOW_SIZE.y as u32)
            .unwrap(),
        cluster_buffer: Buffer::new(1 << 14).unwrap(),
        bvh_node_stack: Buffer::new(10000).unwrap(),
    });

    Ctx::gfx_queue()
        .execute_command(
            cmds.command_buffers[frame.frame_in_flight()],
            Some(sync.fences[frame.frame_in_flight()]),
            &[sync.image_available[frame.frame_in_flight()].info()],
            &[
                sync.render_finished[frame.frame_in_flight()].info(),
            ],
            |cmd| {
                let proj = camera.projection_matrix(UVec2::from_array(swapchain.size).as_vec2());
                let view = camera.view_matrix();

                if instances.bvh_root_nodes.len() > 0 {
                    cmd.raster::<Raster>()
                        .bind(RasterBindings {
                            meshlets: meshlets.mesh_buffers[0].whole().cast(),
                            proj: proj.clone(),
                            view: view.clone(),
                        })
                        .color_attachment(
                            resources.color_attachment.view(),
                            Some([0.2, 0.2, 0.4, 1.0]),
                        )
                        .depth_attachment(resources.depth_attachment.view())
                        .backface_culling(true)
                        .draw(
                            swapchain.size[0],
                            swapchain.size[1],
                            RasterVertexDispatch::Draw {
                                vertex_count: 126,
                                instance_count: 1000,
                            },
                        );
                }

                cmd.compute::<Post>()
                    .bind(PostBindings {
                        color: resources.color_attachment.as_sampled(),
                        depth: resources.depth_attachment.as_sampled(),
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

                cmd.present(swapchain.image());
            },
        )
        .unwrap();
    Ctx::present_queue()
        .present(
            &swapchain,
            swapchain.image_index,
            &[&sync.render_finished[frame.frame_in_flight()]],
        )
        .unwrap();
}
