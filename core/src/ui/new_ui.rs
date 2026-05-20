use anyhow::Result;
use bevy::ecs::message::MessageReader;
use bevy::ecs::system::Local;
use bevy::ecs::system::lifetimeless::Read;
use bevy::input::keyboard::{KeyCode, KeyboardInput};
use bevy::input::mouse::AccumulatedMouseScroll;
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
use glam::{BVec2, I8Vec2, IVec2, U8Vec2, U16Vec2, UVec2, Vec2, Vec2Swizzles, Vec4};
use itertools::Itertools;
use lava::{
    buffer::*,
    image::{Image, format, usage},
};
use smallvec::SmallVec;
use std::collections::HashSet;
use std::f32::consts::PI;
use std::io::Write;
use std::num::{NonZero, NonZeroU32, NonZeroU64};
use std::ops::{Add, RangeBounds};
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

pub const fn hash_location(file: &str, line: u32, col: u32) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let bytes = file.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000003b4c61); 
        i += 1;
    }
    hash ^= line as u64;
    hash = hash.wrapping_mul(0x100000003b4c61);
    hash ^= col as u64;
    hash = hash.wrapping_mul(0x100000003b4c61);
    hash
}

#[macro_export]
macro_rules! id {
    () => {
        $crate::ui::new_ui::hash_location(file!(), line!(), column!())
    };
}

#[derive(Reflect, Clone)]
pub struct FocusedState {
    is_being_draged: bool,
    draging: Option<NonZeroU64>,
    focused: Option<NonZeroU64>,
    cursor: TextCursor,
    selected: Range<usize>,
    offset: f32,
    darg_start: Vec2,
    format_string: String,

    resize_top: bool,
    resize_bottom: bool,
    resize_left: bool,
    resize_right: bool,
}

#[derive(Reflect, Copy, Clone)]
pub struct Scrollable {
    pub content_size: Vec2,
    pub scroll: Vec2,
}

impl Scrollable {
    pub fn scroll(&mut self, delta: Vec2, size: Vec2) {
        let scrollbar_y = self.content_size.y > size.y;
        let scrollbar_x = self.content_size.x > size.x;

        self.scroll -= Vec2::new(
            scrollbar_x as u32 as f32,
            scrollbar_y as u32 as f32,
        ) * delta;

        self.clamp_scroll(size);
    }

    pub fn clamp_scroll(&mut self, size: Vec2) {
        self.scroll = self.scroll.clamp(
            Vec2::ZERO,
            (self.content_size - size)
                .max(Vec2::ZERO),
        );
    }

    fn draw_bar(&mut self, id: NonZeroU64, size: Vec2, pos: Vec2, window: &mut UiWindow, direction: bool, viewport_size: Vec2, cursor_pos: Option<Vec2>, left_mouse_pressed: bool, parent_size: Vec2, parent_pos: Vec2) {
        let b = NUiContext::BORDER as f32;
        let track_pos = if direction {
            Vec2::new(
                pos.x + size.x - NUiContext::BAR_THICKNESS - b,
                pos.y + b,
            ).round()
        } else {
            Vec2::new(
                pos.x + b,
                pos.y + size.y - NUiContext::BAR_THICKNESS - b,
            ).round()
        };

        let track_size = if direction {
            Vec2::new(NUiContext::BAR_THICKNESS, size.y - b * 2.0).round()
        }else {
            Vec2::new(size.x - b * 2.0, NUiContext::BAR_THICKNESS).round()
        };
    
        window.rect(track_pos, track_size, None, NUiContext::S0,
            viewport_size, parent_size, parent_pos, false);

        let scroll_max = (self.content_size - size).max(Vec2::ONE);

        let ratio    = (size / self.content_size).clamp(Vec2::ZERO, Vec2::ONE);
        let thumb_width  = (track_size * ratio).max(Vec2::splat(NUiContext::MIN_THUMB)).round();
        let thumb_t  = (self.scroll / scroll_max).clamp(Vec2::ZERO, Vec2::ONE);
        let thumb  = (track_pos + thumb_t * (track_size - thumb_width)).round();
        let thumb_pos  = if direction {
            Vec2::new(track_pos.x, thumb.y) 
        } else {
            Vec2::new(thumb.x, track_pos.y) 
        };
        let thumb_size = if direction {
            Vec2::new(NUiContext::BAR_THICKNESS, thumb_width.y)
        }else {
            Vec2::new(thumb_width.x, NUiContext::BAR_THICKNESS)
        };

        let id_scroll = id.saturating_add(1).saturating_add(!direction as u64);

        let dragging = window.focused.as_ref()
            .map(|f| f.draging == Some(id_scroll))
            .unwrap_or(false);

        let hovered = cursor_pos
            .map(|p| Rect::from_corners(thumb_pos, thumb_pos + thumb_size).contains(p))
            .unwrap_or(false);

        if left_mouse_pressed && hovered {
            if let Some(f) = &mut window.focused {
                let grab_offset = cursor_pos.map(|p| p - thumb).unwrap_or(Vec2::ZERO);
                f.draging    = Some(id_scroll);
                f.darg_start = grab_offset * Vec2::new(!direction as u32 as f32, direction as u32 as f32);
            }
        }

        if let Some(p) = cursor_pos && dragging {
            let grab_offset  = window.focused.as_ref().map(|f| f.darg_start).unwrap_or(Vec2::ZERO);
            let new_thumb  = p - grab_offset - track_pos;
            let travel       = (track_size - thumb_width).max(Vec2::ONE);
            let t            = (new_thumb / travel).clamp(Vec2::ZERO, Vec2::ONE);
            self.scroll    = t * scroll_max * Vec2::new(!direction as u32 as f32, direction as u32 as f32);
        }

        let thumb_color = if dragging || hovered { NUiContext::GRAB_HOT } else { NUiContext::GRAB };
        let ds = DrawSettings {
            color: thumb_color,
            ..Default::default()
        };
        window.draw_box(thumb_pos, thumb_size, ds,
            viewport_size, parent_size, parent_pos);
    }

    pub fn draw(&mut self, id: NonZeroU64, size: Vec2, pos: Vec2, window: &mut UiWindow, viewport_size: Vec2, cursor_pos: Option<Vec2>, left_mouse_pressed: bool, parent_size: Vec2, parent_pos: Vec2) {
        if self.content_size.y > size.y {
            self.draw_bar(id, size, pos, window, true, viewport_size, cursor_pos, left_mouse_pressed, parent_size, parent_pos);
        }
        if self.content_size.x > size.x {
            self.draw_bar(id, size, pos, window, false, viewport_size, cursor_pos, left_mouse_pressed, parent_size, parent_pos);
        }
    }

}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct UiWindow {
    pub scrollbar_x: bool,
    pub scrollbar_y: bool,
    pub open: bool,
    pub open_headers: HashSet<u64>,
    pub scrollables: HashMap<u64, Scrollable>,
    pub focused: Option<FocusedState>,
    pub label: String,
    pub size: Rect,
    pub id: u64,
    pub layer: u32,
    #[reflect(ignore)]
    pub verticies: Vec<UIVertex>,
    pub indicies: Vec<u32>,
    #[reflect(ignore)]
    pub top_verticies: Vec<UIVertex>,
    pub top_indicies: Vec<u32>,
}

#[derive(SystemParam)]
pub struct UiBuilder<'w, 's, Marker: Component> {
    query: Query<'w, 's, lifetimeless::Write<UiWindow>, With<Marker>>,
    window: Single<'w, 's, lifetimeless::Read<Window>>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    touch: Res<'w, Touches>,
    scroll: Res<'w, AccumulatedMouseScroll>,
    ctx: Res<'w, NUiContext>,
    cursor: Single<'w, 's, lifetimeless::Write<CursorIcon>>,
    keys: MessageReader<'w, 's, KeyboardInput>,
    keyspressed: Res<'w, ButtonInput<KeyCode>>,
}

impl<'s, 'w, Marker: Component> UiBuilder<'w, 's, Marker> {
    pub fn build(
        &mut self,
        f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>),
    ) -> Result<(), QuerySingleError> {
        let mut window = self.query.single_mut()?;
        let mouse = Res::clone(&self.mouse);
        let ctx = Res::clone(&self.ctx);
        let touch = Res::clone(&self.touch);
        let scroll = Res::clone(&self.scroll);
        let shift = self.keyspressed.pressed(KeyCode::ShiftLeft)
            || self.keyspressed.pressed(KeyCode::ShiftRight);
        let strg = self.keyspressed.pressed(KeyCode::ControlLeft)
            || self.keyspressed.pressed(KeyCode::ControlRight);

        window.build(mouse, ctx, &self.window, touch, scroll, &mut self.cursor, &mut self.keys, shift, strg, f);
        Ok(())
    }

    pub fn build_or(&mut self, mut init: impl FnMut(), f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>)) {
        let Ok(mut window) = self.query.single_mut() else {
            init();
            return;
        };
        let mouse = Res::clone(&self.mouse);
        let ctx = Res::clone(&self.ctx);
        let touch = Res::clone(&self.touch);
        let scroll = Res::clone(&self.scroll);

        let shift = self.keyspressed.pressed(KeyCode::ShiftLeft)
            || self.keyspressed.pressed(KeyCode::ShiftRight);
        let strg = self.keyspressed.pressed(KeyCode::ControlLeft)
            || self.keyspressed.pressed(KeyCode::ControlRight);

        window.build(mouse, ctx, &self.window, touch, scroll, &mut self.cursor, &mut self.keys, shift, strg, f);
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
    on_top: bool,

    rounding: u32,
    round_topleft: bool,
    round_topright: bool,
    round_bottomleft: bool,
    round_bottomright: bool,

    border: Option<BorderSettings>,
}

impl Default for DrawSettings {
    fn default() -> Self {
        let bc = NUiContext::S2;
        Self {
            color: NUiContext::S0,
            rounding: NUiContext::ROUNDING,
            border: Some(BorderSettings {
                color_bottom: bc,
                color_left: bc,
                color_right: bc,
                color_top: bc,
                size: NUiContext::BORDER,
            }),
            round_bottomleft: true,
            round_bottomright: true,
            round_topleft: true,
            round_topright: true,
            on_top: false,
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

    pub fn border_color(mut self, color: Vec4) -> Self {
        if let Some(border) = &mut self.border {
            border.color_bottom = color;
            border.color_top = color;
            border.color_left = color;
            border.color_right = color;
        }
        self
    }

    pub fn new(hoverd: bool, clicked: bool) -> Self {
        Self {
            color: if clicked {
                NUiContext::S2
            }else if hoverd {
                NUiContext::S1
            }else {
                NUiContext::S0
            },
            ..Default::default()
        }
    }
}

#[derive(Copy, Clone, Default)]
pub enum TextDirection {
    #[default]
    Right, 
    Left,   
    Up,     
    Down,
}

impl UiWindow {
    pub fn new(label: impl Into<String>) -> Self {
        let str = label.into();
        let mut hash = DefaultHasher::new();
        str.hash(&mut hash);
        let id = hash.finish();
        let pos = Vec2::new(100.0, 100.0);
        let size = Vec2::new(500.0, 500.0);

        let size = Rect::from_corners(pos, pos + size);
        UiWindow {
            scrollbar_x: false,
            scrollbar_y: false,
            scrollables: HashMap::new(),
            open: true,
            open_headers: HashSet::new(),
            id,
            focused: None,
            label: str,
            layer: u32::MAX,
            size,
            verticies: Vec::new(),
            indicies: Vec::new(),
            top_indicies: Vec::new(),
            top_verticies: Vec::new()
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
        let b = ds.border.map(|b| b.size).unwrap_or(0) as f32;
        let r = ds.rounding as f32;
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
            ds.on_top
        );
        let border = ds.border;

        let corner_defs: [(Vec2, f32, bool, bool); 4] = [
            (Vec2::new(rmb, rmb), PI, false, false),
            (Vec2::new(size.x - rmb, rmb), 3.0 * PI / 2.0, false, true),
            (Vec2::new(size.x - rmb, size.y - rmb), 0.0, true, true),
            (Vec2::new(rmb, size.y - rmb), PI / 2.0, true, false),
        ];

        if rmb != 0.0 {
            self.rect(
                pos + Vec2::new(rmb * ds.round_topleft as u32 as f32, 0.0),
                Vec2::new(
                    size.x - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32),
                    rmb,
                ),
                None,
                ds.color,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
            self.rect(
                pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - rmb),
                Vec2::new(
                    size.x
                        - rmb * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32) as f32),
                    rmb,
                ),
                None,
                ds.color,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
            self.rect(
                pos + Vec2::new(0.0, rmb),
                Vec2::new(rmb, size.y - rmb * 2.0),
                None,
                ds.color,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
            self.rect(
                pos + Vec2::new(size.x - rmb, rmb),
                Vec2::new(rmb, size.y - rmb * 2.0),
                None,
                ds.color,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
        }

        for (offset, start_angle, is_bottom, is_right) in corner_defs {
            let center = pos + offset;

            let should_round = match (is_bottom, is_right) {
                (false, false) => ds.round_topleft,
                (false, true) => ds.round_topright,
                (true, false) => ds.round_bottomleft,
                (true, true) => ds.round_bottomright,
            };

            if !should_round || r == 0.0 {
                continue;
            }

            let outer_color = if let Some(border) = border {
                let h_col = if is_bottom {
                    border.color_bottom
                } else {
                    border.color_top
                };
                let v_col = if is_right {
                    border.color_right
                } else {
                    border.color_left
                };
                (h_col + v_col) * 0.5
            } else {
                ds.color
            };
            self.round_corner(
                center,
                r,
                start_angle,
                outer_color,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
            if r > b {
                self.round_corner(
                    center,
                    r - b,
                    start_angle,
                    ds.color,
                    viewport_size,
                    parent_size,
                    parent_pos,
                    ds.on_top
                );
            }
        }

        if let Some(border) = border {
            self.rect(
                pos + Vec2::new(rmb * ds.round_topleft as u32 as f32, 0.0),
                Vec2::new(
                    size.x - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32),
                    b,
                ),
                None,
                border.color_top,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
            self.rect(
                pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - b),
                Vec2::new(
                    size.x
                        - rmb * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32) as f32),
                    b,
                ),
                None,
                border.color_bottom,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
            self.rect(
                pos + Vec2::new(0.0, rmb * ds.round_topleft as u32 as f32),
                Vec2::new(
                    b,
                    size.y - rmb * ((ds.round_topleft as u32 + ds.round_bottomleft as u32) as f32),
                ),
                None,
                border.color_left,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
            self.rect(
                pos + Vec2::new(size.x - b, rmb * ds.round_topright as u32 as f32),
                Vec2::new(
                    b,
                    size.y
                        - rmb * ((ds.round_topright as u32 + ds.round_bottomright as u32) as f32),
                ),
                None,
                border.color_right,
                viewport_size,
                parent_size,
                parent_pos,
                ds.on_top
            );
        }
        let end_idx = self.verticies.len();

        (start_idx, end_idx)
    }

    pub fn build<'w, 's, R>(
        &mut self,
        buttons: Res<'w, ButtonInput<MouseButton>>,
        ctx: Res<'w, NUiContext>,
        window: &Window,
        touch: Res<'w, Touches>,
        scroll: Res<'w, AccumulatedMouseScroll>,
        cursor_icon: &mut Single<'w, 's, lifetimeless::Write<CursorIcon>>,
        keys: &mut MessageReader<'w, 's, KeyboardInput>,
        shift: bool,
        ctrl: bool,
        f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>) -> R,
    ) -> Option<R> {
        let viewport_size = window.size();

        let size = self.size.max - self.size.min;
        let r = NUiContext::WINDOW_ROUNDING as f32;
        let b = NUiContext::BORDER as f32;
        let rmb = r.max(b);

        let header_h = (ctx.acent - ctx.decent + NUiContext::WINDOW_PAD.y as f32 * 2.0).round();

        let focused = self.focused.is_some();

        let mut left_mouse_pressed = buttons.just_pressed(MouseButton::Left);
        let mut left_mouse_pressing = buttons.pressed(MouseButton::Left);
        let mut left_mouse_released = buttons.just_released(MouseButton::Left);
        let mut cursor_pos = window.cursor_position();

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
        ***cursor_icon = CursorIcon::System(SystemCursorIcon::Default);
        if let Some(focused) = &mut self.focused {
            let header_h    = (ctx.acent - ctx.decent + NUiContext::CHILD_PAD.y as f32 * 2.0).round();
            let header_rect = Rect::from_corners(
                self.size.min,
                self.size.min + Vec2::new(self.size.max.x - self.size.min.x, header_h),
            );

            if let Some(cursor_pos) = cursor_pos {
                if header_rect.contains(cursor_pos) {
                    if left_mouse_pressed {
                        focused.darg_start      = self.size.min - cursor_pos;
                        focused.is_being_draged = true;
                    }
                    ***cursor_icon = CursorIcon::System(SystemCursorIcon::Grab);
                } else if !self.size.contains(cursor_pos) {
                    let min = self.size.min;
                    let max = self.size.max;
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

                    ***cursor_icon = match (
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

            if left_mouse_released {
                focused.draging         = None;
                focused.darg_start      = Vec2::ZERO;
                focused.is_being_draged = false;
                focused.resize_bottom   = false;
                focused.resize_top      = false;
                focused.resize_left     = false;
                focused.resize_right    = false;
            }

            if left_mouse_pressing {
                if let Some(cursor_pos) = cursor_pos {
                    let size = self.size.max - self.size.min;
                    if focused.is_being_draged {
                        let drag_pos   = (cursor_pos + focused.darg_start).round();
                        self.size.min = drag_pos;
                        self.size.max = drag_pos + size;
                        ***cursor_icon = CursorIcon::System(SystemCursorIcon::Grabbing);
                    }
                    if focused.resize_top    { self.size.min.y = cursor_pos.y.min(self.size.max.y - 1.0).round(); }
                    if focused.resize_bottom { self.size.max.y = cursor_pos.y.max(self.size.min.y + 1.0).round(); }
                    if focused.resize_left   { self.size.min.x = cursor_pos.x.min(self.size.max.x - 1.0).round(); }
                    if focused.resize_right  { self.size.max.x = cursor_pos.x.max(self.size.min.x + 1.0).round(); }
                }
            }
        }

        let (resize_top, resize_bottom, resize_left, resize_right) = self.focused.as_ref().map(|f| {
            (f.resize_top, f.resize_bottom, f.resize_left, f.resize_right)
        }).unwrap_or_default();

        let border_color = |active: bool| {
            if active { NUiContext::BLUE } else { NUiContext::S1 }
        };

        let mut window_ds = DrawSettings {
            on_top: false,
            color: NUiContext::BG,
            rounding: NUiContext::WINDOW_ROUNDING,
            round_topleft: false,
            round_topright: false,
            round_bottomleft: true,
            round_bottomright: true,
            border: Some(BorderSettings {
                color_top: border_color(resize_top),
                color_bottom: border_color(resize_bottom),
                color_left: border_color(resize_left),
                color_right: border_color(resize_right),
                size: NUiContext::BORDER,
            }),
        };
        let content_area_pos = self.size.min + Vec2::new(0.0, header_h);
        let content_area_size = size - Vec2::new(0.0, header_h);
        if self.open {
            self.draw_box(
                content_area_pos,
                content_area_size,
                window_ds,
                viewport_size,
                viewport_size,
                Vec2::ZERO,
            );
        }

        window_ds.round_topleft = true;
        window_ds.round_topright = true;
        window_ds.round_bottomleft = false;
        window_ds.round_bottomright = false;
        window_ds.color = if focused { NUiContext::BG } else { NUiContext::BG_DARK };
        window_ds.border.as_mut().unwrap().color_bottom = NUiContext::S1;
        self.draw_box(
            self.size.min,
            Vec2::new(size.x, header_h + NUiContext::BORDER as f32),
            window_ds,
            viewport_size,
            viewport_size,
            Vec2::ZERO,
        );

        let label = self.label.clone();
        let header_pos = self.size.min + NUiContext::WINDOW_PAD.as_vec2();
        let header_size = Vec2::new(size.x, header_h);
        self.text(
            &ctx,
            header_pos + if !self.open { Vec2::new(0.0, ctx.acent + 2.0) } else { Vec2::ZERO },
            NUiContext::TEXT,
            "▼",
            viewport_size,
            header_pos,
            header_size - Vec2::new(NUiContext::WINDOW_PAD.x as f32, 0.0),
            false,
            if self.open { TextDirection::Right } else { TextDirection::Up },
        );

        let arrow_size = Vec2::new(ctx.text_size("▼").x, ctx.new_line_size);

        self.text(
            &ctx,
            header_pos + Vec2::new(NUiContext::ELEMENT_GAP.x as f32 + arrow_size.x, 0.0),
            NUiContext::TEXT,
            &label,
            viewport_size,
            header_pos,
            header_size - Vec2::new(NUiContext::WINDOW_PAD.x as f32, 0.0),
            false,
            TextDirection::Right,
        );

        if let Some(cursor_pos) = cursor_pos
            && Rect::from_center_half_size(header_pos, arrow_size).contains(cursor_pos)
            && left_mouse_pressed
        {
            self.open = !self.open;
        }

        let (_, mut scrollable) = self.scrollables.remove_entry(&self.id).unwrap_or((self.id, Scrollable {
            content_size: content_area_size,
            scroll: Vec2::ZERO
        }));
        let cursor =
            (content_area_pos + rmb + NUiContext::WINDOW_PAD.as_vec2() - scrollable.scroll).round();

        if !self.open {
            return None;
        }

        let mut builder = UiWindowBuilder {
            scroll_delta: scroll.delta,
            content_max: Vec2::new(0.0, 0.0),
            focuse_next: false,
            line_height: 0.0,
            ctx,
            parent_content_size: content_area_size - b * 2.0 - NUiContext::BAR_THICKNESS * Vec2::new((scrollable.content_size.y > content_area_size.y) as u32 as f32, (scrollable.content_size.x > content_area_size.x) as u32 as f32),
            parent_content_pos: content_area_pos + b,
            window: self,
            viewport_size,
            cursor,
            cursor_pos,
            left_mouse_pressed,
            left_mouse_pressing,
            left_mouse_released,
            direction: false,
            scroll_consumed: false,
            prev_cursor: cursor,
            keys,
            ctrl,
            shift,
            hovered_smth: false,
        };

        let r = f(&mut builder);
        let content_max = builder.content_max;
        let scroll_consumed = builder.scroll_consumed;

        if !builder.hovered_smth && left_mouse_pressed {
            if let Some(f) = &mut self.focused {
                f.focused = None;
            }
        }

        let content_size = content_max - cursor + Vec2::new(2.0, 10.0);
        
        scrollable.content_size = content_size;
        if !scroll_consumed && focused && self.open {
            scrollable.scroll(scroll.delta, content_area_size);
        }
        scrollable.draw(NonZeroU64::new(self.id).unwrap(), content_area_size, content_area_pos, self, viewport_size, cursor_pos, left_mouse_pressed, viewport_size, Vec2::ZERO);
        self.scrollables.insert(self.id, scrollable);
        Some(r)
    }

    fn text(
        &mut self,
        ctx: &NUiContext,
        pos: Vec2,
        color: Vec4,
        text: &str,
        viewport_size: Vec2,
        parent_pos: Vec2,
        parent_size: Vec2,
        on_top: bool,
        direction: TextDirection,
    ) -> Vec2 {
        let clip_min = parent_pos;
        let clip_max = parent_pos + parent_size;
        let half_vp = viewport_size / 2.0;

        let verticies = if on_top { &mut self.top_verticies } else { &mut self.verticies };
        let indicies  = if on_top { &mut self.top_indicies  } else { &mut self.indicies  };

        let advance_dir: Vec2 = match direction {
            TextDirection::Right => Vec2::new( 1.0,  0.0),
            TextDirection::Left  => Vec2::new(-1.0,  0.0),
            TextDirection::Down  => Vec2::new( 0.0,  1.0),
            TextDirection::Up    => Vec2::new( 0.0, -1.0),
        };

        let ascent_dir: Vec2 = match direction {
            TextDirection::Right => Vec2::new( 0.0,  1.0),
            TextDirection::Left  => Vec2::new( 0.0, -1.0),
            TextDirection::Down  => Vec2::new(-1.0,  0.0),
            TextDirection::Up    => Vec2::new( 1.0,  0.0),
        };

        let mut pen = pos + ascent_dir * ctx.acent;

        for char in text.chars() {
            if char == '\n' {
                pen = pos + ascent_dir * (ctx.acent + ctx.new_line_size);
                continue;
            }

            let atlas_info = ctx
                .atlas_lut
                .get(&char)
                .cloned()
                .unwrap_or_else(|| ctx.atlas_lut.get(&'?').cloned().unwrap());

            let uv      = atlas_info.position.as_vec2() / ctx.atlas_size;
            let uv_size = atlas_info.atlas_size.as_vec2() / ctx.atlas_size;
            let size    = atlas_info.atlas_size.as_vec2();

            let local_x = atlas_info.min.x;
            let local_y = -(atlas_info.bounds.y + atlas_info.min.y);

            let glyph_origin = pen
                + advance_dir * local_x
                + ascent_dir  * local_y;

            let p_tl = glyph_origin;
            let p_tr = glyph_origin + advance_dir * size.x;
            let p_br = glyph_origin + advance_dir * size.x + ascent_dir * size.y;
            let p_bl = glyph_origin +                        ascent_dir * size.y;

            let min_x = p_tl.x.min(p_tr.x).min(p_br.x).min(p_bl.x);
            let min_y = p_tl.y.min(p_tr.y).min(p_br.y).min(p_bl.y);
            let max_x = p_tl.x.max(p_tr.x).max(p_br.x).max(p_bl.x);
            let max_y = p_tl.y.max(p_tr.y).max(p_br.y).max(p_bl.y);

            if min_x >= clip_max.x || max_x <= clip_min.x
            || min_y >= clip_max.y || max_y <= clip_min.y {
                pen += advance_dir * atlas_info.advance_width;
                continue;
            }
            let clamped_min_x = min_x.max(clip_min.x);
            let clamped_min_y = min_y.max(clip_min.y);
            let clamped_max_x = max_x.min(clip_max.x);
            let clamped_max_y = max_y.min(clip_max.y);

            let x_range = max_x - min_x;
            let y_range = max_y - min_y;

            let t_x_min = (clamped_min_x - min_x) / x_range;
            let t_x_max = (clamped_max_x - min_x) / x_range;
            let t_y_min = (clamped_min_y - min_y) / y_range;
            let t_y_max = (clamped_max_y - min_y) / y_range;

            let (t_adv_min, t_adv_max, t_asc_min, t_asc_max) = match direction {
                TextDirection::Right => (t_x_min, t_x_max, t_y_min, t_y_max),
                TextDirection::Left  => (1.0 - t_x_max, 1.0 - t_x_min, 1.0 - t_y_max, 1.0 - t_y_min),
                TextDirection::Down  => (t_y_min, t_y_max, 1.0 - t_x_max, 1.0 - t_x_min),
                TextDirection::Up    => (1.0 - t_y_max, 1.0 - t_y_min, t_x_min, t_x_max),
            };

            let uv_x_min = uv.x + t_adv_min * uv_size.x;
            let uv_x_max = uv.x + t_adv_max * uv_size.x;
            let uv_y_min = uv.y + t_asc_min * uv_size.y;
            let uv_y_max = uv.y + t_asc_max * uv_size.y;

            let to_ndc = |p: Vec2| (p / half_vp) - Vec2::splat(1.0);
            let vertex_id = verticies.len() as u32;

            let (c_tl, c_tr, c_br, c_bl) = match direction {
                TextDirection::Right => (
                    (Vec2::new(clamped_min_x, clamped_min_y), Vec2::new(uv_x_min, uv_y_min)),
                    (Vec2::new(clamped_max_x, clamped_min_y), Vec2::new(uv_x_max, uv_y_min)),
                    (Vec2::new(clamped_max_x, clamped_max_y), Vec2::new(uv_x_max, uv_y_max)),
                    (Vec2::new(clamped_min_x, clamped_max_y), Vec2::new(uv_x_min, uv_y_max)),
                ),
                TextDirection::Left => (
                    (Vec2::new(clamped_max_x, clamped_max_y), Vec2::new(uv_x_min, uv_y_min)),
                    (Vec2::new(clamped_min_x, clamped_max_y), Vec2::new(uv_x_max, uv_y_min)),
                    (Vec2::new(clamped_min_x, clamped_min_y), Vec2::new(uv_x_max, uv_y_max)),
                    (Vec2::new(clamped_max_x, clamped_min_y), Vec2::new(uv_x_min, uv_y_max)),
                ),
                TextDirection::Down => (
                    (Vec2::new(clamped_max_x, clamped_min_y), Vec2::new(uv_x_min, uv_y_min)),
                    (Vec2::new(clamped_max_x, clamped_max_y), Vec2::new(uv_x_max, uv_y_min)),
                    (Vec2::new(clamped_min_x, clamped_max_y), Vec2::new(uv_x_max, uv_y_max)),
                    (Vec2::new(clamped_min_x, clamped_min_y), Vec2::new(uv_x_min, uv_y_max)),
                ),
                TextDirection::Up => (
                    (Vec2::new(clamped_min_x, clamped_max_y), Vec2::new(uv_x_min, uv_y_min)),
                    (Vec2::new(clamped_min_x, clamped_min_y), Vec2::new(uv_x_max, uv_y_min)),
                    (Vec2::new(clamped_max_x, clamped_min_y), Vec2::new(uv_x_max, uv_y_max)),
                    (Vec2::new(clamped_max_x, clamped_max_y), Vec2::new(uv_x_min, uv_y_max)),
                ),
            };

            verticies.extend_from_slice(&[
                UIVertex { color, pos: to_ndc(c_tl.0), uv: c_tl.1 },
                UIVertex { color, pos: to_ndc(c_tr.0), uv: c_tr.1 },
                UIVertex { color, pos: to_ndc(c_br.0), uv: c_br.1 },
                UIVertex { color, pos: to_ndc(c_bl.0), uv: c_bl.1 },
            ]);
            indicies.extend_from_slice(&[
                vertex_id, vertex_id + 1, vertex_id + 2,
                vertex_id, vertex_id + 3, vertex_id + 2,
            ]);

            pen += advance_dir * atlas_info.advance_width;
        }

        pen
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
        on_top: bool,
    ) {
        let clip_min1 = parent_pos;
        let clip_max1 = parent_pos + parent_size;

        let clip_min = clip_max1.min(clip_min1);
        let clip_max = clip_max1.max(clip_min1);


        let clipped_min = pos.max(clip_min);
        let clipped_max = (pos + size).min(clip_max);

        if clipped_min.x >= clipped_max.x || clipped_min.y >= clipped_max.y {
            return;
        }

        let (clipped_uv_min, clipped_uv_max) = if let Some((uv, uv_size)) = uv {
            let uv_scale = uv_size / size;
            let clipped_uv_min = uv + (clipped_min - pos) * uv_scale;
            let clipped_uv_max = uv + (clipped_max - pos) * uv_scale;
            (clipped_uv_min, clipped_uv_max)
        } else {
            (Vec2::splat(0.0), Vec2::splat(0.0))
        };

        let verticies = if on_top {
            &mut self.top_verticies
        }else {
            &mut self.verticies
        };

        let indicies = if on_top {
            &mut self.top_indicies
        }else {
            &mut self.indicies
        };

        let vertex_id = verticies.len() as u32;
        let half_vp = view_port_size / 2.0;

        verticies.extend_from_slice(&[
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
        indicies.extend_from_slice(&[
            vertex_id,
            vertex_id + 1,
            vertex_id + 2,
            vertex_id,
            vertex_id + 3,
            vertex_id + 2,
        ]);
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
        on_top: bool
    ) {
        let segments = rounding.ceil() as u32;
        let half_vp = view_port_size / 2.0;
        let clip_min = parent_pos;
        let clip_max = parent_pos + parent_size;

        let clamp_to_clip = |p: Vec2| p.clamp(clip_min, clip_max);
        let to_ndc = |p: Vec2| (p / half_vp) - Vec2::splat(1.0);

        let verticies = if on_top {
            &mut self.top_verticies
        }else {
            &mut self.verticies
        };

        let indicies = if on_top {
            &mut self.top_indicies
        }else {
            &mut self.indicies
        };

        let center_vertex = verticies.len() as u32;
        let mut prev_vertex = 0u32;

        verticies.push(UIVertex {
            color,
            pos: to_ndc(clamp_to_clip(center)),
            uv: Vec2::splat(20.0),
        });

        for i in 0..=segments {
            let t = i as f32 / segments as f32;
            let angle = start_angle + t * (PI / 2.0);
            let point = clamp_to_clip(center + Vec2::new(angle.cos(), angle.sin()) * rounding);
            let vertex = verticies.len() as u32;

            verticies.push(UIVertex {
                color,
                pos: to_ndc(point),
                uv: Vec2::splat(20.0),
            });

            if i > 0 {
                let a = verticies[center_vertex as usize].pos;
                let b = verticies[prev_vertex as usize].pos;
                let c = verticies[vertex as usize].pos;
                let area = (b - a).perp_dot(c - a).abs();
                if area > 1e-6 {
                    indicies
                        .extend_from_slice(&[center_vertex, prev_vertex, vertex]);
                }
            }

            prev_vertex = vertex;
        }
    }
}

#[derive(Copy, Clone, Reflect)]
pub struct TextCursor {
    pub byte_pos: usize,
}

impl TextCursor {
    pub fn move_right(&mut self, text: &str) {
        if let Some((_, ch)) = text[self.byte_pos..].char_indices().next() {
            self.byte_pos += ch.len_utf8();
        }
    }

    pub fn move_left(&mut self, text: &str) {
        if self.byte_pos == 0 {
            return;
        }
        self.byte_pos -= 1;
        while !text.is_char_boundary(self.byte_pos) {
            self.byte_pos -= 1;
        }
    }

    pub fn insert(&mut self, text: &mut String, str: &str) {
        text.insert_str(self.byte_pos, str);
        self.byte_pos += str.len();
    }

    pub fn delete_before(&mut self, text: &mut String) {
        if self.byte_pos == 0 {
            return;
        }
        self.move_left(text);
        text.remove(self.byte_pos);
    }

    pub fn delete_after(&mut self, text: &mut String) {
        if self.byte_pos < text.len() {
            text.remove(self.byte_pos);
        }
    }

    pub fn ch(&self, text: &str) -> Option<char> {
        text[self.byte_pos..].chars().next()
    }

    pub fn ch_before(&self, text: &str) -> Option<char> {
        if self.byte_pos == 0 {
            None
        }else {
            text[(self.byte_pos-1)..].chars().next()
        }
    }
}

pub struct UiWindowBuilder<'a, 'w, 's> {
    parent_content_pos: Vec2,
    parent_content_size: Vec2,
    window: &'a mut UiWindow,
    keys: &'a mut MessageReader<'w, 's, KeyboardInput>,
    ctx: Res<'w, NUiContext>,

    ctrl: bool,
    shift: bool,
    focuse_next: bool,
    left_mouse_pressed: bool,
    left_mouse_pressing: bool,
    left_mouse_released: bool,
    scroll_delta: Vec2,
    viewport_size: Vec2,
    cursor_pos: Option<Vec2>,
    
    line_height: f32,
    content_max: Vec2,
    prev_cursor: Vec2,
    cursor: Vec2,
    direction: bool,
    hovered_smth: bool,
    scroll_consumed: bool,
}


enum InputMode<'a> {
    String(&'a mut String),
    Float(f32),
    Int(i32),
}

enum InputModeOutput {
    Float(f32),
    Int(i32),
    None,
}

fn rgb_to_hsv(c: Vec4) -> (f32, f32, f32, f32) {
    let r = c.x; let g = c.y; let b = c.z;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let v = max;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    let h = if delta < 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if max == g {
        ((b - r) / delta + 2.0) / 6.0
    } else {
        ((r - g) / delta + 4.0) / 6.0
    };
    (h, s, v, c.w)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32, a: f32) -> Vec4 {
    let h6 = h * 6.0;
    let i  = h6.floor() as i32;
    let f  = h6 - i as f32;
    let p  = v * (1.0 - s);
    let q  = v * (1.0 - s * f);
    let t  = v * (1.0 - s * (1.0 - f));
    let (r, g, b) = match i % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    Vec4::new(r, g, b, a)
}

impl<'a, 'w, 's> UiWindowBuilder<'a, 'w, 's> {
    fn id(&self, h: &impl Hash) -> NonZeroU64 {
        let mut hash = DefaultHasher::new();
        h.hash(&mut hash);
        self.window.id.hash(&mut hash);
        NonZeroU64::new(hash.finish()).unwrap()
    }

    fn element_clicked(&self, pos: Vec2, size: Vec2) -> bool {
        self.hoverd(pos, size) && self.left_mouse_pressed
    }

    fn hoverd(&self, pos: Vec2, size: Vec2) -> bool {
        Self::hoverdp(
            pos,
            size,
            self.parent_content_pos,
            self.parent_content_size,
            self.cursor_pos,
            self.hovered_smth
        ) && self.window.focused.is_some()
    }

    fn hoverdp(
        pos: Vec2,
        size: Vec2,
        parent_content_pos: Vec2,
        parent_content_size: Vec2,
        cursor_pos: Option<Vec2>,
        hovered_smth: bool,
    ) -> bool {
        let clip_pos = pos.max(parent_content_pos);
        let clip_max = (size + pos).min(parent_content_pos + parent_content_size);

        if let Some(mouse_pos) = cursor_pos
            && Rect::from_corners(clip_max, clip_pos).contains(mouse_pos)
            && !hovered_smth
        {
            true
        } else {
            false
        }
    }

    pub fn rect(&mut self, size: Vec2, ds: DrawSettings) {
        self.window.draw_box(
            self.cursor,
            size,
            ds,
            self.viewport_size,
            self.parent_content_size,
            self.parent_content_pos,
        );
        self.finish_element(size, false);
    }

    fn finish_element(&mut self, size: Vec2, consume_scroll: bool) {
        let size = size.round();
        self.line_height = self.line_height.max(size.y);
        self.content_max = self.content_max.max(self.cursor + size);
        if self.hoverd(self.cursor, size) {
            self.hovered_smth = true;
            self.scroll_consumed |= consume_scroll;
        }
        if self.direction {
            self.cursor.x += size.x + NUiContext::ELEMENT_GAP.x as f32;
        } else {
            self.cursor.y += size.y + NUiContext::ELEMENT_GAP.y as f32;
        }
    }

    pub fn text(&mut self, label: impl AsRef<str>) {
        let npos = self.window.text(
            &self.ctx,
            self.cursor,
            NUiContext::TEXT,
            label.as_ref(),
            self.viewport_size,
            self.parent_content_pos,
            self.parent_content_size,
            false,
            TextDirection::Right
        );
        let size = npos - self.cursor;
        self.finish_element(Vec2::new(size.x, self.ctx.new_line_size), false);
    }

    fn child_offset() -> Vec2 {
        NUiContext::CHILD_PAD.as_vec2() + NUiContext::BORDER.max(NUiContext::ROUNDING) as f32
    }

    fn contain_size(size: Vec2) -> Vec2 {
        (size + Self::child_offset() * 2.0).round()
    }
    
    fn child_cursor(&self) -> Vec2 {
        (self.cursor + Self::child_offset()).round()
    }

    pub fn button(&mut self, label: impl AsRef<str>) -> bool {
        let size = Self::contain_size(Vec2::new(self.ctx.text_size(label.as_ref()).x, self.ctx.acent - self.ctx.decent));
        let hoverd = self.hoverd(self.cursor, size);
        let clicked = self.left_mouse_pressed && hoverd;

        self.window.draw_box(self.cursor, size, DrawSettings::new(hoverd, clicked), self.viewport_size, self.parent_content_size, self.parent_content_pos);
        self.window.text(&self.ctx, self.child_cursor(), NUiContext::TEXT, label.as_ref(), self.viewport_size, self.parent_content_pos, self.parent_content_size, false, TextDirection::Right);
        self.finish_element(size, false);
        clicked
    }

    pub fn slider(&mut self, id: impl Hash, min: f32, max: f32, width: f32, value: f32) -> f32 {
        let id = self.id(&id);
        let mut ds = DrawSettings::default();

        let line_size = self.ctx.acent - self.ctx.decent;
        let slider_height = line_size / 3.0;

        let size = Vec2::new(width, slider_height);
        let slide_size = Vec2::new(16.0, line_size);

        let slider_pos = self.cursor + Vec2::new(0.0, (line_size - slider_height) / 2.0);
        self.window.draw_box(slider_pos, size, ds, self.viewport_size, self.parent_content_size, self.parent_content_pos);
        let slide_pos = self.cursor +
            Vec2::new(f32::clamp((value - min) / (max - min) * width, 0.0, width) - slide_size.x * 0.5, 0.0).round();

        if self.element_clicked(slide_pos, slide_size) {
            if let Some(f) = &mut self.window.focused {
                f.draging = Some(id);
                f.darg_start = self.cursor;
            }
        }

        ds.color = NUiContext::BLUE;
        ds.rounding = 4;

        self.window.draw_box(slide_pos, slide_size, ds, self.viewport_size, self.parent_content_size, self.parent_content_pos);

        let mut ret = value;
        if let Some(f) = &self.window.focused {
            if let Some(draging) = f.draging && draging == id.try_into().unwrap() && let Some(cursor) = self.cursor_pos {
                let val = (cursor - f.darg_start).project_onto(Vec2::new(1.0, 0.0)).x;
                ret = f32::clamp(val / width * (max - min) + min, min, max);
            }    
        }
        
        self.finish_element(Vec2::new(width, slide_size.y), false);
        ret
    }

    pub fn parent_bounds(&self, pos: Vec2, size: Vec2) -> (Vec2, Vec2){
        let parent_content_min = (pos).max(self.parent_content_pos);
        let parent_content_max = (pos + size).min(self.parent_content_pos + self.parent_content_size);
        (parent_content_min, parent_content_max - parent_content_min)
    }

    const WORD_DELIMITER: [char; 6] = [' ', '.', ',', ':', '(', ')'];

    fn text_input_private(&mut self, id: impl Hash, width: f32, input_mode: InputMode) -> InputModeOutput {
        let id = self.id(&id);
        let inner_size = Vec2::new(width, self.ctx.acent - self.ctx.decent);
        let size = Self::contain_size(inner_size);
        let clicked = self.element_clicked(self.cursor, size);
        let text_cursor = self.child_cursor();
        
        enum InputType {
            String,
            Float(f32),
            Int(i32)
        }
        let (mut value, need_format_string, input_mode) = match input_mode {
            InputMode::String(s) => (s, false, InputType::String),
            InputMode::Float(f) => {
                (&mut format!("{:.2}", f), true, InputType::Float(f))
            },
            InputMode::Int(i) => {
                (&mut format!("{}", i), true, InputType::Int(i))
            },
        };
        let (parent_content_pos, parent_content_size) = self.parent_bounds(text_cursor, inner_size);

        let mut just_focused = false;
        let mut focused = if let Some(focused) = self.window.focused.as_mut() {
            if (clicked && focused.focused != Some(id)) || self.focuse_next {
                focused.focused = Some(id);
                self.focuse_next = false;
                if need_format_string {
                    focused.format_string = value.clone();
                }
                focused.cursor = TextCursor {
                    byte_pos: value.len(),
                };
                focused.offset = 0.0;
                focused.selected = 0..value.len();
                just_focused = true;
            }
            if focused.focused == Some(id) {
                if need_format_string {
                    value = &mut focused.format_string;
                }

                focused.cursor.byte_pos = focused.cursor.byte_pos.min(value.len());
                focused.selected.end = focused.selected.end.min(value.len()); 
                focused.selected.start = focused.selected.start.min(value.len()); 
                Some((
                    &mut focused.cursor,
                    &mut focused.offset,
                    &mut focused.selected,
                    &mut focused.focused,
                ))
            } else {
                None
            }
        } else {
            None
        };

        let mut ds = DrawSettings::default();
        if let Some((cursor, view, selected, focused)) = &mut focused {
            ds = ds.border_color(NUiContext::BLUE);
            for key in self.keys.read() {
                let has_selection = selected.start != selected.end;
                let sel_min = selected.start.min(selected.end);
                let sel_max = selected.start.max(selected.end);
                if !(key.repeat || key.state.is_pressed()) {
                    continue;
                }

                let mut navigation = false;
                if key.key_code == KeyCode::ArrowLeft {
                    navigation = true;
                    if has_selection && !self.shift {
                        cursor.byte_pos = sel_min;
                    } else {
                        cursor.move_left(&value);
                        if self.ctrl {
                            while let Some(char) = cursor.ch_before(&value)
                                && !Self::WORD_DELIMITER.contains(&char)
                                && cursor.byte_pos != 0
                            {
                                cursor.move_left(&value);
                            }
                        }
                    }
                } else if key.key_code == KeyCode::ArrowRight {
                    navigation = true;
                    if has_selection && !self.shift {
                        cursor.byte_pos = sel_max;
                    } else {
                        cursor.move_right(&value);
                        if self.ctrl {
                            while let Some(char) = cursor.ch(&value)
                                && !Self::WORD_DELIMITER.contains(&char)
                                && cursor.byte_pos != value.len()
                            {
                                cursor.move_right(&value);
                            }
                        }
                    }
                } else if key.key_code == KeyCode::Home {
                    navigation = true;
                    cursor.byte_pos = 0;
                } else if key.key_code == KeyCode::End {
                    navigation = true;
                    cursor.byte_pos = value.len();
                } else if key.key_code == KeyCode::Backspace {
                    if has_selection {
                        value.drain(sel_min..sel_max);
                        cursor.byte_pos = sel_min;
                        selected.start = sel_min;
                        selected.end = sel_min;
                    } else {
                        cursor.delete_before(value);
                        if self.ctrl {
                            while let Some(char) = cursor.ch_before(&value)
                                && !Self::WORD_DELIMITER.contains(&char)
                                && cursor.byte_pos != 0
                            {
                                cursor.delete_before(value);
                            }
                        }
                    }
                    selected.start = cursor.byte_pos;
                    selected.end = cursor.byte_pos;
                    continue;
                } else if key.key_code == KeyCode::Delete {
                    if has_selection {
                        value.drain(sel_min..sel_max);
                        cursor.byte_pos = sel_min;
                        selected.start = sel_min;
                        selected.end = sel_min;
                    } else {
                        cursor.delete_after(value);
                        if self.ctrl {
                            while let Some(char) = cursor.ch(&value)
                                && !Self::WORD_DELIMITER.contains(&char)
                                && cursor.byte_pos != value.len()
                            {
                                cursor.delete_after(value);
                            }
                        }
                    }
                    selected.start = cursor.byte_pos;
                    selected.end = cursor.byte_pos;
                    continue;
                } else if self.ctrl && key.key_code == KeyCode::KeyA {
                    selected.start = 0;
                    selected.end = value.len();
                    cursor.byte_pos = value.len();
                    continue;
                } else if key.key_code == KeyCode::Enter || key.key_code == KeyCode::Escape {
                    **focused = None;
                    break;
                } else if key.key_code == KeyCode::Tab {
                    self.focuse_next = true;
                    break;
                } else if let Some(str) = &key.text {
                    if str.chars().all(|c| self.ctx.atlas_lut.contains_key(&c)) {
                        if has_selection {
                            value.drain(sel_min..sel_max);
                            cursor.byte_pos = sel_min;
                        }
                        cursor.insert(value, str);
                    }
                }
                if !self.shift {
                    selected.start = cursor.byte_pos;
                    selected.end = cursor.byte_pos;
                } else if navigation {
                    selected.end = cursor.byte_pos;
                }
            }

            let mut pos = -**view;
            let mut any_clicked = false;
            for (i, c) in value.char_indices() {
                let ae = self
                    .ctx
                    .atlas_lut
                    .get(&c)
                    .cloned()
                    .unwrap_or_else(|| self.ctx.atlas_lut[&'?']);
                if self.left_mouse_pressing
                    && Self::hoverdp(
                        text_cursor
                            + Vec2::new(pos, 0.0),
                        Vec2::new(ae.advance_width, self.ctx.acent - self.ctx.decent),
                        parent_content_pos,
                        parent_content_size,
                        self.cursor_pos,
                        self.hovered_smth
                    ) && !just_focused
                {   
                    any_clicked = true;
                    cursor.byte_pos = i;
                    if self.shift {
                        selected.end = i;
                    }else {
                        selected.start = i;
                        selected.end = i;
                    }
                }
                pos += ae.advance_width;
            }
            if !any_clicked && clicked && !just_focused {
                let end = value.len();
                cursor.byte_pos = end;
                if self.shift {
                    selected.end = end;
                }else {
                    selected.start = end;
                    selected.end = end;
                }
            }

            let offset = self.ctx.text_size(&value[..cursor.byte_pos]).x.round();
            let left = (offset - 5.0).max(0.0);
            if left < **view {
                **view = left;
            }
            let right = (offset - width + 5.0).max(0.0);
            if right > **view {
                **view = right;
            }
        }
        let focused = focused.map(|(e1, e2, e3, _)| (*e1, *e2, e3.clone()));
        let value = value.clone();
        
        self.window.draw_box(self.cursor, size, ds, self.viewport_size, self.parent_content_size, self.parent_content_pos);
       
        let p = text_cursor - Vec2::new(focused.as_ref().map(|e| e.1).unwrap_or(0.0), 0.0);
        self.window.text(&self.ctx, p, NUiContext::TEXT, &value, self.viewport_size, parent_content_pos, parent_content_size, false, TextDirection::Right);

        if let Some((cursor, offset, selected)) = &focused {
            let x = self.ctx.text_size(&value[..cursor.byte_pos]).x;
            let mut ds = DrawSettings {
                color: Vec4::ONE,
                round_bottomleft: false,
                round_bottomright: false,
                round_topleft: false,
                round_topright: false,
                rounding: 0,
                border: None,
                on_top: false,
            };
            self.window.draw_box(text_cursor + Vec2::new(x - *offset, 0.0), Vec2::new(1.0, self.ctx.acent - self.ctx.decent), ds, self.viewport_size, parent_content_size, parent_content_pos);

            ds.color = NUiContext::BLUE_DIM;
            let start = selected.start.min(selected.end);
            let end = selected.start.max(selected.end);

            let start = self.ctx.text_size(&value[..start]).x;
            let end = self.ctx.text_size(&value[..end]).x;

            self.window.draw_box(text_cursor + Vec2::new(start - offset, 0.0), Vec2::new(end - start, self.ctx.acent - self.ctx.decent), ds, self.viewport_size, parent_content_size, parent_content_pos);
        }

        self.finish_element(size, false);
        match input_mode {
            InputType::Float(f) => InputModeOutput::Float(value.parse().unwrap_or(f)),
            InputType::Int(i) => InputModeOutput::Int(value.parse().unwrap_or(i)),
            InputType::String => InputModeOutput::None
        }
    }

    pub fn text_input(&mut self, id: impl Hash, value: &mut String, width: f32) {
        self.text_input_private(id, width, InputMode::String(value));
    }
    
    pub fn float_input(&mut self, id: impl Hash, value: f32, width: f32) -> f32 {
        if let InputModeOutput::Float(v) = self.text_input_private(id, width, InputMode::Float(value)) {
            v
        }else {
            0.0
        }
    }

    pub fn check_box(&mut self, mut value: bool) -> bool {
        let size = Self::contain_size(Vec2::new(self.ctx.text_size("✓").x, self.ctx.acent - self.ctx.decent));
        let hoverd = self.hoverd(self.cursor, size);
        if self.left_mouse_pressed && hoverd {
            value = !value; 
        }
        let ds = DrawSettings::new(hoverd, false);
        self.window.draw_box(self.cursor, size, ds, self.viewport_size, self.parent_content_size, self.parent_content_pos);
        if value {
            self.window.text(&self.ctx, self.child_cursor(), NUiContext::BLUE, "✓", self.viewport_size, self.parent_content_pos, self.parent_content_size, false, TextDirection::Right);
        }
        self.finish_element(size, false);
        value
    }

    pub fn dropdown(&mut self, id: impl Hash, mut selected: usize, options: &[&str]) -> usize {
        let id = self.id(&id);

        let sizex = options.iter().map(|o| self.ctx.text_size(*o).x).max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Less)).unwrap_or(0.0);
        let arrow_size = self.ctx.text_size("▼").x + NUiContext::ELEMENT_GAP.x as f32;
        let button_size = Self::contain_size(Vec2::new(sizex + arrow_size, self.ctx.acent - self.ctx.decent));
        let hoverd = self.hoverd(self.cursor, button_size);
        let open = self.window.focused.as_ref().map(|e| e.focused == Some(id)).unwrap_or(false);

        let ds = DrawSettings{
            color: if hoverd {NUiContext::S1} else {NUiContext::S0},
            round_bottomleft: !open,
            round_bottomright: !open,
            ..Default::default()
        };

        self.window.draw_box(self.cursor, button_size, ds, self.viewport_size, self.parent_content_size, self.parent_content_pos);
        self.window.text(&self.ctx, self.child_cursor(), NUiContext::TEXT, options[selected], self.viewport_size, self.parent_content_pos, self.parent_content_size, false, TextDirection::Right);
        self.window.text(&self.ctx, self.child_cursor() + Vec2::new(sizex + NUiContext::ELEMENT_GAP.x as f32, if open {self.ctx.acent + 1.0}else{0.0}), NUiContext::TEXT, "▼", self.viewport_size, self.parent_content_pos, self.parent_content_size, false, if !open {TextDirection::Right} else {TextDirection::Up});
        if hoverd && self.left_mouse_pressed{
            if let Some(f) = &mut self.window.focused {
                if f.focused != Some(id) {
                    f.focused = Some(id);
                }else if f.focused == Some(id) {
                    f.focused = None;
                }
            }
        }
        if let Some(f) = &mut self.window.focused && f.focused == Some(id) {
            let mut cursor = self.cursor + Vec2::new(0.0, button_size.y);
            for (i, o) in options.iter().enumerate() {
                let last = i + 1 == options.len();
                let mut ds = DrawSettings {
                    color: NUiContext::S0,
                    border: Some(BorderSettings {
                        color_bottom: if last { NUiContext::S2 } else { NUiContext::S0 },
                        color_top: NUiContext::S0,
                        color_left: NUiContext::S2,
                        color_right: NUiContext::S2,
                        size: NUiContext::BORDER,
                    }),
                    round_bottomleft: last,
                    round_bottomright: last,
                    round_topleft: false,
                    round_topright: false,
                    rounding: 0,
                    on_top: true,
                };
                if self.hoverd(cursor, button_size) {
                    self.hovered_smth = true;
                    ds.color = NUiContext::S1;
                    ds.border.as_mut().unwrap().color_top = NUiContext::S1;
                    if !last {
                        ds.border.as_mut().unwrap().color_bottom = NUiContext::S1;
                    }
                    if self.left_mouse_pressed {
                        selected = i;
                        if let Some(f) = &mut self.window.focused {
                            f.focused = None;
                        }
                    }
                }
                self.window.draw_box(cursor, button_size, ds, self.viewport_size, self.parent_content_size, self.parent_content_pos);
                self.window.text(&self.ctx, (cursor + Self::child_offset()).round(), NUiContext::TEXT, o, self.viewport_size, self.parent_content_pos, self.parent_content_size, true, TextDirection::Right);
                cursor.y += button_size.y;
            }
        }
        self.finish_element(button_size, false);
        selected
    }


    pub fn collapsable<R>(&mut self, label: impl Hash + AsRef<str>, children: impl FnOnce(&mut Self) -> R) -> Option<R> {
        let id = self.id(&label);

        let rmb = NUiContext::ROUNDING.max(NUiContext::BORDER) as f32;
        let size = Vec2::new(self.remaining_width() - (NUiContext::CHILD_PAD.x as f32 + rmb), self.ctx.acent - self.ctx.decent + (NUiContext::CHILD_PAD.y as f32 + rmb) * 2.0);
        let hoverd = self.hoverd(self.cursor, size);
    
        self.window.draw_box(self.cursor, size, DrawSettings::new(hoverd, false), self.viewport_size, self.parent_content_size, self.parent_content_pos);
        
        let text_cursor = self.cursor + NUiContext::CHILD_PAD.as_vec2() + rmb;
        if hoverd && self.left_mouse_pressed {
            if self.window.open_headers.contains(&id.into()) {
                self.window.open_headers.remove(&id.into());
            }else {
                self.window.open_headers.insert(id.into());
            }
        }

        let open = self.window.open_headers.contains(&id.into());

        self.window.text(&self.ctx, text_cursor + if !open {Vec2::new(0.0, self.ctx.acent + 1.0)} else {Vec2::ZERO}, NUiContext::TEXT, "▼", self.viewport_size, self.parent_content_pos, self.parent_content_size, false, if open {TextDirection::Right} else {TextDirection::Up});
        let end = self.ctx.text_size("▼").x;
        self.window.text(&self.ctx, Vec2::new(text_cursor.x + end + NUiContext::ELEMENT_GAP.x as f32 * 2.0, text_cursor.y), NUiContext::TEXT, label.as_ref(), self.viewport_size, self.parent_content_pos, self.parent_content_size, false, TextDirection::Right);

        self.finish_element(size, false);

        let prev = self.cursor;
        let pp = self.parent_content_pos;
        let ps = self.parent_content_size;
        self.cursor += NUiContext::INDENT.as_vec2();
        self.parent_content_size.x -= (self.cursor.x - self.parent_content_pos.x).max(0.0);
        self.parent_content_pos.x = self.parent_content_pos.x.max(self.cursor.x);
        let res = if open {
            Some(children(self))
        }else {
            None
        };
        let cursor = self.cursor;
        self.cursor = prev;
        self.parent_content_pos = pp;
        self.parent_content_size = ps;
        if open {
            self.finish_element(Vec2::new(0.0, cursor.y - prev.y), false);
        }
        res
    }

    pub fn remaining_width(&self) -> f32 {
        self.parent_content_pos.x + self.parent_content_size.x - self.cursor.x
    }

    pub fn color_picker(&mut self, id: impl Hash, color: Vec4) -> Vec4 {
        let id = self.id(&id);

        let picker_size = 150.0f32;
        let bar_width   = 14.0f32;
        let gap         = NUiContext::ELEMENT_GAP.x as f32;
        let rmb         = NUiContext::ROUNDING.max(NUiContext::BORDER) as f32;

        let input_width = picker_size / 4.0 - gap + NUiContext::BORDER as f32;
        let input_h     = self.ctx.acent - self.ctx.decent + (NUiContext::CHILD_PAD.as_vec2().y + rmb) * 2.0;
        let full_width  = picker_size + gap + bar_width + gap + bar_width;

        let total_size = Vec2::new(
            full_width,
            picker_size + gap + bar_width + gap + input_h,
        );

        let sv_pos      = self.cursor;
        let hue_pos     = self.cursor + Vec2::new(picker_size + gap, 0.0);
        let alpha_pos   = self.cursor + Vec2::new(picker_size + gap + bar_width + gap, 0.0);
        let preview_pos = self.cursor + Vec2::new(0.0, picker_size + gap);
        let preview_size = Vec2::new(full_width, bar_width);
        let inputs_pos  = self.cursor + Vec2::new(0.0, picker_size + gap + bar_width + gap);

        let (mut h, mut s, mut v, mut a) = rgb_to_hsv(color);

        let id_sv    = NonZeroU64::new(id.get()    ).unwrap();
        let id_hue   = NonZeroU64::new(id.get() + 1).unwrap();
        let id_alpha = NonZeroU64::new(id.get() + 2).unwrap();

        if let Some(cursor_pos) = self.cursor_pos {
            if self.left_mouse_pressing {
                if let Some(f) = &mut self.window.focused {
                    if self.left_mouse_pressed {
                        let sv_rect    = Rect::from_corners(sv_pos,    sv_pos    + Vec2::splat(picker_size));
                        let hue_rect   = Rect::from_corners(hue_pos,   hue_pos   + Vec2::new(bar_width, picker_size));
                        let alpha_rect = Rect::from_corners(alpha_pos, alpha_pos + Vec2::new(bar_width, picker_size));
                        if sv_rect.contains(cursor_pos) {
                            f.draging = Some(id_sv);
                        } else if hue_rect.contains(cursor_pos) {
                            f.draging = Some(id_hue);
                        } else if alpha_rect.contains(cursor_pos) {
                            f.draging = Some(id_alpha);
                        }
                    }
                    if f.draging == Some(id_sv) {
                        s = ((cursor_pos.x - sv_pos.x) / picker_size).clamp(0.0, 1.0);
                        v = 1.0 - ((cursor_pos.y - sv_pos.y) / picker_size).clamp(0.0, 1.0);
                    } else if f.draging == Some(id_hue) {
                        h = ((cursor_pos.y - hue_pos.y) / picker_size).clamp(0.0, 1.0);
                    } else if f.draging == Some(id_alpha) {
                        a = 1.0 - ((cursor_pos.y - alpha_pos.y) / picker_size).clamp(0.0, 1.0);
                    }
                }
            }
        }

        let new_color = hsv_to_rgb(h, s, v, a);
        let pure_hue  = hsv_to_rgb(h, 1.0, 1.0, 1.0);
        let half_vp   = self.viewport_size / 2.0;
        let clip_min  = self.parent_content_pos;
        let clip_max  = self.parent_content_pos + self.parent_content_size;
        let solid     = Vec2::splat(20.0);

        let to_ndc = |p: Vec2| (p / half_vp) - Vec2::splat(1.0);

        let emit_quad = |
            verts: &mut Vec<UIVertex>,
            idxs: &mut Vec<u32>,
            corners: [(Vec2, Vec4); 4],
        | {
            let min_x = corners.iter().fold(f32::MAX, |acc, (p, _)| acc.min(p.x));
            let min_y = corners.iter().fold(f32::MAX, |acc, (p, _)| acc.min(p.y));
            let max_x = corners.iter().fold(f32::MIN, |acc, (p, _)| acc.max(p.x));
            let max_y = corners.iter().fold(f32::MIN, |acc, (p, _)| acc.max(p.y));

            let cmin_x = min_x.max(clip_min.x);
            let cmin_y = min_y.max(clip_min.y);
            let cmax_x = max_x.min(clip_max.x);
            let cmax_y = max_y.min(clip_max.y);

            if cmin_x >= cmax_x || cmin_y >= cmax_y { return; }

            let x_range = max_x - min_x;
            let y_range = max_y - min_y;

            let bilerp = |px: f32, py: f32| -> Vec4 {
                let tx = if x_range > 0.0 { (px - min_x) / x_range } else { 0.0 };
                let ty = if y_range > 0.0 { (py - min_y) / y_range } else { 0.0 };
                let top    = corners[0].1.lerp(corners[1].1, tx);
                let bottom = corners[3].1.lerp(corners[2].1, tx);
                top.lerp(bottom, ty)
            };

            let vi = verts.len() as u32;
            verts.extend_from_slice(&[
                UIVertex { pos: to_ndc(Vec2::new(cmin_x, cmin_y)), color: bilerp(cmin_x, cmin_y), uv: solid },
                UIVertex { pos: to_ndc(Vec2::new(cmax_x, cmin_y)), color: bilerp(cmax_x, cmin_y), uv: solid },
                UIVertex { pos: to_ndc(Vec2::new(cmax_x, cmax_y)), color: bilerp(cmax_x, cmax_y), uv: solid },
                UIVertex { pos: to_ndc(Vec2::new(cmin_x, cmax_y)), color: bilerp(cmin_x, cmax_y), uv: solid },
            ]);
            idxs.extend_from_slice(&[vi, vi+1, vi+2, vi, vi+3, vi+2]);
        };

        let checker = |win: &mut UiWindow, pos: Vec2, size: Vec2| {
            let check = 3.0f32;
            let dark  = Vec4::new(0.4, 0.4, 0.4, 1.0);
            let light = Vec4::new(0.7, 0.7, 0.7, 1.0);
            let cols = (size.x / check).ceil() as u32;
            let rows = (size.y / check).ceil() as u32;
            for row in 0..rows {
                for col in 0..cols {
                    let c = if (row + col) % 2 == 0 { dark } else { light };
                    let p = pos + Vec2::new(col as f32 * check, row as f32 * check);
                    let s = Vec2::new(
                        check.min(size.x - col as f32 * check),
                        check.min(size.y - row as f32 * check),
                    );
                    win.rect(p, s, None, c, self.viewport_size, self.parent_content_size, self.parent_content_pos, false);
                }
            }
        };

        {
            let tl = sv_pos;
            let tr = sv_pos + Vec2::new(picker_size, 0.0);
            let br = sv_pos + Vec2::splat(picker_size);
            let bl = sv_pos + Vec2::new(0.0, picker_size);

            emit_quad(&mut self.window.verticies, &mut self.window.indicies, [
                (tl, Vec4::ONE),
                (tr, pure_hue),
                (br, pure_hue),
                (bl, Vec4::ONE),
            ]);
            emit_quad(&mut self.window.verticies, &mut self.window.indicies, [
                (tl, Vec4::new(0.0, 0.0, 0.0, 0.0)),
                (tr, Vec4::new(0.0, 0.0, 0.0, 0.0)),
                (br, Vec4::new(0.0, 0.0, 0.0, 1.0)),
                (bl, Vec4::new(0.0, 0.0, 0.0, 1.0)),
            ]);

            let cx = sv_pos + Vec2::new(s * picker_size, (1.0 - v) * picker_size);
            let cross = 4.0f32;
            self.window.rect(cx - Vec2::new(cross, 1.0), Vec2::new(cross * 2.0, 2.0), None, Vec4::ONE, self.viewport_size, self.parent_content_size, self.parent_content_pos, false);
            self.window.rect(cx - Vec2::new(1.0, cross), Vec2::new(2.0, cross * 2.0), None, Vec4::ONE, self.viewport_size, self.parent_content_size, self.parent_content_pos, false);
        }

        {
            let sextants: [(f32, f32); 6] = [
                (0.0/6.0, 1.0/6.0), (1.0/6.0, 2.0/6.0), (2.0/6.0, 3.0/6.0),
                (3.0/6.0, 4.0/6.0), (4.0/6.0, 5.0/6.0), (5.0/6.0, 6.0/6.0),
            ];
            for (t0, t1) in sextants {
                let y0 = hue_pos.y + t0 * picker_size;
                let y1 = hue_pos.y + t1 * picker_size;
                let c0 = hsv_to_rgb(t0, 1.0, 1.0, 1.0);
                let c1 = hsv_to_rgb(t1, 1.0, 1.0, 1.0);
                emit_quad(&mut self.window.verticies, &mut self.window.indicies, [
                    (Vec2::new(hue_pos.x,             y0), c0),
                    (Vec2::new(hue_pos.x + bar_width, y0), c0),
                    (Vec2::new(hue_pos.x + bar_width, y1), c1),
                    (Vec2::new(hue_pos.x,             y1), c1),
                ]);
            }
            let cy = hue_pos.y + h * picker_size;
            self.window.rect(Vec2::new(hue_pos.x, cy - 1.0), Vec2::new(bar_width, 2.0), None, Vec4::ONE, self.viewport_size, self.parent_content_size, self.parent_content_pos, false);
        }

        {
            checker(&mut self.window, alpha_pos, Vec2::new(bar_width, picker_size));
            let c_top = Vec4::new(new_color.x, new_color.y, new_color.z, 1.0);
            let c_bot = Vec4::new(new_color.x, new_color.y, new_color.z, 0.0);
            emit_quad(&mut self.window.verticies, &mut self.window.indicies, [
                (alpha_pos,                                      c_top),
                (alpha_pos + Vec2::new(bar_width, 0.0),         c_top),
                (alpha_pos + Vec2::new(bar_width, picker_size), c_bot),
                (alpha_pos + Vec2::new(0.0,       picker_size), c_bot),
            ]);
            let cy = alpha_pos.y + (1.0 - a) * picker_size;
            self.window.rect(Vec2::new(alpha_pos.x, cy - 1.0), Vec2::new(bar_width, 2.0), None, Vec4::ONE, self.viewport_size, self.parent_content_size, self.parent_content_pos, false);
        }

        {
            checker(&mut self.window, preview_pos, preview_size);
            self.window.rect(preview_pos, preview_size, None, new_color, self.viewport_size, self.parent_content_size, self.parent_content_pos, false);
        }

        {
            let saved_cursor   = self.cursor;
            let saved_direction = self.direction;

            self.cursor = inputs_pos;
            self.direction = true;

            let r = self.float_input(id.get() + 10, new_color.x, input_width);
            let g = self.float_input(id.get() + 11, new_color.y, input_width);
            let b = self.float_input(id.get() + 12, new_color.z, input_width);
            let a = self.float_input(id.get() + 13, new_color.w, input_width);

            self.cursor = saved_cursor;
            self.direction = saved_direction;

            self.finish_element(total_size, false);

            Vec4::new(r, g, b, a).clamp(Vec4::ZERO, Vec4::ONE)
        }
    }

    pub fn container<R>(&mut self, id: impl Hash, size: Vec2, f: impl FnOnce(&mut Self) -> R) -> R{
        let id = self.id(&id);
        let scroll = self.window.scrollables.entry(id.into()).or_insert(Scrollable { content_size: Vec2::ZERO, scroll: Vec2::ZERO }).scroll;
        
        self.window.draw_box(self.cursor, size, DrawSettings::default(), self.viewport_size, self.parent_content_size, self.parent_content_pos);

        let prev_cursor = self.cursor;
        let pp = self.parent_content_pos;
        let ps = self.parent_content_size;

        let (new_pp, new_ps) = self.parent_bounds(self.cursor, size);

        let content_max = self.content_max;
        self.parent_content_pos = new_pp;
        self.parent_content_size = new_ps;
        self.cursor = (self.cursor + NUiContext::WINDOW_PAD.as_vec2() - scroll).round();
        let org = self.cursor;
        self.content_max = Vec2::ZERO;

        let r = f(self);

        let (_, mut scrollable) = self.window.scrollables.remove_entry(&id.into()).unwrap();
        scrollable.content_size = self.content_max - org;
        scrollable.draw(id, size, prev_cursor, self.window, self.viewport_size, self.cursor_pos, self.left_mouse_pressed, self.parent_content_size, self.parent_content_pos);
        
        if self.hoverd(prev_cursor, size) && !self.scroll_consumed {
            scrollable.scroll(self.scroll_delta, size);
        }

        self.window.scrollables.insert(id.into(), scrollable);
        self.cursor = prev_cursor;
        self.content_max = content_max;
        self.finish_element(size, true);
        self.parent_content_pos = pp;
        self.parent_content_size = ps;
        r
    }

    pub fn horizontal(&mut self) {
        if !self.direction {
            self.direction = true;
            self.prev_cursor = self.cursor;
            self.line_height = 0.0;
        }
    }

    pub fn vertical(&mut self) {
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
    atlas_size: U16Vec2,
    bounds: Vec2,
    min: Vec2,
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

    pub const TRACE: Vec4 = Vec4::new(0.380, 0.380, 0.380, 1.0);
    pub const DEBUG: Vec4 = Vec4::new(0.400, 0.560, 0.700, 1.0);
    pub const INFO: Vec4 = Vec4::new(0.820, 0.820, 0.820, 1.0);
    pub const WARN: Vec4 = Vec4::new(0.980, 0.760, 0.110, 1.0);
    pub const ERROR: Vec4 = Vec4::new(0.950, 0.180, 0.180, 1.0);
    pub const BLUE_REALY_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.35);

    pub const ATLAS_WIDTH: u32 = 2048;
    pub const PAD: u32 = 0;
    pub const FONTSCALE: f32 = 15.0;
    pub const DRAG_THRESHHOLD: f32 = 10.0;

    pub const ELEMENT_GAP: UVec2 = UVec2::new(4, 2);
    pub const WINDOW_ROUNDING: u32 = 4;
    pub const ROUNDING: u32 = 2;
    pub const BORDER: u32 = 1;
    pub const CHILD_PAD: UVec2 = UVec2::new(2, 1);
    pub const INDENT: UVec2 = UVec2::new(40, 0);
    pub const WINDOW_PAD: UVec2 = UVec2::new(3, 2);

    pub const BAR_THICKNESS: f32 = 6.0f32;
    pub const MIN_THUMB: f32     = 20.0f32;

    pub(crate) fn build_ui_resources(&mut self) -> Result<NUiResources> {
        let bytes = fs::read(&self.font)?;
        let font = Font::from_bytes(bytes, self.font_settings).unwrap();

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
        self.acent = font_metrics.ascent;
        self.new_line_size = font_metrics.new_line_size;
        self.decent = font_metrics.descent;

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
                    atlas_size: U16Vec2::new(metrics.width as u16, metrics.height as u16),
                    min: Vec2::new(metrics.xmin as f32, metrics.bounds.ymin as f32),
                    bounds: Vec2::new(metrics.bounds.width, metrics.bounds.height),
                    advance_width: metrics.advance_width,
                },
            );

            for y in 0..metrics.height as u32 {
                for x in 0..metrics.width as u32 {
                    let idx = (y * metrics.width as u32 + x) as usize;
                    atlas_data[((row_start + y) * Self::ATLAS_WIDTH + x + row_length) as usize] =
                        data[idx];
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

    fn text_size(&self, str: &str) -> Vec2 {
        let mut len = 0.0;
        let mut height = 0.0f32;
        for char in str.chars() {
            if let Some(b) = self.atlas_lut.get(&char) {
                len += b.advance_width;
                height = height.max(b.bounds.y as f32)
            } else {
                let missing = self.atlas_lut.get(&'?').unwrap();
                len += missing.advance_width;
                height = height.max(missing.bounds.y as f32)
            }
        }
        Vec2::new(len, height)
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
    }
    let mut ctx = world.get_resource_mut::<NUiContext>().unwrap();
    cmd.insert_resource(ctx.build_ui_resources().unwrap());
}

pub fn nextract_ui(mut res: If<ResMut<NUiResources>>, windows: Extract<Query<&UiWindow>>) {
    for window in windows
        .iter()
        .sort_by::<&UiWindow>(|a, b| a.layer.cmp(&b.layer))
    {
        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies
            .extend(window.indicies.iter().map(|e| *e + vertex_offset as u32));
        res.pending_verticies.extend(window.verticies.iter());
        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies
            .extend(window.top_indicies.iter().map(|e| *e + vertex_offset as u32));
        res.pending_verticies.extend(window.top_verticies.iter());
    }
}

pub fn update_windows(
    mut windows: Query<(Entity, &mut UiWindow)>,
    desktop_window: Single<&Window>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    touch: Res<Touches>,
    ctx: Res<NUiContext>,
) {
    let mut left_mouse_pressed = mouse_buttons.just_pressed(MouseButton::Left);
    let mut cursor_pos = desktop_window.cursor_position();

    if let Some(touch) = touch.iter_just_pressed().next() {
        cursor_pos = Some(touch.position());
        left_mouse_pressed = true;
    }

    let mut now_focused = None;
    for (e, mut window) in windows
        .iter_mut()
        .sort_by::<&UiWindow>(|a, b| a.layer.cmp(&b.layer))
    {
        let header_h = (ctx.acent - ctx.decent + NUiContext::CHILD_PAD.y as f32 * 2.0).round();
        let border_rect = Rect::from_corners(
            window.size.min - Vec2::splat(NUiContext::DRAG_THRESHHOLD),
            window.size.max + Vec2::splat(NUiContext::DRAG_THRESHHOLD),
        );
        let header_rect = Rect::from_corners(
            window.size.min,
            window.size.min + Vec2::new(window.size.max.x - window.size.min.x, header_h),
        );
        if let Some(cursor_pos) = cursor_pos
            && ((border_rect.contains(cursor_pos) && window.open) || (header_rect.contains(cursor_pos) && !window.open))
            && now_focused == None
            && !window.indicies.is_empty()
        {
            if left_mouse_pressed && window.focused.is_none() {
                now_focused = Some(e);
                window.focused = Some(FocusedState {
                    format_string: String::new(),
                    cursor: TextCursor { byte_pos: 0 },
                    selected: 0..0,
                    offset: 0.0,
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
                if *e1 == focused {
                    std::cmp::Ordering::Greater
                } else if *e2 == focused {
                    std::cmp::Ordering::Less
                } else {
                    a.layer.cmp(&b.layer)
                }
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
        window.top_verticies.clear();
        window.top_indicies.clear();
    }
}

#[derive(Component, Reflect)]
#[component(storage = "SparseSet")]
#[reflect(Component)]
pub struct TestWindow;

pub fn test_ui(mut cmd: Commands, mut ui: UiBuilder<TestWindow>, mut value: Local<(f32, String, bool, usize, Vec4)>) {
    ui.build_or(
        || {
            cmd.spawn((
                UiWindow::new("Entity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdlEntity 5 adhiasodhasdklagsdgasldgalsdglaksdasdl"),
                TestWindow,
            ));
        },
        |b| {
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
            b.vertical();

            value.0 = b.slider(id!(), -10.0, 8.0, 100.0, value.0);

            b.horizontal();
            value.0 = b.slider(id!(), -10.0, 8.0, 100.0, value.0);
            value.0 = b.slider(id!(), -10.0, 8.0, 100.0, value.0);
            b.text(format!("{}", value.0));
            b.text("Test");
            b.vertical();

            value.0 = b.slider(id!(), -10.0, 8.0, 100.0, value.0);
            value.0 = b.slider(id!(), -10.0, 8.0, 100.0, value.0);
            value.0 = b.float_input(id!(), value.0, 100.0);
            b.text_input(id!(), &mut value.1, 100.0);
            value.2 = b.check_box(value.2);

            value.3 = b.dropdown(id!(), value.3, &[
                "Test1",
                "Test2",
                "Test3",
            ]);

            b.text("Test");
            if b.button("Dont Press Me") {
                log::error!("YOU PRESSED ME");
            }

            b.collapsable("Collapsable1", |ui| {
                if ui.button("Child") {
                    log::info!("Test");
                }
                value.4 = ui.color_picker("Color Picker", value.4);
                ui.collapsable("Collapsable2", |ui| {
                    ui.button("Child2");
                });
            });

            b.container("Console123", Vec2::new(500.0, 500.0), |ui| {
                for i in 0..500 {
                    ui.text("Ich hab dich lieb Helena");
                }
            });
        },
    );
}
