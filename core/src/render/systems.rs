use bevy::ecs::system::Query;
use bevy::ecs::system::Res;
use bevy::ecs::system::ResMut;
use lava::image::format;
use lava::image::slice::ImageView;
use lava::image::usage::ColorAttachmentStorage;
use lava::state::Ctx;

use crate::INITIAL_WINDOW_SIZE;
use crate::{components::camera::Camera, render::FRAMES_IN_FLIGHT};

#[derive(Resource)]
pub struct CommandPools {
    pub command_buffers: [CommandBufferMemory; FRAMES_IN_FLIGHT],
    pub pools: [CommandPool; FRAMES_IN_FLIGHT],
}

#[derive(Resource)]
pub struct SynchronizationResources {
    pub timeline: Semaphore<Timeline>,
    pub image_available: [Semaphore<Binary>; FRAMES_IN_FLIGHT],
    pub render_finished: [Semaphore<Binary>; FRAMES_IN_FLIGHT],
}

#[derive(Resource)]
pub struct FrameCount(pub u64);

impl FrameCount {
    fn frame_in_flight(&self) -> usize {
        self.0 as usize % FRAMES_IN_FLIGHT
    }
}

#[derive(Resource)]
pub struct SwapchainImage(pub ImageView<format::Swapchain, ColorAttachmentStorage>);

pub fn wait_frames_in_flight(
    sync: Res<SynchronizationResources>,
    mut command_pools: ResMut<CommandPools>,
    mut frame: ResMut<FrameCount>,
) {
    let next_frame = frame.0 + 1;
    let next_frame_in_flight = next_frame as usize % FRAMES_IN_FLIGHT;

    sync.timeline.block_until_value(next_frame);
    command_pools.pools[next_frame_in_flight].reset();

    frame.0 += 1;
}

pub fn aquire_swapchain_image(
    sync: Res<SynchronizationResources>,
    frame: Res<FrameCount>,
    swapchain_image: ResMut<SwapchainImage>,
) {
    let image_index = Swapchain::aquire_image(&sync.image_available[frame.frame_in_flight()]);
    swapchain_image.0 = Ctx::get_swapchain_image(image_index);
}

struct RenderResources {
    depth_attachment: Image<D32Sfloat, DepthAttachmentSampled>,
    color_attachment: Image<R32G32B32A32Sfloat, ColorAttachmentSampled>,
    cluster_buffer: Buffer<InstancedOffset>,
    // dispatch_params: Buffer<DispatchParams>,
    bvh_node_stack: Buffer<InstancedOffset>,
}

pub fn render(
    query: Query<&Camera>,
    instances: Res<InstanceManager>,
    meshlets: Res<MeshletManager>,
    mut resources: Local<Option<RenderResources>>,
    ui_resources: Res<UiResources>,
    time: Res<Time>,
    mut cmds: ResMut<CommandPools>,
    frame: Res<FrameCount>,
    sync: Res<SynchronizationResources>,
) {
    let camera = query.single().unwrap();

    let resources = resources.get_or_insert_with(|| RenderResources {
        depth_attachment: Image::new(ImageSize::XY(
            INITIAL_WINDOW_SIZE.x as u32,
            INITIAL_WINDOW_SIZE.y as u32,
        ))
        .unwrap(),
        color_attachment: Image::new(ImageSize::XY(
            INITIAL_WINDOW_SIZE.x as u32,
            INITIAL_WINDOW_SIZE.y as u32,
        ))
        .unwrap(),
        cluster_buffer: Buffer::new(1 << 14).unwrap(),
        // dispatch_params: Buffer::<_, GpuBuffer>::from_data(
        //     BufferUsageFlags::INDIRECT_COMMAND | BufferUsageFlags::STORAGE,
        //     &mut staging_buffer.0,
        //     &[DispatchParams {
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
        //     }],
        // )
        // .unwrap(),
        bvh_node_stack: Buffer::new(10000).unwrap(),
    });

    Ctx::gfx_queue()
        .execute_command(
            &mut cmds.command_buffers[frame.frame_in_flight()],
            None,
            &[sync.image_avalible[frame.frame_in_flight()]],
            &[
                sync.timeline.info(frame.frame_in_flight() + 1),
                sync.render_finished[frame.frame_in_flight()].info(),
            ],
            |cmd| {
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
                if instances.bvh_root_nodes.len() > 0 {
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
                            instance_offsets: resources.cluster_buffer.whole(),
                            instance_transforms: instances.transforms.whole(),
                            indicies: meshlets.indices.whole(),
                            meshlets: meshlets.meshlets.whole(),
                            verticies: meshlets.vertices.whole(),
                            proj: camera.projection_matrix(),
                            view: camera.view_matrix(),
                        })
                        .color_attachment(
                            resources.color_attachment.view(),
                            Some([0.2, 0.2, 0.4, 1.0]),
                        )
                        .depth_attachment(resources.depth_attachment.view())
                        .backface_culling(true)
                        .draw_fullscreen(RasterVertexDispatch::Draw {
                            vertex_count: 128 * 3,
                            instance_count: meshlets.meshlets.len() as u32,
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
                        color: resources.color_attachment.as_sampled(),
                        depth: resources.depth_attachment.as_sampled(),
                        out: swapchain_image.as_storage(),
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

                // if let Some(atlas) = &ui_resources.font_atlas {
                //     cmd.raster::<RasterUi>()
                //         .bind(RasterUiBindings {
                //             verticies: ui_resources.verticies,
                //             indicies: ui_resources.indicies,
                //             font_atlas: atlas,
                //         })
                //         .color_attachment(&swapchain_image, None)
                //         .backface_culling(false)
                //         .wire_frame(false)
                //         .draw_fullscreen(RasterVertexDispatch::Draw { vertex_count: , instance_count: () });
                // }

                cmd.present(swapchain_image);
            },
        )
        .unwrap();
    Ctx::present_queue()
        .present(image_index, &present_sem)
        .unwrap()
}
