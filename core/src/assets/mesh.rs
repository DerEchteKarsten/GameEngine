use bevy::{
    math::bounding::{Aabb3d, BoundingVolume},
    prelude::*,
    tasks::{AsyncComputeTaskPool, ParallelSlice},
};
use bytemuck::{Pod, Zeroable};
use glam::{Vec2, Vec3, Vec3A, Vec4, Vec4Swizzles};
use itertools::Itertools;
use meshopt::{
    SimplifyOptions, VertexDataAdapter, VertexStream, build_meshlets, generate_vertex_remap_multi,
    simplify_with_attributes_and_locks,
};
use metis::{Graph, option::Opt};
use smallvec::SmallVec;
use std::sync::Arc;
use std::{collections::HashMap, ops::Range};

use crate::bindings::{AabbError, AabbPtr, BvhNode, CullData, Meshlet, Vertex};
const SIMPLIFICATION_FAILURE_PERCENTAGE: f32 = 0.60;
const TARGET_MESHLETS_PER_GROUP: usize = 8;

impl Default for AabbPtr {
    fn default() -> Self {
        AabbPtr {
            center_and_offset_high: Default::default(),
            half_extent_and_offset_low: Default::default(),
        }
    }
}

impl Default for BvhNode {
    fn default() -> Self {
        BvhNode {
            aabb_and_offsets: Default::default(),
            errors: Default::default(),
            lod_bounds: Default::default(),
            child_counts: Default::default(),
            pad: Default::default(),
        }
    }
}

impl BvhNode {
    pub fn child_counts(&self, i: usize) -> u8 {
        ((self.child_counts >> (i * 8)) & 0xFF) as u8
    }
    pub fn set_child_count(&mut self, i: usize, value: u8) {
        let shift = i * 8;
        let mask = !(0xFFu64 << shift);
        self.child_counts = (self.child_counts & mask) | ((value as u64) << shift);
    }
}

impl AabbPtr {
    pub fn offset(&self) -> u64 {
        (self.center_and_offset_high.w.to_bits() as u64) << 32
            | self.half_extent_and_offset_low.w.to_bits() as u64
    }
    pub fn set_offset(&mut self, value: u64) {
        self.center_and_offset_high.w = f32::from_bits((value >> 32) as u32);
        self.half_extent_and_offset_low.w = f32::from_bits(value as u32);
    }
    pub fn new(aabb: Aabb3d, offset: u32) -> Self {
        Self {
            center_and_offset_high: aabb.center().extend(f32::from_bits(0)),
            half_extent_and_offset_low: aabb.half_size().extend(f32::from_bits(offset)),
        }
    }
}

#[derive(Clone)]
pub struct MeshletMesh {
    pub vertices: Arc<[Vertex]>,
    pub indices: Arc<[u8]>,
    pub meshlets: Arc<[Meshlet]>,
    pub bvh: Arc<[BvhNode]>,
    pub cull_data: Arc<[CullData]>,

    pub aabb: AabbError,
    pub bvh_depth: u32,
    pub bvh_root_node_index: u32,
}

impl MeshletMesh {
    pub fn new(
        indices: &[u32],
        vertices: &[Vec3],
        vertex_normals: &[f32],
        vertex_uvs: &[f32],
    ) -> Self {
        let vertex_stride = size_of::<Vec3>();
        let (position_only_vertex_count, position_only_vertex_remap) = generate_vertex_remap_multi(
            vertices.len(),
            &[VertexStream::new_with_stride::<Vec3, _>(
                vertices.as_ptr(),
                vertex_stride,
            )],
            Some(&indices),
        );
        let vertex_adapter =
            VertexDataAdapter::new(bytemuck::cast_slice(vertices), vertex_stride, 0).unwrap();

        // Split the mesh into an initial list of meshlets (LOD 0)
        let (mut meshlets, mut cull_data) = compute_meshlets(
            &indices,
            &vertex_adapter,
            &position_only_vertex_remap,
            position_only_vertex_count,
            None,
        );

        let mut vertex_locks = vec![false; vertices.len()];

        // Build further LODs
        let mut bvh = BvhBuilder::default();
        let mut all_groups = Vec::new();
        let mut simplification_queue: Vec<_> = (0..meshlets.len() as u32).collect();
        let mut stuck = Vec::new();
        while !simplification_queue.is_empty() {
            let s = debug_span!("simplify lod", meshlets = simplification_queue.len());
            let _e = s.enter();

            // For each meshlet build a list of connected meshlets (meshlets that share a vertex)
            let connected_meshlets_per_meshlet = find_connected_meshlets(
                &simplification_queue,
                &meshlets,
                &position_only_vertex_remap,
                position_only_vertex_count,
            );

            // Group meshlets into roughly groups of size TARGET_MESHLETS_PER_GROUP,
            // grouping meshlets with a high number of shared vertices
            let groups = group_meshlets(
                &simplification_queue,
                &cull_data,
                &connected_meshlets_per_meshlet,
            );
            simplification_queue.clear();

            // Lock borders between groups to prevent cracks when simplifying
            lock_group_borders(
                &mut vertex_locks,
                &groups,
                &meshlets,
                &position_only_vertex_remap,
                position_only_vertex_count,
            );

            let simplified = groups.par_chunk_map(AsyncComputeTaskPool::get(), 1, |_, groups| {
                let mut group = groups[0].clone();

                // If the group only has a single meshlet we can't simplify it
                if group.meshlets.len() == 1 {
                    return Err(group);
                }

                let s = debug_span!("simplify group", meshlets = group.meshlets.len());
                let _e = s.enter();

                // Simplify the group to ~50% triangle count
                let Some((simplified_group_indices, mut group_error)) = simplify_meshlet_group(
                    &group,
                    &meshlets,
                    &vertex_adapter,
                    vertex_normals,
                    size_of::<Vertex>(),
                    &vertex_locks,
                ) else {
                    // Couldn't simplify the group enough
                    return Err(group);
                };

                // Force the group error to be atleast as large as all of its constituent meshlet's
                // individual errors.
                for &id in group.meshlets.iter() {
                    group_error = group_error.max(cull_data[id as usize].error);
                }
                group.parent_error = group_error;

                // Build new meshlets using the simplified group
                let new_meshlets = compute_meshlets(
                    &simplified_group_indices,
                    &vertex_adapter,
                    &position_only_vertex_remap,
                    position_only_vertex_count,
                    Some((group.lod_bounds, group.parent_error)),
                );

                Ok((group, new_meshlets))
            });

            let first_group = all_groups.len() as u32;
            let mut passed_tris = 0;
            let mut stuck_tris = 0;
            for group in simplified {
                match group {
                    Ok((group, (new_meshlets, new_cull_data))) => {
                        let start = meshlets.len();
                        merge_meshlets(&mut meshlets, new_meshlets);
                        cull_data.extend(new_cull_data);
                        let end = meshlets.len();
                        let new_meshlet_ids = start as u32..end as u32;

                        passed_tris += triangles_in_meshlets(&meshlets, new_meshlet_ids.clone());
                        simplification_queue.extend(new_meshlet_ids);
                        all_groups.push(group);
                    }
                    Err(group) => {
                        stuck_tris +=
                            triangles_in_meshlets(&meshlets, group.meshlets.iter().copied());
                        stuck.push(group);
                    }
                }
            }

            // If we have enough triangles that passed, we can retry simplifying the stuck
            // meshlets.
            if passed_tris > stuck_tris / 3 {
                simplification_queue.extend(stuck.drain(..).flat_map(|group| group.meshlets));
            }

            bvh.add_lod(first_group, &all_groups);
        }

        // If there's any stuck meshlets left, add another LOD level with only them
        if !stuck.is_empty() {
            let first_group = all_groups.len() as u32;
            all_groups.extend(stuck);
            bvh.add_lod(first_group, &all_groups);
        }

        let (bvh, aabb, depth) = bvh.build(&mut meshlets, all_groups, &mut cull_data);

        let mut mmeshlets = Vec::with_capacity(meshlets.len());
        for meshlet in meshlets.meshlets.iter() {
            mmeshlets.push(Meshlet {
                triangle_count: meshlet.triangle_count,
                triangle_index: meshlet.triangle_offset as u64,
                vertex_count: meshlet.vertex_count,
                vertex_index: meshlet.vertex_offset as u64,
            });
        }

        let verticies = vertices
            .iter()
            .enumerate()
            .map(|(i, v)| Vertex {
                position_and_uv1: (*v).extend(vertex_uvs[i * 2 + 0]),
                normal_and_uv2: Vec4::new(
                    vertex_normals[i * 3 + 0],
                    vertex_normals[i * 3 + 1],
                    vertex_normals[i * 3 + 2],
                    vertex_uvs[i * 2 + 1],
                ),
            })
            .collect::<Vec<Vertex>>();

        let duped_verticies = meshlets
            .vertices
            .iter()
            .map(|v| verticies[(*v) as usize])
            .collect::<Vec<_>>();

        Self {
            vertices: duped_verticies.into(),
            indices: meshlets.triangles.into(),
            bvh: bvh.into(),
            meshlets: mmeshlets.into(),
            cull_data: cull_data
                .into_iter()
                .map(|cull_data| CullData {
                    aabb: AabbError {
                        center_and_error: cull_data.aabb.center().extend(cull_data.error),
                        half_extent: cull_data.aabb.half_size().extend(0.0),
                    },
                    lod_group_sphere: cull_data.lod_group_sphere,
                })
                .collect(),
            aabb,
            bvh_depth: depth,
            bvh_root_node_index: !0u32,
        }
    }
}

fn triangles_in_meshlets(meshlets: &meshopt::Meshlets, ids: impl IntoIterator<Item = u32>) -> u32 {
    ids.into_iter()
        .map(|id| meshlets.get(id as _).triangles.len() as u32 / 3)
        .sum()
}

fn compute_meshlets(
    indices: &[u32],
    vertices: &VertexDataAdapter,
    position_only_vertex_remap: &[u32],
    position_only_vertex_count: usize,
    prev_lod_data: Option<(Vec4, f32)>,
) -> (meshopt::Meshlets, Vec<TempMeshletCullData>) {
    // For each vertex, build a list of all triangles that use it
    let mut vertices_to_triangles = vec![Vec::new(); position_only_vertex_count];
    for (i, index) in indices.iter().enumerate() {
        let vertex_id = position_only_vertex_remap[*index as usize];
        let vertex_to_triangles = &mut vertices_to_triangles[vertex_id as usize];
        vertex_to_triangles.push(i / 3);
    }

    // For each triangle pair, count how many vertices they share
    let mut triangle_pair_to_shared_vertex_count = <HashMap<_, _>>::default();
    for vertex_triangle_ids in vertices_to_triangles {
        for (triangle_id1, triangle_id2) in vertex_triangle_ids.into_iter().tuple_combinations() {
            let count = triangle_pair_to_shared_vertex_count
                .entry((
                    triangle_id1.min(triangle_id2),
                    triangle_id1.max(triangle_id2),
                ))
                .or_insert(0);
            *count += 1;
        }
    }

    // For each triangle, gather all other triangles that share at least one vertex along with their shared vertex count
    let triangle_count = indices.len() / 3;
    let mut connected_triangles_per_triangle = vec![Vec::new(); triangle_count];
    for ((triangle_id1, triangle_id2), shared_vertex_count) in triangle_pair_to_shared_vertex_count
    {
        // We record both id1->id2 and id2->id1 as adjacency is symmetrical
        connected_triangles_per_triangle[triangle_id1].push((triangle_id2, shared_vertex_count));
        connected_triangles_per_triangle[triangle_id2].push((triangle_id1, shared_vertex_count));
    }

    // The order of triangles depends on hash traversal order; to produce deterministic results, sort them
    // TODO: Wouldn't it be faster to use a `BTreeMap` above instead of `HashMap` + sorting?
    for list in connected_triangles_per_triangle.iter_mut() {
        list.sort_unstable();
    }

    let mut xadj = Vec::with_capacity(triangle_count + 1);
    let mut adjncy = Vec::new();
    let mut adjwgt = Vec::new();
    for connected_triangles in connected_triangles_per_triangle {
        xadj.push(adjncy.len() as i32);
        for (connected_triangle_id, shared_vertex_count) in connected_triangles {
            adjncy.push(connected_triangle_id as i32);
            adjwgt.push(shared_vertex_count);
            // TODO: Additional weight based on triangle center spatial proximity?
        }
    }
    xadj.push(adjncy.len() as i32);

    let mut options = [-1; metis::NOPTIONS];
    options[metis::option::Seed::INDEX] = 17;
    options[metis::option::UFactor::INDEX] = 1; // Important that there's very little imbalance between partitions

    let mut meshlet_per_triangle = vec![0; triangle_count];
    let partition_count = triangle_count.div_ceil(126); // Need to undershoot to prevent METIS from going over 128 triangles per meshlet
    Graph::new(1, partition_count as i32, &xadj, &adjncy)
        .unwrap()
        .set_options(&options)
        .set_adjwgt(&adjwgt)
        .part_recursive(&mut meshlet_per_triangle)
        .unwrap();

    let mut indices_per_meshlet = vec![Vec::new(); partition_count];
    for (triangle_id, meshlet) in meshlet_per_triangle.into_iter().enumerate() {
        let meshlet_indices = &mut indices_per_meshlet[meshlet as usize];
        let base_index = triangle_id * 3;
        meshlet_indices.extend_from_slice(&indices[base_index..(base_index + 3)]);
    }

    // Use meshopt to build meshlets from the sets of triangles
    let mut meshlets = meshopt::Meshlets {
        meshlets: Vec::new(),
        vertices: Vec::new(),
        triangles: Vec::new(),
    };
    let mut cull_data = Vec::new();
    let get_vertex = |&v: &u32| {
        *bytemuck::from_bytes::<Vec3>(
            &vertices.reader.get_ref()
                [vertices.position_offset + v as usize * vertices.vertex_stride..][..12],
        )
    };
    for meshlet_indices in &indices_per_meshlet {
        let meshlet = build_meshlets(meshlet_indices, vertices, 255, 128, 0.0);
        for meshlet in meshlet.iter() {
            let (lod_group_sphere, error) = prev_lod_data.unwrap_or_else(|| {
                let bounds = meshopt::compute_meshlet_bounds(meshlet, vertices);
                (Vec3::from_array(bounds.center).extend(bounds.radius), 0.0)
            });

            cull_data.push(TempMeshletCullData {
                aabb: Aabb3d::from_point_cloud(
                    Isometry3d::IDENTITY,
                    meshlet.vertices.iter().map(get_vertex),
                ),
                lod_group_sphere,
                error,
            });
        }
        merge_meshlets(&mut meshlets, meshlet);
    }
    (meshlets, cull_data)
}

fn find_connected_meshlets(
    simplification_queue: &[u32],
    meshlets: &meshopt::Meshlets,
    position_only_vertex_remap: &[u32],
    position_only_vertex_count: usize,
) -> Vec<Vec<(usize, usize)>> {
    // For each vertex, build a list of all meshlets that use it
    let mut vertices_to_meshlets = vec![Vec::new(); position_only_vertex_count];
    for (id_index, &meshlet_id) in simplification_queue.iter().enumerate() {
        let meshlet = meshlets.get(meshlet_id as _);
        for index in meshlet.triangles {
            let vertex_id = position_only_vertex_remap[meshlet.vertices[*index as usize] as usize];
            let vertex_to_meshlets = &mut vertices_to_meshlets[vertex_id as usize];
            // Meshlets are added in order, so we can just check the last element to deduplicate,
            // in the case of two triangles sharing the same vertex within a single meshlet
            if vertex_to_meshlets.last() != Some(&id_index) {
                vertex_to_meshlets.push(id_index);
            }
        }
    }

    // For each meshlet pair, count how many vertices they share
    let mut meshlet_pair_to_shared_vertex_count = <HashMap<_, _>>::default();
    for vertex_meshlet_ids in vertices_to_meshlets {
        for (meshlet_id1, meshlet_id2) in vertex_meshlet_ids.into_iter().tuple_combinations() {
            let count = meshlet_pair_to_shared_vertex_count
                .entry((meshlet_id1.min(meshlet_id2), meshlet_id1.max(meshlet_id2)))
                .or_insert(0);
            *count += 1;
        }
    }

    // For each meshlet, gather all other meshlets that share at least one vertex along with their shared vertex count
    let mut connected_meshlets_per_meshlet = vec![Vec::new(); simplification_queue.len()];
    for ((meshlet_id1, meshlet_id2), shared_vertex_count) in meshlet_pair_to_shared_vertex_count {
        // We record both id1->id2 and id2->id1 as adjacency is symmetrical
        connected_meshlets_per_meshlet[meshlet_id1].push((meshlet_id2, shared_vertex_count));
        connected_meshlets_per_meshlet[meshlet_id2].push((meshlet_id1, shared_vertex_count));
    }

    // The order of meshlets depends on hash traversal order; to produce deterministic results, sort them
    // TODO: Wouldn't it be faster to use a `BTreeMap` above instead of `HashMap` + sorting?
    for list in connected_meshlets_per_meshlet.iter_mut() {
        list.sort_unstable();
    }

    connected_meshlets_per_meshlet
}

// METIS manual: https://github.com/KarypisLab/METIS/blob/e0f1b88b8efcb24ffa0ec55eabb78fbe61e58ae7/manual/manual.pdf
fn group_meshlets(
    simplification_queue: &[u32],
    meshlet_cull_data: &[TempMeshletCullData],
    connected_meshlets_per_meshlet: &[Vec<(usize, usize)>],
) -> Vec<TempMeshletGroup> {
    let mut xadj = Vec::with_capacity(simplification_queue.len() + 1);
    let mut adjncy = Vec::new();
    let mut adjwgt = Vec::new();
    for connected_meshlets in connected_meshlets_per_meshlet {
        xadj.push(adjncy.len() as i32);
        for (connected_meshlet_id, shared_vertex_count) in connected_meshlets {
            adjncy.push(*connected_meshlet_id as i32);
            adjwgt.push(*shared_vertex_count as i32);
            // TODO: Additional weight based on meshlet spatial proximity
        }
    }
    xadj.push(adjncy.len() as i32);

    let mut options = [-1; metis::NOPTIONS];
    options[metis::option::Seed::INDEX] = 17;
    options[metis::option::UFactor::INDEX] = 200;

    let mut group_per_meshlet = vec![0; simplification_queue.len()];
    let partition_count = simplification_queue
        .len()
        .div_ceil(TARGET_MESHLETS_PER_GROUP); // TODO: Nanite uses groups of 8-32, probably based on some kind of heuristic
    Graph::new(1, partition_count as i32, &xadj, &adjncy)
        .unwrap()
        .set_options(&options)
        .set_adjwgt(&adjwgt)
        .part_recursive(&mut group_per_meshlet)
        .unwrap();

    let mut groups = vec![TempMeshletGroup::default(); partition_count];
    for (i, meshlet_group) in group_per_meshlet.into_iter().enumerate() {
        let group = &mut groups[meshlet_group as usize];
        let meshlet_id = simplification_queue[i];

        group.meshlets.push(meshlet_id);
        let data = &meshlet_cull_data[meshlet_id as usize];
        group.aabb = group.aabb.merge(&data.aabb);
        group.lod_bounds = merge_spheres(group.lod_bounds, data.lod_group_sphere);
    }
    groups
}

fn lock_group_borders(
    vertex_locks: &mut [bool],
    groups: &[TempMeshletGroup],
    meshlets: &meshopt::Meshlets,
    position_only_vertex_remap: &[u32],
    position_only_vertex_count: usize,
) {
    let mut position_only_locks = vec![-1; position_only_vertex_count];

    // Iterate over position-only based vertices of all meshlets in all groups
    for (group_id, group) in groups.iter().enumerate() {
        for &meshlet_id in group.meshlets.iter() {
            let meshlet = meshlets.get(meshlet_id as usize);
            for index in meshlet.triangles {
                let vertex_id =
                    position_only_vertex_remap[meshlet.vertices[*index as usize] as usize] as usize;

                // If the vertex is not yet claimed by any group, or was already claimed by this group
                if position_only_locks[vertex_id] == -1
                    || position_only_locks[vertex_id] == group_id as i32
                {
                    position_only_locks[vertex_id] = group_id as i32; // Then claim the vertex for this group
                } else {
                    position_only_locks[vertex_id] = -2; // Else vertex was already claimed by another group or was already locked, lock it
                }
            }
        }
    }

    // Lock vertices used by more than 1 group
    for i in 0..vertex_locks.len() {
        let vertex_id = position_only_vertex_remap[i] as usize;
        vertex_locks[i] = position_only_locks[vertex_id] == -2;
    }
}

fn simplify_meshlet_group(
    group: &TempMeshletGroup,
    meshlets: &meshopt::Meshlets,
    vertices: &VertexDataAdapter,
    vertex_normals: &[f32],
    vertex_stride: usize,
    vertex_locks: &[bool],
) -> Option<(Vec<u32>, f32)> {
    // Build a new index buffer into the mesh vertex data by combining all meshlet data in the group
    let group_indices = group
        .meshlets
        .iter()
        .flat_map(|&meshlet_id| {
            let meshlet = meshlets.get(meshlet_id as _);
            meshlet
                .triangles
                .iter()
                .map(|&meshlet_index| meshlet.vertices[meshlet_index as usize])
        })
        .collect::<Vec<_>>();

    // Simplify the group to ~50% triangle count
    let mut error = 0.0;
    let simplified_group_indices = simplify_with_attributes_and_locks(
        &group_indices,
        vertices,
        vertex_normals,
        &[0.5; 3],
        vertex_stride,
        vertex_locks,
        group_indices.len() / 2,
        f32::MAX,
        SimplifyOptions::Sparse | SimplifyOptions::ErrorAbsolute,
        Some(&mut error),
    );

    // Check if we were able to simplify
    if simplified_group_indices.len() as f32 / group_indices.len() as f32
        > SIMPLIFICATION_FAILURE_PERCENTAGE
    {
        return None;
    }

    Some((simplified_group_indices, error))
}

fn merge_meshlets(meshlets: &mut meshopt::Meshlets, merge: meshopt::Meshlets) {
    let vertex_offset = meshlets.vertices.len() as u32;
    let triangle_offset = meshlets.triangles.len() as u32;
    meshlets.vertices.extend_from_slice(&merge.vertices);
    meshlets.triangles.extend_from_slice(&merge.triangles);
    meshlets
        .meshlets
        .extend(merge.meshlets.into_iter().map(|mut meshlet| {
            meshlet.vertex_offset += vertex_offset;
            meshlet.triangle_offset += triangle_offset;
            meshlet
        }));
}

fn merge_spheres(a: Vec4, b: Vec4) -> Vec4 {
    let sr = a.w.min(b.w);
    let br = a.w.max(b.w);
    let len = a.xyz().distance(b.xyz());
    if len + sr <= br || sr == 0.0 || len == 0.0 {
        if a.w > b.w { a } else { b }
    } else {
        let radius = (sr + br + len) / 2.0;
        let center = (a.xyz() + b.xyz() + (a.w - b.w) * (a.xyz() - b.xyz()) / len) / 2.0;
        center.extend(radius)
    }
}

#[derive(Copy, Clone)]
struct TempMeshletCullData {
    aabb: Aabb3d,
    lod_group_sphere: Vec4,
    error: f32,
}

#[derive(Clone)]
struct TempMeshletGroup {
    aabb: Aabb3d,
    lod_bounds: Vec4,
    parent_error: f32,
    meshlets: SmallVec<[u32; TARGET_MESHLETS_PER_GROUP]>,
}

impl Default for TempMeshletGroup {
    fn default() -> Self {
        Self {
            aabb: aabb_default(), // Default AABB to merge into
            lod_bounds: Vec4::ZERO,
            parent_error: f32::MAX,
            meshlets: SmallVec::new(),
        }
    }
}

// All the BVH build code was stolen from https://github.com/SparkyPotato/radiance/blob/4aa17a3a5be7a0466dc69713e249bbcee9f46057/crates/rad-renderer/src/assets/mesh/virtual_mesh.rs because it works and I'm lazy and don't want to reimplement it
struct TempBvhNode {
    group: u32,
    aabb: Aabb3d,
    children: SmallVec<[u32; 8]>,
}

#[derive(Default)]
struct BvhBuilder {
    nodes: Vec<TempBvhNode>,
    lods: Vec<Range<u32>>,
}

impl BvhBuilder {
    fn add_lod(&mut self, offset: u32, all_groups: &[TempMeshletGroup]) {
        let first = self.nodes.len() as u32;
        self.nodes.extend(
            all_groups
                .iter()
                .enumerate()
                .skip(offset as _)
                .map(|(i, group)| TempBvhNode {
                    group: i as u32,
                    aabb: group.aabb,
                    children: SmallVec::new(),
                }),
        );
        let end = self.nodes.len() as u32;
        if first != end {
            self.lods.push(first..end);
        }
    }

    fn surface_area(&self, nodes: &[u32]) -> f32 {
        nodes
            .iter()
            .map(|&x| self.nodes[x as usize].aabb)
            .reduce(|a, b| a.merge(&b))
            .expect("cannot find surface area of zero nodes")
            .visible_area()
    }

    fn sort_nodes_by_sah(&self, nodes: &mut [u32], splits: [usize; 8]) {
        // We use a BVH8, so just recursively binary split 3 times for near-optimal SAH
        for i in 0..3 {
            let parts = 1 << i; // 2^i
            let nodes_per_split = 8 >> i; // 8 / 2^i
            let half_count = nodes_per_split / 2;
            let mut offset = 0;
            for p in 0..parts {
                let first = p * nodes_per_split;
                let mut s0 = 0;
                let mut s1 = 0;
                for i in 0..half_count {
                    s0 += splits[first + i];
                    s1 += splits[first + half_count + i];
                }
                let c = s0 + s1;
                let nodes = &mut nodes[offset..(offset + c)];
                offset += c;

                let mut cost = f32::MAX;
                let mut axis = 0;
                let key = |x, ax| self.nodes[x as usize].aabb.center()[ax];
                for ax in 0..3 {
                    nodes.sort_unstable_by(|&x, &y| key(x, ax).partial_cmp(&key(y, ax)).unwrap());
                    let (left, right) = nodes.split_at(s0);
                    let c = self.surface_area(left) + self.surface_area(right);
                    if c < cost {
                        axis = ax;
                        cost = c;
                    }
                }
                if axis != 2 {
                    nodes.sort_unstable_by(|&x, &y| {
                        key(x, axis).partial_cmp(&key(y, axis)).unwrap()
                    });
                }
            }
        }
    }

    fn build_temp_inner(&mut self, nodes: &mut [u32], optimize: bool) -> u32 {
        let count = nodes.len();
        if count == 1 {
            nodes[0]
        } else if count <= 8 {
            let i = self.nodes.len();
            self.nodes.push(TempBvhNode {
                group: u32::MAX,
                aabb: aabb_default(),
                children: nodes.iter().copied().collect(),
            });
            i as _
        } else {
            // We need to split the nodes into 8 groups, with the smallest possible tree depth.
            // Additionally, no child should be more than one level deeper than the others.
            // At `l` levels, we can fit upto 8^l nodes.
            // The `max_child_size` is the largest power of 8 <= `count` (any larger and we'd have
            // unfilled nodes).
            // The `min_child_size` is thus 1 level (8 times) smaller.
            // After distributing `min_child_size` to all children, we have distributed
            // `min_child_size * 8` nodes (== `max_child_size`).
            // The remaining nodes are then distributed left to right.
            let max_child_size = 1 << ((count.ilog2() / 3) * 3);
            let min_child_size = max_child_size >> 3;
            let max_extra_per_node = max_child_size - min_child_size;
            let mut extra = count - max_child_size; // 8 * min_child_size
            let splits = core::array::from_fn(|_| {
                let size = extra.min(max_extra_per_node);
                extra -= size;
                min_child_size + size
            });

            if optimize {
                self.sort_nodes_by_sah(nodes, splits);
            }

            let mut offset = 0;
            let children = splits
                .into_iter()
                .map(|size| {
                    let i = self.build_temp_inner(&mut nodes[offset..(offset + size)], optimize);
                    offset += size;
                    i
                })
                .collect();

            let i = self.nodes.len();
            self.nodes.push(TempBvhNode {
                group: u32::MAX,
                aabb: aabb_default(),
                children,
            });
            i as _
        }
    }

    fn build_temp(&mut self) -> u32 {
        let mut lods = Vec::with_capacity(self.lods.len());
        for lod in core::mem::take(&mut self.lods) {
            let mut lod: Vec<_> = lod.collect();
            let root = self.build_temp_inner(&mut lod, true);
            let node = &self.nodes[root as usize];
            if node.group != u32::MAX || node.children.len() == 8 {
                lods.push(root);
            } else {
                lods.extend(node.children.iter().copied());
            }
        }
        self.build_temp_inner(&mut lods, false)
    }

    fn build_inner(
        &self,
        groups: &[TempMeshletGroup],
        out: &mut Vec<BvhNode>,
        max_depth: &mut u32,
        node: u32,
        depth: u32,
    ) -> u32 {
        *max_depth = depth.max(*max_depth);
        let node = &self.nodes[node as usize];
        let onode = out.len();
        out.push(BvhNode::default());

        for (i, &child_id) in node.children.iter().enumerate() {
            let child = &self.nodes[child_id as usize];
            if child.group != u32::MAX {
                let group = &groups[child.group as usize];
                let out = &mut out[onode];
                out.aabb_and_offsets[i] = aabb_to_meshlet(group.aabb, group.meshlets[0]);
                out.errors[i] = group.parent_error;
                out.lod_bounds[i] = group.lod_bounds;
                out.set_child_count(i, group.meshlets[1] as _);
            } else {
                let child_id = self.build_inner(groups, out, max_depth, child_id, depth + 1);
                let child = &out[child_id as usize];
                let mut aabb = aabb_default();
                let mut parent_error = 0.0f32;
                let mut lod_bounds = Vec4::ZERO;

                for i in 0..8 {
                    if child.child_counts(i) == 0 {
                        break;
                    }

                    aabb = aabb.merge(&Aabb3d::new(
                        child.aabb_and_offsets[i].center_and_offset_high.xyz(),
                        child.aabb_and_offsets[i].half_extent_and_offset_low.xyz(),
                    ));
                    lod_bounds = merge_spheres(lod_bounds, child.lod_bounds[i]);
                    parent_error = parent_error.max(child.errors[i]);
                }

                let out = &mut out[onode];
                out.aabb_and_offsets[i] = aabb_to_meshlet(aabb, child_id);
                out.errors[i] = parent_error;
                out.lod_bounds[i] = lod_bounds;
                out.set_child_count(i, u8::MAX);
            }
        }

        onode as _
    }

    fn build(
        mut self,
        meshlets: &mut meshopt::Meshlets,
        mut groups: Vec<TempMeshletGroup>,
        cull_data: &mut Vec<TempMeshletCullData>,
    ) -> (Vec<BvhNode>, AabbError, u32) {
        // The BVH requires group meshlets to be contiguous, so remap them first.
        let mut remap = Vec::with_capacity(meshlets.meshlets.len());
        let mut remapped_cull_data = Vec::with_capacity(cull_data.len());
        for group in groups.iter_mut() {
            let first = remap.len() as u32;
            let count = group.meshlets.len() as u32;
            remap.extend(
                group
                    .meshlets
                    .iter()
                    .map(|&m| meshlets.meshlets[m as usize]),
            );
            remapped_cull_data.extend(group.meshlets.iter().map(|&m| cull_data[m as usize]));
            group.meshlets.resize(2, 0);
            group.meshlets[0] = first;
            group.meshlets[1] = count;
        }
        meshlets.meshlets = remap;
        *cull_data = remapped_cull_data;

        let mut out = vec![];
        let mut aabb = aabb_default();
        let mut max_depth = 0;

        if self.nodes.len() == 1 {
            let mut o = BvhNode::default();
            let group = &groups[0];
            o.aabb_and_offsets[0] = aabb_to_meshlet(group.aabb, group.meshlets[0]);
            o.errors[0] = group.parent_error;
            o.lod_bounds[0] = group.lod_bounds;
            o.set_child_count(0, group.meshlets[1] as _);
            out.push(o);
            aabb = group.aabb;
            max_depth = 1;
        } else {
            let root = self.build_temp();
            let root = self.build_inner(&groups, &mut out, &mut max_depth, root, 1);
            assert_eq!(root, 0, "root must be 0");

            let root = &out[0];
            for i in 0..8 {
                if root.child_counts(i) == 0 {
                    break;
                }

                aabb = aabb.merge(&Aabb3d::new(
                    root.aabb_and_offsets[i].center_and_offset_high.xyz(),
                    root.aabb_and_offsets[i].half_extent_and_offset_low.xyz(),
                ));
            }
        }

        let mut reachable = vec![false; meshlets.meshlets.len()];
        verify_bvh(&out, cull_data, &mut reachable, 0);
        assert!(
            reachable.iter().all(|&x| x),
            "all meshlets must be reachable"
        );

        (
            out,
            AabbError {
                center_and_error: aabb.center().to_vec3().extend(0.0),
                half_extent: aabb.half_size().to_vec3().extend(0.0),
            },
            max_depth,
        )
    }
}

fn verify_bvh(
    out: &[BvhNode],
    cull_data: &[TempMeshletCullData],
    reachable: &mut [bool],
    node: u32,
) {
    let node = &out[node as usize];
    for i in 0..8 {
        let sphere = node.lod_bounds[i];
        let error = node.errors[i];
        if node.child_counts(i) == u8::MAX {
            let child = &out[node.aabb_and_offsets[i].offset() as usize];
            for i in 0..8 {
                if child.child_counts(i) == 0 {
                    break;
                }
                assert!(child.errors[i] <= error, "BVH errors are not monotonic");
                let sphere_error = (sphere.xyz() - child.lod_bounds[i].xyz()).length()
                    - (sphere.w - child.lod_bounds[i].w);
                assert!(
                    sphere_error <= 0.001,
                    "BVH lod spheres are not monotonic ({sphere_error})"
                );
            }
            verify_bvh(
                out,
                cull_data,
                reachable,
                node.aabb_and_offsets[i].offset() as u32,
            );
        } else {
            for m in 0..node.child_counts(i) as u32 {
                let mid = (m + node.aabb_and_offsets[i].offset() as u32) as usize;
                let meshlet = &cull_data[mid];
                assert!(
                    meshlet.error <= error || meshlet.error.is_infinite() || error.is_infinite(),
                    "meshlet errors are not monotonic: {} <= {}",
                    meshlet.error,
                    error
                );
                let sphere_error = (sphere.xyz() - meshlet.lod_group_sphere.xyz()).length()
                    - (sphere.w - meshlet.lod_group_sphere.w);
                assert!(
                    sphere_error <= 0.001,
                    "meshlet lod spheres are not monotonic: ({sphere_error})"
                );
                reachable[mid] = true;
            }
        }
    }
}

fn aabb_default() -> Aabb3d {
    Aabb3d {
        min: Vec3A::INFINITY,
        max: Vec3A::NEG_INFINITY,
    }
}

fn aabb_to_meshlet(aabb: Aabb3d, child_offset: u32) -> AabbPtr {
    AabbPtr::new(aabb, child_offset)
}
