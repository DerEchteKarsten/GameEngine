use ash::vk::{Format, ImageUsageFlags};
use bevy_app::{App, PreUpdate, Startup, Update};
use bevy_ecs::{
    component::Component, event::EventReader, query::With, resource::Resource, system::{Commands, Query, Res, ResMut, Single}
};
use bevy_ecs::schedule::IntoScheduleConfigs;
use bevy_input::{
    keyboard::{Key, KeyCode, KeyboardInput},
    mouse::{MouseButton, MouseButtonInput, MouseScrollUnit, MouseWheel},
};
use bevy_time::Time;
use bevy_window::{
    CursorLeft, CursorMoved, PrimaryWindow, Window, WindowEvent, WindowFocused, WindowResized,
    WindowTheme, WindowThemeChanged,
};
use egui::{MouseWheelUnit, PointerButton, Pos2, RawInput, TextureId, epaint::{Primitive, RectShape}};
use glam::{UVec2, UVec4, Vec4};
use gltf::json::extensions::mesh;
use lava::{bindless::BindlessHandle, command_buffer::CommandBuffer, state::Ctx, vkobjects::{buffer::{BufferUsageFlags, CpuBuffer, StorageBuffer}, image::{Image, ImageSize}}};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut}, random::{self, random},
};

use crate::{bindings::{self, Meshlet, UIVertex}, world::StagingBuffer};

#[derive(Resource)]
pub struct EguiContext {
    ctx: egui::Context,
}

#[derive(Default, Resource)]
pub struct Input {
    input: RawInput,
}

#[inline(always)]
pub fn bevy_to_egui_key(key: &KeyCode, str: Option<&str>) -> Option<egui::Key> {
    if let Some(str) = str {
        return egui::Key::from_name(str);
    }
    let key = match key {
        KeyCode::Unidentified(_) => return None,

        KeyCode::Enter => egui::Key::Enter,
        KeyCode::Tab => egui::Key::Tab,
        KeyCode::Space => egui::Key::Space,
        KeyCode::ArrowDown => egui::Key::ArrowDown,
        KeyCode::ArrowLeft => egui::Key::ArrowLeft,
        KeyCode::ArrowRight => egui::Key::ArrowRight,
        KeyCode::ArrowUp => egui::Key::ArrowUp,
        KeyCode::End => egui::Key::End,
        KeyCode::Home => egui::Key::Home,
        KeyCode::PageDown => egui::Key::PageDown,
        KeyCode::PageUp => egui::Key::PageUp,
        KeyCode::Backspace => egui::Key::Backspace,
        KeyCode::Delete => egui::Key::Delete,
        KeyCode::Insert => egui::Key::Insert,
        KeyCode::Escape => egui::Key::Escape,
        KeyCode::F1 => egui::Key::F1,
        KeyCode::F2 => egui::Key::F2,
        KeyCode::F3 => egui::Key::F3,
        KeyCode::F4 => egui::Key::F4,
        KeyCode::F5 => egui::Key::F5,
        KeyCode::F6 => egui::Key::F6,
        KeyCode::F7 => egui::Key::F7,
        KeyCode::F8 => egui::Key::F8,
        KeyCode::F9 => egui::Key::F9,
        KeyCode::F10 => egui::Key::F10,
        KeyCode::F11 => egui::Key::F11,
        KeyCode::F12 => egui::Key::F12,
        KeyCode::F13 => egui::Key::F13,
        KeyCode::F14 => egui::Key::F14,
        KeyCode::F15 => egui::Key::F15,
        KeyCode::F16 => egui::Key::F16,
        KeyCode::F17 => egui::Key::F17,
        KeyCode::F18 => egui::Key::F18,
        KeyCode::F19 => egui::Key::F19,
        KeyCode::F20 => egui::Key::F20,

        _ => return None,
    };
    Some(key)
}

#[inline(always)]
fn egui_mouse_button(button: &MouseButton) -> Option<egui::PointerButton> {
    match button {
        MouseButton::Left => Some(PointerButton::Primary),
        MouseButton::Right => Some(PointerButton::Secondary),
        MouseButton::Middle => Some(PointerButton::Middle),
        MouseButton::Forward => Some(PointerButton::Extra1),
        MouseButton::Back => Some(PointerButton::Extra2),
        MouseButton::Other(_) => None,
    }
}

fn read_input(
    mut events: EventReader<WindowEvent>,
    mut input: ResMut<Input>,
    time: Res<Time>,
    windows: Single<&Window, With<PrimaryWindow>>,
) {
    input.input.time = Some(time.elapsed_secs_f64());
    for event in events.read() {
        match event {
            WindowEvent::WindowFocused(WindowFocused { focused, .. }) => {
                input.input.focused = *focused
            }
            WindowEvent::WindowThemeChanged(WindowThemeChanged { theme, .. }) => {
                input.input.system_theme = Some(match theme {
                    WindowTheme::Dark => egui::Theme::Dark,
                    WindowTheme::Light => egui::Theme::Light,
                })
            }
            WindowEvent::KeyboardInput(KeyboardInput {
                key_code,
                logical_key,
                repeat,
                state,
                text,
                ..
            }) => {
                if *key_code == KeyCode::ControlLeft || *key_code == KeyCode::ControlRight {
                    input.input.modifiers.ctrl = true;
                    input.input.modifiers.command = true;
                    input.input.modifiers.mac_cmd = true;
                } else if *key_code == KeyCode::AltLeft || *key_code == KeyCode::AltRight {
                    input.input.modifiers.alt = true;
                }
                if let Some(key) = bevy_to_egui_key(key_code, text.as_ref().map(|str| str.as_str()))
                {
                    let modifiers = input.input.modifiers;
                    input.input.events.push(egui::Event::Key {
                        key,
                        physical_key: None,
                        pressed: state.is_pressed(),
                        repeat: *repeat,
                        modifiers,
                    });
                }
            }
            WindowEvent::CursorMoved(CursorMoved {
                position, delta, ..
            }) => input
                .input
                .events
                .push(egui::Event::PointerMoved(egui::Pos2 {
                    x: position.x,
                    y: position.y,
                })),
            WindowEvent::MouseButtonInput(MouseButtonInput { button, state, .. }) => {
                if let Some(button) = egui_mouse_button(&button) {
                    let pos = windows
                        .cursor_position()
                        .map(|v| Pos2::new(v.x, v.y))
                        .unwrap_or(Pos2::new(f32::MAX, f32::MAX));
                    let modifiers = input.input.modifiers;
                    input.input.events.push(egui::Event::PointerButton {
                        button,
                        modifiers: modifiers,
                        pos,
                        pressed: state.is_pressed(),
                    });
                }
            }
            WindowEvent::CursorLeft(CursorLeft { .. }) => {
                input.input.events.push(egui::Event::PointerGone)
            }
            WindowEvent::MouseWheel(MouseWheel { unit, x, y, .. }) => {
                let modifiers = input.input.modifiers;
                input.input.events.push(egui::Event::MouseWheel {
                    unit: match unit {
                        MouseScrollUnit::Line => MouseWheelUnit::Line,
                        MouseScrollUnit::Pixel => MouseWheelUnit::Point,
                    },
                    delta: egui::Vec2 { x: *x, y: *y },
                    modifiers,
                })
            }
            _ => {}
        }
    }
}

fn update_ui(mut input: ResMut<Input>, ctx: Res<EguiContext>, mut resources: ResMut<UiResources>, mut staging_buffer: ResMut<StagingBuffer>) {
    if !lava::is_init() {
        return;
    }
    let full_output = ctx.ctx.run(input.input.clone(), |ctx| {
        egui::CentralPanel::default().show(&ctx, |ui| {
            ui.label("Test");
            if ui.button("Press Me!").clicked() {
                println!("Hello World!");
            }
        });
    });
    input.input.events.clear();

    for f in &full_output.textures_delta.free {
        if let Some(image) = resources.texture_map.remove(f) {
            image.destroy();
        }
    }

    for (f, texture) in &full_output.textures_delta.set {
        let egui::ImageData::Color(image_data) = &texture.image;
        let image = Image::new_2d(ImageUsageFlags::SAMPLED, Format::R8G8B8A8_SNORM, ImageSize::XY(image_data.size[0] as u32, image_data.size[1] as u32)).unwrap();
        staging_buffer.copy_from_slice(image_data.as_raw(), 0).unwrap();
        Ctx::transfer_queue().execute_command_wait(|cmd| unsafe{
            let regions = [
                ash::vk::BufferImageCopy {
                    buffer_image_height: image_data.size[1] as u32,
                    buffer_offset: 0,
                    buffer_row_length: image_data.size[0] as u32,
                    image_extent: ash::vk::Extent3D { width: image_data.size[0] as u32, height: image_data.size[1] as u32, depth: 1 },
                    image_offset: ash::vk::Offset3D { x: 0, y: 0, z: 0 },
                    image_subresource: ash::vk::ImageSubresourceLayers { aspect_mask: ash::vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 1, layer_count: 0 }
                }
            ];
            Ctx::device().cmd_copy_buffer_to_image(*cmd, staging_buffer.handle, image.handle, ash::vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, &regions);
        }).unwrap();
        resources.texture_map.insert(*f, image);
    }

    let clipped_primitives = ctx
        .ctx
        .tessellate(full_output.shapes, full_output.pixels_per_point);

    let mut num_verticies = 0;
    let mut num_indicies = 0;

    let meshes = clipped_primitives.into_iter().filter_map(|prim| if let Primitive::Mesh(mesh) = prim.primitive {
        num_verticies += mesh.vertices.len();
        num_indicies += mesh.indices.len();        
        Some(mesh)
    }else {
        None
    }).collect::<Vec<_>>();

    resources.verticies.clear();
    resources.indicies.clear();
    resources.verticies.assert_size(num_verticies as u64 * size_of::<egui::epaint::Vertex>() as u64).unwrap();
    resources.indicies.assert_size(num_verticies as u64 * size_of::<u32>() as u64).unwrap();
    for (i, mesh) in meshes.iter().enumerate() {
        let vertex_offset = resources.verticies.len() as u32;
        let texture_index = resources.texture_map.get(&mesh.texture_id).unwrap().bindless_handle.unwrap();
        let indicies = mesh.indices.iter().map(|i| i+vertex_offset).collect::<Vec<_>>();
        let verticies = mesh.vertices.iter().map(|v| UIVertex {
            pos_and_uv: glam::Vec4::new(-1.0 + (v.pos.x / Ctx::window_width().unwrap() as f32)*2.0, -1.0 + (v.pos.y / Ctx::window_height().unwrap() as f32)*2.0, v.uv.x, v.uv.y),
            color: Vec4::new(v.color.r() as f32 / 255.0, v.color.g() as f32 / 255.0, v.color.b() as f32 / 255.0, v.color.a() as f32 / 255.0),
            texture_index,
        }).collect::<Vec<_>>();
        
        resources.verticies.push(&verticies);
        resources.indicies.push(&indicies);
    }
}

fn init(mut commands: Commands) {
    commands.insert_resource(UiResources {
        indicies: StorageBuffer::new(BufferUsageFlags::INDEX).unwrap(),
        verticies: StorageBuffer::default(),
        texture_map: HashMap::new(),
    });
}

#[derive(Resource)]
pub struct UiResources {
    pub verticies: StorageBuffer<bindings::UIVertex, CpuBuffer>,
    pub indicies: StorageBuffer<u32, CpuBuffer>,
    pub texture_map: HashMap<TextureId, Image>,
}

#[allow(non_snake_case)]
pub fn UiPlugin(app: &mut App) {
    app.add_systems(PreUpdate, (read_input, update_ui.after(read_input).after(init)))
        .add_systems(Startup, init)
        .insert_resource(EguiContext {
            ctx: egui::Context::default(),
        })
        .init_resource::<Input>();
}
