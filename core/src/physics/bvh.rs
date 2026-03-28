use std::{process::Child, slice::Iter};

use bevy::{
    ecs::system::{SystemParam, lifetimeless::Read},
    math::{
        VectorSpace,
        bounding::{Aabb3d, BoundingVolume, RayCast3d},
    },
    prelude::*,
};
use bytemuck::{Pod, Zeroable, bytes_of};
use itertools::Itertools;
use tracing_log::log;

use crate::{assets::Mesh, components::gizzmos::BoxGizzmo, render::{render::RenderSettings, world::Model}};
#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum ChildType {
    Empty = 0b0000_0000,
    HasBlas = 0b0000_0011,
    HasLeaf = 0b0000_0101,
    HasNode = 0b0000_1001, // interior BVH node
}

impl ChildType {
    fn size(&self) -> usize {
        match self {
            ChildType::Empty => 0,
            ChildType::HasBlas => size_of::<HasBlas>(),
            ChildType::HasLeaf => size_of::<HasLeaf>(),
            ChildType::HasNode => size_of::<HasNode>(),
        }
    }
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct HasNode {
    ptr: usize,
}

unsafe impl Pod for HasBlas {}
unsafe impl Zeroable for HasBlas {}

#[derive(Clone, Copy)]
#[repr(C)]
struct HasBlas {
    transform: [f32; 16],
    sub_instance_index: usize,
    blas_root_node_index: usize,
    entity: Entity,
}

#[derive(Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub(crate) struct HasLeaf {
    pub(crate) meshlet: usize,
}

#[derive(Clone, Copy)]
pub(crate) enum ChildData {
    HasBlas(HasBlas),
    HasLeaf(HasLeaf),
    HasNode(HasNode),
}

pub(crate) struct NodeBuilder {
    child_mask: [u8; 8],
    child_data: [Option<ChildData>; 8],
    child_aabbs: [Aabb3d; 8],
    child_offset: usize,
}

fn aabb_to_pod(aabb: Aabb3d) -> [u8; 32] {
    let mut array = [0.0; 8];
    array[..4].copy_from_slice(&aabb.max.extend(0.0).to_array());
    array[4..].copy_from_slice(&aabb.min.extend(0.0).to_array());
    bytemuck::cast(array)
}

fn aabb_from_pod(pod: &[u8]) -> Aabb3d {
    let max = bytemuck::from_bytes::<[f32; 4]>(&pod[..4 * 4]);
    let min = bytemuck::from_bytes::<[f32; 4]>(&pod[4 * 4..8 * 4]);
    Aabb3d {
        max: Vec4::from_array(*max).xyz().into(),
        min: Vec4::from_array(*min).xyz().into(),
    }
}
impl NodeBuilder {
    fn new() -> Self {
        Self {
            child_aabbs: [Aabb3d::new(Vec3A::ZERO, Vec3A::ZERO); 8],
            child_data: [None; 8],
            child_mask: [0; 8],
            child_offset: 0,
        }
    }
    fn push_child(
        mut self,
        child_data: Option<ChildData>,
        child_type: ChildType,
        aabb: Aabb3d,
    ) -> Self {
        self.child_data[self.child_offset] = child_data;
        self.child_mask[self.child_offset] = child_type as u8;
        self.child_aabbs[self.child_offset] = aabb;
        self.child_offset += 1;
        self
    }
    fn push_leaf_child(mut self, data: HasLeaf, aabb: Aabb3d) -> Self {
        self.push_child(Some(ChildData::HasLeaf(data)), ChildType::HasLeaf, aabb)
    }
    fn push_blas_child(mut self, data: HasBlas, aabb: Aabb3d) -> Self {
        self.push_child(Some(ChildData::HasBlas(data)), ChildType::HasBlas, aabb)
    }
    fn push_node_child(self, data: HasNode, aabb: Aabb3d) -> Self {
        assert!(data.ptr % 8 == 0);
        self.push_child(Some(ChildData::HasNode(data)), ChildType::HasNode, aabb)
    }
    fn build(self, data: &mut Vec<u8>) -> usize {
        let ptr = data.len();
        assert!(data.len() % 8 == 0);
        data.extend_from_slice(&self.child_mask);
        for aabb in self.child_aabbs {
            data.extend_from_slice(&aabb_to_pod(aabb));
        }
        for cdata in &self.child_data {
            let Some(cdata) = cdata else {
                continue;
            };
            match cdata {
                ChildData::HasNode(cdata) => data.extend_from_slice(bytes_of(cdata)),
                ChildData::HasBlas(cdata) => data.extend_from_slice(bytes_of(cdata)),
                ChildData::HasLeaf(cdata) => data.extend_from_slice(bytes_of(cdata)),
            }
        }
        data.extend_from_slice(bytes_of(&67u64));
        ptr
    }
    fn total_aabb(&self) -> Option<Aabb3d> {
        let mut total: Option<Aabb3d> = None;
        for aabb in &self.child_aabbs {
            if let Some(total) = &mut total {
                *total = total.merge(aabb);
            } else {
                total = Some(*aabb);
            }
        }
        total
    }
}

struct NodeView<'a> {
    offset: usize,
    data: &'a [u8],
}

impl<'a> NodeView<'a> {
    fn new(data: &'a [u8], offset: usize) -> Self {
        assert!(offset % 8 == 0);
        NodeView { offset, data }
    }
    fn get_child_mask(&self) -> u64 {
        *bytemuck::from_bytes::<u64>(&self.data[self.offset..self.offset + 8])
    }
    fn child_count(&self) -> usize {
        let child_mask = self.get_child_mask();
        (64 - child_mask.trailing_zeros()) as usize / 8
    }
    fn get_type(&self, i: usize) -> ChildType {
        let child_mask = self.get_child_mask();
        unsafe { std::mem::transmute::<u8, ChildType>(((child_mask >> i * 8) & 0xFF) as u8) }
    }
    fn get_data(&self, i: usize) -> Option<ChildData> {
        let mut offset = 8 + 8 * 32;
        for c in 0..i {
            offset += self.get_type(c).size();
        }
        self.get_data_offset(offset, self.get_type(i))
    }
    fn get_data_offset(&self, offset: usize, stype: ChildType) -> Option<ChildData> {
        let slice = &self.data[self.offset + offset..self.offset + offset + stype.size()];
        match stype {
            ChildType::Empty => None,
            ChildType::HasBlas => Some(ChildData::HasBlas(*bytemuck::from_bytes(slice))),
            ChildType::HasLeaf => Some(ChildData::HasLeaf(*bytemuck::from_bytes(slice))),
            ChildType::HasNode => Some(ChildData::HasNode(*bytemuck::from_bytes(slice))),
        }
    }
    fn get_aabb(&self, i: usize) -> Aabb3d {
        aabb_from_pod(&self.data[self.offset + 8 + i * 32..self.offset + 8 + i * 32 + 32])
    }
    fn child_iter(self) -> ChildIter<'a> {
        ChildIter {
            data_offset: 8 + 8 * 32,
            child: 0,
            view: self,
        }
    }
}

struct ChildIter<'a> {
    data_offset: usize,
    child: usize,
    view: NodeView<'a>,
}

impl<'a> Iterator for &mut ChildIter<'a> {
    type Item = (ChildData, Aabb3d);
    fn next(&mut self) -> Option<Self::Item> {
        if self.child >= 8 {
            let offset = self.view.offset + self.data_offset;
            assert!(67u64 == *bytemuck::from_bytes::<u64>(&self.view.data[offset..offset + 8]));
            return None;
        }
        let stype = self.view.get_type(self.child);
        if stype == ChildType::Empty {
            return None;
        }
        let item = self.view.get_data_offset(self.data_offset, stype).unwrap();
        let aabb = self.view.get_aabb(self.child);
        self.child += 1;
        self.data_offset += stype.size();
        Some((item, aabb))
    }
}

fn transform_ray(ray: &RayCast3d, mat: &Mat4) -> RayCast3d {
    let origin = mat.transform_point3(ray.origin.into());
    let direction = mat.transform_vector3(ray.direction.to_vec3());
    RayCast3d::new(origin, Dir3::new(direction).unwrap(), ray.max)
}

fn transform_aabb(aabb: &Aabb3d, mat: &Mat4) -> Aabb3d {
    let center = mat.transform_point3(aabb.center().into());

    let half = aabb.half_size();
    let x = mat.x_axis.truncate() * half.x;
    let y = mat.y_axis.truncate() * half.y;
    let z = mat.z_axis.truncate() * half.z;

    let new_half = x.abs() + y.abs() + z.abs();

    Aabb3d::new(center, new_half)
}

pub(crate) fn vec3_to_morton(
    pos: Vec3A,
    bounds_min: Vec3A,
    bounds_max: Vec3A,
    resolution: u32,
) -> u64 {
    let normalized = (pos - bounds_min) / (bounds_max - bounds_min);
    let scale = (resolution - 1) as f32;
    let x = (normalized.x * scale).clamp(0.0, scale) as u32;
    let y = (normalized.y * scale).clamp(0.0, scale) as u32;
    let z = (normalized.z * scale).clamp(0.0, scale) as u32;
    morton3d(x, y, z)
}

fn spread_bits(x: u32) -> u64 {
    let mut x = x as u64;
    x = (x | (x << 32)) & 0x1f00000000ffff;
    x = (x | (x << 16)) & 0x1f0000ff0000ff;
    x = (x | (x << 8)) & 0x100f00f00f00f00f;
    x = (x | (x << 4)) & 0x10c30c30c30c30c3;
    x = (x | (x << 2)) & 0x1249249249249249;
    x
}

fn morton3d(x: u32, y: u32, z: u32) -> u64 {
    spread_bits(x) | (spread_bits(y) << 1) | (spread_bits(z) << 2)
}

#[derive(Clone, Copy, Debug)]
pub struct RayCastResult {
    pub entity: Entity,
    pub sub_instance: usize,
    pub meshlet: usize,
    pub aabb: Aabb3d,
    pub t: f32,
}

#[derive(Resource)]
pub struct SceneBvh {
    pub(crate) bvh: Vec<u8>,
    pub(crate) root: usize,
}
impl SceneBvh {
    fn raycast(
        &self,
        ray: &RayCast3d,
        assets: &Assets<Mesh>,
        models: &Query<&Model>,
    ) -> Option<RayCastResult> {
        if self.bvh.is_empty() {
            return None;
        }

        let mut best: Option<RayCastResult> = None;
        let mut stack: Vec<(usize, &[u8], f32)> = vec![(self.root, &self.bvh, 0.0)];

        while let Some((offset, bvh, t_entry)) = stack.pop() {
            if best.map_or(false, |b| t_entry >= b.t) {
                continue;
            }

            let view = NodeView::new(bvh, offset);

            // Gather intersecting children, sort closest-last for stack ordering
            let mut hits: Vec<(f32, ChildData, Aabb3d)> = Vec::new();
            for (data, aabb) in &mut view.child_iter() {
                let Some(t) = ray.aabb_intersection_at(&aabb) else {
                    continue;
                };
                if best.map_or(false, |b| t >= b.t) {
                    continue;
                }
                hits.push((t, data, aabb));
            }
            hits.sort_unstable_by(|a, b| {
                b.0.total_cmp(&a.0)
            });

            for (t, data, _) in hits {
                match data {
                    ChildData::HasNode(node) => {
                        stack.push((node.ptr, bvh, t));
                    }

                    ChildData::HasBlas(blas) => {
                        // Resolve the per-mesh BLAS buffer through the entity
                        let Ok(model_handle) = models.get(blas.entity) else {
                            continue;
                        };
                        let Some(model) = assets.get(&model_handle.model) else {
                            continue;
                        };
                        let mesh = &model.meshes[blas.sub_instance_index];
                        let blas_bvh: &[u8] = &mesh.colission_bvh;

                        // Transform ray into local space of this instance
                        let inv = Mat4::from_cols_array(&blas.transform).inverse();
                        let local_ray = transform_ray(ray, &inv);

                        // Traverse the BLAS inline — push its root onto the
                        // stack with the local ray. We can't mix rays though,
                        // so we do a nested traversal here rather than sharing
                        // the outer stack.
                        let blas_result = raycast_blas(
                            &local_ray,
                            blas_bvh,
                            blas.blas_root_node_index,
                            blas.entity,
                            blas.sub_instance_index,
                            best.map(|b| b.t),
                        );

                        if let Some(result) = blas_result {
                            if best.map_or(true, |b| result.t < b.t) {
                                best = Some(result);
                            }
                        }
                    }

                    // TLAS should only contain HasNode and HasBlas at this point
                    ChildData::HasLeaf(_) => {}
                }
            }
        }

        best
    }
}

fn raycast_blas(
    ray: &RayCast3d,
    bvh: &[u8],
    root: usize,
    entity: Entity,
    sub_instance: usize,
    t_max: Option<f32>,
) -> Option<RayCastResult> {
    let mut best: Option<RayCastResult> = None;
    let mut stack: Vec<(usize, f32)> = vec![(root, 0.0)];

    while let Some((offset, t_entry)) = stack.pop() {
        // Prune against both local best and the t_max hint from the TLAS
        if best.map_or(false, |b| t_entry >= b.t) {
            continue;
        }
        if t_max.map_or(false, |t| t_entry >= t) {
            continue;
        }

        let view = NodeView::new(bvh, offset);

        let mut hits: Vec<(f32, ChildData, Aabb3d)> = Vec::with_capacity(8);
        for (data, aabb) in &mut view.child_iter() {
            let Some(t) = ray.aabb_intersection_at(&aabb) else {
                continue;
            };
            if best.map_or(false, |b| t >= b.t) {
                continue;
            }
            if t_max.map_or(false, |tm| t >= tm) {
                continue;
            }
            hits.push((t, data, aabb));
        }
        hits.sort_unstable_by(|a, b| b.0.total_cmp(&a.0));

        for (t, data, aabb) in hits {
            match data {
                ChildData::HasNode(node) => {
                    stack.push((node.ptr, t));
                }
                ChildData::HasLeaf(leaf) => {
                    if best.map_or(true, |b| t < b.t) {
                        best = Some(RayCastResult {
                            entity,
                            sub_instance,
                            meshlet: leaf.meshlet,
                            aabb,
                            t,
                        });
                    }
                }
                ChildData::HasBlas(_) => {}
            }
        }
    }

    best
}

#[derive(SystemParam)]
pub struct Raycast<'w, 's> {
    meshes: Res<'w, Assets<Mesh>>,
    models: Query<'w, 's, Read<Model>>,
    bvh: Res<'w, SceneBvh>,
}

impl<'w, 's> Raycast<'w, 's> {
    pub fn raycast(&self, ray: &RayCast3d) -> Option<RayCastResult> {
        self.bvh.raycast(ray, &self.meshes, &self.models)
    }
    pub fn get_instance_transform(&self, hit: &RayCastResult) -> Mat4 {
        let Ok(model) = self.models.get(hit.entity) else {
            return Mat4::IDENTITY;
        };
        let Some(mesh) = self.meshes.get(&model.model) else {
            return Mat4::IDENTITY;
        };
        mesh.instance_transforms[hit.sub_instance]
    }
}

pub(crate) struct LeafData {
    pub(crate) mortan_code: u64,
    pub(crate) data: ChildData,
    pub(crate) typ: ChildType,
    pub(crate) aabb: Aabb3d,
}

pub(crate) fn build_intial_nodes(
    leafs: &mut [LeafData],
    max_aabb: Aabb3d,
) -> Vec<(NodeBuilder, u64, Aabb3d)> {
    if leafs.len() == 0 {
        return Vec::new();
    }
    leafs.sort_by(|a, b| a.mortan_code.cmp(&b.mortan_code));
    let mut out = Vec::new();
    let mut builder = NodeBuilder::new();
    for leaf in leafs {
        if builder.child_offset == 8 {
            let aabb = builder.total_aabb().unwrap();
            let mortan_code = vec3_to_morton(aabb.center(), max_aabb.min, max_aabb.max, 1024);
            out.push((
                std::mem::replace(&mut builder, NodeBuilder::new()),
                mortan_code,
                aabb,
            ));
        }
        builder = builder.push_child(Some(leaf.data), leaf.typ, leaf.aabb);
    }
    let aabb = builder.total_aabb().unwrap();
    let mortan_code = vec3_to_morton(aabb.center(), max_aabb.min, max_aabb.max, 1024);
    out.push((builder, mortan_code, aabb));
    out
}

pub(crate) fn build_bvh(
    mut nodes: Vec<(NodeBuilder, u64, Aabb3d)>,
    max_aabb: Aabb3d,
    data: &mut Vec<u8>,
) -> usize {
    if nodes.len() == 0 {
        return 0;
    }
    let mut builder = NodeBuilder::new();
    nodes.sort_by(|a, b| a.1.cmp(&b.1));
    let mut out = Vec::new();
    for node in nodes {
        if builder.child_offset == 8 {
            let aabb = builder.total_aabb().unwrap();
            let mortan_code = vec3_to_morton(aabb.center(), max_aabb.min, max_aabb.max, 1024);
            out.push((
                std::mem::replace(&mut builder, NodeBuilder::new()),
                mortan_code,
                aabb,
            ));
        }
        let ptr = node.0.build(data);
        builder = builder.push_node_child(HasNode { ptr }, node.2);
    }
    let aabb = builder.total_aabb().unwrap();
    let mortan_code = vec3_to_morton(aabb.center(), max_aabb.min, max_aabb.max, 1024);
    out.push((builder, mortan_code, aabb));

    if out.len() != 1 {
        build_bvh(out, max_aabb, data)
    } else {
        out.into_iter().next().unwrap().0.build(data)
    }
}

pub(crate) fn update_bvh(
    assets: Res<Assets<Mesh>>,
    models: Query<(Entity, &Model, &GlobalTransform)>,
    mut scene_bvh: ResMut<SceneBvh>,
) {
    scene_bvh.bvh.clear();

    let mut total_aabb: Option<Aabb3d> = None;
    for (_, model, transform) in &models {
        let Some(model) = assets.get(&model.model) else {
            continue;
        };
        for instance in model.instance_mesh.iter().copied() {
            let mesh = &model.meshes[instance as usize];
            let aabb = transform_aabb(
                &Aabb3d::new(
                    Vec3A::from_array(mesh.header.aabb.center),
                    Vec3A::from_array(mesh.header.aabb.half_extend),
                ),
                &model.instance_transforms[instance as usize],
            );
            if let Some(total_aabb) = &mut total_aabb {
                *total_aabb = total_aabb.merge(&aabb);
            } else {
                total_aabb = Some(aabb);
            }
        }
    }

    let Some(total_aabb) = total_aabb else { return };
    let mut initial_nodes = Vec::new();
    for (entity, model, transform) in &models {
        let Some(model) = assets.get(&model.model) else {
            continue;
        };
        for instance in model.instance_mesh.iter().copied() {
            let mesh = &model.meshes[instance as usize];
            let sub_transform = model.instance_transforms[instance as usize];
            let total_transform = transform.to_matrix() * sub_transform;
            let aabb = transform_aabb(
                &Aabb3d::new(
                    Vec3A::from_array(mesh.header.aabb.center),
                    Vec3A::from_array(mesh.header.aabb.half_extend),
                ),
                &sub_transform,
            );
            initial_nodes.push(LeafData {
                data: ChildData::HasBlas(HasBlas {
                    entity,
                    blas_root_node_index: mesh.header.colission_bvh_root_node as usize,
                    sub_instance_index: instance as usize,
                    transform: total_transform.to_cols_array(),
                }),
                typ: ChildType::HasBlas,
                mortan_code: vec3_to_morton(aabb.center(), total_aabb.min, total_aabb.max, 1024),
                aabb,
            });
        }
    }
    let nodes = build_intial_nodes(&mut initial_nodes, total_aabb);
    let root = build_bvh(nodes, total_aabb, &mut scene_bvh.bvh);
    scene_bvh.root = root;
}

pub(crate) fn debug_draw_scene_bvh(
    mut cmd: Commands,
    scene_bvh: Res<SceneBvh>,
    assets: Res<Assets<Mesh>>,
    models: Query<&Model>,
    setting: Res<RenderSettings>,
) {
    if scene_bvh.bvh.is_empty() || !setting.draw_scene_bvh {
        return;
    }

    // (bvh buffer, node offset, transform)
    let mut stack: Vec<(&[u8], usize, Mat4)> = vec![(&scene_bvh.bvh, scene_bvh.root, Mat4::IDENTITY)];

    while let Some((bvh, offset, transform)) = stack.pop() {
        let view = NodeView::new(bvh, offset);
        let child_count = view.child_count();

        for i in 0..child_count {
            let aabb = view.get_aabb(i);
            let Some(data) = view.get_data(i) else { continue };

            let color = match data {
                ChildData::HasNode(_) => Vec4::new(0.0, 1.0, 0.0, 0.2),
                ChildData::HasBlas(_) => Vec4::new(0.0, 0.0, 1.0, 0.2),
                ChildData::HasLeaf(_) => Vec4::new(1.0, 0.0, 0.0, 0.2),
            };

            cmd.spawn(BoxGizzmo::with_local_tranform(
                aabb.center().into(),
                aabb.half_size().into(),
                color,
                transform,
            ));

            match data {
                ChildData::HasNode(node) => {
                    stack.push((bvh, node.ptr, transform));
                }
                ChildData::HasBlas(blas) => {
                    let Ok(model_handle) = models.get(blas.entity) else { continue };
                    let Some(model) = assets.get(&model_handle.model) else { continue };
                    let mesh = &model.meshes[blas.sub_instance_index];
                    let blas_transform = Mat4::from_cols_array(&blas.transform);
                    // SAFETY: we're pushing a reference to mesh data that lives in the
                    // Assets collection. The borrow is valid for this system's duration.
                    let blas_bvh: &[u8] = unsafe {
                        std::slice::from_raw_parts(mesh.colission_bvh.as_ptr(), mesh.colission_bvh.len())
                    };
                    stack.push((blas_bvh, blas.blas_root_node_index, blas_transform));
                }
                ChildData::HasLeaf(_) => {}
            }
        }
    }
}
