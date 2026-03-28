use bevy::ecs::component::Component;
use glam::{Mat4, Quat, Vec3, Vec4};

pub trait GizzmoShape {
    fn local_transform(&self) -> Mat4;
    fn color(&self) -> Vec4;
}

#[derive(Component, Clone, Copy)]
pub struct BoxGizzmo {
    transform: Mat4,
    color: Vec4,
}

impl BoxGizzmo {
    pub fn with_local_tranform(
        center: Vec3,
        half_extend: Vec3,
        color: Vec4,
        transform: Mat4,
    ) -> Self {
        Self {
            transform: transform
                * Mat4::from_scale_rotation_translation(
                    half_extend * 2.0,
                    Quat::IDENTITY,
                    center - half_extend,
                ),
            color,
        }
    }
    pub fn new(center: Vec3, half_extend: Vec3, color: Vec4) -> Self {
        Self::with_local_tranform(center, half_extend, color, Mat4::IDENTITY)
    }
}

#[derive(Component, Clone, Copy)]
pub struct ArrowGizzmo {
    pub start: Vec3,
    pub end: Vec3,
    pub width: f32,
    pub color: Vec4,
}

#[derive(Component, Clone, Copy)]
pub struct SphereGizzmo {
    pub pos: Vec3,
    pub radius: f32,
    pub color: Vec4,
}

impl GizzmoShape for BoxGizzmo {
    fn local_transform(&self) -> Mat4 {
        self.transform
    }
    fn color(&self) -> Vec4 {
        self.color
    }
}

impl GizzmoShape for SphereGizzmo {
    fn local_transform(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::splat(self.radius),
            Quat::IDENTITY,
            self.pos - Vec3::splat(self.radius / 2.0),
        )
    }
    fn color(&self) -> Vec4 {
        self.color
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
}
