#![feature(f16)]
#![feature(random)]

use std::{ops::Deref, path::PathBuf, random::random};

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
use glam::{IVec3, Mat4, Vec2, Vec3, Vec3Swizzles, Vec4};
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

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct CompressedNode {
    pub data: [u32; 3],
}

impl CompressedNode {
    fn new_leaf(mask: u64) -> Self {
        // bit0 = 1 (IsLeaf), ChildPtr unused (0)
        CompressedNode { data: [1u32, mask as u32, (mask >> 32) as u32] }
    }
    fn new_internal(child_ptr: u32, child_mask: u64) -> Self {
        CompressedNode { data: [child_ptr << 2, child_mask as u32, (child_mask >> 32) as u32] }
    }

    pub fn is_leaf(&self) -> bool { (self.data[0] & 1) != 0 }
    pub fn child_ptr(&self) -> u32 { self.data[0] >> 2 }
    pub fn pop_mask(&self) -> u64 {
        (self.data[1] as u64) | ((self.data[2] as u64) << 32)
    }
}

const MAX_DEPTH: u32 = 4;

pub fn generate_random_tree() -> Vec<CompressedNode> {
    let file = fastanvil::RegionFileLoader::new(PathBuf::from("./minecraft"));

    fastanvil::render_region(x, z, loader, renderer).unwrap();
    
    let mut nodes: Vec<CompressedNode> = Vec::with_capacity(1024);
    nodes.push(CompressedNode {data: [0;3]});

    fn build(nodes: &mut Vec<CompressedNode>, depth: u32) -> CompressedNode {
        if depth >= MAX_DEPTH {
            let mask = random::<u64>().wrapping_add(1);
            return CompressedNode::new_leaf(mask);
        }

        let child_mask = random::<u64>().wrapping_add(1);
        let num_children = child_mask.count_ones() as usize;

        let child_ptr = nodes.len();
        nodes.extend(vec![CompressedNode::default(); num_children]);

        for i in 0..child_mask.count_ones() {
            nodes[i as usize + child_ptr] = build(nodes, depth + 1);
        }
        
        CompressedNode::new_internal(child_ptr as u32, child_mask)
    }

    let _root = build(&mut nodes, 0);
    nodes[0] = _root;
    nodes
}


fn init_voxel_world(mut cmd: Commands) {

    let nodes = generate_random_tree();
    log::info!("Generated {} Nodes", nodes.len());
    let mut voxel_world = VoxelWorld {
        buffer: DynamicBuffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            1<<30,
            None
        )
        .unwrap(),
        leaf_data: vec![],
        nodes,
    };

    let staging_buffer = Buffer::new(
        BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::CpuToGpu,
        1<<26,
    )
    .unwrap();

    voxel_world.buffer.push(&staging_buffer, &voxel_world.nodes);
    // voxel_world.buffer.push(&staging_buffer, &voxel_world.leaf_data);

    
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
    tree_scale: u32,
    pad: u32,
    camera_position: Vec4,
}

fn commands(
    world: Res<VoxelWorld>,
    query: Query<&Camera>,
) {
    let camera = query.single().unwrap();
    let constants = Constants {
        proj_inverse: camera.projection_matrix().inverse(),
        camera_position: camera.position.xyzx(),
        view_inverse: camera.view_matrix().inverse(),
        window_size: Vec2::new(
            Ctx::window_width().unwrap() as f32,
            Ctx::window_height().unwrap() as f32,
        ),
        tree_scale: 1 << MAX_DEPTH,
        pad: 0,
    };

    // let depth = rg.0.image(ImageSize::FullScreen, Format::D32_SFLOAT, "depth");
    // let color = rg.0.image(ImageSize::FullScreen, Format::R32G32B32A32_SFLOAT, "color");

    Ctx::next_frame(&mut |cmd, swapchain_image| {
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
            .dispatch_fractional_fullscreen(8, 4);

        cmd.present(swapchain_image);
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
            // AssetPlugin {
            //     mode: bevy_asset::AssetMode::Processed,
            //     ..Default::default()
            // },
            WinitPlugin::<bevy_winit::WakeUp>::default(),
            TimePlugin,
            CameraPlugin,
            TaskPoolPlugin::default(),
            // MeshAssets,
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
