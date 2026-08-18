use anyhow::Result;
use bevy::app::AppExit;

use bevy::ecs::change_detection::Tick;
use bevy::ecs::message::MessageReader;
use bevy::ecs::query::WorldQuery;
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
use imgui::ComboBoxHeight::Small;
use ini::Ini;
use itertools::Itertools;
use lava::state::raw_vulkan::native::{StdVideoH264DisableDeblockingFilterIdc_STD_VIDEO_H264_DISABLE_DEBLOCKING_FILTER_IDC_ENABLED, StdVideoH264DisableDeblockingFilterIdc_STD_VIDEO_H264_DISABLE_DEBLOCKING_FILTER_IDC_PARTIAL};
use lava::{
    buffer::*,
    image::{Image, format, usage},
};
use ron::ser::PrettyConfig;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::cell::UnsafeCell;
use std::collections::HashSet;
use std::f32::consts::PI;
use std::io::Write;
use std::marker::PhantomData;
use std::num::{NonZero, NonZeroU32, NonZeroU64};
use std::ops::{Add, RangeBounds};
use std::sync::Mutex;
use std::time::{Duration, Instant};
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

use crate::ui::OldUiContext;
use crate::ui::builder::{TextCursor, UiWindowBuilder};
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

#[derive(Clone)]
pub struct FocusedState {
    pub is_being_draged: bool,
    pub draging: Option<NonZeroU64>,
    pub focused: Option<NonZeroU64>,
    pub cursor: TextCursor,
    pub selected: Range<usize>,
    pub offset: f32,
    pub darg_start: Vec2,
    pub format_string: String,

    pub resize_top: bool,
    pub resize_bottom: bool,
    pub resize_left: bool,
    pub resize_right: bool,
}

impl Default for FocusedState {
    fn default() -> Self {
        Self {
            is_being_draged: false,
            draging: None,
            focused: None,
            cursor: TextCursor { byte_pos: 0 },
            selected: 0..0,
            offset: 0.0,
            darg_start: Vec2::ZERO,
            format_string: String::new(),
            resize_top: false,
            resize_bottom: false,
            resize_left: false,
            resize_right: false,
        }
    }
}

#[derive(Copy, Clone)]
pub struct Scrollable {
    pub content_size: Vec2,
    pub scroll: Vec2,
}

impl Scrollable {
    pub fn scroll(&mut self, delta: Vec2, size: Vec2) {
        let scrollbar_y = self.content_size.y > size.y;
        let scrollbar_x = self.content_size.x > size.x;

        self.scroll -= Vec2::new(
            scrollbar_x as u32 as f32 * delta.x,
            scrollbar_y as u32 as f32 * delta.y,
        );
        self.clamp_scroll(size);
    }

    pub fn clamp_scroll(&mut self, size: Vec2) {
        self.scroll = self.scroll.clamp(
            Vec2::ZERO,
            (self.content_size - size)
                .max(Vec2::ZERO),
        );
    }

    fn draw_bar(&mut self, id: NonZeroU64, area: Rect, window: &mut UiWindow, direction: bool, viewport_size: Vec2, cursor_pos: Option<Vec2>, left_mouse_pressed: bool, clip_rect: Rect) {
        let size = area.size();
        let pos = area.min;
        let b = UiContext::BORDER as f32;
        let track_pos = if direction {
            Vec2::new(
                pos.x + size.x - UiContext::BAR_THICKNESS - b,
                pos.y + b,
            ).round()
        } else {
            Vec2::new(
                pos.x + b,
                pos.y + size.y - UiContext::BAR_THICKNESS - b,
            ).round()
        };

        let track_size = if direction {
            Vec2::new(UiContext::BAR_THICKNESS, size.y - b * 2.0).round()
        }else {
            Vec2::new(size.x - b * 2.0, UiContext::BAR_THICKNESS).round()
        };
    
        window.rect(Rect::from_corners(track_pos, track_pos + track_size), None, UiContext::S0,
            viewport_size, clip_rect, false);

        let scroll_max = (self.content_size - size).max(Vec2::ONE);

        let ratio    = (size / self.content_size).clamp(Vec2::ZERO, Vec2::ONE);
        let thumb_width  = (track_size * ratio).max(Vec2::splat(UiContext::MIN_THUMB)).round();
        let thumb_t  = (self.scroll / scroll_max).clamp(Vec2::ZERO, Vec2::ONE);
        let thumb  = (track_pos + thumb_t * (track_size - thumb_width)).round();
        let thumb_pos  = if direction {
            Vec2::new(track_pos.x, thumb.y) 
        } else {
            Vec2::new(thumb.x, track_pos.y) 
        };
        let thumb_size = if direction {
            Vec2::new(UiContext::BAR_THICKNESS, thumb_width.y)
        }else {
            Vec2::new(thumb_width.x, UiContext::BAR_THICKNESS)
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
            let grab_offset = window.focused.as_ref().map(|f| f.darg_start).unwrap_or(Vec2::ZERO);
            let new_thumb = p - track_pos;
            let travel = (track_size - thumb_width).max(Vec2::ONE);
            if direction {
                let t = ((new_thumb.y - grab_offset.y) / travel.y).clamp(0.0, 1.0);
                self.scroll.y = t * scroll_max.y;
            } else {
                let t = ((new_thumb.x - grab_offset.x) / travel.x).clamp(0.0, 1.0);
                self.scroll.x = t * scroll_max.x;
            }
        }

        let thumb_color = if dragging || hovered { UiContext::GRAB_HOT } else { UiContext::GRAB };
        let ds = DrawSettings {
            color: thumb_color,
            ..Default::default()
        };
        window.draw_box(from_pos_size(thumb_pos, thumb_size), ds,
            viewport_size, clip_rect);
    }

    pub fn draw(&mut self, id: NonZeroU64, area: Rect, window: &mut UiWindow, viewport_size: Vec2, cursor_pos: Option<Vec2>, left_mouse_pressed: bool, clip_rect: Rect) {
        if self.content_size.y > area.size().y {
            self.draw_bar(id, area, window, true, viewport_size, cursor_pos, left_mouse_pressed, clip_rect);
        }
        if self.content_size.x > area.size().x {
            self.draw_bar(id, area, window, false, viewport_size, cursor_pos, left_mouse_pressed, clip_rect);
        }
    }

}

pub struct UiWindow {
    pub label: String,
    pub open: bool,
    pub docked: bool,
    pub sibling_pressed: Option<u32>,
    pub layer: u32,
    pub open_headers: HashSet<u64>,
    pub scrollables: HashMap<u64, Scrollable>,
    pub focused: Option<FocusedState>,
    pub dock_rect: Rect,
    pub rect: Rect,
    pub verticies: Vec<UIVertex>,
    pub indicies: Vec<u32>,
    pub top_verticies: Vec<UIVertex>,
    pub top_indicies: Vec<u32>,
}
use bevy::ecs::{
    archetype::Archetype,
    component::{ComponentId, Components},
    query::{FilteredAccess, QueryData, ReadOnlyQueryData},
    storage::Table,
    world::{unsafe_world_cell::UnsafeWorldCell, World},
};

#[derive(SystemParam)]
pub struct UiBuilder<'w, 's> {
    window: Single<'w, 's, lifetimeless::Read<Window>>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    touch: Res<'w, Touches>,
    scroll: Res<'w, AccumulatedMouseScroll>,
    ctx: Res<'w, UiContext>,
    keys: MessageReader<'w, 's, KeyboardInput>,
    keyspressed: Res<'w, ButtonInput<KeyCode>>,
}

impl<'s, 'w> UiBuilder<'w, 's> {
    pub fn build(
        &mut self,
        label: impl AsRef<str>,
        f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>),
    ) {
        let Some(window_idx) = self.ctx.window_labels.get(label.as_ref()) else { 
            let Ok(mut lock) = self.ctx.add_windows.lock() else {
                return;
            };
            lock.push(label.as_ref().into());
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

        let Ok(mut window) = self.ctx.windows[*window_idx as usize].lock() else {
            return;
        };
        window.build(mouse, ctx, &self.window, touch, scroll, &mut self.keys, shift, strg, f);
    }
}

#[derive(Copy, Clone)]
pub struct BorderSettings {
    pub color_top: Vec4,
    pub color_bottom: Vec4,
    pub color_left: Vec4,
    pub color_right: Vec4,
    pub size: u32,
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
    pub color: Vec4,
    pub on_top: bool,

    pub rounding: u32,
    pub round_topleft: bool,
    pub round_topright: bool,
    pub round_bottomleft: bool,
    pub round_bottomright: bool,
 
    pub border: Option<BorderSettings>,
}

impl Default for DrawSettings {
    fn default() -> Self {
        let bc = UiContext::S2;
        Self {
            color: UiContext::S0,
            rounding: UiContext::ROUNDING,
            border: Some(BorderSettings {
                color_bottom: bc,
                color_left: bc,
                color_right: bc,
                color_top: bc,
                size: UiContext::BORDER,
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
                UiContext::S2
            }else if hoverd {
                UiContext::S1
            }else {
                UiContext::S0
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

pub fn from_pos_size(pos: Vec2, size: Vec2) -> Rect {
    Rect::from_corners(pos, pos + size)
}

impl UiWindow {
    pub fn size(&self) -> Rect {
        if self.docked {
            self.dock_rect
        }else {
            self.rect
        }
    }

    pub fn new(label: String, rect: Rect, open: bool, docked: bool) -> Self {
        UiWindow {
            label,
            sibling_pressed: None,
            layer: 0,
            dock_rect: Rect {
                min: rect.min.round(),
                max: rect.max.round(),
            },
            docked,
            scrollables: HashMap::new(),
            open,
            open_headers: HashSet::new(),
            focused: None,
            rect: Rect {
                min: rect.min.round(),
                max: rect.max.round(),
            },
            verticies: Vec::new(),
            indicies: Vec::new(),
            top_indicies: Vec::new(),
            top_verticies: Vec::new()
        }
    }

    pub fn draw_box(
        &mut self,
        rect: Rect,
        ds: DrawSettings,
        viewport_size: Vec2,
        clip_rect: Rect,
    ) -> (usize, usize) {
        let size = rect.size();
        let pos = rect.min;
        let b = ds.border.map(|b| b.size).unwrap_or(0) as f32;
        let r = ds.rounding as f32;
        let rmb = r.max(b) as f32;

        let start_idx = self.verticies.len();
        self.rect(
            rect.inflate(-rmb),
            None,
            ds.color,
            viewport_size,
            clip_rect,
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
                from_pos_size(
                pos + Vec2::new(rmb * ds.round_topleft as u32 as f32, 0.0),
                Vec2::new(
                    size.x - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32),
                    rmb,
                ),
            ),
                None,
                ds.color,
                viewport_size,
                clip_rect,
                ds.on_top
            );
            self.rect(
                from_pos_size(

                pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - rmb),
                Vec2::new(
                    size.x
                        - rmb * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32) as f32),
                    rmb,
                ),
            ),
                None,
                ds.color,
                viewport_size,
                clip_rect,
                ds.on_top
            );
            self.rect(
                from_pos_size(
                pos + Vec2::new(0.0, rmb),
                Vec2::new(rmb, size.y - rmb * 2.0),
                ),
                None,
                ds.color,
                viewport_size,
                clip_rect,
                ds.on_top
            );
            self.rect(
                from_pos_size(
                pos + Vec2::new(size.x - rmb, rmb),
                Vec2::new(rmb, size.y - rmb * 2.0),
                ),
                None,
                ds.color,
                viewport_size,
                clip_rect,
                ds.on_top
            );
        }
        
        if ds.rounding != 0 {
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
                    clip_rect,
                    ds.on_top
                );
                if r > b {
                    self.round_corner(
                        center,
                        r - b,
                        start_angle,
                        ds.color,
                        viewport_size,
                        clip_rect,
                        ds.on_top
                    );
                }
            }
        }

        if let Some(border) = border {
            self.rect(
                from_pos_size(
                pos + Vec2::new(rmb * ds.round_topleft as u32 as f32, 0.0),
                Vec2::new(
                    size.x - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32),
                    b,
                ),
            ),
                None,
                border.color_top,
                viewport_size,
                clip_rect,
                ds.on_top
            );
            self.rect(
                from_pos_size(
                pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - b),
                Vec2::new(
                    size.x
                        - rmb * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32) as f32),
                    b,
                ),
            ),
                None,
                border.color_bottom,
                viewport_size,
                clip_rect,
                ds.on_top
            );
            self.rect(
                from_pos_size(
                pos + Vec2::new(0.0, rmb * ds.round_topleft as u32 as f32),
                Vec2::new(
                    b,
                    size.y - rmb * ((ds.round_topleft as u32 + ds.round_bottomleft as u32) as f32),
                ),
            ),
                None,
                border.color_left,
                viewport_size,
                clip_rect,
                ds.on_top
            );
            self.rect(
                from_pos_size(
                pos + Vec2::new(size.x - b, rmb * ds.round_topright as u32 as f32),
                Vec2::new(
                    b,
                    size.y
                        - rmb * ((ds.round_topright as u32 + ds.round_bottomright as u32) as f32),
                ),
            ),
                None,
                border.color_right,
                viewport_size,
                clip_rect,
                ds.on_top
            );
        }
        let end_idx = self.verticies.len();

        (start_idx, end_idx)
    }

    pub fn build<'w, 's, R>(
        &mut self,
        buttons: Res<'w, ButtonInput<MouseButton>>,
        ctx: Res<'w, UiContext>,
        window: &Window,
        touch: Res<'w, Touches>,
        scroll: Res<'w, AccumulatedMouseScroll>,
        keys: &mut MessageReader<'w, 's, KeyboardInput>,
        shift: bool,
        ctrl: bool,
        f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>) -> R,
    ) -> Option<R> {
        let mut id = DefaultHasher::new();
        self.label.hash(&mut id);
        let id = id.finish();
        let viewport_size = window.size();

        let r = UiContext::WINDOW_ROUNDING as f32;
        let b = UiContext::BORDER as f32;
        let rmb = r.max(b);

        let header_h = (ctx.acent - ctx.decent + UiContext::WINDOW_PAD.y as f32 * 2.0).round();
        let focused = self.focused.is_some();
        let input = MultiInput::new(&window, &buttons, &touch);

        let content_area = Rect { min: self.size().min + Vec2::new(0.0, header_h), max: self.size().max };

        let (_, mut scrollable) = self.scrollables.remove_entry(&id).unwrap_or((id, Scrollable {
            content_size: content_area.size(),
            scroll: Vec2::ZERO
        }));
        let cursor =
            (content_area.min + rmb + UiContext::WINDOW_PAD.as_vec2() - scrollable.scroll).round();

        if !self.open {
            return None;
        }
        let bar_size = UiContext::BAR_THICKNESS * Vec2::new((scrollable.content_size.y > content_area.size().y) as u32 as f32, (scrollable.content_size.x > content_area.size().x) as u32 as f32);
        let clip_rect = from_pos_size(content_area.min + b, content_area.size() - b * 2.0 - bar_size);
        let max_width = (content_area.size().max(scrollable.content_size)).x - UiContext::WINDOW_PAD.x as f32 - rmb - bar_size.x;

        let mut builder = UiWindowBuilder {
            max_width,
            window_id: id,
            scroll_delta: scroll.delta,
            content_max: cursor,
            focuse_next: false,
            line_height: 0.0,
            ctx,
            clip_rect,
            window: self,
            viewport_size,
            cursor,
            cursor_origin: cursor,
            prev_element_hoverd: true,
            prev_element: content_area,
            input,
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

        if !builder.hovered_smth && input.left_mouse_pressed {
            if let Some(f) = &mut self.focused {
                f.focused = None;
            }
        }

        let content_size = content_max + UiContext::WINDOW_PAD.as_vec2() + rmb - cursor;
        
        scrollable.content_size = content_size;
        if !scroll_consumed && focused && self.open {
            scrollable.scroll(scroll.delta, content_area.size());
        }
        scrollable.draw(NonZeroU64::new(id).unwrap(), content_area, self, viewport_size, input.cursor_pos, input.left_mouse_pressed, self.size());
        self.scrollables.insert(id, scrollable);
        Some(r)
    }

    pub fn text(
        &mut self,
        ctx: &UiContext,
        pos: Vec2,
        color: Vec4,
        text: &str,
        viewport_size: Vec2,
        clip_rect: Rect,
        on_top: bool,
    ) -> Vec2 { 
        let mut pen = pos + Vec2::new(0.0, ctx.acent);
        for char in text.chars() {
            if char == '\n' {
                pen.x = pos.x;
                pen.y += ctx.new_line_size;
                continue;
            }
            let atlas_info = ctx
                .get_char(char);
            let tpos = Vec2::new(
                pen.x + atlas_info.min.x,
                pen.y - (atlas_info.height as f32 + atlas_info.min.y) 
            );
            let uv      = atlas_info.position.as_vec2() / ctx.atlas_size;
            let uv_size = atlas_info.atlas_size.as_vec2() / ctx.atlas_size;
            let size    = atlas_info.atlas_size.as_vec2();
            let rect = Rect::from_corners(tpos, tpos+size);
            self.rect(rect, Some((uv, uv_size)), color, viewport_size, clip_rect, on_top);
            pen.x += atlas_info.advance_width;
        }

        pen
    }

    pub fn text_direction(
        &mut self,
        ctx: &UiContext,
        pos: Vec2,
        color: Vec4,
        text: &str,
        viewport_size: Vec2,
        clip_rect: Rect,
        on_top: bool,
        direction: TextDirection,
    ) -> Vec2 {
        let clip_min = clip_rect.min;
        let clip_max =  clip_rect.max;
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

            let atlas_info = ctx.get_char(char);

            let uv      = atlas_info.position.as_vec2() / ctx.atlas_size;
            let uv_size = atlas_info.atlas_size.as_vec2() / ctx.atlas_size;
            let size    = atlas_info.atlas_size.as_vec2();

            let local_x = atlas_info.min.x;
            let local_y = -(atlas_info.height + atlas_info.min.y);

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
    pub fn rect(
        &mut self,
        rect: Rect,
        uv: Option<(Vec2, Vec2)>,
        color: Vec4,
        view_port_size: Vec2,
        clip_rect: Rect,
        on_top: bool,
    ) {
        let clipped_rect = clip_rect.intersect(rect);
        if clipped_rect.is_empty() {
            return;
        }

        let (clipped_uv_min, clipped_uv_max) = if let Some((uv, uv_size)) = uv {
            let uv_scale = uv_size / rect.size();
            let clipped_uv_min = uv + (clipped_rect.min - rect.min) * uv_scale;
            let clipped_uv_max = uv + (clipped_rect.max - rect.min) * uv_scale;
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
                pos: (clipped_rect.min / half_vp) - Vec2::splat(1.0),
                uv: clipped_uv_min,
            },
            UIVertex {
                color,
                pos: (clipped_rect.min.with_x(clipped_rect.max.x) / half_vp) - Vec2::splat(1.0),
                uv: clipped_uv_min.with_x(clipped_uv_max.x),
            },
            UIVertex {
                color,
                pos: (clipped_rect.max / half_vp) - Vec2::splat(1.0),
                uv: clipped_uv_max,
            },
            UIVertex {
                color,
                pos: (clipped_rect.min.with_y(clipped_rect.max.y) / half_vp) - Vec2::splat(1.0),
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
        clip_rect: Rect,
        on_top: bool
    ) {
        let segments = rounding.ceil() as u32;
        let half_vp = view_port_size / 2.0;
        let clip_min = clip_rect.min;
        let clip_max = clip_rect.max;

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


#[derive(Serialize, Deserialize, Clone)]
pub enum Split {
    Horizontal,
    Vertical
}

impl Split {
    pub fn direction_vec(&self) -> Vec2 {
        match self {
            Self::Horizontal => Vec2::new(1.0, 0.0),
            Self::Vertical => Vec2::new(0.0, 1.0),
        }
    }
    pub fn to_bytes(&self) -> u8 {
        match self {
            Self::Horizontal => 0,
            Self::Vertical => 1,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Resource)]
pub enum DockingNode {
    Leaf {
        focused: u32,
        windows: SmallVec<[u32; 8]>,
        root: bool,
    },
    Node {
        split: Split,
        extend: f32,
        left: Box<DockingNode>,
        right: Box<DockingNode>,
    }
}

impl DockingNode {
    fn split_area(area: Rect, split: Split, extend: f32) -> (Rect, Rect) {
        let size = (1.0 - extend) * area.size();
        let left_area = Rect {min: area.min, max: area.max - split.direction_vec() * size};
        let size = extend * area.size();
        let right_area = Rect {min: area.min + split.direction_vec() * size, max: area.max};
        (left_area, right_area)
    }

    fn dock(&mut self, window: u32, cursor_pos: Vec2, area: Rect, header_h: f32) -> bool {
        match self {
            DockingNode::Leaf { windows, root, focused } => {
                if from_pos_size(area.min, Vec2::new(area.width(), header_h)).contains(cursor_pos) {
                    windows.push(window);
                    return true;
                }else if area.contains(cursor_pos) {
                    let thickness = 40.0;
                    let top = Rect::from_corners(area.min, Vec2::new(area.max.x, area.min.y + thickness)).contains(cursor_pos);
                    let bottom = Rect::from_corners(Vec2::new(area.min.x, area.max.y - thickness), area.max).contains(cursor_pos);
                    let left = Rect::from_corners(Vec2::new(area.min.x, area.min.y + thickness), Vec2::new(area.min.x + thickness, area.max.y - thickness)).contains(cursor_pos);
                    let right = Rect::from_corners(Vec2::new(area.max.x - thickness, area.min.y + thickness), Vec2::new(area.max.x, area.max.y - thickness)).contains(cursor_pos);
                    
                    let split = if bottom || top {Split::Vertical} else {Split::Horizontal};
                    
                    if right || bottom {
                        let right = Box::new(DockingNode::Leaf { windows: SmallVec::from_slice(&[window]), root: false, focused: 0 });
                        let root = std::mem::take(self);
                        *self = DockingNode::Node { split, extend: 0.5, left: Box::new(root), right };
                        return true;
                    }else if left || top{
                        let left = Box::new(DockingNode::Leaf { windows: SmallVec::from_slice(&[window]), root: false, focused: 0 });
                        let root = std::mem::take(self);
                        *self = DockingNode::Node { split, extend: 0.5, left, right: Box::new(root) };
                        return true;
                    }else {
                        return false;
                    }
                }
                return false;
            },
            DockingNode::Node { split, extend, left, right } => {
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);
                if left_area.contains(cursor_pos) {
                    Self::dock(left, window, cursor_pos, left_area, header_h)
                }else if right_area.contains(cursor_pos) {
                    Self::dock(right, window, cursor_pos, right_area, header_h)
                }else {
                    return false;
                }
            },
        }
    }

    fn undock(&mut self, window: u32) -> bool {
        match self {
            DockingNode::Leaf { windows, root, focused } => {
                if let Some(i) = windows.iter().position(|e| *e == window) {
                    windows.remove(i);
                }
                windows.is_empty() && !*root
            },
            DockingNode::Node { split: _, extend: _, left, right } => {
                let left_empty = left.undock(window);
                let right_empty = right.undock(window);
                if left_empty && right_empty {
                    return true;
                }
                if left_empty {
                    let root = std::mem::take(right);
                    *self = *root; 
                } else if right_empty {
                    let root = std::mem::take(left);
                    *self = *root;                
                }
                false
            },
        }
    }

    fn find_resize(node: &DockingNode, cursor_pos: Vec2, area: Rect, path: u64, depth: usize) -> (u64, u32, Vec2) {
        match node {
            DockingNode::Leaf { windows, root, focused } => {(u64::MAX, 0, Vec2::ZERO)},
            DockingNode::Node { split, extend, left, right } => {
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);
                let thickness = 100.0;
                let d = split.direction_vec();
                let perp = d.yx();

                let split_point = area.min + d * (area.size() * *extend);
                let divider = Rect {
                    min: split_point - d * (thickness / 2.0),
                    max: split_point + d * (thickness / 2.0) + perp * area.size(),
                };

                let new_path = (path << 1) | 0b1;
                if divider.contains(cursor_pos) {
                    (path, depth as u32, area.min)
                }else if left_area.contains(cursor_pos) {
                    Self::find_resize(left, cursor_pos, left_area, new_path, depth + 1)
                }else if right_area.contains(cursor_pos) {
                    Self::find_resize(right, cursor_pos, right_area, path << 1, depth + 1)
                }else {
                    (u64::MAX, 0, Vec2::ZERO)
                }
            },
        }
    }

    fn resize(node: &mut DockingNode, path: u64, max_depth: u32, depth: u32, delta: Vec2, area: Rect) {
        match node {
            DockingNode::Node { split, extend, left, right } => {
                if depth == max_depth {
                    *extend = (delta.project_onto(split.clone().direction_vec()) / area.size()).length();
                }
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);

                if ((path >> depth) & 1) as u64 > 0 {
                    Self::resize(left.as_mut(), path, max_depth, depth + 1, delta, left_area);
                }else {
                    Self::resize(right.as_mut(), path, max_depth, depth + 1, delta, right_area);
                }
            }
            DockingNode::Leaf { .. } => {}
        }
    }

    fn dock_info(node: &DockingNode, window: u32, area: Rect) -> Option<(Rect, SmallVec<[u32; 8]>, u32)> {
        match node {
            DockingNode::Leaf { windows, root, focused } => {
                if windows.contains(&window) {
                    Some((area, windows.clone(), *focused))
                }else {
                    None
                }
            },
            DockingNode::Node { split, extend, left, right } => {
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);

                let left = Self::dock_info(left, window, left_area);
                if left.is_some() {
                    left
                }else {
                    Self::dock_info(right, window, right_area)
                }
            },
        }
    }
}

impl Default for DockingNode{
    fn default() -> Self {
        DockingNode::Leaf { windows: SmallVec::new(), focused: 0, root: false }
    }
}

#[derive(Resource)]
pub struct UiContext {
    pub font: Option<fontdue::Font>,
    pub atlas_lut: Box<[AtlasEntry]>,
    pub atlas_widths: Box<[f32]>,
    pub atlas_size: Vec2,
    pub new_line_size: f32,
    pub acent: f32,
    pub decent: f32,
    pub min_character_width: f32,
    pub max_character_width: f32,
    pub resize_path: u64,
    pub resize_depth: u32,
    pub drag_start: Vec2,
}

#[derive(Resource)]
pub struct UiWindows {
    pub add_windows: Mutex<SmallVec<[String; 4]>>,
    pub window_labels: HashMap<String, u32>,
    pub windows: Vec<Mutex<UiWindow>>,
}


#[derive(Default, Copy, Clone)]
pub struct AtlasEntry {
    pub position: U16Vec2,
    pub atlas_size: U16Vec2,
    pub min: Vec2,
    pub height: f32,
    pub advance_width: f32,
}

impl AtlasEntry {
    fn is_empty(&self) -> bool {
        self.position.x == u16::MAX && self.position.y == u16::MAX
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

impl UiContext {
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


    pub(crate) fn new() -> Result<(Self, UiWindows, DockingNode)> {
        let bytes = fs::read("/home/karsten/code/GameEngine/editor_font.ttf")?;
        let font = Font::from_bytes(bytes, FontSettings::default()).unwrap();
        let font_metrics = font.horizontal_line_metrics(Self::FONTSCALE).unwrap();

        let SaveState {docking_nodes, window_labels, windows} = ron::from_str(&fs::read_to_string("windows.ron").unwrap_or("".to_owned())).unwrap_or(SaveState { docking_nodes: DockingNode::Leaf { windows: SmallVec::new(), root: true, focused: 0 }, windows: Vec::new(), window_labels: HashMap::new() });
        let windows = UiWindows {
            add_windows: Mutex::new(SmallVec::new()),
            windows: windows.into_iter().map(|w| {Mutex::new(UiWindow::new(w.label, w.size, w.open, w.docked))}).collect(),
            window_labels,
        };

        let mut max_character_width = 0.0f32;
        let mut min_character_width = 0.0f32;
        let mut atlas_lut = vec![AtlasEntry {
                position: U16Vec2::MAX,
                ..Default::default()
            }; u16::MAX as usize].into_boxed_slice();

        let mut atlas_widths = vec![-1.0; u16::MAX as usize].into_boxed_slice();

        for (c, _) in font.chars().iter() {
            let idx = *c as usize;
            if idx >= u16::MAX as usize{
                continue;
            }
            let metrics = font.metrics(*c, Self::FONTSCALE);
            if metrics.advance_width != 0.0 {
                max_character_width = max_character_width.max(metrics.advance_width);
                min_character_width = min_character_width.min(metrics.advance_width);
            }
            atlas_lut[idx] = AtlasEntry {
                position: U16Vec2::new(0, 0),
                atlas_size: U16Vec2::new(metrics.width as u16, metrics.height as u16),
                min: Vec2::new(metrics.xmin as f32, metrics.bounds.ymin as f32),
                height: metrics.bounds.height,
                advance_width: metrics.advance_width,
            };
            atlas_widths[idx] = metrics.advance_width;
        }

        atlas_lut['\t' as usize].advance_width = 32.0;
        atlas_lut['\t' as usize].position = U16Vec2::ZERO;

        atlas_widths['\t' as usize] = 32.0;


        Ok((Self {
            atlas_lut,
            atlas_widths,
            atlas_size: Vec2::ZERO,
            resize_path: u64::MAX,
            resize_depth: 0, 
            font: Some(font),
            max_character_width,
            min_character_width,
            new_line_size: font_metrics.new_line_size.round(),
            acent: font_metrics.ascent,
            decent: font_metrics.descent,
            drag_start: Vec2::ZERO,
        }, windows, docking_nodes))
    }

    pub(crate) fn build_ui_resources(&mut self) -> Result<NUiResources> {
        let font = self.font.take().unwrap();
        let mut chars = Vec::new();
        for (c, i) in font.chars().iter() {
            if (*c as usize) > u16::MAX as usize {
                continue;
            } 
            let (metrics, data) = font.rasterize_config(GlyphRasterConfig {
                glyph_index: (*i).into(),
                px: Self::FONTSCALE,
                font_hash: 0,
            });
            chars.push((metrics, data, *c));
        }
        chars.sort_unstable_by(|a, b| a.1.cmp(&b.1));

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

            self.atlas_lut[*char as usize] = 
                AtlasEntry {
                    position,
                    atlas_size: U16Vec2::new(metrics.width as u16, metrics.height as u16),
                    min: Vec2::new(metrics.xmin as f32, metrics.bounds.ymin as f32),
                    height: metrics.bounds.height,
                    advance_width: metrics.advance_width,
                };

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


    fn header_height(&self) -> f32 {
        (self.acent - self.decent + UiContext::CHILD_PAD.y as f32 * 2.0).round()
    }

    pub fn get_char(&self, c: char) -> AtlasEntry {
        if (c as usize) < u16::MAX as usize {
            let entry = self.atlas_lut[c as usize];
            if entry.is_empty() {
                self.atlas_lut['?' as usize]
            }else {
                entry
            }
        }else {
            self.atlas_lut['?' as usize]
        }
    }
    pub fn get_width(&self, c: char) -> f32 {
        if (c as usize) < u16::MAX as usize {
            let entry = self.atlas_widths[c as usize];
            if entry < 0.0 {
                self.atlas_widths['?' as usize]
            }else {
                entry
            }
        }else {
            self.atlas_widths['?' as usize]
        }
    }

    pub fn has_char(&self, c: char) -> bool {
        (c as usize) < u16::MAX as usize && !self.atlas_lut[c as usize].is_empty()
    }

    pub fn text_size(&self, str: &str) -> Vec2 {
        let mut len = 0.0f32;
        let mut line_length = 0.0f32;
        let mut height = self.new_line_size;
        for char in str.chars() {
            if char == '\n'{
                len = len.max(line_length);
                line_length = 0.0;
                height += self.new_line_size;
            }
            line_length += self.get_width(char);
        }
        len = len.max(line_length);
        Vec2::new(len, height)
    }

    pub fn text_len(&self, str: &str) -> f32 {
        let mut len = 0.0;
        for char in str.chars() {
            len += self.get_width(char);
        }
        len
    }

    pub fn min_text_len(&self, len: usize) -> f32 {
        len as f32 * self.min_character_width
    }

    pub fn max_text_len(&self, len: usize) -> f32 {
        len as f32 * self.max_character_width
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
    let mut ctx = world.get_resource_mut::<UiContext>().unwrap();
    cmd.insert_resource(ctx.build_ui_resources().unwrap());
}

pub fn nextract_ui(mut res: If<ResMut<NUiResources>>, ctx: Extract<Res<UiContext>>) {
    for window in ctx.windows
        .iter()
        .sorted_by_key(|a| a.lock().unwrap().layer)
    {
        let window = window.lock().unwrap();
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

#[derive(Clone, Copy)]
pub struct MultiInput {
    pub left_mouse_pressed: bool,
    pub left_mouse_pressing: bool,
    pub left_mouse_released: bool,
    pub cursor_pos: Option<Vec2>
}

impl MultiInput {
    fn new(desktop_window: &Window, buttons: &ButtonInput<MouseButton>, touch: &Touches) -> Self {
        let mut this = Self {
            left_mouse_pressed: buttons.just_pressed(MouseButton::Left),
            left_mouse_pressing: buttons.pressed(MouseButton::Left),
            left_mouse_released: buttons.just_released(MouseButton::Left),
            cursor_pos: desktop_window.cursor_position(),
        };
    
        if let Some(touch) = touch.iter().next() {
            this.cursor_pos = Some(touch.position());
            this.left_mouse_pressing = true;
        }
        if let Some(touch) = touch.iter_just_pressed().next() {
            this.cursor_pos = Some(touch.position());
            this.left_mouse_pressed = true;
        }
        if let Some(touch) = touch.iter_just_released().next() {
            this.cursor_pos = Some(touch.position());
            this.left_mouse_released = true;
        }
        this
    }
}


pub fn update_windows(
    desktop_window: Single<&Window>,
    touch: Res<Touches>,
    mut ctx: ResMut<UiContext>,
    mut cursor_icon: Single<&mut CursorIcon>,
    buttons: Res<ButtonInput<MouseButton>>
) {
    {
        let Ok(add_windows) = ctx.add_windows.replace(SmallVec::new()) else {
            return
        };
        
        for label in add_windows {
            let idx = ctx.windows.len();
            ctx.windows.push(Mutex::new(UiWindow::new(label.clone(), Rect::from_center_half_size(desktop_window.size() / 2.0, Vec2::splat(100.0)), true, false)));
            ctx.window_labels.insert(label, idx as u32);
        }
    }

    let input = MultiInput::new(&desktop_window, &buttons, &touch);
    let viewport_size = desktop_window.size();

    let header_h = ctx.header_height();
    let mut newly_focused: Option<usize> = None;
    **cursor_icon = CursorIcon::System(SystemCursorIcon::Default);
    let mut focused_window = None;
    for (i, window_cell) in ctx.windows.iter().enumerate().sorted_by(|a, b| a.1.lock().unwrap().layer.cmp(&b.1.lock().unwrap().layer)).rev() {
        let mut window = window_cell.lock().unwrap();

        let mut id = DefaultHasher::new();
        window.label.hash(&mut id);
        let id = id.finish();

        let interactive_rect = if window.open {
            Rect::from_corners(
                window.size().min - Vec2::splat(UiContext::DRAG_THRESHHOLD),
                window.size().max + Vec2::splat(UiContext::DRAG_THRESHHOLD),
            )
        } else {
            Rect::from_corners(
                window.size().min,
                window.size().min + Vec2::new(window.size().max.x - window.size().min.x, header_h),
            )
        };

        let cursor_inside = input.cursor_pos
            .map(|p| interactive_rect.contains(p))
            .unwrap_or(false);

        if cursor_inside && newly_focused.is_none() {
            if input.left_mouse_pressed {
                newly_focused = Some(i);
                if window.focused.is_none() {
                    window.focused = Some(FocusedState::default());
                }
            }
        } else if input.left_mouse_pressed {
            window.focused = None;
        }

        let siblings = if window.docked {
            if let Some((size, siblings, focused)) = UiContext::dock_info(&ctx.docking_nodes, i as u32, Rect::from_corners(Vec2::ZERO, desktop_window.size())) {
                window.dock_rect = Rect {
                    max: size.max.round(),
                    min: size.min.round()
                };
                Some((siblings, focused))
            }else {
                None
            }
        }else {
            None
        };

        window.open = siblings.as_ref().map(|s| s.0[s.1 as usize] as usize == i).unwrap_or(window.open);


        let mut focused = window.focused.take();
        if let Some(focused) = &mut focused {
            focused_window = Some(i);
            let header_h    = ctx.header_height();
            let header_rect = Rect::from_corners(
                window.size().min,
                window.size().min + Vec2::new(window.size().max.x - window.size().min.x, header_h),
            );

            if let Some(cursor_pos) = input.cursor_pos {
                if header_rect.contains(cursor_pos) {
                    if input.left_mouse_pressed {
                        focused.darg_start      = window.size().min - cursor_pos;
                        focused.is_being_draged = true;
                    }
                    **cursor_icon = CursorIcon::System(SystemCursorIcon::Grab);
                } else if !window.size().contains(cursor_pos) && !window.docked{
                    let min = window.size().min;
                    let max = window.size().max;
                    let t   = UiContext::DRAG_THRESHHOLD;

                    let resize_top    = Rect::from_corners(Vec2::new(min.x - t, min.y - t), Vec2::new(max.x + t, min.y + t)).contains(cursor_pos);
                    let resize_bottom = Rect::from_corners(Vec2::new(min.x - t, max.y - t), Vec2::new(max.x + t, max.y + t)).contains(cursor_pos);
                    let resize_left   = Rect::from_corners(Vec2::new(min.x - t, min.y - t), Vec2::new(min.x + t, max.y + t)).contains(cursor_pos);
                    let resize_right  = Rect::from_corners(Vec2::new(max.x - t, min.y - t), Vec2::new(max.x + t, max.y + t)).contains(cursor_pos);

                    if input.left_mouse_pressed {
                        focused.resize_bottom = resize_bottom;
                        focused.resize_left   = resize_left;
                        focused.resize_top    = resize_top;
                        focused.resize_right  = resize_right;
                    }

                    **cursor_icon = match (
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

                if input.left_mouse_pressing && !window.docked {
                    let size = window.rect.max - window.rect.min;
                    if focused.is_being_draged {
                        let drag_pos   = (cursor_pos + focused.darg_start).round();
                        window.rect.min = drag_pos;
                        window.rect.max = drag_pos + size;
                        **cursor_icon = CursorIcon::System(SystemCursorIcon::Grabbing);
                    }
                    if focused.resize_top    { window.rect.min.y = cursor_pos.y.min(window.rect.max.y - (header_h + 10.0)).round(); }
                    if focused.resize_bottom { window.rect.max.y = cursor_pos.y.max(window.rect.min.y + (header_h + 10.0)).round(); }
                    if focused.resize_left   { window.rect.min.x = cursor_pos.x.min(window.rect.max.x - 10.0).round(); }
                    if focused.resize_right  { window.rect.max.x = cursor_pos.x.max(window.rect.min.x + 10.0).round(); }
                }
            }

            if input.left_mouse_released {
                focused.draging         = None;
                focused.darg_start      = Vec2::ZERO;
                focused.is_being_draged = false;
                focused.resize_bottom   = false;
                focused.resize_top      = false;
                focused.resize_left     = false;
                focused.resize_right    = false;
            }
        }

        window.focused = focused;
        window.indicies.clear();
        window.verticies.clear();
        window.top_indicies.clear();
        window.top_verticies.clear();

        let size = window.size().size();
        let pos = window.size().min;

        let header_h = (ctx.acent - ctx.decent + UiContext::WINDOW_PAD.y as f32 * 2.0).round();
        let focused = window.focused.is_some();

        let (resize_top, resize_bottom, resize_left, resize_right) = window.focused.as_ref().map(|f| {
            (f.resize_top, f.resize_bottom, f.resize_left, f.resize_right)
        }).unwrap_or_default();

        let border_color = |active: bool| {
            if active { UiContext::BLUE } else { UiContext::S1 }
        };

        let mut window_ds = DrawSettings {
            on_top: false,
            color: if window.docked {UiContext::BG_DARK} else {UiContext::BG},
            rounding: UiContext::WINDOW_ROUNDING,
            round_topleft: false,
            round_topright: false,
            round_bottomleft: !window.docked,
            round_bottomright: !window.docked,
            border: Some(BorderSettings {
                color_top: border_color(resize_top),
                color_bottom: border_color(resize_bottom),
                color_left: border_color(resize_left),
                color_right: border_color(resize_right),
                size: UiContext::BORDER,
            }),
        };
        let full_screen = Rect::from_corners(Vec2::ZERO, viewport_size);
        let content_area = Rect { min: window.size().min + Vec2::new(0.0, header_h), max: window.size().max };

        if window.open {
            window.draw_box(
                content_area,
                window_ds,
                viewport_size,
                full_screen,
            );
        }

        window_ds.round_topleft = !window.docked;
        window_ds.round_topright = !window.docked;
        window_ds.round_bottomleft = false;
        window_ds.round_bottomright = false;
        window_ds.color = if focused { UiContext::BG } else { UiContext::BG_DARK };
        window_ds.border.as_mut().unwrap().color_bottom = UiContext::S1;
        window.draw_box(
            from_pos_size(
            pos,
            Vec2::new(size.x, header_h + UiContext::BORDER as f32),
            ),
            window_ds,
            viewport_size,
            full_screen,
        );

        let header_pos = window.size().min;
        let header_size = Vec2::new(size.x, header_h);
        let header_clip = from_pos_size(
            header_pos,
            header_size - Vec2::new(UiContext::WINDOW_PAD.x as f32, 0.0),
        );
        if let Some((siblings, focused)) = &siblings {
            let mut text_cursor = header_pos + Vec2::new(UiContext::ELEMENT_GAP.x as f32, 0.0) ;
            for j in siblings.iter() {
                let sibling;
                let label = if i != *j as usize  {
                    sibling = ctx.windows[*j as usize].lock().unwrap();
                    sibling.label.clone()
                } else {
                    window.label.clone()
                };
                let ds = DrawSettings {
                    round_bottomleft: false,
                    round_bottomright: false,
                    border: None,
                    ..Default::default()
                };
                let size = ctx.text_len(&label);

                let label_rect = from_pos_size(text_cursor + UiContext::WINDOW_PAD.as_vec2(), Vec2::new(size, header_h) + UiContext::WINDOW_PAD.as_vec2());
                window.draw_box(label_rect, ds, viewport_size, header_clip);
                if let Some(c) = input.cursor_pos {
                    if label_rect.contains(c) {

                    }
                }


                window.text(
                    &ctx,
                    text_cursor,
                    UiContext::TEXT,
                    &label,
                    viewport_size,
                    header_clip,
                    false,
                );
                text_cursor += Vec2::new(size + UiContext::ELEMENT_GAP.as_vec2().x, 0.0);
            }
        }else {
            let open = window.open;
            window.text_direction(
                &ctx,
                header_pos + if !open { Vec2::new(0.0, ctx.acent + 2.0) } else { Vec2::ZERO },
                UiContext::TEXT,
                "▼",
                viewport_size,
                header_clip,
                false,
                if open { TextDirection::Right } else { TextDirection::Up },
            );
    
            let arrow_size = Vec2::new(ctx.text_len("▼"), ctx.new_line_size);
    
            let lable = window.label.clone();
            window.text(
                &ctx,
                header_pos + Vec2::new(UiContext::ELEMENT_GAP.x as f32 + arrow_size.x, 0.0),
                UiContext::TEXT,
                &lable,
                viewport_size,
                header_clip,
                false,
            );

            if let Some(cursor_pos) = input.cursor_pos
                && Rect::from_center_half_size(header_pos, arrow_size).contains(cursor_pos)
                && input.left_mouse_pressed
                && focused
                && !(resize_top || resize_left)
            {
                window.open = !window.open;
            }
        }
 
        let (_, mut scrollable) = window.scrollables.remove_entry(&id).unwrap_or((id, Scrollable {
            content_size: content_area.size(),
            scroll: Vec2::ZERO
        }));
        let rect = window.size();
        scrollable.draw(NonZeroU64::new(id).unwrap(), content_area, &mut window, viewport_size, input.cursor_pos, input.left_mouse_pressed, rect);
        window.scrollables.insert(id, scrollable);
    }

    let full_screen = Rect::from_corners(Vec2::ZERO, viewport_size);
 
    if let Some(window_idx) = focused_window {
        if ctx.windows[window_idx].lock().unwrap().focused.as_ref().unwrap().is_being_draged && let Some(cursor_pos) = input.cursor_pos {
            UiContext::undock(window_idx as u32, &mut ctx.docking_nodes);
            let header_h = ctx.header_height();
            let dock = UiContext::dock(&mut ctx.docking_nodes, window_idx as u32, cursor_pos, full_screen, header_h);
            ctx.windows[window_idx].lock().unwrap().docked = dock;
        }

    }
    if let Some(cursor_pos) = input.cursor_pos {
        if ctx.resize_path == u64::MAX && input.left_mouse_pressed{
            let (path, depth, drag_start) = UiContext::find_resize(&ctx.docking_nodes, cursor_pos, full_screen, 0, 0);
            ctx.resize_path = path;
            ctx.resize_depth = depth;
            ctx.drag_start = drag_start;
            log::info!("{:b}, {}", ctx.resize_path, ctx.resize_depth)
        } 
        if ctx.resize_path != u64::MAX && input.left_mouse_pressing {
            let delta = cursor_pos - ctx.drag_start;
            let path = ctx.resize_path;
            let depth = ctx.resize_depth;
            UiContext::resize(&mut ctx.docking_nodes, path, depth, 0, delta, full_screen);
        }
        if input.left_mouse_released {
            ctx.resize_path = u64::MAX;
            ctx.resize_depth = 0;
        }
    }

    if let Some(focused_entity) = newly_focused {
        for (i, window) in ctx.windows.iter().enumerate().sorted_by_key(|(entity, window)| {
                let w = window.lock().unwrap();
                let tier = if w.docked {0} else if *entity == focused_entity { 2 } else { 1 };
                (tier, w.layer)
            }).map(|(_, window)| window)
            .enumerate()
        {
           window.lock().unwrap().layer = i as u32;
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SaveWindow {
    label: String,
    size: Rect,
    open: bool,
    docked: bool
}

#[derive(Serialize, Deserialize)]
struct SaveState {
    docking_nodes: DockingNode,
    windows: Vec<SaveWindow>,
    window_labels: HashMap<String, u32>,
}

pub fn save_windows(events: MessageReader<AppExit>, windows: Res<UiWindows>, docking_nodes: Res<DockingNode>) {
    if events.is_empty() {
        return;
    }

    let save_state = SaveState {
        docking_nodes: docking_nodes.clone(),
        window_labels: windows.window_labels.clone(),
        windows: windows.windows.iter().map(|w| {
            let window = w.lock().unwrap();
            SaveWindow {
                label: window.label.clone(),
                docked: window.docked,
                open: window.open,
                size: window.size(),
            }
        }).collect::<Vec<_>>()
    };

    let config = PrettyConfig::new();
    std::fs::write("windows.ron", ron::ser::to_string_pretty(&save_state, config).unwrap()).unwrap();    
}
