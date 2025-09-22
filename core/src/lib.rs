#![feature(f16)]
#![feature(random)]

use std::{collections::HashMap, ops::Deref, path::PathBuf, random::random};

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
use fastnbt::{from_bytes, DeOpts, Value};
use glam::{IVec3, Mat4, Vec2, Vec3, Vec3Swizzles, Vec4};
use gpu_allocator::MemoryLocation;
use lava::{
    state::Ctx,
    vkobjects::{buffer::{Buffer, DynamicBuffer}, image::ImageSize},
};

use crate::{
    assets::{MeshAssets},
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

const MAX_DEPTH: u32 = 3;


fn as_i8<'a>(v: &'a Value) -> i8 {
    match v {
        Value::Byte(v) => *v,
        _ => {unreachable!()}
    }
}

fn as_compound<'a>(v: &'a Value) -> &'a HashMap<String, Value> {
    match v {
        Value::Compound(v) => v,
        _ => {unreachable!()}
    }
}

fn bits_per_block_from_palette(palette_len: usize) -> u32 {
    let mut bpb = (usize::BITS - (palette_len.saturating_sub(1)).leading_zeros()) as u32;
    if bpb < 4 { bpb = 4; }
    bpb
}

fn get_packed_index(block_states: &[i64], idx: usize, bpb: u32) -> u64 {
    let block_states: &[u64] = unsafe {
        std::slice::from_raw_parts(block_states.as_ptr() as *const u64, block_states.len())
    };

    let start_bit = (idx as u64) * (bpb as u64);
    let word = (start_bit / 64) as usize;
    let bit_off = (start_bit % 64) as u32;
    let mask = if bpb == 64 { u64::MAX } else { (1u64 << bpb) - 1 };

    let first = block_states.get(word).copied().unwrap_or(0);
    let mut value = first >> bit_off;

    let spill = (bit_off + bpb) > 64;
    if spill {
        let next = block_states.get(word + 1).copied().unwrap_or(0);
        let lo_bits = 64 - bit_off;
        value |= next << lo_bits;
    }
    value & mask
}

pub fn generate_random_tree() -> Vec<CompressedNode> {
    let file = std::fs::File::open("./core/minecraft/r.0.0.mca").unwrap();
    let mut region = fastanvil::Region::from_stream(file).unwrap();

    let chunk = region.read_chunk(0, 0).unwrap().unwrap();

    let root: Value = from_bytes(&chunk).unwrap();
    let level = as_compound(&root);

    let mut chunk_data = [[false; 16*16*16]; 20];

    if let Some(Value::List(sections)) = level.get("sections") {
        for sec in sections {
            let sec = as_compound(sec);

            let y = as_i8(sec.get("Y").unwrap());
            let block_states = as_compound(sec.get("block_states").unwrap());
            let palette = block_states.get("palette");
            let data = block_states.get("data");

            if let (Some(Value::List(palette)), Some(Value::LongArray(data))) = (palette, data) {
                let parsed_palette: Vec<(String, Option<&HashMap<String, Value>>)> =
                    palette.iter().map(|entry| {
                        let c = as_compound(entry);
                        let name = c.get("Name").and_then(Value::as_str).unwrap_or("").to_string();
                        let props = c.get("Properties").map(|e| as_compound(e));
                        (name, props)
                    }).collect();

                let bpb = bits_per_block_from_palette(parsed_palette.len());
                
                for i in 0..(16*16*16) {
                    let pal_idx = get_packed_index(&data[..], i, bpb) as usize;
                    if let Some((name, _props)) = parsed_palette.get(pal_idx) {
                        chunk_data[(-y) as usize][i] = name != "minecraft:air";
                    }
                }
            }
        }
    }

    let mut nodes: Vec<CompressedNode> = Vec::with_capacity(1024);
    nodes.push(CompressedNode {data: [0;3]});

    fn build(nodes: &mut Vec<CompressedNode>, depth: u32) -> CompressedNode {
        if depth >= MAX_DEPTH {
            let mut mask = random::<u64>();
            if mask == 0 {
                mask += 1;
            }
            return CompressedNode::new_leaf(mask);
        }

        let mut child_mask = random::<u64>();
        if child_mask == 0 {
            child_mask += 1;
        }
        let num_children = child_mask.count_ones() as usize;

        let child_ptr = nodes.len();
        nodes.extend(vec![CompressedNode::default(); num_children]);

        for i in 0..num_children {
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
            1<<20,
            None
        )
        .unwrap(),
        leaf_data: vec![],
        nodes,
    };

    let staging_buffer = Buffer::new(
        BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST,
        MemoryLocation::CpuToGpu,
        1<<20,
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
