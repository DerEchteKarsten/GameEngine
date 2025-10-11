
// #[derive(Resource)]
// struct VoxelWorld {
//     buffer: DynamicBuffer,
//     nodes: Vec<CompressedNode>,
//     leaf_data: Vec<u8>,
// }

// #[derive(Clone, Copy, Debug, Default, bincode::Encode, bincode::Decode)]
// #[repr(C)]
// pub struct CompressedNode {
//     pub data: [u32; 3],
// }

// impl CompressedNode {
//     fn new_leaf(child_ptr: u32, mask: u64, full: bool) -> Self {
//         // bit0 = 1 (IsLeaf), ChildPtr unused (0)
//         CompressedNode {
//             data: [(child_ptr << 2) | (full as u32) << 1 | 1u32, mask as u32, (mask >> 32) as u32],
//         }
//     }
//     fn new_internal(child_ptr: u32, child_mask: u64, full: bool) -> Self {
//         CompressedNode {
//             data: [child_ptr << 2 | (full as u32) << 1, child_mask as u32, (child_mask >> 32) as u32],
//         }
//     }

//     pub fn is_leaf(&self) -> bool {
//         (self.data[0] & 1) != 0
//     }
//     pub fn is_full(&self) -> bool {
//         (self.data[0] & 0b10) != 0
//     }
//     pub fn child_ptr(&self) -> u32 {
//         self.data[0] >> 2
//     }
//     pub fn pop_mask(&self) -> u64 {
//         (self.data[1] as u64) | ((self.data[2] as u64) << 32)
//     }
// }

// const MAX_DEPTH: u32 = 4;

// fn index_to_3d(mut idx: i32) -> IVec3 {
//     assert!(idx < 4 * 4 * 4, "Index out of bounds");
//     let x = idx % 4;
//     idx /= 4;
//     let y = idx % 4;
//     idx /= 4;
//     let z = idx;
//     IVec3::new(x, y, z)
// }


// fn side_length(value: u32) -> u32{
//     1<<(2*(value))
// }

// pub fn generate_random_tree() -> Vec<CompressedNode> {
//     let hight_map = image::open("./core/minecraft/hightmap_small.png").unwrap().to_rgb32f();
//     let mips = vec![hight_map];
//     let mut level = 0;
//     loop {
//         let (image_width, image_height) = mips[level].dimensions();
//         let mut image = image::ImageBuffer::new(image_width/2, image_height/2);
        
//         for x in 0..image_width {
//             for y in 0..image_height {

//             }
//         } 
//     }

//     log::info!("Generated Hightes");

//     let mut nodes = Vec::new();
//     nodes.push(CompressedNode::default());
//     fn build(
//         nodes: &mut Vec<CompressedNode>,
//         depth: u32,
//         pos: IVec3,
//         hight_map: &Vec<ImageBuffer<Rgb<f32>, Vec<f32>>>,
//     ) -> CompressedNode {
//         if depth >= MAX_DEPTH {
//             let mut mask = 0;
//             for i in 0..64 {
//                 let child_pos = pos + index_to_3d(i);
//                 let (image_width, image_height) = hight_map[0].dimensions();
//                 let aspect_x = (1<<(2*MAX_DEPTH)) as f32 / image_width as f32;
//                 let aspect_y = (1<<(2*MAX_DEPTH)) as f32 / image_height as f32;

//                 let filled = (child_pos.z as f32) > (1.0-hight_map[0][(child_pos.x as f32*aspect_x, child_pos.y as f32*aspect_y)].0[0]) * 1000.0;
//                 mask |= (filled as u64) << i;
//             }
//             let child_ptr = nodes.len() as u32;
//             return CompressedNode::new_leaf(child_ptr, mask, mask == u64::MAX);
//         }

//         let mut child_mask = 0u64;
//         let mut full = true;
//         let mut children = SmallVec::<[CompressedNode; 64]>::new();
//         for i in 0..64 {
//             let child_pos = pos + index_to_3d(i) * (1<<(2*(MAX_DEPTH - depth)));
//             let child = build(nodes, depth + 1, child_pos, hight_map);
//             full &= child.is_full();
            
//             if child.pop_mask() != 0 {
//                 child_mask |= 1u64 << i;
//                 children.push(child);
//             }
//         }
//         let child_ptr = nodes.len();
//         if full {
//             CompressedNode::new_leaf(child_ptr as u32, child_mask, true)
//         }else {
//             nodes.extend(children);
//             CompressedNode::new_internal(child_ptr as u32, child_mask, false)
//         }
//     }

//     nodes[0] = build(&mut nodes, 0, IVec3::splat(0), &hight_map);
//     nodes
// }

// fn init_voxel_world(mut cmd: Commands) {
//     let file = fs::File::open("./core/minecraft/world.wrld");

//     let nodes = if let Ok(file) = file {
//         let buf_reader = BufReader::new(file);
//         bincode::decode_from_reader(buf_reader, CONFIG).unwrap()
//     } else {
//         let nodes = generate_random_tree();
//         let encoded = bincode::encode_to_vec(&nodes, CONFIG).unwrap();
//         fs::File::create_new("./core/minecraft/world.wrld").unwrap().write_all(&encoded).unwrap();
//         nodes
//     };

//     log::info!("Generated {} Nodes", nodes.len());
//     let mut voxel_world = VoxelWorld {
//         buffer: DynamicBuffer::new(
//             vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST,
//             MemoryLocation::GpuOnly,
//             1 << 31,
//             None,
//         )
//         .unwrap(),
//         leaf_data: vec![],
//         nodes,
//     };

//     let staging_buffer = Buffer::new(
//         BufferUsageFlags::TRANSFER_SRC | BufferUsageFlags::TRANSFER_DST,
//         MemoryLocation::CpuToGpu,
//         1 << 28,
//     )
//     .unwrap();

//     voxel_world.buffer.push(&staging_buffer, &voxel_world.nodes);
//     // voxel_world.buffer.push(&staging_buffer, &voxel_world.leaf_data);

//     cmd.insert_resource(VoxelWorld { ..voxel_world });
//     cmd.insert_resource(StagingBuffer(staging_buffer));
// }
