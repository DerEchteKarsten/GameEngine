use anyhow::Result;
use bevy::ecs::message::MessageReader;
use bevy::ecs::system::Local;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::math::VectorSpace;
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
use glam::{I8Vec2, IVec2, U8Vec2, U16Vec2, UVec2, Vec2, Vec2Swizzles, Vec4};
use itertools::Itertools;
use lava::{
    buffer::*,
    image::{Image, format, usage},
};
use smallvec::SmallVec;
use std::f32::consts::PI;
use std::num::{NonZero, NonZeroU32, NonZeroU64};
use std::ops::RangeBounds;
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
    draging: Option<NonZeroU64>,
    focused: Option<NonZeroU64>,
    cursor: usize,
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
}

#[derive(SystemParam)]
pub struct UiBuilder<'w, 's, Marker: Component> {
    query: Query<'w, 's, lifetimeless::Write<UiWindow>, With<Marker>>,
    window: Single<'w, 's, lifetimeless::Read<Window>>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    touch: Res<'w, Touches>,
    ctx: Res<'w, NUiContext>,
    keys: MessageReader<'w, 's, KeyboardInput>,
}

impl<'s, 'w, Marker: Component> UiBuilder<'w, 's, Marker>{
    pub fn build(
        &mut self,
        f: impl FnOnce(UiWindowBuilder<'_, 'w>),
    ) -> Result<(), QuerySingleError> {
        let mut window = self.query.single_mut()?;
        let mouse = Res::clone(&self.mouse);
        let ctx = Res::clone(&self.ctx);
        let touch = Res::clone(&self.touch);
        let keys = self.keys.read().cloned().collect::<SmallVec<[KeyboardInput; 8]>>();
        window.build(mouse, ctx, &self.window, touch, keys, f);
        Ok(())
    }

    pub fn build_or(&mut self, mut init: impl FnMut(), f: impl FnOnce(UiWindowBuilder<'_, 'w>)) {
        let Ok(mut window) = self.query.single_mut() else {
            init();
            return;
        };
        let mouse = Res::clone(&self.mouse);
        let ctx = Res::clone(&self.ctx);
        let touch = Res::clone(&self.touch);
        let keys = self.keys.read().cloned().collect::<SmallVec<[KeyboardInput; 8]>>();
        window.build(mouse, ctx, &self.window, touch, keys, f);
    }
}

#[derive(Copy, Clone)]
pub struct BorderSettings {
    color_top: Vec4,
    color_bottom: Vec4,
    color_left: Vec4,
    color_right: Vec4,
    size: u32,
}

impl BorderSettings {
    pub fn uniform(color: Vec4, size: u32) -> Self {
        Self {
            color_top: color,
            color_bottom: color,
            color_left: color,
            color_right: color,
            size,
        }
    }
}

#[derive(Copy, Clone)]
pub struct DrawSettings {
    color: Vec4,

    rounding: u32,
    round_topleft: bool,
    round_topright: bool,
    round_bottomleft: bool,
    round_bottomright: bool,

    border: Option<BorderSettings>,
}

impl Default for DrawSettings {
    fn default() -> Self {
        Self {
            border: None,
            color: NUiContext::S0,
            round_bottomleft: false,
            round_bottomright: false,
            round_topleft: false,
            round_topright: false,
            rounding: 0,
        }
    }
}

impl DrawSettings {
    pub fn all_rounded(mut self) -> Self {
        self.round_topleft = true;
        self.round_topright = true;
        self.round_bottomleft = true;
        self.round_bottomright = true;
        self
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
            rect,
            verticies: Vec::new(),
            indicies: Vec::new(),
        }
    }

    pub fn draw_box(
        &mut self,
        pos: Vec2,
        size: Vec2,
        ds: DrawSettings,
        viewport_size: Vec2,
        parent_size: Vec2,
        parent_pos: Vec2,
    ) -> (usize, usize) {
        let b   = ds.border.map(|b| b.size).unwrap_or(0) as f32;
        let r   = ds.rounding as f32;
        let rmb = r.max(b) as f32;

        let start_idx = self.verticies.len();
        self.rect(
            pos + Vec2::splat(rmb),
            size - Vec2::splat(rmb * 2.0),
            None,
            ds.color,
            viewport_size,
            parent_size,
            parent_pos,
        );
        let border = ds.border;

        let corner_defs: [(Vec2, f32, bool, bool); 4] = [
            (Vec2::new(rmb,          rmb         ), PI,              false, false),
            (Vec2::new(size.x - rmb, rmb         ), 3.0 * PI / 2.0, false, true ),
            (Vec2::new(size.x - rmb, size.y - rmb), 0.0,             true,  true ),
            (Vec2::new(rmb,          size.y - rmb), PI / 2.0,        true,  false),
        ];

        if rmb != 0.0 {
            self.rect(pos + Vec2::new(rmb * ds.round_topleft as u32 as f32, 0.0),        Vec2::new(size.x - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32), rmb), None, ds.color, viewport_size, parent_size, parent_pos);
            self.rect(pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - rmb), Vec2::new(size.x - rmb * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32) as f32), rmb), None, ds.color, viewport_size, parent_size, parent_pos);
            self.rect(pos + Vec2::new(0.0, rmb),        Vec2::new(rmb, size.y - rmb * 2.0), None, ds.color, viewport_size, parent_size, parent_pos);
            self.rect(pos + Vec2::new(size.x - rmb, rmb), Vec2::new(rmb, size.y - rmb * 2.0), None, ds.color, viewport_size, parent_size, parent_pos);
        }
        
        for (offset, start_angle, is_bottom, is_right) in corner_defs {
            let center = pos + offset;

            let should_round = match (is_bottom, is_right) {
                (false, false) => ds.round_topleft,
                (false, true)  => ds.round_topright,
                (true,  false) => ds.round_bottomleft,
                (true,  true)  => ds.round_bottomright,
            };

            if !should_round || r == 0.0 {
                continue;
            }

            let outer_color = if let Some(border) = border {
                let h_col = if is_bottom { border.color_bottom } else { border.color_top };
                let v_col = if is_right  { border.color_right  } else { border.color_left };
                (h_col + v_col) * 0.5
            } else {
                ds.color
            };
            self.round_corner(center, r, start_angle, outer_color, viewport_size, parent_size, parent_pos);
            if r > b {
                self.round_corner(center, r - b, start_angle, ds.color, viewport_size, parent_size, parent_pos);
            }
        }
        
        if let Some(border) = border {
            self.rect(pos + Vec2::new(rmb * ds.round_topleft as u32 as f32, 0.0),          Vec2::new(size.x - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32), b), None, border.color_top,    viewport_size, parent_size, parent_pos);
            self.rect(pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - b),   Vec2::new(size.x - rmb * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32) as f32), b), None, border.color_bottom, viewport_size, parent_size, parent_pos);
            self.rect(pos + Vec2::new(0.0, rmb * ds.round_topleft as u32 as f32),          Vec2::new(b, size.y - rmb * ((ds.round_topleft as u32 + ds.round_bottomleft as u32) as f32)), None, border.color_left,   viewport_size, parent_size, parent_pos);
            self.rect(pos + Vec2::new(size.x - b, rmb * ds.round_topright as u32 as f32),   Vec2::new(b, size.y - rmb * ((ds.round_topright as u32 + ds.round_bottomright as u32) as f32)), None, border.color_right,  viewport_size, parent_size, parent_pos);
        }
        let end_idx = self.verticies.len();
        
        (start_idx, end_idx)
    }

    pub fn build<'a, 'w, 's, R>(
        &'a mut self,
        buttons: Res<'w, ButtonInput<MouseButton>>,
        ctx: Res<'w, NUiContext>,
        window: &'w Window,
        touch: Res<'w, Touches>,
        keys: SmallVec<[KeyboardInput; 8]>,
        f: impl FnOnce(UiWindowBuilder<'a, 'w>) -> R,
    ) -> R {
        let viewport_size = window.size();

        let size = self.rect.max - self.rect.min;
        let r = NUiContext::WINDOW_ROUNDING as f32;
        let b = NUiContext::BORDER as f32;
        let rmb = r.max(b);

        let header_h = (ctx.acent - ctx.decent + NUiContext::WINDOW_PAD.y as f32 * 2.0).round();

        let focused = self.focused.is_some();
        let (resize_top, resize_bottom, resize_left, resize_right) = if let Some(f) = &self.focused {
            (f.resize_top, f.resize_bottom, f.resize_left, f.resize_right)
        } else {
            Default::default()
        };

        let border_color = |active: bool| if active { NUiContext::BLUE } else { NUiContext::S1 };

        let mut window_ds = DrawSettings {
            color: NUiContext::BG,
            rounding: NUiContext::WINDOW_ROUNDING,
            round_topleft: false,
            round_topright: false,
            round_bottomleft: true,
            round_bottomright: true,
            border: Some(BorderSettings {
                color_top:    border_color(resize_top),
                color_bottom: border_color(resize_bottom),
                color_left:   border_color(resize_left),
                color_right:  border_color(resize_right),
                size: NUiContext::BORDER,
            }),
            ..Default::default()
        };
        self.draw_box(self.rect.min + Vec2::new(0.0, header_h), size - Vec2::new(0.0, header_h), window_ds, viewport_size, viewport_size, Vec2::ZERO);

        let main_contend_pos  = self.rect.min + Vec2::splat(rmb as f32);
        let main_contend_size = size - Vec2::splat(rmb as f32) * 2.0;
        window_ds.round_topleft = true;
        window_ds.round_topright = true;
        window_ds.round_bottomleft = false;
        window_ds.round_bottomright = false;
        window_ds.color = if focused { NUiContext::BG } else { NUiContext::BG_DARK };
        self.draw_box(self.rect.min, Vec2::new(size.x, header_h + NUiContext::BORDER as f32), window_ds, viewport_size, viewport_size, Vec2::ZERO);

        let label      = self.label.clone();
        let header_pos = self.rect.min + NUiContext::WINDOW_PAD.as_vec2();
        let header_size = Vec2::new(main_contend_size.x, header_h);
        self.text(&ctx, header_pos, NUiContext::TEXT, &label, viewport_size, header_pos, header_size - Vec2::new(NUiContext::WINDOW_PAD.x as f32, 0.0));


        let mut left_mouse_pressed  = buttons.just_pressed(MouseButton::Left);
        let mut left_mouse_pressing = buttons.pressed(MouseButton::Left);
        let mut left_mouse_released = buttons.just_released(MouseButton::Left);
        let mut cursor_pos          = window.cursor_position();

        if let Some(touch) = touch.iter().next() {
            cursor_pos = Some(touch.position());
            left_mouse_pressing = true;
        }
        if let Some(touch) = touch.iter_just_pressed().next() {
            cursor_pos = Some(touch.position());
            left_mouse_pressed = true;
        }
        if let Some(touch) = touch.iter_just_released().next() {
            cursor_pos = Some(touch.position());
            left_mouse_released = true;
        }

        let content_pos  = main_contend_pos + Vec2::new(0.0, header_h + b) + NUiContext::WINDOW_PAD.as_vec2();
        f(UiWindowBuilder {
            line_height: 0.0,
            ctx,
            parent_content_size: size,
            parent_content_pos: self.rect.min,
            window: self,
            viewport_size,
            cursor: content_pos,
            cursor_pos,
            left_mouse_pressed,
            left_mouse_pressing,
            left_mouse_released,
            direction: false,
            prev_cursor: content_pos,
            keys,
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
        parent_size: Vec2
    ) -> Vec2 {
        pos.y += ctx.acent;
        for char in text.chars() {
            if char == '\n' {
                pos.y += ctx.new_line_size;
                continue;
            }
            let atlas_info = ctx.atlas_lut.get(&char).cloned().unwrap_or_else(|| {
                ctx.atlas_lut.get(&'?').cloned().unwrap()
            });

            let uv = atlas_info.position.as_vec2() / ctx.atlas_size;
            let size = atlas_info.size.as_vec2();
            let uv_size = size / ctx.atlas_size; 
            let tpos = Vec2::new(
                pos.x + atlas_info.xmin,
                pos.y - atlas_info.height,
            );            
            self.rect(tpos, size, Some((uv, uv_size)), color, viewport_size, parent_size, parent_pos);
            pos.x += atlas_info.width + atlas_info.xmin;
        }
        pos
    }   

    fn rect(
        &mut self,
        mut pos: Vec2,
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

        let (clipped_uv_min, clipped_uv_max) = if let Some((uv, uv_size)) = uv {
            let uv_scale      = uv_size / size;
            let clipped_uv_min = uv + (clipped_min - pos) * uv_scale;
            let clipped_uv_max = uv + (clipped_max - pos) * uv_scale;
            (clipped_uv_min, clipped_uv_max)
        } else {
            (Vec2::splat(0.0), Vec2::splat(0.0))
        };

        let vertex_id = self.verticies.len() as u32;
        let half_vp   = view_port_size / 2.0;

        self.verticies.extend_from_slice(&[
            UIVertex { color, pos: (clipped_min / half_vp) - Vec2::splat(1.0),                       uv: clipped_uv_min },
            UIVertex { color, pos: (clipped_min.with_x(clipped_max.x) / half_vp) - Vec2::splat(1.0), uv: clipped_uv_min.with_x(clipped_uv_max.x) },
            UIVertex { color, pos: (clipped_max / half_vp) - Vec2::splat(1.0),                       uv: clipped_uv_max },
            UIVertex { color, pos: (clipped_min.with_y(clipped_max.y) / half_vp) - Vec2::splat(1.0), uv: clipped_uv_min.with_y(clipped_uv_max.y) },
        ]);
        self.indicies.extend_from_slice(&[
            vertex_id, vertex_id + 1, vertex_id + 2,
            vertex_id, vertex_id + 3, vertex_id + 2,
        ]);
        Some(vertex_id as usize)
    }

    fn round_corner(
        &mut self,
        center: Vec2,
        rounding: f32,
        start_angle: f32,
        color: Vec4,
        view_port_size: Vec2,
        parent_size: Vec2,
        parent_pos: Vec2,
    ) -> Option<(usize, usize)> {
        let segments  = rounding.ceil() as u32;
        let half_vp   = view_port_size / 2.0;
        let clip_min  = parent_pos;
        let clip_max  = parent_pos + parent_size;

        let clamp_to_clip = |p: Vec2| p.clamp(clip_min, clip_max);
        let to_ndc        = |p: Vec2| (p / half_vp) - Vec2::splat(1.0);

        let first_vertex  = self.verticies.len();
        let center_vertex = self.verticies.len() as u32;
        let mut prev_vertex = 0u32;

        self.verticies.push(UIVertex {
            color,
            pos: to_ndc(clamp_to_clip(center)),
            uv: Vec2::splat(20.0),
        });

        for i in 0..=segments {
            let t     = i as f32 / segments as f32;
            let angle = start_angle + t * (PI / 2.0);
            let point = clamp_to_clip(center + Vec2::new(angle.cos(), angle.sin()) * rounding);
            let vertex = self.verticies.len() as u32;

            self.verticies.push(UIVertex { color, pos: to_ndc(point), uv: Vec2::splat(20.0) });

            if i > 0 {
                let a = self.verticies[center_vertex as usize].pos;
                let b = self.verticies[prev_vertex  as usize].pos;
                let c = self.verticies[vertex       as usize].pos;
                let area = (b - a).perp_dot(c - a).abs();
                if area > 1e-6 {
                    self.indicies.extend_from_slice(&[center_vertex, prev_vertex, vertex]);
                }
            }

            prev_vertex = vertex;
        }

        let last_vertex = self.verticies.len().saturating_sub(1);
        if self.verticies.len() > first_vertex {
            Some((first_vertex, last_vertex))
        } else {
            None
        }
    }
}

pub struct UiWindowBuilder<'a, 'w> {
    parent_content_pos: Vec2,
    parent_content_size: Vec2,
    window: &'a mut UiWindow,
    ctx: Res<'w, NUiContext>,
    keys: SmallVec<[KeyboardInput; 8]>,
    left_mouse_pressed: bool,
    left_mouse_pressing: bool,
    left_mouse_released: bool,
    cursor_pos: Option<Vec2>,
    viewport_size: Vec2,

    line_height: f32,
    prev_cursor: Vec2,
    cursor: Vec2,
    direction: bool,
}

impl<'a, 'w> UiWindowBuilder<'a, 'w> {
    fn element_clicked(&mut self, pos: Vec2, size: Vec2) -> bool {
        self.hoverd(pos, size) && self.clicked()
    } 

    fn clicked(&mut self) -> bool {
       self.left_mouse_pressed
    }

    fn hoverd(&mut self, pos: Vec2, size: Vec2) -> bool {
        let clip_pos = pos.max(self.parent_content_pos);
        let clip_max = (size + pos).min(self.parent_content_pos + self.parent_content_size);

        if let Some(mouse_pos) = self.cursor_pos && Rect::from_corners(clip_max, clip_pos).contains(mouse_pos) {
            true
        }else {
            false
        }
    } 

    fn rect(&mut self, size: Vec2, ds: DrawSettings) {
        let bg_id = self.window.draw_box(
            self.cursor,
            size,
            ds,
            self.viewport_size,
            self.parent_content_size,
            self.parent_content_pos,
        );
        self.finish_element(size);
    }


    fn finish_element(&mut self, size: Vec2) {
        self.line_height = self.line_height.max(size.y);
        if self.direction {
            self.cursor.x += size.x + NUiContext::ELEMENT_GAP.x as f32;
        }else {
            self.cursor.y += size.y + NUiContext::ELEMENT_GAP.y as f32;
        }
    }

    fn text(&mut self, label: impl AsRef<str>) {
        let npos = self.window.text(&self.ctx, self.cursor.with_y(self.cursor.y.round()), NUiContext::TEXT, label.as_ref(), self.viewport_size, self.parent_content_pos, self.parent_content_size);
        let size = npos - self.cursor;
        self.finish_element(size);
    }

    fn button(&mut self, label: impl AsRef<str>) -> bool {
        let bc = NUiContext::S2;
        let mut ds = DrawSettings {
            rounding: NUiContext::ROUNDING,
            color: NUiContext::S0,
            border: Some(BorderSettings {
                color_bottom: bc,
                color_left: bc,
                color_right: bc,
                color_top: bc,
                size: NUiContext::BORDER, 
            }),
            ..Default::default()
        }.all_rounded();

        let ts = self.ctx.text_len(label.as_ref());
        let rmb = NUiContext::BORDER.max(NUiContext::ROUNDING) as f32;
        let size = Vec2::new(ts, self.ctx.acent - self.ctx.decent) + (NUiContext::CHILD_PAD.as_vec2() + Vec2::splat(rmb)) * 2.0;

        let mut clicked = false;
        if self.hoverd(self.cursor, size) {
            if self.clicked() {
                ds.color = NUiContext::S2;
                clicked = true;
            }else {
                ds.color = NUiContext::S1;
            }
        }

        let prev = self.cursor;
        let e = self.rect(size, ds);
        self.cursor = prev;

        let prev = self.cursor;
        self.cursor += NUiContext::CHILD_PAD.as_vec2() + rmb;
        self.text(label);
        self.cursor = prev;
        self.finish_element(size);
        clicked
    }

    fn slider(&mut self, id: impl Hash, min: f32, max: f32, width: f32, value: f32) -> f32 {
        let mut hash = DefaultHasher::new();
        id.hash(&mut hash);
        let id = hash.finish();
        let bc = NUiContext::S2;
        let mut ds = DrawSettings {
            rounding: NUiContext::ROUNDING,
            color: NUiContext::S1,
            border: Some(BorderSettings {
                color_bottom: bc,
                color_left: bc,
                color_right: bc,
                color_top: bc,
                size: NUiContext::BORDER, 
            }),
            ..Default::default()
        }.all_rounded();

        let line_size = self.ctx.acent - self.ctx.decent;
        let slider_height = line_size / 3.0;

        let size = Vec2::new(width, slider_height);
        let slide_size = Vec2::new(8.0, line_size);

        let prev = self.cursor;
        self.cursor.y += (line_size - slider_height) / 2.0;
        self.rect(size, ds);
        self.cursor = prev;
        self.cursor.x += f32::clamp((value - min) / (max - min) * width, 0.0, width) - slide_size.x * 0.5;
        ds.color = NUiContext::BLUE;
        ds.rounding = 4;

        if self.element_clicked(self.cursor, slide_size) {
            if let Some(f) = &mut self.window.focused {
                f.draging = Some(id.try_into().unwrap());
                f.darg_start = prev;
            }
        }
        self.rect(slide_size, ds);

        let mut ret = value;
        if let Some(f) = &self.window.focused {
            if let Some(draging) = f.draging && draging == id.try_into().unwrap() && let Some(cursor) = self.cursor_pos {
                let val = (cursor - f.darg_start).project_onto(Vec2::new(1.0, 0.0)).x;
                ret = f32::clamp(val / width * (max - min) + min, min, max);
            }    
        }
        
        self.cursor = prev;

        self.finish_element(Vec2::new(width, slide_size.y));
        ret
    }

    fn text_input(&mut self, id: impl Hash, value: &mut String, width: f32) {
        let bc = NUiContext::S2;
        let mut ds = DrawSettings {
            rounding: NUiContext::ROUNDING,
            color: NUiContext::S0,
            border: Some(BorderSettings {
                color_bottom: bc,
                color_left: bc,
                color_right: bc,
                color_top: bc,
                size: NUiContext::BORDER, 
            }),
            ..Default::default()
        }.all_rounded();

        let mut hash = DefaultHasher::new();
        id.hash(&mut hash);
        let id = hash.finish();

        let rmb = NUiContext::BORDER.max(NUiContext::ROUNDING) as f32;
        let size = Vec2::new(width, self.ctx.acent - self.ctx.decent) + (NUiContext::CHILD_PAD.as_vec2() + Vec2::splat(rmb)) * 2.0;
        let prev = self.cursor;

        let clicked = self.element_clicked(self.cursor, size);
        let id = Some(NonZeroU64::new(id).unwrap());
        let mut focused = if let Some(focused) = self.window.focused.as_mut() {
            if clicked {
                focused.focused = id;
                focused.cursor = value.len();
            }
            if focused.focused == id {
                Some(&mut focused.cursor)
            }else {
                None
            }
        } else {
            None
        };
        if let Some(cursor) = &mut focused {
            ds.border.as_mut().unwrap().color_bottom = NUiContext::BLUE;
            ds.border.as_mut().unwrap().color_left = NUiContext::BLUE;
            ds.border.as_mut().unwrap().color_right = NUiContext::BLUE;
            ds.border.as_mut().unwrap().color_top = NUiContext::BLUE;

            for key in &self.keys {
                if !(key.repeat || key.state.is_pressed()) {
                    continue;
                }
                if key.key_code == KeyCode::ArrowLeft { 
                    **cursor = cursor.saturating_sub(1);
                    **cursor = value.floor_char_boundary(**cursor);
                } else if key.key_code == KeyCode::ArrowRight {
                    **cursor += 1;
                    **cursor = value.ceil_char_boundary(**cursor);
                } else if key.key_code == KeyCode::Backspace {
                    if **cursor != 0 {
                        **cursor -= 1;
                        value.remove(**cursor);
                        **cursor = value.floor_char_boundary(**cursor);
                    }
                } else if let Some(str) = &key.text {
                    if self.ctx.atlas_lut.contains_key(&str.chars().next().unwrap()) {
                        value.insert_str(**cursor, str.as_str());
                        **cursor += 1;
                        **cursor = value.ceil_char_boundary(**cursor);
                    }
                }
            }
        }

        
        let focused = focused.cloned();

        self.rect(size, ds);
        self.cursor = prev;
        self.parent_content_size = size - rmb * 2.0;
        self.parent_content_pos = self.cursor + rmb;

        let prev = self.cursor;
        self.cursor += NUiContext::CHILD_PAD.as_vec2() + rmb;
        let mut offset = None;
        if let Some(cursor) = focused {
            let len = self.ctx.text_len(&value[..cursor]);
            offset = Some((len - width + 5.0).max(0.0));
            self.cursor.x -= offset.unwrap();
        }

        self.text(&value);

        if let Some(cursor) = focused {
            let x = self.ctx.text_len(&value[..cursor]);
            self.cursor = prev + Vec2::new(x, 0.0) + NUiContext::CHILD_PAD.as_vec2() + rmb; 
            self.cursor.x -= offset.unwrap();
            let ds = DrawSettings {
                color: Vec4::ONE,
                ..Default::default()
            };
            self.rect(Vec2::new(1.0, self.ctx.acent - self.ctx.decent), ds);
        }

        self.cursor = prev;
        self.finish_element(size);
    }

    fn horizontal(&mut self) {
        if !self.direction {
            self.direction = true;
            self.prev_cursor = self.cursor;
            self.line_height = 0.0;
        }
    }

    fn vertical(&mut self) {
        if self.direction {
            self.direction = false;
            self.cursor = self.prev_cursor;

            self.cursor.y += self.line_height + NUiContext::ELEMENT_GAP.y as f32;
        }
    }

}

#[derive(Resource)]
pub struct NUiContext {
    pub font: PathBuf,
    pub font_settings: FontSettings,
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
    width: f32,
    advance_width: f32,
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
    pub const BG: Vec4 = Vec4::new(0.155, 0.155, 0.155, 1.0);
    pub const BG_DARK: Vec4 = Vec4::new(0.130, 0.130, 0.130, 1.0);
    pub const S0: Vec4 = Vec4::new(0.220, 0.220, 0.220, 1.0);
    pub const S1: Vec4 = Vec4::new(0.260, 0.260, 0.260, 1.0);
    pub const S2: Vec4 = Vec4::new(0.300, 0.300, 0.300, 1.0);
    pub const GRAB: Vec4 = Vec4::new(0.370, 0.370, 0.370, 1.0);
    pub const GRAB_HOT: Vec4 = Vec4::new(0.490, 0.490, 0.490, 1.0);
    pub const TEXT: Vec4 = Vec4::new(0.880, 0.880, 0.880, 1.0);
    pub const TEXT_DIM: Vec4 = Vec4::new(0.550, 0.550, 0.550, 1.0);

    pub const BLUE: Vec4 = Vec4::new(0.118, 0.565, 0.831, 1.0);
    pub const BLUE_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.6);
    pub const BLUE_REALY_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.35);

    pub const TRACE: Vec4 = Vec4::new(0.380, 0.380, 0.380, 1.0);
    pub const DEBUG: Vec4 = Vec4::new(0.400, 0.560, 0.700, 1.0);
    pub const INFO: Vec4 = Vec4::new(0.820, 0.820, 0.820, 1.0);
    pub const WARN: Vec4 = Vec4::new(0.980, 0.760, 0.110, 1.0);
    pub const ERROR: Vec4 = Vec4::new(0.950, 0.180, 0.180, 1.0);

    pub const ATLAS_WIDTH: u32 = 2048;
    pub const PAD: u32 = 0;
    pub const FONTSCALE: f32 = 15.0;
    pub const DRAG_THRESHHOLD: f32 = 10.0;

    pub const ELEMENT_GAP: UVec2 = UVec2::new(4, 2);
    pub const WINDOW_ROUNDING: u32 = 4;
    pub const ROUNDING: u32 = 2;
    pub const BORDER: u32 = 1;
    pub const CHILD_PAD: UVec2 = UVec2::new(2, 1);
    pub const WINDOW_PAD: UVec2 = UVec2::new(3, 2);

    pub(crate) fn build_ui_resources(&mut self) -> Result<NUiResources> {
        let bytes = fs::read(&self.font)?;
        let font  = Font::from_bytes(bytes, self.font_settings).unwrap();

        let mut chars = Vec::new();
        for (c, i) in font.chars().iter() {
            let (metrics, data) = font.rasterize_config(GlyphRasterConfig {
                glyph_index: (*i).into(),
                px: Self::FONTSCALE,
                font_hash: 0,
            });
            chars.push((metrics, data, *c));
        }
        chars.sort_unstable_by(|a, b| a.1.cmp(&b.1));

        let font_metrics = font.horizontal_line_metrics(Self::FONTSCALE).unwrap();
        self.acent         = font_metrics.ascent;
        self.new_line_size = font_metrics.new_line_size;
        self.decent        = font_metrics.descent;

        let mut atlas_row_height_prefix_sum = Vec::new();
        let mut atlas_height = Self::PAD;
        let mut row_length   = 1;
        let mut row_height   = Self::PAD;
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
        for b in &mut atlas_data { *b = 0; }

        let mut row_length = 1;
        atlas_data[0] = 255;
        let mut row_index = 0;
        for (metrics, data, char) in &chars {
            if row_length + (metrics.width as u32 + Self::PAD) >= Self::ATLAS_WIDTH {
                row_length = 0;
                row_index += 1;
            }

            let row_start = atlas_row_height_prefix_sum[row_index];
            let position  = U16Vec2::new(row_length as u16, row_start as u16);

            self.atlas_lut.insert(*char, AtlasEntry {
                position,
                size: U16Vec2::new(metrics.width as u16, metrics.height as u16),
                xmin: metrics.bounds.xmin,
                width: metrics.bounds.width,
                height: metrics.bounds.height + metrics.bounds.ymin,
                advance_width: metrics.advance_width,
            });

            for y in 0..metrics.height as u32 {
                for x in 0..metrics.width as u32 {
                    let idx = (y * metrics.width as u32 + x) as usize;
                    atlas_data[((row_start + y) * Self::ATLAS_WIDTH + x + row_length) as usize] = data[idx];
                }
            }

            row_length += metrics.width as u32 + Self::PAD;
        }

        let font_atlas = Image::new(Self::ATLAS_WIDTH, atlas_height)?;
        self.atlas_size = Vec2::new(Self::ATLAS_WIDTH as f32, atlas_height as f32);
        let future    = UploadQueue::push_image(atlas_data, font_atlas);
        let font_atlas = block_on(future)?;

        Ok(NUiResources {
            font_atlas,
            indicies:  [Buffer::new(16 * 1024, true)?, Buffer::new(16 * 1024, true)?],
            verticies: [Buffer::new(16 * 1024, true)?, Buffer::new(16 * 1024, true)?],
            pending_indicies:  Vec::with_capacity(16 * 1024),
            pending_verticies: Vec::with_capacity(16 * 1024),
            num_indicies:  0,
            num_verticies: 0,
        })
    }

    fn text_len(&self, str: &str) -> f32 {
        let mut len = 0.0;
        for char in str.chars() {
            if let Some(b) = self.atlas_lut.get(&char) {
                len += b.advance_width;
            }else {
                len += self.atlas_lut.get(&'?').unwrap().advance_width;
            }
        }
        len
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

    resources.num_indicies  = resources.pending_indicies.len();
    resources.num_verticies = resources.pending_verticies.len();

    resources.pending_verticies.clear();
    resources.pending_indicies.clear();
}

pub fn create_ui_resources(
    mut cmd: Commands,
    res: Option<Res<NUiResources>>,
    mut world: ResMut<MainWorld>,
) {
    if res.is_some() { return; }
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
) {
    let mut cursor_icon         = CursorIcon::System(SystemCursorIcon::Default);
    let mut left_mouse_pressed  = mouse_buttons.just_pressed(MouseButton::Left);
    let mut left_mouse_pressing = mouse_buttons.pressed(MouseButton::Left);
    let mut left_mouse_released = mouse_buttons.just_released(MouseButton::Left);
    let mut cursor_pos          = desktop_window.cursor_position();

    if let Some(touch) = touch.iter().next() {
        cursor_pos = Some(touch.position());
        left_mouse_pressing = true;
    }
    if let Some(touch) = touch.iter_just_pressed().next() {
        cursor_pos = Some(touch.position());
        left_mouse_pressed = true;
    }
    if let Some(touch) = touch.iter_just_released().next() {
        cursor_pos = Some(touch.position());
        left_mouse_released = true;
    }

    let mut now_focused = None;
    for (e, mut window) in windows
        .iter_mut()
        .sort_by::<&UiWindow>(|a, b| a.layer.cmp(&b.layer))
    {
        let border_rect = Rect::from_corners(
            window.rect.min - Vec2::splat(NUiContext::DRAG_THRESHHOLD),
            window.rect.max + Vec2::splat(NUiContext::DRAG_THRESHHOLD),
        );
        if let Some(cursor_pos) = cursor_pos
            && border_rect.contains(cursor_pos)
            && matches!(now_focused, None)
            && !window.indicies.is_empty()
        {
            if left_mouse_pressed && window.focused.is_none() {
                now_focused = Some(e);
                window.focused = Some(FocusedState {
                    cursor: 0,
                    draging: None,
                    focused: None,
                    resize_bottom: false,
                    resize_left: false,
                    resize_right: false,
                    resize_top: false,
                    is_being_draged: false,
                    darg_start: Vec2::ZERO,
                });
            }
        } else if left_mouse_pressed {
            window.focused = None;
        }
    }

    if let Some(focused) = now_focused {
        let mut layers = Vec::new();
        for (entity, _) in windows
            .iter_mut()
            .sort_by::<(Entity, &UiWindow)>(|(e1, a), (e2, b)| {
                if *e1 == focused      { std::cmp::Ordering::Greater }
                else if *e2 == focused { std::cmp::Ordering::Less }
                else                   { a.layer.cmp(&b.layer) }
            })
        {
            layers.push(entity);
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
            let header_h    = (ctx.acent - ctx.decent + NUiContext::CHILD_PAD.y as f32 * 2.0).round();
            let header_rect = Rect::from_corners(
                window.rect.min,
                window.rect.min + Vec2::new(window.rect.max.x - window.rect.min.x, header_h),
            );

            if let Some(cursor_pos) = cursor_pos {
                if header_rect.contains(cursor_pos) {
                    if left_mouse_pressed {
                        focused.darg_start      = window.rect.min - cursor_pos;
                        focused.is_being_draged = true;
                    }
                    cursor_icon = CursorIcon::System(SystemCursorIcon::Grab);
                } else if !window.rect.contains(cursor_pos) {
                    let min = window.rect.min;
                    let max = window.rect.max;
                    let t   = NUiContext::DRAG_THRESHHOLD;

                    let resize_top    = Rect::from_corners(Vec2::new(min.x - t, min.y - t), Vec2::new(max.x + t, min.y + t)).contains(cursor_pos);
                    let resize_bottom = Rect::from_corners(Vec2::new(min.x - t, max.y - t), Vec2::new(max.x + t, max.y + t)).contains(cursor_pos);
                    let resize_left   = Rect::from_corners(Vec2::new(min.x - t, min.y - t), Vec2::new(min.x + t, max.y + t)).contains(cursor_pos);
                    let resize_right  = Rect::from_corners(Vec2::new(max.x - t, min.y - t), Vec2::new(max.x + t, max.y + t)).contains(cursor_pos);

                    if left_mouse_pressed {
                        focused.resize_bottom = resize_bottom;
                        focused.resize_left   = resize_left;
                        focused.resize_top    = resize_top;
                        focused.resize_right  = resize_right;
                    }

                    cursor_icon = match (
                        focused.resize_top    || resize_top,
                        focused.resize_bottom || resize_bottom,
                        focused.resize_left   || resize_left,
                        focused.resize_right  || resize_right,
                    ) {
                        (true,  false, true,  false) => CursorIcon::System(SystemCursorIcon::NwseResize),
                        (true,  false, false, true ) => CursorIcon::System(SystemCursorIcon::NeswResize),
                        (false, true,  true,  false) => CursorIcon::System(SystemCursorIcon::NeswResize),
                        (false, true,  false, true ) => CursorIcon::System(SystemCursorIcon::NwseResize),
                        (true,  false, false, false) => CursorIcon::System(SystemCursorIcon::NsResize),
                        (false, true,  false, false) => CursorIcon::System(SystemCursorIcon::NsResize),
                        (false, false, true,  false) => CursorIcon::System(SystemCursorIcon::EwResize),
                        (false, false, false, true ) => CursorIcon::System(SystemCursorIcon::EwResize),
                        _ => CursorIcon::System(SystemCursorIcon::Default),
                    };
                }
            }

            if left_mouse_pressed {
                focused.focused = None;
            }

            if left_mouse_released {
                focused.darg_start      = Vec2::ZERO;
                focused.is_being_draged = false;
                focused.resize_bottom   = false;
                focused.resize_top      = false;
                focused.resize_left     = false;
                focused.resize_right    = false;
                focused.draging         = None;
            }

            if left_mouse_pressing {
                if let Some(cursor_pos) = cursor_pos {
                    let size = window.rect.max - window.rect.min;
                    if focused.is_being_draged {
                        let drag_pos   = (cursor_pos + focused.darg_start).round();
                        window.rect.min = drag_pos;
                        window.rect.max = drag_pos + size;
                        cursor_icon = CursorIcon::System(SystemCursorIcon::Grabbing);
                    }
                    if focused.resize_top    { window.rect.min.y = cursor_pos.y.min(window.rect.max.y - 1.0).round(); }
                    if focused.resize_bottom { window.rect.max.y = cursor_pos.y.max(window.rect.min.y + 1.0).round(); }
                    if focused.resize_left   { window.rect.min.x = cursor_pos.x.min(window.rect.max.x - 1.0).round(); }
                    if focused.resize_right  { window.rect.max.x = cursor_pos.x.max(window.rect.min.x + 1.0).round(); }
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

pub fn test_ui(mut cmd: Commands, mut ui: UiBuilder<TestWindow>, mut value: Local<(f32, String)>) {
    ui.build_or(
        || {
            cmd.spawn((
                UiWindow::new("Entity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdl"),
                TestWindow,
            ));
        },
        |mut b| {
            b.horizontal();
            b.button("TEst");
            b.button("TEst");
            b.button("TEst");
            b.button("TEst");
            b.vertical();

            b.horizontal();
            b.text("Text");
            b.text("Text");
            b.text("Text");
            b.text("Text");
            b.vertical();

            value.0 = b.slider("Test1", -10.0, 8.0, 100.0, value.0);

            b.horizontal();
            value.0 = b.slider("Test2", -10.0, 8.0, 100.0, value.0);
            value.0 = b.slider("Test3", -10.0, 8.0, 100.0, value.0);
            b.text(format!("{}", value.0));
            b.text("Test");
            b.vertical();

            value.0 = b.slider("Test4", -10.0, 8.0, 100.0, value.0);
            value.0 = b.slider("Test5", -10.0, 8.0, 100.0, value.0);
            b.text_input("Text Input", &mut value.1, 100.0);
        },
    );
}