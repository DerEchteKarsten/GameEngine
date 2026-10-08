//! Scene camera component: view/projection matrices, ray generation and per-frame updates. A camera looks along its local +X, with +Y to the right and +Z up.
use glam::{Mat4, Vec3, Vec4};

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

#[derive(Bundle)]
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
        *self.proj_inv.get_or_insert(self.proj.inverse())
    }

    pub fn view_inv(&mut self) -> Mat4 {
        *self.view_inv.get_or_insert(self.view.inverse())
    }

    pub fn ray_direction(
        &self,
        transform: &GlobalTransform,
        pixel: Vec2,
        resolution: UVec2,
    ) -> Vec3 {
        let resolution = resolution.as_vec2();
        let half_h = (self.fov * 0.5).tan();
        let half_w = half_h * resolution.x / resolution.y;
        let right = (pixel.x / resolution.x * 2.0 - 1.0) * half_w;
        let up = (1.0 - pixel.y / resolution.y * 2.0) * half_h;
        (transform.rotation() * Vec3::new(1.0, right, up)).normalize()
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

        (a * e - b * d) / denom
    }
}

/// Turns the camera's axes (looking along +X, +Y right, +Z up) into Vulkan's view space
/// (+X right, +Y down, looking along +Z).
const VIEW_FROM_CAMERA: Mat4 = Mat4::from_cols(
    Vec4::Z,
    Vec4::X,
    Vec4::NEG_Y,
    Vec4::W,
);

pub(super) fn update_camera(
    mut cameras: Query<(&mut Camera, &GlobalTransform)>,
    view_port: ViewPortProxy,
) {
    let size = view_port.size();
    let ar = size.x as f32 / size.y as f32;
    for (mut camera, transform) in &mut cameras {
        // Near and far are swapped for a reversed depth buffer.
        camera.proj = Mat4::perspective_lh(camera.fov, ar, camera.z_far, camera.z_near);
        camera.view = VIEW_FROM_CAMERA * transform.to_matrix().inverse();
        camera.view_inv = None;
        camera.proj_inv = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::headless::HeadlessSize;
    use bevy::ecs::system::RunSystemOnce;

    /// Picking and the gizmo drag invert the projection: a point drawn on a pixel lies on that
    /// pixel's ray, and on an axis through it at the right distance.
    #[test]
    fn picking_inverts_the_projection() {
        let size = UVec2::new(1280, 720);
        let transform = Transform::from_xyz(1.0, 2.0, 3.0)
            .with_rotation(Quat::from_rotation_z(0.5) * Quat::from_rotation_y(-0.3));
        let global = GlobalTransform::from(transform);
        let mut world = World::new();
        world.insert_resource(HeadlessSize(size));
        let entity = world
            .spawn((CameraBundle::new(transform, 1.2, 0.01, 100.0), global))
            .id();
        world.run_system_once(update_camera).unwrap();
        let camera = world.get::<Camera>(entity).unwrap();

        // Ahead of the camera, to its right and above it.
        let point = transform.transform_point(Vec3::new(10.0, 2.0, 1.0));
        let clip = camera.proj * camera.view * point.extend(1.0);
        let ndc = clip.truncate() / clip.w;
        // Vulkan's +Y is down, and the depth is reversed.
        assert!(ndc.x > 0.0 && ndc.y < 0.0 && ndc.z > 0.0 && ndc.z < 1.0);
        let pixel = (ndc.truncate() * 0.5 + 0.5) * size.as_vec2();

        let ray = camera.ray_direction(&global, pixel, size);
        assert!(ray.abs_diff_eq((point - transform.translation).normalize(), 1e-4));
        let t = camera.closest_t_on_axis(&global, pixel, size, point - Vec3::Y * 3.0, Vec3::Y);
        assert!((t - 3.0).abs() < 1e-3, "{t}");
    }
}
