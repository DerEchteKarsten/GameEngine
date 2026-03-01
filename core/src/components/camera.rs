use glam::{Mat4, Vec3, vec3};
use std::f32::consts::PI;

use bevy::{
    input::{
        ButtonState,
        mouse::{MouseButtonInput, MouseMotion},
    },
    prelude::*,
};
use lava::state::Ctx;

const MOVE_SPEED: f32 = 1.0;
const SENSITIVITY: f32 = 0.5;

const UP: Vec3 = vec3(0.0, 1.0, 0.0);

#[derive(Debug, Clone, Copy, PartialEq, Component, Resource)]
pub struct Camera {
    pub position: Vec3,
    pub direction: Vec3,
    pub fov: f32,
    pub z_near: f32,
    pub z_far: f32,
}

impl Camera {
    pub fn new(position: Vec3, fov: f32, z_near: f32, z_far: f32) -> Self {
        Self {
            position,
            direction: Vec3::default(),
            fov,
            z_near,
            z_far,
        }
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.position + self.direction, UP)
    }

    pub fn projection_matrix(&self, size: Vec2) -> Mat4 {
        Mat4::perspective_rh(self.fov, size.x / size.y, self.z_far, self.z_near)
    }
}

#[derive(Default, Debug, Clone, Copy, Resource)]
pub struct Controls {
    pub go_forward: bool,
    pub go_backward: bool,
    pub strafe_right: bool,
    pub strafe_left: bool,
    pub go_up: bool,
    pub go_down: bool,
    pub look_around: bool,
    pub pitch: f32,
    pub yaw: f32,
    pub cursor_position: [f64; 2],
}

pub fn update_mouse_buttons(
    mut controls: ResMut<Controls>,
    mut mousebtn_evr: MessageReader<MouseButtonInput>,
) {
    for ev in mousebtn_evr.read() {
        if ev.button == MouseButton::Right && ev.state == ButtonState::Pressed {
            controls.look_around = true;
            // window.cursor_options.grab_mode = CursorGrabMode::Confined;
            // window.cursor_options.visible = false;
        }
        if ev.button == MouseButton::Right && ev.state == ButtonState::Released {
            controls.look_around = false;
            // window.cursor_options.grab_mode = CursorGrabMode::None;
            // window.cursor_options.visible = true;
        }
    }
}
pub fn update_mouse_move(
    mut controls: ResMut<Controls>,
    mut evr_motion: MessageReader<MouseMotion>,
    keys: Res<ButtonInput<KeyCode>>,
    window: Single<&Window>,
) {
    if keys.pressed(KeyCode::ArrowUp) {
        controls.pitch -= 0.01 * SENSITIVITY;
    }
    if keys.pressed(KeyCode::ArrowDown) {
        controls.pitch += 0.01 * SENSITIVITY;
    }
    if keys.pressed(KeyCode::ArrowLeft) {
        controls.yaw -= 0.01 * SENSITIVITY;
    }
    if keys.pressed(KeyCode::ArrowRight) {
        controls.yaw += 0.01 * SENSITIVITY;
    }

    let size = window.size();
    if controls.look_around {
        for ev in evr_motion.read() {
            controls.pitch += (ev.delta.y / size.y) * SENSITIVITY;
            controls.yaw += (ev.delta.x / size.x) * SENSITIVITY;
        }
    }

    if controls.pitch < -PI / 2.0 + 0.1 {
        controls.pitch = -PI / 2.0 + 0.1;
    }
    if controls.pitch > PI / 2.0 - 0.1 {
        controls.pitch = PI / 2.0 - 0.1;
    }
}

pub fn update_keyboard(keys: Res<ButtonInput<KeyCode>>, mut controls: ResMut<Controls>) {
    controls.go_forward = keys.pressed(KeyCode::KeyW);
    controls.go_backward = keys.pressed(KeyCode::KeyS);
    controls.strafe_right = keys.pressed(KeyCode::KeyD);
    controls.strafe_left = keys.pressed(KeyCode::KeyA);
    controls.go_up = keys.pressed(KeyCode::Space);
    controls.go_down = keys.pressed(KeyCode::ShiftLeft);
}

pub fn editor_camera(mut query: Query<&mut Camera>, controls: Res<Controls>, time: Res<Time>) {
    let delta_time = time.delta_secs();
    for mut camera in &mut query {
        let side = camera.direction.cross(UP);

        camera.direction.x = controls.yaw.cos() * controls.pitch.cos();
        camera.direction.y = controls.pitch.sin();
        camera.direction.z = controls.yaw.sin() * controls.pitch.cos();

        // Update position
        let mut direction = Vec3::ZERO;

        if controls.go_forward {
            direction += camera.direction;
        }
        if controls.go_backward {
            direction -= camera.direction;
        }
        if controls.strafe_right {
            direction += side;
        }
        if controls.strafe_left {
            direction -= side;
        }
        if controls.go_up {
            direction -= UP;
        }
        if controls.go_down {
            direction += UP;
        }

        let direction = if direction.length_squared() == 0.0 {
            direction
        } else {
            direction.normalize()
        };

        camera.position += direction * MOVE_SPEED * delta_time;
    }
}

// fn reset(mut controls: ResMut<Controls>) {
//     controls.cursor_delta = [0.0; 2];
// }

#[allow(non_snake_case)]
pub fn CameraPlugin(app: &mut App) {
    app.init_resource::<Controls>()
        .add_systems(
            PreUpdate,
            (update_mouse_move, update_mouse_buttons, update_keyboard),
        )
        .add_systems(Update, editor_camera);
}
