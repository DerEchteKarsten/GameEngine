#![feature(f16)]
#![feature(random)]

use std::{ops::Deref, random::random};

use ash::vk::{self, BufferUsageFlags, Format, VideoChromaSubsamplingFlagsKHR};
use bevy_a11y::AccessibilityPlugin;
use bevy_app::{App, PostUpdate, PreStartup, PreUpdate, Startup, TaskPoolPlugin, Update};
use bevy_asset::AssetPlugin;
use bevy_ecs::{
    event::EventReader,
    resource::Resource,
    system::{Commands, Query, Res, ResMut},
    world::World,
};
use bevy_input::InputPlugin;
use bevy_log::LogPlugin;
use bevy_time::TimePlugin;
use bevy_window::{
    ExitCondition, Window, WindowEvent, WindowPlugin, WindowResized, WindowResolution,
    WindowScaleFactorChanged,
};
use bevy_winit::{WinitPlugin, WinitWindows};
use glam::{IVec3, Mat4, Vec2, Vec3};
use gpu_allocator::MemoryLocation;
use lava::{
    state::Ctx,
    vkobjects::{buffer::{Buffer, DynamicBuffer}, image::ImageSize},
};

use crate::{
    assets::MeshAssets,
    components::camera::{Camera, CameraPlugin},
    world::{
        RenderWorld, STAGING_BUFFER_SIZE, StagingBuffer, add_instance, init_world,
        load_assets, transform_child_changed, transform_parent_changed,
    },
};

pub mod assets;
pub mod components;
pub mod world;

pub const INITIAL_WINDOW_SIZE: Vec2 = Vec2::new(1280.0, 720.0);


pub fn init(world: &mut World) {
    let windows = world.get_non_send_resource::<WinitWindows>().unwrap();
    let window = windows.windows.values().into_iter().last().unwrap().deref();

    lava::init(Some(&window), true).unwrap();
}

pub fn on_resize(mut event_reader: EventReader<WindowResized>) {
    for e in event_reader.read() {
        log::info!("test, {}, {}", e.width, e.height);
        Ctx::resize_swapchain(e.width as u32, e.height as u32);
    }
}

// fn on_scale_changed(
//     mut ev: EventReader<WindowScaleFactorChanged>,
//     windows: Query<&Window>,
// ) {
//     for e in ev.read() {
//         if let Ok(win) = windows.get(e.window) {
//             let phys = win.physical_size();
//             log::info!("scale changed -> scale:{}, new physical: {}x{}",
//                   e.scale_factor, phys.x, phys.y);
//         }
//     }
// }

#[derive(Resource)]
struct VoxelWorld {
    buffer: DynamicBuffer,
    nodes: Vec<CompressedNode>,
    leaf_data: Vec<u8>,
}

#[derive(Default, Clone, Copy)]
#[repr(C)]
struct CompressedNode {
    child_ptr: u32,
    pop_mask: u64,
}

impl CompressedNode {
    fn is_leaf(&self) -> bool {
        self.child_ptr & 1 != 0
    }
    fn make_leaf(&mut self) {
        self.child_ptr |= 1;
    }
    fn child_ptr(&self) -> u32 {
        self.child_ptr >> 2
    }
    fn set_child_ptr(&mut self, ptr: u32) {
        assert!(ptr < 0x3fff_ffff);
        self.child_ptr = (self.child_ptr & 3) | ptr << 2;
    }
}


fn build_voxel_world(data: &mut Vec<CompressedNode>, leaf_data: &mut Vec<u8>, mut scale: u32, pos: IVec3) -> CompressedNode {
    let mut node = CompressedNode::default();
    if scale == 2 {
        assert!((pos.x | pos.y | pos.z) % 4 == 0);
        node.make_leaf();
        node.set_child_ptr((leaf_data.len() / 4) as u32);
        node.pop_mask = random();
        let num_children = node.pop_mask.count_ones();
        let mut children_data = [1u8; 64];
        // children_data[0..num_children as usize].copy_from_slice(&vec![1u8; num_children as usize]);

        let size = (num_children + 3) & !3; //align as 4 bytes

        leaf_data.extend_from_slice(&children_data[0..size as usize]);
        return node;
    }

    scale -= 2;

    let mut children = Vec::new();
    for i in 0..64 {
        let child_pos = IVec3::splat(i) >> IVec3::new(0, 4, 2) & 3;
        let child = build_voxel_world(data, leaf_data, scale, pos + (child_pos << scale));

        if child.pop_mask != 0 {
            node.pop_mask |= 1u64 << i;
            children.push(child);
        }
    }

    node.set_child_ptr(data.len() as u32);
    data.extend(children);

    node
}


fn init_voxel_world(mut cmd: Commands) {
    let mut data = Vec::new();
    let mut leaf_data = Vec::new();
    data.push(CompressedNode::default());
    let root_node = build_voxel_world(&mut data, &mut leaf_data, 4, IVec3::new(0,0,0));
    data[0] = root_node;

    let mut voxel_world = VoxelWorld {
        buffer: DynamicBuffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            10000,
            Some(64),
        )
        .unwrap(),
        leaf_data,
        nodes: data,
    };

    let staging_buffer = Buffer::new(
        BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::CpuToGpu,
        STAGING_BUFFER_SIZE,
    )
    .unwrap();

    // staging_buffer
    //     .copy_data_to_buffer(&voxel_world.data)
    //     .unwrap();

    // Ctx::queue()
    //     .execute_command_wait(|cmd_buf| {
    //         let copy_region = vk::BufferCopy::default().size(64);
    //         unsafe {
    //             Ctx::device().cmd_copy_buffer(
    //                 *cmd_buf,
    //                 staging_buffer.buffer,
    //                 voxel_world.buffer.buffer,
    //                 &[copy_region],
    //             );
    //         }
    //     })
    //     .unwrap();
    let mut node = CompressedNode::default();
    node.set_child_ptr(3);
    node.make_leaf();
    node.pop_mask = u64::MAX;
    let nodes = vec![
        node
    ];
    let leaf_data = vec![
        1u8; 128
    ];
    voxel_world.buffer.push(&staging_buffer, &nodes);
    voxel_world.buffer.push(&staging_buffer, &leaf_data);

    
    cmd.insert_resource(VoxelWorld {
        ..voxel_world
    });
    cmd.insert_resource(StagingBuffer(staging_buffer));
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Constants {
    proj_inverse: Mat4,
    view_inverse: Mat4,
    window_size: Vec2,
    camera_position: Vec3,
}

fn commands(
    world: Res<VoxelWorld>,
    query: Query<&Camera>,
) {
    let camera = query.single().unwrap();
    let constants = Constants {
        proj_inverse: camera.projection_matrix().inverse(),
        camera_position: camera.position,
        view_inverse: camera.view_matrix().inverse(),
        window_size: Vec2::new(
            Ctx::window_width().unwrap() as f32,
            Ctx::window_height().unwrap() as f32,
        )
    };

    // let depth = rg.0.image(ImageSize::FullScreen, Format::D32_SFLOAT, "depth");
    // let color = rg.0.image(ImageSize::FullScreen, Format::R32G32B32A32_SFLOAT, "color");

    Ctx::next_frame(&mut |mut cmd, swapchain_image| {
        // let test2 = RasterPass::new(&mut rg, "test2")
        //     .fragment("fragment", "bindless_test2")
        //     .mesh("mesh", "bindless_test2")
        //     .task("amp", "bindless_test2")
        //     .constants(gconst.as_ref())
        //     .read(IMPORTED, world.dgf_buffer)
        //     .read(IMPORTED, world.material_buffer)
        //     .read(IMPORTED, world.instance_buffer)
        //     .read(IMPORTED, world.draw_tasks)
        //     .depth_attachment(IMPORTED, depth)
        //     .color_attachment(IMPORTED, color, Some([0.1, 0.15, 0.3, 1.0]))
        //     .render_area(WorkSize2D::FullScreen)
        //     .draw(DispatchSize::X(
        //         (render_world.num_instance_indices as u32).div_ceil(64),
        //     ));
        // let test2 = RasterPass::new(rg, "test2")
        //     .fragment("frag", "test")
        //     .vertex("vert", "test")
        //     .constants(gconst.as_ref())
        //     .depth_attachment(IMPORTED, depth)
        //     .color_attachment(IMPORTED, swapchain, Some([0.1, f32::sin(Ctx::current_frame() as f32 / 100.0), 0.3, 1.0]))
        //     .render_area(WorkSize2D::FullScreen)
        //     .backface_culling(false)
        //     .draw(DispatchSize::VertexCountInstanceCount(
        //         3, 1,
        //     ));
        // ComputePass::new(&mut rg, "test")
        //     .shader("bindless_test")
        //     .read(test2, depth)
        //     .read(test2, color)
        //     .write(IMPORTED, swapchain)
        //     .dispatch(DispatchSize::FullScreen);

        cmd.compute()
            .shader_path("gbuffer")
            .constant(&constants)
            .read(&world.buffer)
            .write(&swapchain_image)
            .dispatch_fullscreen();
        Ok(())
    }).unwrap();
}

pub fn CorePlugin(app: &mut App) {
    app.add_systems(PreStartup, init)
        .add_systems(Startup, init_voxel_world)
        .add_plugins((
            LogPlugin {
                filter: "".to_owned(),
                level: bevy_log::Level::DEBUG,
                ..Default::default()
            },
            AccessibilityPlugin,
            InputPlugin,
            WindowPlugin {
                close_when_requested: true,
                exit_condition: ExitCondition::OnPrimaryClosed,
                primary_window: Some(Window {
                    resolution: WindowResolution::new(
                        INITIAL_WINDOW_SIZE.x as f32,
                        INITIAL_WINDOW_SIZE.y as f32,
                    ),
                    present_mode: bevy_window::PresentMode::AutoNoVsync,
                    title: "RayTracer".to_owned(),
                    resizable: true,

                    ..Default::default()
                }),
            },
            AssetPlugin {
                mode: bevy_asset::AssetMode::Processed,
                ..Default::default()
            },
            WinitPlugin::<bevy_winit::WakeUp>::default(),
            TimePlugin,
            CameraPlugin,
            TaskPoolPlugin::default(),
            MeshAssets,
        ))
        .add_systems(PreUpdate, on_resize)
        // .add_systems(
        //     Update,
        //     (
        //         load_assets,
        //         add_instance,
        //         transform_child_changed,
        //         transform_parent_changed,
        //     ),
        // )
        .add_systems(PostUpdate, commands);
}
