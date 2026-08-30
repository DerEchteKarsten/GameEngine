use std::ops::Range;

use crate::{
    bindings::{
        DrawAabbs, DrawAabbsBindings, DrawArrows, DrawArrowsBindings, DrawSpheres,
        DrawSpheresBindings, Gizzmo,
    },
    editor::viewport::{ViewPort, ViewPortProxy},
    render::{
        FRAMES_IN_FLIGHT, MainWorld,
        render::{FrameCount, RenderCamera, Swapchain},
    },
    scene::camera::Camera,
};
use bevy::{
    ecs::{
        resource::Resource,
        system::{
            Commands, Local, Res, ResMut, Single, SystemParam, SystemState, lifetimeless::Read,
        },
    },
    transform::components::GlobalTransform,
};
use glam::{Mat4, Quat, UVec2, Vec2, Vec3, Vec4};
use lava::{
    buffer::Buffer,
    command_buffer::{CommandBuffer, RasterVertexDispatch, Scissor, Viewport},
};

const MAX_GIZZMOS: usize = 1_000_000;

pub trait GizzmoShape {
    fn local_transform(&self) -> Mat4;
    fn color(&self) -> Vec4;
    fn ty(&self) -> GizzmoType;
    fn clicked(
        &self,
        mouse_pos: Vec2,
        window_size: UVec2,
        camera: &Camera,
        camera_transfrom: &GlobalTransform,
        local_transfrom: Mat4,
    ) -> bool;
}

#[derive(Clone, Copy)]
pub struct BoxGizzmo {
    pub color: Vec4,
    pub center: Vec3,
    pub half_extend: Vec3,
}

#[derive(Clone, Copy)]
pub struct ArrowGizzmo {
    pub start: Vec3,
    pub end: Vec3,
    pub width: f32,
    pub color: Vec4,
}

#[derive(Clone, Copy)]
pub struct SphereGizzmo {
    pub pos: Vec3,
    pub radius: f32,
    pub color: Vec4,
}

impl GizzmoShape for BoxGizzmo {
    fn local_transform(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            self.half_extend * 2.0,
            Quat::IDENTITY,
            self.center - self.half_extend,
        )
    }
    fn color(&self) -> Vec4 {
        self.color
    }
    fn ty(&self) -> GizzmoType {
        GizzmoType::Box
    }
    fn clicked(
        &self,
        mouse_pos: Vec2,
        window_size: UVec2,
        camera: &Camera,
        camera_transfrom: &GlobalTransform,
        local_transform: Mat4,
    ) -> bool {
        let dir = camera.ray_direction(camera_transfrom, mouse_pos, window_size);
        let origin = camera_transfrom.translation();

        let inv = local_transform.inverse();
        let origin = inv.transform_point3(origin);
        let dir = inv.transform_vector3(dir);

        let inv_dir = Vec3::ONE / dir;

        let t1 = (origin) * inv_dir;
        let t2 = (Vec3::splat(1.0) - origin) * inv_dir;

        let t_min = t1.min(t2);
        let t_max = t1.max(t2);

        let t_enter = t_min.x.max(t_min.y).max(t_min.z);
        let t_exit = t_max.x.min(t_max.y).min(t_max.z);

        t_enter <= t_exit && t_exit >= 0.0
    }
}

impl GizzmoShape for SphereGizzmo {
    fn local_transform(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(self.radius * 2.0),
            Quat::IDENTITY,
            self.pos - Vec3::splat(self.radius / 2.0) - Vec3::splat(0.5),
        )
    }
    fn color(&self) -> Vec4 {
        self.color
    }
    fn ty(&self) -> GizzmoType {
        GizzmoType::Sphere
    }
    fn clicked(
        &self,
        mouse_pos: Vec2,
        window_size: UVec2,
        camera: &Camera,
        camera_transfrom: &GlobalTransform,
        local_transform: Mat4,
    ) -> bool {
        let dir = camera.ray_direction(camera_transfrom, mouse_pos, window_size);
        let origin = camera_transfrom.translation();

        let inv = local_transform.inverse();
        let origin = inv.transform_point3(origin);
        let dir = inv.transform_vector3(dir).normalize();

        let oc = origin - Vec3::splat(0.5);
        let b = oc.dot(dir);
        let c = oc.dot(oc) - 0.25;
        let discriminant = b * b - c;

        discriminant >= 0.0
    }
}

impl GizzmoShape for ArrowGizzmo {
    fn local_transform(&self) -> Mat4 {
        let dir = self.end - self.start;
        let quat = Quat::from_rotation_arc(Vec3::Z, dir.normalize());
        Mat4::from_scale_rotation_translation(
            Vec3::new(self.width, self.width, dir.length()),
            quat,
            self.start,
        )
    }
    fn color(&self) -> Vec4 {
        self.color
    }
    fn ty(&self) -> GizzmoType {
        GizzmoType::Arrow
    }
    fn clicked(
        &self,
        mouse_pos: Vec2,
        window_size: UVec2,
        camera: &Camera,
        camera_transform: &GlobalTransform,
        local_transform: Mat4,
    ) -> bool {
        let dir = camera.ray_direction(camera_transform, mouse_pos, window_size);
        let origin = camera_transform.translation();

        let inv = local_transform.inverse();
        let origin = inv.transform_point3(origin);
        let dir = inv.transform_vector3(dir).normalize();

        let a = dir.x * dir.x + dir.y * dir.y;
        let b = origin.x * dir.x + origin.y * dir.y;
        let c = origin.x * origin.x + origin.y * origin.y - 0.15 * 0.15;

        if a.abs() < 1e-6 {
            return false;
        }

        let discriminant = b * b - a * c;
        if discriminant < 0.0 {
            return false;
        }

        let t = (-b - discriminant.sqrt()) / a;
        if t < 0.0 {
            return false;
        }

        let hit_z = origin.z + dir.z * t;
        (0.0..=1.0).contains(&hit_z)
    }
}

#[derive(PartialEq, Debug)]
pub enum GizzmoType {
    Box,
    Sphere,
    Arrow,
}

#[derive(Resource)]
pub struct Gizzmos {
    pub gizzmos: Vec<(Gizzmo, GizzmoType)>,
}

#[derive(SystemParam)]
pub struct DrawGizzmos<'w, 's> {
    vp: ViewPortProxy<'s, 'w>,
    camera: Single<'w, 's, (Read<Camera>, Read<GlobalTransform>)>,
    gizzmos: Option<ResMut<'w, Gizzmos>>,
}

impl<'w, 's> DrawGizzmos<'w, 's> {
    pub fn draw_gizzmo(&mut self, shape: &impl GizzmoShape) {
        self.draw_gizzmo_with_transform(shape, Mat4::IDENTITY)
    }
    pub fn draw_gizzmo_with_transform(&mut self, shape: &impl GizzmoShape, transform: Mat4) {
        let Some(gizzmos) = self.gizzmos.as_mut() else {
            return;
        };
        gizzmos.gizzmos.push((
            Gizzmo {
                transform: transform * shape.local_transform(),
                color: shape.color(),
            },
            shape.ty(),
        ));
    }
    pub fn draw_gizzmo_check_clicked(
        &mut self,
        shape: &impl GizzmoShape,
        click: Option<Vec2>,
        transform: Mat4,
    ) -> bool {
        let Some(gizzmos) = self.gizzmos.as_mut() else {
            return false;
        };
        let full_transform = transform * shape.local_transform();

        gizzmos.gizzmos.push((
            Gizzmo {
                transform: full_transform,
                color: shape.color(),
            },
            shape.ty(),
        ));

        if let Some(pos) = click {
            shape.clicked(
                pos,
                self.vp.size(),
                self.camera.0,
                self.camera.1,
                full_transform,
            )
        } else {
            false
        }
    }
}

pub(crate) fn write_gizzmos(mut gizzmos: ResMut<GizzmoResources>, frame: Res<FrameCount>) {
    let frame_in_flight = frame.frame_in_flight();
    for slot in 0..gizzmos.pendings_gizzmos.len() {
        let gizz = gizzmos.pendings_gizzmos[slot];
        gizzmos.gizzmos[slot + frame_in_flight * MAX_GIZZMOS] = gizz;
    }
}

pub(crate) fn extract_gizzmos(
    mut gizzmos: ResMut<GizzmoResources>,
    mut main_world: ResMut<MainWorld>,
    mut system_state: Local<Option<SystemState<ResMut<Gizzmos>>>>,
) {
    if system_state.is_none() {
        *system_state = Some(SystemState::new(&mut main_world));
    }
    let system_state = system_state.as_mut().unwrap();
    let mut draw_gizzmos = system_state.get_mut(&mut main_world);
    gizzmos.pendings_gizzmos.clear();

    let start = 0;
    gizzmos.pendings_gizzmos.extend(
        draw_gizzmos
            .gizzmos
            .iter()
            .filter(|(_, ty)| *ty == GizzmoType::Box)
            .map(|(giz, _)| giz),
    );
    let after_aabb = gizzmos.pendings_gizzmos.len();
    gizzmos.aabb_range = start..after_aabb;

    gizzmos.pendings_gizzmos.extend(
        draw_gizzmos
            .gizzmos
            .iter()
            .filter(|(_, ty)| *ty == GizzmoType::Sphere)
            .map(|(giz, _)| giz),
    );
    let after_sphere = gizzmos.pendings_gizzmos.len();
    gizzmos.sphere_range = after_aabb..after_sphere;

    gizzmos.pendings_gizzmos.extend(
        draw_gizzmos
            .gizzmos
            .iter()
            .filter(|(_, ty)| *ty == GizzmoType::Arrow)
            .map(|(giz, _)| giz),
    );
    gizzmos.arrow_range = after_sphere..gizzmos.pendings_gizzmos.len();
    draw_gizzmos.gizzmos.clear();
}

#[derive(Resource)]
pub struct GizzmoResources {
    pub gizzmos: Buffer<Gizzmo>,
    pub pendings_gizzmos: Vec<Gizzmo>,
    pub aabb_range: Range<usize>,
    pub sphere_range: Range<usize>,
    pub arrow_range: Range<usize>,
}

pub(crate) fn init_gizzmos(mut cmd: Commands) {
    cmd.insert_resource(GizzmoResources {
        gizzmos: Buffer::new(MAX_GIZZMOS * FRAMES_IN_FLIGHT, true).unwrap(),
        pendings_gizzmos: Vec::with_capacity(MAX_GIZZMOS),
        aabb_range: 0..0,
        arrow_range: 0..0,
        sphere_range: 0..0,
    });
}

impl GizzmoResources {
    pub(crate) fn draw<'a>(
        &self,
        cmd: &mut CommandBuffer,
        swapchain: &Swapchain,
        camera: &RenderCamera,
        viewport: &ViewPort,
        frame_in_flight: usize,
    ) {
        if !self.aabb_range.is_empty() {
            cmd.raster::<DrawAabbs>()
                .bind(DrawAabbsBindings {
                    world_to_clip: camera.camera.proj * camera.camera.view,
                    gizzmos: self
                        .gizzmos
                        .range((MAX_GIZZMOS * frame_in_flight + self.aabb_range.start)..),
                })
                .color_attachment(swapchain.image(), None)
                .backface_culling(false)
                .draw_with_dynstates(
                    RasterVertexDispatch::Draw {
                        vertex_count: 36,
                        instance_count: self.aabb_range.len() as u32,
                    },
                    swapchain.size,
                    &[Scissor {
                        extent: viewport.visible_rect.size().as_uvec2(),
                        offset: viewport.visible_rect.min.as_ivec2(),
                    }],
                    Viewport {
                        extent: viewport.rect.size().as_uvec2(),
                        offset: viewport.rect.min.as_ivec2(),
                    },
                );
        }
        if !self.sphere_range.is_empty() {
            cmd.raster::<DrawSpheres>()
                .bind(DrawSpheresBindings {
                    world_to_clip: camera.camera.proj * camera.camera.view,
                    gizzmos: self
                        .gizzmos
                        .range((MAX_GIZZMOS * frame_in_flight + self.sphere_range.start)..),
                })
                .color_attachment(swapchain.image(), None)
                .backface_culling(false)
                .draw_with_dynstates(
                    RasterVertexDispatch::Draw {
                        vertex_count: 576,
                        instance_count: self.sphere_range.len() as u32,
                    },
                    swapchain.size,
                    &[Scissor {
                        extent: viewport.visible_rect.size().as_uvec2(),
                        offset: viewport.visible_rect.min.as_ivec2(),
                    }],
                    Viewport {
                        extent: viewport.rect.size().as_uvec2(),
                        offset: viewport.rect.min.as_ivec2(),
                    },
                );
        }
        if !self.arrow_range.is_empty() {
            cmd.raster::<DrawArrows>()
                .bind(DrawArrowsBindings {
                    world_to_clip: camera.camera.proj * camera.camera.view,
                    gizzmos: self
                        .gizzmos
                        .range((MAX_GIZZMOS * frame_in_flight + self.arrow_range.start)..),
                })
                .color_attachment(swapchain.image(), None)
                .backface_culling(false)
                .draw_with_dynstates(
                    RasterVertexDispatch::Draw {
                        vertex_count: 216,
                        instance_count: self.arrow_range.len() as u32,
                    },
                    swapchain.size,
                    &[Scissor {
                        extent: viewport.visible_rect.size().as_uvec2(),
                        offset: viewport.visible_rect.min.as_ivec2(),
                    }],
                    Viewport {
                        extent: viewport.rect.size().as_uvec2(),
                        offset: viewport.rect.min.as_ivec2(),
                    },
                );
        }
    }
}
