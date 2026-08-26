use std::f32::consts::PI;

use bevy::{
    ecs::{
        component::Component,
        query::With,
        resource::Resource,
        system::{Local, Query, Res, Single},
    },
    input::{
        ButtonInput,
        keyboard::KeyCode,
        mouse::{AccumulatedMouseMotion, MouseButton},
    },
    reflect::Reflect,
    time::Time,
    transform::components::Transform,
    window::{CursorGrabMode, CursorOptions},
};
use glam::{Quat, Vec3};

use crate::{editor::viewport::ViewPortProxy, scene::camera::Camera};

#[derive(Default, Debug, Clone, Copy, Resource)]
pub struct CameraSettings {
    pub move_speed: f32,
    pub sensitivity: f32,
    pub keyboard_sensitivity: f32,
}
use bevy::ecs::reflect::ReflectComponent;

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct EditorCamera;

pub fn update_camera(
    settings: Res<CameraSettings>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    vp: ViewPortProxy,
    mut camera: Query<(&mut Camera, &mut Transform), With<EditorCamera>>,
    time: Res<Time>,
    mut camera_rotation: Local<(f32, f32)>,
    mut cursor: Single<&mut CursorOptions>,
) {
    let delta_time = time.delta_secs();
    let Ok((_camera, mut transform)) = camera.single_mut() else {
        return;
    };

    let (pitch, yaw) = &mut *camera_rotation;
    if vp.focused() {
        if keys.pressed(KeyCode::ArrowUp) {
            *pitch += settings.keyboard_sensitivity * delta_time;
        }
        if keys.pressed(KeyCode::ArrowDown) {
            *pitch -= settings.keyboard_sensitivity * delta_time;
        }
        if keys.pressed(KeyCode::ArrowLeft) {
            *yaw -= settings.keyboard_sensitivity * delta_time;
        }
        if keys.pressed(KeyCode::ArrowRight) {
            *yaw += settings.keyboard_sensitivity * delta_time;
        }
        let size = vp.size();
        if mouse_buttons.pressed(MouseButton::Right) {
            *pitch -= mouse_motion.delta.y / size.y as f32 * settings.sensitivity;
            *yaw += mouse_motion.delta.x / size.x as f32 * settings.sensitivity;
            cursor.grab_mode = CursorGrabMode::Locked;
            cursor.visible = false;
        } else {
            cursor.grab_mode = CursorGrabMode::None;
            cursor.visible = true;
        }
    }

    *pitch = pitch.clamp(
        -std::f32::consts::FRAC_PI_2 + 0.001,
        std::f32::consts::FRAC_PI_2 - 0.001,
    );

    *yaw = yaw.rem_euclid(2.0 * PI);
    transform.rotation =
        Quat::from_axis_angle(Vec3::Y, *yaw) * Quat::from_axis_angle(Vec3::X, *pitch);

    if vp.focused() {
        let mut direction = Vec3::ZERO;
        if keys.pressed(KeyCode::KeyW) {
            direction += *transform.forward();
        }
        if keys.pressed(KeyCode::KeyS) {
            direction += *transform.back();
        }
        if keys.pressed(KeyCode::KeyA) {
            direction += *transform.right();
        }
        if keys.pressed(KeyCode::KeyD) {
            direction += *transform.left();
        }
        if keys.pressed(KeyCode::Space) {
            direction += Vec3::Y;
        }
        if keys.pressed(KeyCode::ShiftLeft) {
            direction += Vec3::NEG_Y;
        }

        let direction = if direction.length_squared() == 0.0 {
            direction
        } else {
            direction.normalize()
        };

        transform.translation += direction * settings.move_speed * delta_time;
    }
}
