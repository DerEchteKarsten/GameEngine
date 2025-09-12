use std::fmt::Debug;

use bevy_ecs::component::Component;
use glam::*;

#[derive(Component, Clone)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ZERO,
    };
    pub fn position(position: Vec3) -> Self {
        Self {
            position,
            rotation: Quat::IDENTITY,
            scale: Vec3::ZERO,
        }
    }
    pub fn scale(scale: Vec3) -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale,
        }
    }
    pub fn rotation(rotation: Quat) -> Self {
        Self {
            position: Vec3::ZERO,
            rotation,
            scale: Vec3::ZERO,
        }
    }
    pub fn from_euler(a: f32, b: f32, c: f32) -> Self {
        Self {
            rotation: Quat::from_euler(glam::EulerRot::XYZ, a, b, c),
            position: Vec3::ZERO,
            scale: Vec3::ZERO,
        }
    }

    pub fn new(position: Vec3, scale: Vec3, rotation: Quat) -> Self {
        Self {
            position,
            rotation,
            scale,
        }
    }
    pub fn new_euler(position: Vec3, scale: Vec3, rotation: Vec3) -> Self {
        Self {
            position,
            rotation: Quat::from_euler(glam::EulerRot::XYZ, rotation.x, rotation.y, rotation.z),
            scale,
        }
    }

    pub fn from_matrix(mat: Mat4) -> Self {
        let (scale, rotation, position) = mat.to_scale_rotation_translation();
        Self {
            position,
            rotation,
            scale,
        }
    }

    pub fn as_matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

impl Debug for Transform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Position: {:?}\nRotation: {:?}\nScale: {:?}",
            self.position, self.rotation, self.scale
        )
    }
}
