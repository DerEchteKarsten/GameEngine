use glam::{Mat4, Vec3};

use bevy::prelude::*;

use crate::editor::viewport::ViewPortProxy;

#[derive(Debug, Clone, Copy, PartialEq, Component, Reflect)]
#[reflect(Component, Debug, Clone, PartialEq)]
pub struct Camera {
    pub view: Mat4,
    pub proj: Mat4,
    pub view_inv: Option<Mat4>,
    pub proj_inv: Option<Mat4>,
    pub fov: f32,
    pub z_near: f32,
    pub z_far: f32,
}

#[derive(Bundle, Resource)]
pub struct CameraBundle {
    pub camera: Camera,
    pub transform: Transform,
}

impl CameraBundle {
    pub fn new(transform: Transform, fov: f32, z_near: f32, z_far: f32) -> Self {
        Self {
            transform,
            camera: Camera {
                proj: Mat4::IDENTITY,
                proj_inv: None,
                view: Mat4::IDENTITY,
                view_inv: None,
                fov,
                z_near,
                z_far,
            },
        }
    }
}
impl Camera {
    pub fn proj_inv(&mut self) -> Mat4 {
        self.proj_inv.get_or_insert(self.proj.inverse()).clone()
    }

    pub fn view_inv(&mut self) -> Mat4 {
        self.view_inv.get_or_insert(self.view.inverse()).clone()
    }

    pub fn ray_direction(
        &self,
        transform: &GlobalTransform,
        pixel: Vec2,
        resolution: UVec2,
    ) -> Vec3 {
        let resolution = resolution.as_vec2();
        let aspect = resolution.x / resolution.y;

        let ndc = Vec2::new(
            1.0 - (pixel.x / resolution.x) * 2.0,
            1.0 - (pixel.y / resolution.y) * 2.0,
        );

        let up = transform.up();
        let right = transform.right();
        let forward = transform.forward();

        let half_h = (self.fov * 0.5).tan();
        let half_w = half_h * aspect;

        (*forward + *right * ndc.x * half_w + *up * ndc.y * half_h).normalize()
    }

    pub fn closest_t_on_axis(
        &self,
        transform: &GlobalTransform,
        pixel: Vec2,
        resolution: UVec2,
        axis_origin: Vec3,
        axis: Vec3,
    ) -> f32 {
        let ray_origin = transform.translation();
        let ray_dir = self.ray_direction(transform, pixel, resolution);

        let w = ray_origin - axis_origin;
        let a = ray_dir.dot(ray_dir);
        let b = ray_dir.dot(axis);
        let c = axis.dot(axis);
        let d = ray_dir.dot(w);
        let e = axis.dot(w);

        let denom = a * c - b * b;

        if denom.abs() < 1e-6 {
            return e / c;
        }

        (b * d - a * e) / denom
    }
}

pub(super) fn update_camera(
    mut cameras: Query<(&mut Camera, &GlobalTransform)>,
    view_port: ViewPortProxy,
) {
    let size = view_port.size();
    let ar = size.x as f32 / size.y as f32;
    for (mut camera, transform) in &mut cameras {
        camera.proj = Mat4::perspective_rh(camera.fov, ar, camera.z_far, camera.z_near);
        camera.view = Mat4::look_at_rh(
            transform.translation(),
            transform.translation() + *(transform.forward()),
            Vec3::NEG_Y,
        );
        camera.view_inv = None;
        camera.proj_inv = None;
    }
}
