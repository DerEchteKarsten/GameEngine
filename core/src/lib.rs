#![feature(f16)]
#![feature(random)]

use std::{
    collections::{HashMap, HashSet}, fs, io::{BufReader, BufWriter, Read, Seek, Write}, ops::Deref, path::PathBuf, random::random
};

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
use fastnbt::{DeOpts, Value, from_bytes};
use glam::{IVec3, Mat4, Vec2, Vec3, Vec3Swizzles, Vec4};
use gpu_allocator::MemoryLocation;
use image::{DynamicImage, ImageBuffer, Rgb};
use lava::{
    state::Ctx,
    vkobjects::{
        buffer::{Buffer, DynamicBuffer},
        image::ImageSize,
    },
};
use noise::{MultiFractal, NoiseFn, Perlin};
use smallvec::SmallVec;

use crate::{
    assets::{MeshAssets, CONFIG},
    components::camera::{Camera, CameraPlugin},
    world::{
        add_instance, init_world, load_assets, transform_child_changed, transform_parent_changed, RenderWorld, StagingBuffer, STAGING_BUFFER_SIZE
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

#[derive(Clone, Copy, Debug, Default, bincode::Encode, bincode::Decode)]
#[repr(C)]
pub struct CompressedNode {
    pub data: [u32; 3],
}

impl CompressedNode {
    fn new_leaf(child_ptr: u32, mask: u64, full: bool) -> Self {
        // bit0 = 1 (IsLeaf), ChildPtr unused (0)
        CompressedNode {
            data: [(child_ptr << 2) | (full as u32) << 1 | 1u32, mask as u32, (mask >> 32) as u32],
        }
    }
    fn new_internal(child_ptr: u32, child_mask: u64, full: bool) -> Self {
        CompressedNode {
            data: [child_ptr << 2 | (full as u32) << 1, child_mask as u32, (child_mask >> 32) as u32],
        }
    }

    pub fn is_leaf(&self) -> bool {
        (self.data[0] & 1) != 0
    }
    pub fn is_full(&self) -> bool {
        (self.data[0] & 0b10) != 0
    }
    pub fn child_ptr(&self) -> u32 {
        self.data[0] >> 2
    }
    pub fn pop_mask(&self) -> u64 {
        (self.data[1] as u64) | ((self.data[2] as u64) << 32)
    }
}

const MAX_DEPTH: u32 = 4;

fn index_to_3d(mut idx: i32) -> IVec3 {
    assert!(idx < 4 * 4 * 4, "Index out of bounds");
    let x = idx % 4;
    idx /= 4;
    let y = idx % 4;
    idx /= 4;
    let z = idx;
    IVec3::new(x, y, z)
}


fn side_length(value: u32) -> u32{
    1<<(2*(value))
}

pub fn generate_random_tree() -> Vec<CompressedNode> {
    let hight_map = image::open("./core/minecraft/hightmap_small.png").unwrap().to_rgb32f();
    let mips = vec![hight_map];
    let mut level = 0;
    loop {
        let (image_width, image_height) = mips[level].dimensions();
        let mut image = image::ImageBuffer::new(image_width/2, image_height/2);
        
        for x in 0..image_width {
            for y in 0..image_height {

            }
        } 
    }

    log::info!("Generated Hightes");

    let mut nodes = Vec::new();
    nodes.push(CompressedNode::default());
    fn build(
        nodes: &mut Vec<CompressedNode>,
        depth: u32,
        pos: IVec3,
        hight_map: &Vec<ImageBuffer<Rgb<f32>, Vec<f32>>>,
    ) -> CompressedNode {
        if depth >= MAX_DEPTH {
            let mut mask = 0;
            for i in 0..64 {
                let child_pos = pos + index_to_3d(i);
                let (image_width, image_height) = hight_map[0].dimensions();
                let aspect_x = (1<<(2*MAX_DEPTH)) as f32 / image_width as f32;
                let aspect_y = (1<<(2*MAX_DEPTH)) as f32 / image_height as f32;

                let filled = (child_pos.z as f32) > (1.0-hight_map[0][(child_pos.x as f32*aspect_x, child_pos.y as f32*aspect_y)].0[0]) * 1000.0;
                mask |= (filled as u64) << i;
            }
            let child_ptr = nodes.len() as u32;
            return CompressedNode::new_leaf(child_ptr, mask, mask == u64::MAX);
        }

        let mut child_mask = 0u64;
        let mut full = true;
        let mut children = SmallVec::<[CompressedNode; 64]>::new();
        for i in 0..64 {
            let child_pos = pos + index_to_3d(i) * (1<<(2*(MAX_DEPTH - depth)));
            let child = build(nodes, depth + 1, child_pos, hight_map);
            full &= child.is_full();
            
            if child.pop_mask() != 0 {
                child_mask |= 1u64 << i;
                children.push(child);
            }
        }
        let child_ptr = nodes.len();
        if full {
            CompressedNode::new_leaf(child_ptr as u32, child_mask, true)
        }else {
            nodes.extend(children);
            CompressedNode::new_internal(child_ptr as u32, child_mask, false)
        }
    }

    nodes[0] = build(&mut nodes, 0, IVec3::splat(0), &hight_map);
    nodes
}

fn init_voxel_world(mut cmd: Commands) {
    let file = fs::File::open("./core/minecraft/world.wrld");

    let nodes = if let Ok(file) = file {
        let buf_reader = BufReader::new(file);
        bincode::decode_from_reader(buf_reader, CONFIG).unwrap()
    } else {
        let nodes = generate_random_tree();
        let encoded = bincode::encode_to_vec(&nodes, CONFIG).unwrap();
        fs::File::create_new("./core/minecraft/world.wrld").unwrap().write_all(&encoded).unwrap();
        nodes
    };

    log::info!("Generated {} Nodes", nodes.len());
    let mut voxel_world = VoxelWorld {
        buffer: DynamicBuffer::new(
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
            MemoryLocation::GpuOnly,
            1 << 31,
            None,
        )
        .unwrap(),
        leaf_data: vec![],
        nodes,
    };

    let staging_buffer = Buffer::new(
        BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::CpuToGpu,
        1 << 28,
    )
    .unwrap();

    voxel_world.buffer.push(&staging_buffer, &voxel_world.nodes);
    // voxel_world.buffer.push(&staging_buffer, &voxel_world.leaf_data);

    cmd.insert_resource(VoxelWorld { ..voxel_world });
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

fn commands(world: Res<VoxelWorld>, query: Query<&Camera>) {
    let camera = query.single().unwrap();
    let constants = Constants {
        proj_inverse: camera.projection_matrix().inverse(),
        camera_position: camera.position.xyzx(),
        view_inverse: camera.view_matrix().inverse(),
        window_size: Vec2::new(
            Ctx::window_width().unwrap() as f32,
            Ctx::window_height().unwrap() as f32,
        ),
        tree_scale: 8,
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
    })
    .unwrap();
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
