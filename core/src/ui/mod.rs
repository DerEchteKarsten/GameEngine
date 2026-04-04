use bevy::{
    app::{App, PreUpdate, Update},
    ecs::{
        message::MessageReader,
        query::With,
        resource::Resource,
        schedule::IntoScheduleConfigs,
        system::{Commands, NonSendMut, Res, ResMut, Single},
        world::Mut,
    },
    input::{
        ButtonState,
        keyboard::{KeyCode, KeyboardInput},
        mouse::{MouseButton, MouseButtonInput, MouseWheel},
        touch::{TouchInput, TouchPhase},
    },
    tasks::block_on,
    time::Time,
    window::{CursorMoved, PrimaryWindow, Window, WindowEvent},
};
use bytemuck::Zeroable;
use futures::channel::oneshot;
use glam::{FloatExt, IVec2, Mat4, Quat, UVec2, UVec4, Vec2, Vec4};
use gltf::json::extensions::mesh;
use imgui::{ConfigFlags, DrawCmd, FontConfig, FontSource, Io, StyleColor};
use lava::{
    bindless::BindlessHandle,
    buffer::{Buffer, slice::BufferSlice},
    command_buffer::CommandBuffer,
    state::Ctx,
};
use lava::{
    command_buffer::Scissor,
    image::{Image, format::R8Unorm, slice::AsImage, usage::Sampled},
    state::raw_vulkan::{self, Offset2D},
    vkobjects,
};

use std::{
    collections::HashMap,
    fs,
    ops::{Deref, DerefMut},
    ptr::NonNull,
    random::{self, random},
    sync::{Arc, Mutex},
    time::Instant,
};
use tracing::info;
use tracing_log::log::{self, error};

use crate::{
    bindings::{self, UIVertex},
    render::{
        self, ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, Render, RenderApp, RenderStartup,
        RenderSystems,
        extract_param::Extract,
        render::{FrameCount, Swapchain},
        world::UploadQueue,
    },
};

#[derive(Resource)]
pub struct UiContext {
    pub(crate) ctx: imgui::Context,
}

impl UiContext {
    pub fn want_capture_mouse(&self) -> bool {
        self.ctx.io().want_capture_mouse
    }
    pub fn want_capture_keyboard(&self) -> bool {
        self.ctx.io().want_capture_keyboard
    }
    pub fn want_text_input(&self) -> bool {
        self.ctx.io().want_text_input
    }
    pub fn want_set_mouse_pos(&self) -> bool {
        self.ctx.io().want_set_mouse_pos
    }
}

unsafe impl Send for UiContext {}
unsafe impl Sync for UiContext {}

#[derive(Resource)]
pub struct UiBuilder {
    ui: *mut imgui::Ui,
}

unsafe impl Send for UiBuilder {}
unsafe impl Sync for UiBuilder {}

impl UiBuilder {
    pub fn ui(&mut self) -> Option<&mut imgui::Ui> {
        unsafe { self.ui.as_mut() }
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
            error!("Unknown Key");
            // Ignore unknown keys
            return;
        }
    };
    io.add_key_event(igkey, pressed);
}

fn read_input(
    mut events: MessageReader<WindowEvent>,
    mut ctx: ResMut<UiContext>,
    window: Single<&Window, With<PrimaryWindow>>,
    time: Res<Time>,
) {
    let size = window.size();
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
            WindowEvent::TouchInput(TouchInput {
                position, phase, ..
            }) => {
                io.add_mouse_pos_event([position.x, position.y]);
                match *phase {
                    TouchPhase::Canceled => {
                        io.add_mouse_button_event(imgui::MouseButton::Left, false);
                    }
                    TouchPhase::Started => {
                        io.add_mouse_button_event(imgui::MouseButton::Left, true);
                    }
                    TouchPhase::Ended => {
                        io.add_mouse_button_event(imgui::MouseButton::Left, false);
                    }
                    TouchPhase::Moved => {
                        // io.add_mouse_button_event(imgui::MouseButton::Left, true);
                    }
                }
            }
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
    io.display_size = size.to_array();
    io.display_framebuffer_scale = [window.scale_factor(); 2];
}

fn write_ui_data(mut resources: ResMut<UiResources>, frame: Res<FrameCount>) {
    if resources.indicies[frame.frame_in_flight()].len() < resources.pending_indicies.len() {
        resources.indicies[frame.frame_in_flight()] =
            Buffer::new(resources.pending_indicies.len().next_power_of_two(), true).unwrap();
    }
    if resources.verticies[frame.frame_in_flight()].len() < resources.pending_verticies.len() {
        resources.verticies[frame.frame_in_flight()] =
            Buffer::new(resources.pending_verticies.len().next_power_of_two(), true).unwrap();
    }

    resources.verticies[frame.frame_in_flight()]
        .range(..)
        .copy_from(&resources.pending_verticies);
    resources.indicies[frame.frame_in_flight()]
        .range(..)
        .copy_from(&resources.pending_indicies);
    resources.draw_lists[frame.frame_in_flight()].clear();
    let elements = resources.pending_draw_lists.drain(..).collect::<Vec<_>>();
    resources.draw_lists[frame.frame_in_flight()].extend(elements);
    resources.pending_verticies.clear();
    resources.pending_indicies.clear();
}

fn extract_ui(mut world: ResMut<MainWorld>, mut resources: ResMut<UiResources>) {
    world.resource_scope(|world, mut builder: Mut<UiBuilder>| {
        let mut ctx = world.get_resource_mut::<UiContext>().unwrap();
        if builder.ui().is_none() {
            info!("building font atlas");
            let atlas = ctx.ctx.fonts().build_alpha8_texture();
            let image = Image::new(atlas.width, atlas.height).unwrap();
            let mut data = Vec::with_capacity(atlas.data.len());
            data.extend_from_slice(atlas.data);
            let image = block_on(UploadQueue::push_image(data, image)).unwrap();

            resources.font_atlas = Some(image);
            ctx.ctx.style_mut().colors[StyleColor::WindowBg as usize] = [0.0; 4];
            builder.ui = ctx.ctx.new_frame() as *mut _;
            builder.ui().unwrap().dockspace_over_main_viewport();
            return;
        }
        let draw_data = ctx.ctx.render();

        let transform = Vec2::from(draw_data.display_pos);
        let scale = Vec2::from(draw_data.display_size);
        if draw_data.draw_lists_count() != 0 {
            for list in draw_data.draw_lists() {
                let vertex_offset = resources.pending_verticies.len() as u32;
                let index_offset = resources.pending_indicies.len() as u32;
                let indicies = list.idx_buffer().iter().map(|i| *i as u32);
                let verticies = list.vtx_buffer().iter().map(|v| UIVertex {
                    color: Vec4::new(
                        v.col[0] as f32 / 255.0,
                        v.col[1] as f32 / 255.0,
                        v.col[2] as f32 / 255.0,
                        v.col[3] as f32 / 255.0,
                    ),
                    pos: (((Vec2::new(v.pos[0], v.pos[1])/ scale + transform)) * 2.0
                        - Vec2::splat(1.0)),
                    uv: Vec2::new(v.uv[0], v.uv[1]),
                });
                resources.pending_indicies.extend(indicies);
                resources.pending_verticies.extend(verticies);
                for cmd in list.commands() {
                    if let DrawCmd::Elements { count, cmd_params } = cmd {
                        resources.pending_draw_lists.push(DrawList {
                            clip_rect: Scissor {
                                offset: IVec2::new(
                                    cmd_params.clip_rect[0] as i32,
                                    cmd_params.clip_rect[1] as i32,
                                ),
                                extent: UVec2::new(
                                    (cmd_params.clip_rect[2] as u32)
                                        .saturating_sub(cmd_params.clip_rect[0] as u32),
                                    (cmd_params.clip_rect[3] as u32)
                                        .saturating_sub(cmd_params.clip_rect[1] as u32),
                                ),
                            },
                            start_index: cmd_params.idx_offset as u32 + index_offset,
                            start_vertex: cmd_params.vtx_offset as u32 + vertex_offset,
                            count: count as u32,
                        });
                    }
                }
            }
        }
        ctx.ctx.style_mut().colors[StyleColor::WindowBg as usize] = [0.0; 4];
        builder.ui = ctx.ctx.new_frame() as *mut _;
        builder.ui().unwrap().dockspace_over_main_viewport();
    })
}

fn init(mut commands: Commands) {
    commands.insert_resource(UiResources {
        verticies: [
            Buffer::new(10000, true).unwrap(),
            Buffer::new(10000, true).unwrap(),
        ],
        indicies: [
            Buffer::new(10000, true).unwrap(),
            Buffer::new(10000, true).unwrap(),
        ],
        font_atlas: None,
        pending_indicies: Vec::new(),
        pending_verticies: Vec::new(),
        draw_lists: [Vec::new(), Vec::new()],
        pending_draw_lists: Vec::new(),
    });
}

#[derive(Copy, Clone)]
pub struct DrawList {
    pub count: u32,
    pub start_vertex: u32,
    pub start_index: u32,
    pub clip_rect: Scissor,
}

#[derive(Resource)]
pub struct UiResources {
    pub draw_lists: [Vec<DrawList>; FRAMES_IN_FLIGHT],
    pub pending_draw_lists: Vec<DrawList>,
    pub verticies: [Buffer<UIVertex>; FRAMES_IN_FLIGHT],
    pub pending_verticies: Vec<UIVertex>,
    pub indicies: [Buffer<u32>; FRAMES_IN_FLIGHT],
    pub pending_indicies: Vec<u32>,
    pub font_atlas: Option<Image<R8Unorm, Sampled>>,
}

#[allow(non_snake_case)]
pub fn UiPlugin(app: &mut App) {
    let sub_app = app.get_sub_app_mut(RenderApp).unwrap();
    sub_app
        .add_systems(RenderStartup, init)
        .add_systems(Render, write_ui_data.in_set(RenderSystems::PreRender))
        .add_systems(ExtractSchedule, extract_ui);
    app.add_systems(PreUpdate, read_input)
        .insert_resource({
            let mut ctx = imgui::Context::create();
            ctx.io_mut().config_flags |= ConfigFlags::DOCKING_ENABLE;
            ctx.fonts().add_font(&[FontSource::TtfData {
                data: &fs::read("/home/karsten/code/GameEngine/editor_font.ttf").unwrap(),
                size_pixels: 18.0,  
                config: Some(FontConfig {
                    pixel_snap_h: true,
                    ..Default::default()
                }),
            }]);

            let style = ctx.style_mut();
            let colors = &mut style.colors;
            // Neutrals
            let bg =          [0.155, 0.155, 0.155, 1.0]; // #272727 – slightly darker bg
            let bg_dark =     [0.130, 0.130, 0.130, 1.0]; // #212121
            let s0 =          [0.220, 0.220, 0.220, 1.0]; // #383838 – buttons/frames, bigger jump from bg
            let s1 =          [0.260, 0.260, 0.260, 1.0]; // #424242 – hovered
            let s2 =          [0.300, 0.300, 0.300, 1.0]; // #3c3c50 – active
            let grab =        [0.370, 0.370, 0.370, 1.0]; // #5e5e5e
            let grab_hot =    [0.490, 0.490, 0.490, 1.0]; // #7d7d7d
            let text =        [0.880, 0.880, 0.880, 1.0]; // #e0e0e0
            let text_dim =    [0.550, 0.550, 0.550, 1.0]; // #8c8c8c

            // Accents
            let blue =        [0.118, 0.565, 0.831, 1.0]; // #1e90d4 – UE blue
            let blue_dim =    [0.118, 0.565, 0.831, 0.6]; // UE blue dimmed
            let blue_realy_dim = [0.118, 0.565, 0.831, 0.35];

            colors[StyleColor::WindowBg as usize]              = bg;
            colors[StyleColor::ChildBg as usize]               = bg;
            colors[StyleColor::PopupBg as usize]               = s0;
            colors[StyleColor::Border as usize]                = s1;
            colors[StyleColor::BorderShadow as usize]          = [0.0, 0.0, 0.0, 0.0];
            colors[StyleColor::FrameBg as usize]               = s0;
            colors[StyleColor::FrameBgHovered as usize]        = s1;
            colors[StyleColor::FrameBgActive as usize]         = s2;
            colors[StyleColor::TitleBg as usize]               = bg_dark;
            colors[StyleColor::TitleBgActive as usize]         = bg;
            colors[StyleColor::TitleBgCollapsed as usize]      = bg_dark;
            colors[StyleColor::MenuBarBg as usize]             = bg_dark;
            colors[StyleColor::ScrollbarBg as usize]           = bg;
            colors[StyleColor::ScrollbarGrab as usize]         = grab;
            colors[StyleColor::ScrollbarGrabHovered as usize]  = grab_hot;
            colors[StyleColor::ScrollbarGrabActive as usize]   = text_dim;
            colors[StyleColor::CheckMark as usize]             = blue;
            colors[StyleColor::SliderGrab as usize]            = blue_dim;
            colors[StyleColor::SliderGrabActive as usize]      = blue; // brighter blue
            colors[StyleColor::Button as usize]                = s0;
            colors[StyleColor::ButtonHovered as usize]         = s1;
            colors[StyleColor::ButtonActive as usize]          = s2;
            colors[StyleColor::Header as usize]                = s0;
            colors[StyleColor::HeaderHovered as usize]         = s1;
            colors[StyleColor::HeaderActive as usize]          = s2;
            colors[StyleColor::Separator as usize]             = s1;
            colors[StyleColor::SeparatorHovered as usize]      = blue_dim;
            colors[StyleColor::SeparatorActive as usize]       = blue;
            colors[StyleColor::ResizeGrip as usize]            = s2;
            colors[StyleColor::ResizeGripHovered as usize]     = blue_dim;
            colors[StyleColor::ResizeGripActive as usize]      = blue;
            colors[StyleColor::Tab as usize]                   = bg_dark;
            colors[StyleColor::TabHovered as usize]            = blue;
            colors[StyleColor::TabActive as usize]             = blue_dim;
            colors[StyleColor::TabUnfocused as usize]          = bg_dark;
            colors[StyleColor::TabUnfocusedActive as usize]    = s0;
            colors[StyleColor::DockingPreview as usize]        = blue_dim;
            colors[StyleColor::DockingEmptyBg as usize]        = bg_dark;
            colors[StyleColor::PlotLines as usize]             = blue_dim;
            colors[StyleColor::PlotLinesHovered as usize]      = blue;
            colors[StyleColor::PlotHistogram as usize]         = blue_dim;
            colors[StyleColor::PlotHistogramHovered as usize]  = blue;
            colors[StyleColor::TableHeaderBg as usize]         = bg_dark;
            colors[StyleColor::TableBorderStrong as usize]     = s1;
            colors[StyleColor::TableBorderLight as usize]      = s0;
            colors[StyleColor::TableRowBg as usize]            = [0.0, 0.0, 0.0, 0.0];
            colors[StyleColor::TableRowBgAlt as usize]         = [1.0, 1.0, 1.0, 0.04];
            colors[StyleColor::TextSelectedBg as usize]        = blue_realy_dim; // blue selection
            colors[StyleColor::DragDropTarget as usize]        = blue;
            colors[StyleColor::NavHighlight as usize]          = blue;
            colors[StyleColor::NavWindowingHighlight as usize] = [1.0, 1.0, 1.0, 0.7];
            colors[StyleColor::NavWindowingDimBg as usize]     = [0.0, 0.0, 0.0, 0.2];
            colors[StyleColor::ModalWindowDimBg as usize]      = [0.0, 0.0, 0.0, 0.45];
            colors[StyleColor::Text as usize]                  = text;
            colors[StyleColor::TextDisabled as usize]          = text_dim;

            // Rounded corners
            style.window_rounding = 3.0;
            style.child_rounding = 1.0;
            style.frame_rounding = 1.0;
            style.popup_rounding = 1.0;
            style.scrollbar_rounding = 1.0;
            style.grab_rounding = 1.0;
            style.tab_rounding = 1.0;

            // Padding and spacing
            style.window_padding = [8.0, 8.0];
            style.frame_padding = [5.0, 3.0];
            style.item_spacing = [8.0, 4.0];
            style.item_inner_spacing = [4.0, 4.0];
            style.indent_spacing = 21.0;
            style.scrollbar_size = 14.0;
            style.grab_min_size = 10.0;

            // Borders
            style.window_border_size = 1.0;
            style.child_border_size = 1.0;
            style.popup_border_size = 1.0;
            style.frame_border_size = 1.0;
            style.tab_border_size = 0.0;
            UiContext { ctx }
        })
        .insert_resource(UiBuilder {
            ui: std::ptr::null_mut(),
        });
}
