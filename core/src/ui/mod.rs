use ash::vk::{self, Format, ImageUsageFlags, Rect2D};
use bevy::{
    app::{App, Update},
    ecs::{
        message::MessageReader,
        resource::Resource,
        system::{Commands, NonSendMut, Res, ResMut},
    },
    input::{
        ButtonState,
        keyboard::{KeyCode, KeyboardInput},
        mouse::{MouseButton, MouseButtonInput, MouseWheel},
    },
    time::Time,
    window::{CursorMoved, WindowEvent},
};
use glam::{Mat4, Quat, UVec2, UVec4, Vec2, Vec4};
use gltf::json::extensions::mesh;
use imgui::{FontSource, Io};
use lava::{
    bindless::BindlessHandle,
    buffer::{Buffer, BufferUsageFlags, slice::BufferSlice},
    command_buffer::CommandBuffer,
    state::Ctx,
};
use lava::{
    buffer::{AsBuffer, CpuBuffer},
    image::{Image, format::R8Unorm, usage::Sampled},
};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    random::{self, random},
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    bindings::{self, UIVertex},
    render::{
        self, ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, RenderStartup,
        extract_param::Extract,
        systems::{FrameCount, Swapchain},
        world::UploadQueue,
    },
};

pub struct UiContext {
    ctx: imgui::Context,
    ui: *mut imgui::Ui,
}

impl UiContext {
    pub fn ui(&mut self) -> &mut imgui::Ui {
        unsafe { self.ui.as_mut().unwrap() }
    }
}

fn handle_key(io: &mut Io, key: &KeyCode, pressed: bool) {
    let igkey = match key {
        KeyCode::KeyA => imgui::Key::A,
        KeyCode::KeyB => imgui::Key::B,
        KeyCode::KeyC => imgui::Key::C,
        KeyCode::KeyD => imgui::Key::D,
        KeyCode::KeyE => imgui::Key::E,
        KeyCode::KeyF => imgui::Key::F,
        KeyCode::KeyG => imgui::Key::G,
        KeyCode::KeyH => imgui::Key::H,
        KeyCode::KeyI => imgui::Key::I,
        KeyCode::KeyJ => imgui::Key::J,
        KeyCode::KeyK => imgui::Key::K,
        KeyCode::KeyL => imgui::Key::L,
        KeyCode::KeyM => imgui::Key::M,
        KeyCode::KeyN => imgui::Key::N,
        KeyCode::KeyO => imgui::Key::O,
        KeyCode::KeyP => imgui::Key::P,
        KeyCode::KeyQ => imgui::Key::Q,
        KeyCode::KeyR => imgui::Key::R,
        KeyCode::KeyS => imgui::Key::S,
        KeyCode::KeyT => imgui::Key::T,
        KeyCode::KeyU => imgui::Key::U,
        KeyCode::KeyV => imgui::Key::V,
        KeyCode::KeyW => imgui::Key::W,
        KeyCode::KeyX => imgui::Key::X,
        KeyCode::KeyY => imgui::Key::Y,
        KeyCode::KeyZ => imgui::Key::Z,
        KeyCode::Digit1 => imgui::Key::Keypad1,
        KeyCode::Digit2 => imgui::Key::Keypad2,
        KeyCode::Digit3 => imgui::Key::Keypad3,
        KeyCode::Digit4 => imgui::Key::Keypad4,
        KeyCode::Digit5 => imgui::Key::Keypad5,
        KeyCode::Digit6 => imgui::Key::Keypad6,
        KeyCode::Digit7 => imgui::Key::Keypad7,
        KeyCode::Digit8 => imgui::Key::Keypad8,
        KeyCode::Digit9 => imgui::Key::Keypad9,
        KeyCode::Digit0 => imgui::Key::Keypad0,
        KeyCode::Enter => imgui::Key::Enter, // TODO: Should this be treated as alias?
        KeyCode::Escape => imgui::Key::Escape,
        KeyCode::Backspace => imgui::Key::Backspace,
        KeyCode::Tab => imgui::Key::Tab,
        KeyCode::Space => imgui::Key::Space,
        KeyCode::Minus => imgui::Key::Minus,
        KeyCode::Equal => imgui::Key::Equal,
        KeyCode::BracketLeft => imgui::Key::LeftBracket,
        KeyCode::BracketRight => imgui::Key::RightBracket,
        KeyCode::Backslash => imgui::Key::Backslash,
        KeyCode::Semicolon => imgui::Key::Semicolon,
        KeyCode::Comma => imgui::Key::Comma,
        KeyCode::Period => imgui::Key::Period,
        KeyCode::Slash => imgui::Key::Slash,
        KeyCode::CapsLock => imgui::Key::CapsLock,
        KeyCode::F1 => imgui::Key::F1,
        KeyCode::F2 => imgui::Key::F2,
        KeyCode::F3 => imgui::Key::F3,
        KeyCode::F4 => imgui::Key::F4,
        KeyCode::F5 => imgui::Key::F5,
        KeyCode::F6 => imgui::Key::F6,
        KeyCode::F7 => imgui::Key::F7,
        KeyCode::F8 => imgui::Key::F8,
        KeyCode::F9 => imgui::Key::F9,
        KeyCode::F10 => imgui::Key::F10,
        KeyCode::F11 => imgui::Key::F11,
        KeyCode::F12 => imgui::Key::F12,
        KeyCode::PrintScreen => imgui::Key::PrintScreen,
        KeyCode::ScrollLock => imgui::Key::ScrollLock,
        KeyCode::Pause => imgui::Key::Pause,
        KeyCode::Insert => imgui::Key::Insert,
        KeyCode::Home => imgui::Key::Home,
        KeyCode::PageUp => imgui::Key::PageUp,
        KeyCode::Delete => imgui::Key::Delete,
        KeyCode::End => imgui::Key::End,
        KeyCode::PageDown => imgui::Key::PageDown,
        KeyCode::ArrowRight => imgui::Key::RightArrow,
        KeyCode::ArrowLeft => imgui::Key::LeftArrow,
        KeyCode::ArrowDown => imgui::Key::DownArrow,
        KeyCode::ArrowUp => imgui::Key::UpArrow,
        KeyCode::NumpadDivide => imgui::Key::KeypadDivide,
        KeyCode::NumpadMultiply => imgui::Key::KeypadMultiply,
        KeyCode::Minus => imgui::Key::KeypadSubtract,
        KeyCode::NumpadAdd => imgui::Key::KeypadAdd,
        KeyCode::NumpadEnter => imgui::Key::KeypadEnter,
        KeyCode::Numpad1 => imgui::Key::Keypad1,
        KeyCode::Numpad2 => imgui::Key::Keypad2,
        KeyCode::Numpad3 => imgui::Key::Keypad3,
        KeyCode::Numpad4 => imgui::Key::Keypad4,
        KeyCode::Numpad5 => imgui::Key::Keypad5,
        KeyCode::Numpad6 => imgui::Key::Keypad6,
        KeyCode::Numpad7 => imgui::Key::Keypad7,
        KeyCode::Numpad8 => imgui::Key::Keypad8,
        KeyCode::Numpad9 => imgui::Key::Keypad9,
        KeyCode::Numpad0 => imgui::Key::Keypad0,
        KeyCode::NumpadDecimal => imgui::Key::KeypadDecimal,
        KeyCode::ContextMenu => imgui::Key::Menu,
        KeyCode::NumpadEqual => imgui::Key::KeypadEqual,
        KeyCode::ControlLeft => imgui::Key::LeftCtrl,
        KeyCode::ShiftLeft => imgui::Key::LeftShift,
        KeyCode::AltLeft => imgui::Key::LeftAlt,
        KeyCode::ControlRight => imgui::Key::RightCtrl,
        KeyCode::ShiftRight => imgui::Key::RightShift,
        KeyCode::AltRight => imgui::Key::RightAlt,
        KeyCode::SuperRight => imgui::Key::RightSuper,
        KeyCode::SuperLeft => imgui::Key::LeftSuper,
        _ => {
            log::error!("Unknown Key");
            // Ignore unknown keys
            return;
        }
    };
    io.add_key_event(igkey, pressed);
}

fn read_input(
    mut events: MessageReader<WindowEvent>,
    mut ctx: NonSendMut<UiContext>,
    swapchain: Res<Swapchain>,
    time: Res<Time>,
) {
    let io = ctx.ctx.io_mut();
    io.update_delta_time(time.delta());

    for event in events.read() {
        match event {
            WindowEvent::KeyboardInput(KeyboardInput {
                key_code,
                logical_key,
                repeat,
                state,
                text,
                ..
            }) => {
                if let Some(char) = text
                    && *state == ButtonState::Pressed
                {
                    let char = char.as_bytes()[0].into();
                    io.add_input_character(char);
                }
                handle_key(io, key_code, *state == ButtonState::Pressed)
            }
            WindowEvent::CursorMoved(CursorMoved {
                position, delta, ..
            }) => io.add_mouse_pos_event([position.x, position.y]),
            WindowEvent::MouseButtonInput(MouseButtonInput { button, state, .. }) => io
                .add_mouse_button_event(
                    match button {
                        MouseButton::Forward => imgui::MouseButton::Extra1,
                        MouseButton::Back => imgui::MouseButton::Extra2,
                        MouseButton::Left => imgui::MouseButton::Left,
                        MouseButton::Right => imgui::MouseButton::Right,
                        MouseButton::Middle => imgui::MouseButton::Middle,
                        _ => imgui::MouseButton::Extra1,
                    },
                    *state == ButtonState::Pressed,
                ),
            WindowEvent::MouseWheel(MouseWheel { unit, x, y, .. }) => {
                io.add_mouse_wheel_event([*x, *y]);
            }
            _ => {}
        }
    }
    io.font_global_scale = 1.0;
    io.display_size = [swapchain.size[0] as f32, swapchain.size[1] as f32];
}

fn extract_ui(
    mut world: ResMut<MainWorld>,
    mut resources: ResMut<UiResources>,
    mut queue: ResMut<UploadQueue>,
    frame: Res<FrameCount>,
) {
    let mut ctx = world.get_non_send_resource_mut::<UiContext>().unwrap();
    if resources.font_atlas.is_none() {
        let atlas = ctx.ctx.fonts().build_alpha8_texture();
        let image = Image::new(atlas.width, atlas.height).unwrap();

        resources.font_atlas = Some(image);
    }
    let draw_data = ctx.ctx.render();

    let transform = Vec2::from(draw_data.display_pos);
    let scale = Vec2::from(draw_data.display_size);
    if draw_data.draw_lists_count() == 0 {
        return;
    }
    let mut vertex_slice = resources.verticies[frame.frame_in_flight()].whole();
    let mut index_slice = resources.indicies[frame.frame_in_flight()].whole();
    for list in draw_data.draw_lists() {
        let vertex_offset = resources.verticies.len() as u32;
        let indicies = list
            .idx_buffer()
            .iter()
            .map(|i| *i as u32 + vertex_offset)
            .collect::<Vec<_>>();
        let verticies = list
            .vtx_buffer()
            .iter()
            .map(|v| UIVertex {
                color: Vec4::new(
                    v.col[0] as f32 / 255.0,
                    v.col[1] as f32 / 255.0,
                    v.col[2] as f32 / 255.0,
                    v.col[3] as f32 / 255.0,
                ),
                pos: (((Vec2::new(v.pos[0], v.pos[1]) + transform) / scale) * 2.0
                    - Vec2::splat(1.0)),
                uv: Vec2::new(v.uv[0], v.uv[1]),
            })
            .collect::<Vec<_>>();
        vertex_slice.push(BufferSlice::from(verticies.as_slice()));
        index_slice.push(BufferSlice::from(indicies.as_slice()));
    }

    ctx.ui = unsafe { ctx.ctx.new_frame() as *mut _ };
}

fn init(mut commands: Commands) {
    commands.insert_resource(UiResources {
        verticies: [Buffer::new(1000).unwrap(), Buffer::new(1000).unwrap()],
        indicies: [Buffer::new(1000).unwrap(), Buffer::new(1000).unwrap()],
        font_atlas: None,
    });
}

#[derive(Resource)]
pub struct UiResources {
    pub verticies: [Buffer<UIVertex, CpuBuffer>; FRAMES_IN_FLIGHT],
    pub indicies: [Buffer<u32, CpuBuffer>; FRAMES_IN_FLIGHT],
    pub font_atlas: Option<Image<R8Unorm, Sampled>>,
}

#[allow(non_snake_case)]
pub fn UiPlugin(app: &mut App) {
    app.add_systems(Update, read_input)
        .add_systems(RenderStartup, init)
        .add_systems(ExtractSchedule, extract_ui)
        .insert_non_send_resource({
            let mut ctx = imgui::Context::create();
            ctx.fonts()
                .add_font(&[FontSource::DefaultFontData { config: None }]);
            UiContext {
                ctx,
                ui: std::ptr::null_mut(),
            }
        });
}
