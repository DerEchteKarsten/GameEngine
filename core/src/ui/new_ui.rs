use anyhow::Result;
use bevy::ecs::system::Local;
use bevy::tasks::block_on;
use bevy::window::{CursorIcon, CursorOptions, SystemCursorIcon};
use bevy::{
    ecs::{
        component::Component,
        entity::Entity,
        query::{QuerySingleError, With},
        reflect::ReflectComponent,
        resource::Resource,
        system::{Commands, If, Query, Res, ResMut, Single, SystemParam, lifetimeless},
    },
    input::{
        ButtonInput, ButtonState,
        mouse::MouseButton,
        touch::{Touch, TouchInput, Touches},
    },
    math::Rect,
    reflect::{self, Reflect},
    window::Window,
};
use bytemuck::Pod;
use bytemuck::Zeroable;
use fontdue::layout::GlyphRasterConfig;
use fontdue::*;
use glam::{I8Vec2, U8Vec2, U16Vec2, UVec2, Vec2, Vec4};
use itertools::Itertools;
use lava::{
    buffer::*,
    image::{Image, format, usage},
};
use std::f32::consts::PI;
use std::{
    collections::HashMap,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    mem::swap,
    ops::Range,
    path::PathBuf,
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
};
use tracing_log::log;

use crate::{
    bindings::{self, UIVertex},
    render::{
        FRAMES_IN_FLIGHT, MainWorld, extract_param::Extract, render::FrameCount, world::UploadQueue,
    },
};

#[derive(Reflect, Clone)]
pub struct FocusedState {
    is_being_draged: bool,
    darg_start: Vec2,

    resize_top: bool,
    resize_bottom: bool,
    resize_left: bool,
    resize_right: bool,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct UiWindow {
    pub focused: Option<FocusedState>,
    pub label: String,
    pub rect: Rect,
    pub id: u64,
    pub layer: u32,
    #[reflect(ignore)]
    pub verticies: Vec<UIVertex>,
    pub indicies: Vec<u32>,
    pub state: State,
}

const NONE: u64 = !0;

#[derive(Clone, Reflect)]
pub struct State {
    active: u64,
    hoverd: u64,
    focused: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            active: NONE,
            focused: NONE,
            hoverd: NONE,
        }
    }
}

#[derive(SystemParam)]
pub struct UiBuilder<'w, 's, Marker: Component> {
    query: Query<'w, 's, lifetimeless::Write<UiWindow>, With<Marker>>,
    window: Single<'w, 's, lifetimeless::Read<Window>>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    touch: Res<'w, Touches>,
    ctx: Res<'w, NUiContext>,
}

impl<'w, 's, Marker: Component> UiBuilder<'w, 's, Marker> {
    pub fn build<'a>(
        &mut self,
        f: impl FnOnce(UiWindowBuilder<'a>),
    ) -> Result<(), QuerySingleError> {
        let mut window = self.query.single_mut()?;
        let window: &'a mut UiWindow = unsafe { &mut *(window.as_mut() as *mut UiWindow) };
        self.build_private(window, f);
        Ok(())
    }

    fn build_private<'a>(&mut self, window: &'a mut UiWindow, f: impl FnOnce(UiWindowBuilder<'a>)) {
        let mouse: &'a ButtonInput<MouseButton> = unsafe { &*(self.mouse.as_ref() as *const _) };
        let touch: &'a Touches = unsafe { &*(self.touch.as_ref() as *const _) };
        let win: &'a Window = unsafe { &*(*self.window as *const _) };
        let ctx: &'a NUiContext = unsafe { &*(&*self.ctx as *const _) };
        window.build(mouse, ctx, win, touch, f);
    }

    pub fn build_or<'a>(&mut self, mut init: impl FnMut(), f: impl FnOnce(UiWindowBuilder<'a>)) {
        let Ok(mut window) = self.query.single_mut() else {
            init();
            return;
        };
        let window: &'a mut UiWindow = unsafe { &mut *(window.as_mut() as *mut UiWindow) };
        self.build_private(window, f)
    }
}

impl UiWindow {
    pub fn new(label: impl Into<String>) -> Self {
        let str = label.into();
        let mut hash = DefaultHasher::new();
        str.hash(&mut hash);
        let id = hash.finish();
        let pos = Vec2::new(100.0, 100.0);
        let size = Vec2::new(500.0, 500.0);


        let rect = Rect::from_corners(pos, pos + size);
        UiWindow {
            id,
            focused: None,
            label: str,
            layer: u32::MAX,
            state: State::default(),
            rect,
            verticies: Vec::new(),
            indicies: Vec::new(),
        }
    }
    pub fn build<'s, 'a, 'b, R>(
        &'s mut self,
        buttons: &'b ButtonInput<MouseButton>,
        ctx: &NUiContext,
        window: &'b Window,
        touch: &'b Touches,
        f: impl FnOnce(UiWindowBuilder<'a>) -> R,
    ) -> R
    where
        's: 'a,
        'b: 'a,
    {        
        self.state.hoverd = NONE;
        let viewport_size = window.size();

        let size = self.rect.max - self.rect.min;
        self.border_rect(
            self.rect.min,
            size,
            ctx.style.window_border,
            NUiContext::BG,
            NUiContext::S0,
            viewport_size,
            Vec2::ZERO,
            viewport_size,
        );

        let header_content_size = Vec2::new(size.x, ctx.acent + ctx.style.child_pad.y * 2.0);
        self.border_rect(
            self.rect.min,
            header_content_size,
            ctx.style.window_border,
            if self.focused.is_some() {
                NUiContext::BG
            } else {
                NUiContext::BG_DARK
            },
            NUiContext::S0,
            viewport_size,
            Vec2::ZERO,
            viewport_size,
        );
        let label = self.label.clone(); // FUCK YOU RUST
        self.text(
            ctx,
            self.rect.min + ctx.style.child_pad,
            NUiContext::TEXT,
            &label,
            viewport_size,
            self.rect.min,
            header_content_size,
        );
        let pos = self.rect.min + Vec2::new(0.0, header_content_size.y + ctx.style.window_border);

        let mut left_mouse_pressed = buttons.just_pressed(MouseButton::Left);
        let mut left_mouse_pressing = buttons.pressed(MouseButton::Left); 
        let mut left_mouse_released = buttons.just_released(MouseButton::Left);
        let cursor_pos = window.cursor_position().or_else(|| {
            let t = touch.iter().next()?;
            left_mouse_pressed = touch.just_pressed(t.id());
            left_mouse_pressing = true;
            left_mouse_released = touch.just_released(t.id());
            
            Some(t.position())
        });

        f(UiWindowBuilder {
            parent_content_size: size - Vec2::new(0.0, header_content_size.y + ctx.style.window_border),
            window: self,
            first_child: true,
            viewport_size,
            style: ctx.style.clone(),
            cursor: pos + ctx.style.child_pad,
            cursor_pos,
            left_mouse_pressed,
            left_mouse_pressing,
            left_mouse_released,
            parent_content_pos: pos,
        })
    }

    fn text(
        &mut self,
        ctx: &NUiContext,
        mut pos: Vec2,
        color: Vec4,
        text: &str,
        viewport_size: Vec2,
        parent_pos: Vec2,
        parent_size: Vec2,
    ) -> Vec2 {
        pos.y += ctx.acent;
        for char in text.chars() {
            if char == '\n' {
                pos.y += ctx.new_line_size;
                continue;
            }
            let atlas_info = ctx.atlas_lut.get(&chars[i]).cloned().unwrap_or(AtlasEntry {
                position: U16Vec2::MAX,
                ..Default::default()
            });

            let uv = (atlas_info.position.as_vec2() + Vec2::splat(0.0)) / ctx.atlas_size;
            let uv_size = atlas_info.size.as_vec2() / ctx.atlas_size;
            let size = atlas_info.size.as_vec2();
            let tpos = Vec2::new(
                pos.x,
                pos.y - atlas_info.height,
            );            
            self.rect(tpos, size, Some((uv, uv_size)), color, viewport_size, parent_size, parent_pos);
            if i != chars.len()-1{
                pos.x += (atlas_info.advance_width + ctx.atlas_lut.get(&chars[i+1]).map(|e| e.xmin).unwrap_or(0.0)).round();
            }
        }
        pos
    }   

    fn rect(
        &mut self,
        pos: Vec2,
        size: Vec2,
        uv: Option<(Vec2, Vec2)>,
        color: Vec4,
        view_port_size: Vec2,
        parent_size: Vec2,
        parent_pos: Vec2,
    ) -> Option<usize> {
        let clip_min = parent_pos;
        let clip_max = parent_pos + parent_size;

        let clipped_min = pos.max(clip_min);
        let clipped_max = (pos + size).min(clip_max);

        if clipped_min.x >= clipped_max.x || clipped_min.y >= clipped_max.y {
            return None;
        }

        let (clipped_uv_min,clipped_uv_max) = if let Some((uv, uv_size)) = uv {
            let uv_scale = uv_size / size;
            let clipped_uv_min = uv + (clipped_min - pos) * uv_scale;
            let clipped_uv_max = uv + (clipped_max - pos) * uv_scale;
            (clipped_uv_min, clipped_uv_max)
        } else {
            (Vec2::splat(0.0), Vec2::splat(0.0))
        };

        let vertex_id = self.verticies.len() as u32;
        let half_vp = view_port_size / 2.0;

        self.verticies.extend_from_slice(&[
            UIVertex {
                color,
                pos: (clipped_min / half_vp) - Vec2::splat(1.0),
                uv: clipped_uv_min,
            },
            UIVertex {
                color,
                pos: (clipped_min.with_x(clipped_max.x) / half_vp) - Vec2::splat(1.0),
                uv: clipped_uv_min.with_x(clipped_uv_max.x),
            },
            UIVertex {
                color,
                pos: (clipped_max / half_vp) - Vec2::splat(1.0),
                uv: clipped_uv_max,
            },
            UIVertex {
                color,
                pos: (clipped_min.with_y(clipped_max.y) / half_vp) - Vec2::splat(1.0),
                uv: clipped_uv_min.with_y(clipped_uv_max.y),
            },
        ]);
        self.indicies.extend_from_slice(&[
            vertex_id,
            vertex_id + 1,
            vertex_id + 2,
            vertex_id,
            vertex_id + 3,
            vertex_id + 2,
        ]);
        Some(vertex_id as usize)
    }
    fn round_rect(
        &mut self,
        pos: Vec2,
        size: Vec2,
        rounding: f32,
        color: Vec4,
        view_port_size: Vec2,
        parent_size: Vec2,
        parent_pos: Vec2,
    ) -> Option<usize> {
        let left   = (pos + Vec2::new(0.0,            rounding),          Vec2::new(rounding,               size.y - rounding * 2.0));
        let right  = (pos + Vec2::new(size.x - rounding, rounding),       Vec2::new(rounding,               size.y - rounding * 2.0));
        let top    = (pos + Vec2::new(rounding,        0.0),               Vec2::new(size.x - rounding * 2.0, rounding));
        let bottom = (pos + Vec2::new(rounding,        size.y - rounding), Vec2::new(size.x - rounding * 2.0, rounding));

        self.rect(left.0,   left.1,   None, color, view_port_size, parent_size, parent_pos);
        self.rect(right.0,  right.1,  None, color, view_port_size, parent_size, parent_pos);
        self.rect(top.0,    top.1,    None, color, view_port_size, parent_size, parent_pos);
        self.rect(bottom.0, bottom.1, None, color, view_port_size, parent_size, parent_pos);

        let rect = self.rect(
            pos + Vec2::splat(rounding),
            size - Vec2::splat(rounding * 2.0),
            None, color, view_port_size, parent_size, parent_pos,
        );

        let segments: u32 = rounding.ceil() as u32;
        let half_vp = view_port_size / 2.0;

        if let Some(rect_vertices) = rect {
            for cx in 0..2u32 {
                for cy in 0..2u32 {
                    let center_vertex = (rect_vertices as u32) + cy * 2 + cx;
                    let center_pos = self.verticies[center_vertex as usize].pos;

                    let start_angle = match (cx, cy) {
                        (0, 0) => PI,           // top-left
                        (1, 0) => 3.0 * PI / 2.0, // top-right  (if y grows downward swap these two)
                        (0, 1) => 0.0,     // bottom-left
                        _      => PI / 2.0,          // bottom-right
                    };

                    let mut prev_vertex: u32 = 0;

                    for i in 0..=segments {
                        let t = i as f32 / segments as f32;
                        let angle = start_angle + t * (PI / 2.0);

                        let offset = Vec2::new(angle.cos(), angle.sin()) * rounding;

                        let vertex = self.verticies.len() as u32;
                        self.verticies.push(UIVertex {
                            pos: center_pos + (offset / half_vp),
                            uv: Vec2::splat(20.0),
                            color,
                        });

                        if i > 0 {
                            self.indicies.extend_from_slice(&[
                                center_vertex,
                                prev_vertex,
                                vertex,
                            ]);
                        }

                        prev_vertex = vertex;
                    }
                }
            }
        }

        Some(0)
    }
    fn border_rect(
        &mut self,
        pos: Vec2,
        size: Vec2,
        border_size: f32,
        color: Vec4,
        border_color: Vec4,
        viewport_size: Vec2,
        parent_pos: Vec2,
        parent_size: Vec2,
    ) -> Option<usize> {
        self.rect(
            pos - Vec2::splat(border_size),
            size + Vec2::splat(border_size * 2.0),
            None,
            border_color,
            viewport_size,
            parent_size,
            parent_pos,
        );
        self.rect(pos, size, None, color, viewport_size, parent_size, parent_pos)
    }
}

pub struct UiWindowBuilder<'a> {
    parent_content_pos: Vec2,
    parent_content_size: Vec2,
    window: &'a mut UiWindow,
    left_mouse_pressed: bool,
    left_mouse_pressing: bool,
    left_mouse_released: bool,
    cursor_pos: Option<Vec2>,
    viewport_size: Vec2,
    cursor: Vec2,
    style: Style,
    first_child: bool,
}

impl<'a> UiWindowBuilder<'a> {
    fn rect(&mut self, id: u64, size: Vec2, color: Vec4, hover_color: Option<Vec4>, click_color: Option<Vec4>, rounding: f32, children: impl FnOnce(&mut Self)) -> bool {
        if !self.first_child {
            self.cursor.x += self.style.element_gap;
        }

        let pos = self.cursor;
        let rect_id = if rounding == 0.0 {
            self
                .window
                .rect(self.cursor, size, None, color, self.viewport_size, self.parent_content_size, self.parent_content_pos)
        }else {
            self
                .window
                .round_rect(self.cursor, size, rounding, color, self.viewport_size, self.parent_content_size, self.parent_content_pos)
        };
    
        self.first_child = true;
        let parent_content_size = self.parent_content_size;
        let parent_content_pos = self.parent_content_pos;
        
        let clip_max = (self.cursor + size).min(parent_content_size + parent_content_pos);
        let clip_size = clip_max - self.cursor;
        self.parent_content_size = clip_size;
        self.parent_content_pos = self.cursor;
        
        self.cursor += self.style.child_pad;
        if rect_id.is_some() {    
            children(self);
        }
        self.first_child = false;
        self.cursor -= self.style.child_pad;
        self.parent_content_pos = parent_content_pos;
        self.parent_content_size = parent_content_size;

        self.cursor.x += size.x;

        if let Some(mouse_pos) = self.cursor_pos && let Some(rect_id) = rect_id {
            if self.window.state.hoverd == NONE
                && Rect::from_corners(pos, clip_max).contains(mouse_pos)
            {
                self.window.state.hoverd = id;

                let pressed = self.left_mouse_pressing && self.window.focused.is_some();
                
                let color = if let Some(click_color) = click_color && pressed {
                    Some(click_color)
                }else if let Some(hover_color) = hover_color {
                    Some(hover_color)
                }else {
                    None
                };

                if let Some(color) = color {
                    for i in rect_id..rect_id + 4 {
                        self.window.verticies[i].color = color;
                    }
                }

                return self.left_mouse_pressed && self.window.focused.is_some();
            }
        }
        false
    }

    fn buttom(&mut self, label: impl AsRef<str>) -> bool {
        true
    }
}

#[derive(Resource)]
pub struct NUiContext {
    pub font: PathBuf,
    pub font_settings: FontSettings,
    pub style: Style,
    pub atlas_lut: HashMap<char, AtlasEntry>,
    pub atlas_size: Vec2,
    pub new_line_size: f32,
    pub acent: f32,
    pub decent: f32,
}

#[derive(Default, Copy, Clone)]
pub struct AtlasEntry {
    position: U16Vec2,
    size: U16Vec2,
    height: f32,
    xmin: f32,
    advance_width: f32, 
}

#[derive(Clone)]
pub struct Style {
    child_pad: Vec2,
    element_gap: f32,
    rounding: f32,
    ident_size: f32,
    window_border: f32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            element_gap: 3.0,
            child_pad: Vec2::new(4.0, 4.0),
            rounding: 1.0,
            ident_size: 21.0,
            window_border: 1.0,
        }
    }
}

#[derive(Resource)]
pub struct NUiResources {
    pub font_atlas: Image<format::R8Unorm, usage::Sampled>,
    pub verticies: [Buffer<UIVertex>; FRAMES_IN_FLIGHT],
    pub indicies: [Buffer<u32>; FRAMES_IN_FLIGHT],
    pub num_verticies: usize,
    pub num_indicies: usize,
    pub pending_verticies: Vec<UIVertex>,
    pub pending_indicies: Vec<u32>,
}

impl NUiContext {
    pub const BG: Vec4 = Vec4::new(0.155, 0.155, 0.155, 1.0); // #272727 – slightly darker bg
    pub const BG_DARK: Vec4 = Vec4::new(0.130, 0.130, 0.130, 1.0); // #212121
    pub const S0: Vec4 = Vec4::new(0.220, 0.220, 0.220, 1.0); // #383838 – buttons/frames, bigger jump from bg
    pub const S1: Vec4 = Vec4::new(0.260, 0.260, 0.260, 1.0); // #424242 – hovered
    pub const S2: Vec4 = Vec4::new(0.300, 0.300, 0.300, 1.0); // #3c3c50 – active
    pub const GRAB: Vec4 = Vec4::new(0.370, 0.370, 0.370, 1.0); // #5e5e5e
    pub const GRAB_HOT: Vec4 = Vec4::new(0.490, 0.490, 0.490, 1.0); // #7d7d7d
    pub const TEXT: Vec4 = Vec4::new(0.880, 0.880, 0.880, 1.0); // #e0e0e0
    pub const TEXT_DIM: Vec4 = Vec4::new(0.550, 0.550, 0.550, 1.0); // #8c8c8c

    pub const BLUE: Vec4 = Vec4::new(0.118, 0.565, 0.831, 1.0); // #1e90d4 – UE blue
    pub const BLUE_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.6); // UE blue dimmed
    pub const BLUE_REALY_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.35);

    pub const TRACE: Vec4 = Vec4::new(0.380, 0.380, 0.380, 1.0); // trace
    pub const DEBUG: Vec4 = Vec4::new(0.400, 0.560, 0.700, 1.0); // debug
    pub const INFO: Vec4 = Vec4::new(0.820, 0.820, 0.820, 1.0); // info
    pub const WARN: Vec4 = Vec4::new(0.980, 0.760, 0.110, 1.0); // warn
    pub const ERROR: Vec4 = Vec4::new(0.950, 0.180, 0.180, 1.0); // error

    pub const ATLAS_WIDTH: u32 = 2048;
    pub const PAD: u32 = 0;
    pub const FONTSCALE: f32 = 15.0;
    pub const DRAG_THRESHHOLD: f32 = 4.0;
    pub(crate) fn build_ui_resources(&mut self) -> Result<NUiResources> {
        let bytes = fs::read(&self.font)?;
        let font = Font::from_bytes(bytes, self.font_settings).unwrap();

        let mut chars = Vec::new();
        for (c, i) in font.chars().iter() {
            let (metrics, data) = font.rasterize_config_subpixel(GlyphRasterConfig { glyph_index: (*i).into(), px: Self::FONTSCALE, font_hash: 0 } );
            
            chars.push((
                metrics,
                data,
                *c,
            ));
        }
        chars.sort_unstable_by(|a, b| a.1.cmp(&b.1));
        let font_metrics = font
            .horizontal_line_metrics(Self::FONTSCALE)
            .unwrap();
        self.acent = font_metrics.ascent.ceil();
        self.new_line_size = font_metrics.new_line_size.ceil();
        self.decent = font_metrics.descent.floor();

        let mut atlas_row_height_prefix_sum = Vec::new();
        let mut atlas_height = Self::PAD;
        let mut row_length = 1;
        let mut row_height = Self::PAD;
        for (metrics, _, _) in &chars {
            row_length += metrics.width as u32 + Self::PAD;
            if row_length >= Self::ATLAS_WIDTH {
                row_length = metrics.width as u32 + Self::PAD;
                atlas_row_height_prefix_sum.push(atlas_height);
                atlas_height += row_height;
                row_height = 0;
            }
            row_height = row_height.max(metrics.height as u32 + Self::PAD);
        }
        atlas_row_height_prefix_sum.push(atlas_height);
        atlas_height += row_height;

        let mut atlas_data: Vec<u8> =
            Vec::with_capacity(atlas_height as usize * Self::ATLAS_WIDTH as usize);
        unsafe { atlas_data.set_len(atlas_data.capacity()) };

        for b in &mut atlas_data {
            *b = 0;
        }

        let mut row_length = 1;
        atlas_data[0] = 255;
        let mut row_index = 0;
        for (metrics, data, char) in &chars {
            if metrics.width != 0 {
                assert!(data.len() / (metrics.width * 3) == metrics.height);
            }
            if row_length + (metrics.width as u32 + Self::PAD) >= Self::ATLAS_WIDTH {
                row_length = 0;
                row_index += 1;
            }

            let row_start = atlas_row_height_prefix_sum[row_index];

            let position = U16Vec2::new(row_length as u16, row_start as u16);
            self.atlas_lut.insert(
                *char,
                AtlasEntry {
                    position,
                    size: U16Vec2::new(
                        metrics.width as u16,
                        metrics.height as u16,
                    ),
                    xmin: metrics.bounds.xmin,
                    height: (metrics.bounds.height + metrics.bounds.ymin).floor(),
                    advance_width: metrics.advance_width,
                },
            );

            for y in 0..metrics.height as u32 {
                for x in 0..metrics.width as u32 {
                    let mut accum = 0;
                    for c in 0..3 {
                        let idx = (y * metrics.width as u32 * 3 + x * 3 + c) as usize;
                        accum += data[idx] as u32;
                    }
                    atlas_data[((row_start + y) * Self::ATLAS_WIDTH + x + row_length) as usize] = (accum / 3) as u8;
                }
            }

            row_length += metrics.width as u32 + Self::PAD;
        }

        let font_atlas = Image::new(Self::ATLAS_WIDTH, atlas_height)?;
        self.atlas_size = Vec2::new(Self::ATLAS_WIDTH as f32, atlas_height as f32);
        let future = UploadQueue::push_image(atlas_data, font_atlas);
        let font_atlas = block_on(future)?;

        Ok(NUiResources {
            font_atlas,
            indicies: [Buffer::new(16 * 1024, true)?, Buffer::new(16 * 1024, true)?],
            verticies: [Buffer::new(16 * 1024, true)?, Buffer::new(16 * 1024, true)?],
            pending_indicies: Vec::with_capacity(16 * 1024),
            pending_verticies: Vec::with_capacity(16 * 1024),
            num_indicies: 0,
            num_verticies: 0,
        })
    }
}

pub fn nwrite_ui_data(mut resources: ResMut<NUiResources>, frame: Res<FrameCount>) {
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

    resources.num_indicies = resources.pending_indicies.len();
    resources.num_verticies = resources.pending_verticies.len();

    resources.pending_verticies.clear();
    resources.pending_indicies.clear();
}

pub fn create_ui_resources(
    mut cmd: Commands,
    res: Option<Res<NUiResources>>,
    mut world: ResMut<MainWorld>,
) {
    if res.is_some() {
        return;
    };
    let mut ctx = world.get_resource_mut::<NUiContext>().unwrap();
    cmd.insert_resource(ctx.build_ui_resources().unwrap());
}

pub fn nextract_ui(mut res: If<ResMut<NUiResources>>, windows: Extract<Query<&UiWindow>>) {
    for window in windows
        .iter()
        .sort_by::<&UiWindow>(|a, b| a.layer.cmp(&b.layer))
    {
        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies.extend(window.indicies.iter().map(|e| *e + vertex_offset as u32));
        res.pending_verticies.extend(window.verticies.iter());
    }
}

pub fn update_windows(
    mut windows: Query<(Entity, &mut UiWindow)>,
    desktop_window: Single<&Window>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    touch: Res<Touches>,
    mut cursor: Single<&mut bevy::window::CursorIcon>,
    ctx: Res<NUiContext>,
    mut current_touch: Local<Option<u64>>
) {
    let mut cursor_icon = CursorIcon::System(SystemCursorIcon::Default);
    let mut left_mouse_pressed = mouse_buttons.just_pressed(MouseButton::Left);
    let mut left_mouse_pressing = mouse_buttons.pressed(MouseButton::Left); 
    let mut left_mouse_released = mouse_buttons.just_released(MouseButton::Left);
    let cursor_pos = desktop_window.cursor_position().or_else(|| {
        let t = current_touch.or_else(|| touch.iter().next().map(|e| e.id()))?;

        let pos = touch.get_pressed(t);
        left_mouse_pressed = touch.just_pressed(t);
        left_mouse_pressing = pos.is_some();
        left_mouse_released = touch.any_just_released();//touch.just_released(t) || touch.just_canceled(t);
        if left_mouse_released {
            *current_touch = None;
        }
        pos.map(|e| e.position())
    });

    if left_mouse_released {
        log::info!("released");
    }

    if left_mouse_pressed {
        log::info!("pressed");
    }
    
    let mut now_focused = None;
    for (e, mut window) in windows
    .iter_mut()
    .sort_by::<&UiWindow>(|a, b| a.layer.cmp(&b.layer))
    {
        let border_rect = Rect::from_corners(window.rect.min - Vec2::splat(NUiContext::DRAG_THRESHHOLD), window.rect.max + Vec2::splat(NUiContext::DRAG_THRESHHOLD));
        if let Some(cursor_pos) = cursor_pos && border_rect.contains(cursor_pos) && matches!(now_focused, None) && !window.indicies.is_empty() {
            if left_mouse_pressed && window.focused.is_none() {
                now_focused = Some(e);
                window.focused = Some(FocusedState { resize_bottom: false, resize_left: false, resize_right: false, resize_top: false, is_being_draged: false, darg_start: Vec2::new(0.0, 0.0) });
            }
        } else if left_mouse_pressed {
            window.focused = None;
        }
    }
    if let Some(focused) = now_focused {
        let mut layers = Vec::new();
        for (entitiy, _) in
            windows
                .iter_mut()
                .sort_by::<(Entity, &UiWindow)>(|(e1, a), (e2, b)| {
                    if *e1 == focused {
                        std::cmp::Ordering::Greater
                    } else if *e2 == focused {
                        std::cmp::Ordering::Less
                    } else {
                        a.layer.cmp(&b.layer)
                    }
                })
        {
            layers.push(entitiy);
        }
        for (i, l) in layers.into_iter().enumerate() {
            windows.get_mut(l).unwrap().1.layer = i as u32;
        }
    }

    for (_, mut window) in &mut windows {
        window.indicies.clear();
        window.verticies.clear();
        let mut focused = window.focused.take();
        if let Some(focused) = &mut focused {
            let header_rect = Rect::from_corners(window.rect.min, window.rect.min + Vec2::new(window.rect.max.x, ctx.acent + ctx.style.child_pad.y * 2.0));
            
            if let Some(cursor_pos) = cursor_pos {
                if header_rect.contains(cursor_pos) {
                    if left_mouse_pressed {
                        focused.darg_start = window.rect.min - cursor_pos;
                        focused.is_being_draged = true;
                    }
                    cursor_icon = CursorIcon::System(SystemCursorIcon::Grab);
                }else if !window.rect.contains(cursor_pos) {
                    let min = window.rect.min;
                    let max = window.rect.max;
                    let t = NUiContext::DRAG_THRESHHOLD;
                    let resize_top    = Rect::from_corners(Vec2::new(min.x - t, min.y - t), Vec2::new(max.x + t, min.y + t)).contains(cursor_pos);
                    let resize_bottom = Rect::from_corners(Vec2::new(min.x - t, max.y - t), Vec2::new(max.x + t, max.y + t)).contains(cursor_pos);
                    let resize_left   = Rect::from_corners(Vec2::new(min.x - t, min.y - t), Vec2::new(min.x + t, max.y + t)).contains(cursor_pos);
                    let resize_right  = Rect::from_corners(Vec2::new(max.x - t, min.y - t), Vec2::new(max.x + t, max.y + t)).contains(cursor_pos);
                    if left_mouse_pressed {
                        focused.resize_bottom = resize_bottom;
                        focused.resize_left = resize_left;
                        focused.resize_top = resize_top;
                        focused.resize_right = resize_right;
                    }
                    cursor_icon = match (focused.resize_top || resize_top, focused.resize_bottom||resize_bottom, focused.resize_left||resize_left, focused.resize_right||resize_right) {
                        (true,  false, true,  false) => CursorIcon::System(SystemCursorIcon::NwseResize), // top-left
                        (true,  false, false, true)  => CursorIcon::System(SystemCursorIcon::NeswResize), // top-right
                        (false, true,  true,  false) => CursorIcon::System(SystemCursorIcon::NeswResize), // bottom-left
                        (false, true,  false, true)  => CursorIcon::System(SystemCursorIcon::NwseResize), // bottom-right
                        (true,  false, false, false) => CursorIcon::System(SystemCursorIcon::NsResize),   // top
                        (false, true,  false, false) => CursorIcon::System(SystemCursorIcon::NsResize),   // bottom
                        (false, false, true,  false) => CursorIcon::System(SystemCursorIcon::EwResize),   // left
                        (false, false, false, true)  => CursorIcon::System(SystemCursorIcon::EwResize),   // right
                        _ => CursorIcon::System(SystemCursorIcon::Default),
                    };
                }
            }

            if left_mouse_released {
                focused.darg_start = Vec2::ZERO;
                focused.is_being_draged = false;
                focused.resize_bottom = false;
                focused.resize_top = false;
                focused.resize_left = false;
                focused.resize_right = false;
            }else if left_mouse_pressing {
                if let Some(cursor_pos) = cursor_pos {
                    let size = window.rect.max - window.rect.min;
                    if focused.is_being_draged {
                        let drag_pos = cursor_pos + focused.darg_start;
                        window.rect.min = drag_pos;
                        window.rect.max = drag_pos + size;
                        cursor_icon = CursorIcon::System(SystemCursorIcon::Grabbing);
                    }
    
                    if focused.resize_top {
                        window.rect.min.y = (cursor_pos.y).min(window.rect.max.y - 1.0);
                    }
                    if focused.resize_bottom {
                        window.rect.max.y = (cursor_pos.y).max(window.rect.min.y + 1.0);
                    }
                    if focused.resize_left {
                        window.rect.min.x = (cursor_pos.x).min(window.rect.max.x - 1.0);
                    }
                    if focused.resize_right {
                        window.rect.max.x = (cursor_pos.x).max(window.rect.min.x + 1.0);
                    }
                }
            }
        }
        window.focused = focused;
    }

    *(*cursor) = cursor_icon;
}

#[derive(Component, Reflect)]
#[component(storage = "SparseSet")]
#[reflect(Component)]
pub struct TestWindow;

pub fn test_ui(mut cmd: Commands, mut ui: UiBuilder<TestWindow>) {
    ui.build_or(
        || {
            cmd.spawn((
                UiWindow::new("Entity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdl"),
                TestWindow,
            ));
        },
        |mut b| {
            let cc = Vec4::new(1.0, 0.0, 0.0, 1.0);
            let hv = NUiContext::BLUE;
            b.rect(
                0,
                Vec2::new(500.0, 150.0),
                Vec4::new(1.0, 0.0, 1.0, 1.0),
                None,
                None,
                0.0,
                |b| {
                    b.rect(1, Vec2::splat(100.0), Vec4::new(0.0, 1.0, 1.0, 1.0), None, Some(cc), 10.0, |_| {});
                    b.rect(2, Vec2::splat(100.0), Vec4::new(1.0, 1.0, 1.0, 1.0), None, Some(cc), 0.0, |_| {});
                    if b.rect(3, Vec2::splat(100.0), Vec4::new(0.0, 0.0, 1.0, 1.0), None, Some(cc), 0.0, |_| {}) {
                        log::info!("TEst");
                    }
                },
            );
        },
    );
}
