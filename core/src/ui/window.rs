use std::{
    collections::{HashMap, HashSet},
    f32::consts::PI,
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU64,
};

use bevy::{
    ecs::{message::MessageReader, system::Res},
    input::{
        ButtonInput,
        keyboard::KeyboardInput,
        mouse::{AccumulatedMouseScroll, MouseButton},
        touch::Touches,
    },
    math::Rect,
    window::Window,
};
use glam::{Vec2, Vec4};

use crate::{
    bindings::UIVertex,
    ui::{
        builder::UiWindowBuilder,
        new_ui::{FocusedState, MultiInput, UiContext, from_pos_size},
        scrollable::Scrollable,
    },
};

pub struct UiWindow {
    pub label: String,
    pub open: bool,
    pub docked: bool,
    pub layer: u32,
    pub open_headers: HashSet<u64>,
    pub scroll: Scrollable,
    pub scrollables: HashMap<u64, Scrollable>,
    pub focused: Option<FocusedState>,
    pub dock_rect: Rect,
    pub rect: Rect,
    pub verticies: Vec<UIVertex>,
    pub indicies: Vec<u32>,
    pub top_verticies: Vec<UIVertex>,
    pub top_indicies: Vec<u32>,
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
            } else if hoverd {
                UiContext::S1
            } else {
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

impl UiWindow {
    pub fn full_rect(&self) -> Rect {
        if self.docked {
            self.dock_rect
        } else if !self.open {
            Rect::from_corners(
                self.rect.min,
                self.rect.min + Vec2::new(self.rect.width(), UiContext::WINDOW_HEADER_HEIGHT),
            )
        } else {
            self.rect
        }
    }

    pub fn content_rect(&self) -> Rect {
        if !self.open {
            // Not the active tab (when docked) or collapsed (when floating): no content is shown.
            Rect::EMPTY
        } else if self.docked {
            Rect::from_corners(
                self.dock_rect.min + Vec2::new(0.0, UiContext::WINDOW_HEADER_HEIGHT),
                self.dock_rect.max,
            )
        } else {
            Rect::from_corners(
                self.rect.min + Vec2::new(0.0, UiContext::WINDOW_HEADER_HEIGHT),
                self.rect.max,
            )
        }
    }

    pub fn header_rect(&self) -> Rect {
        let rect = self.full_rect();
        Rect::from_corners(
            rect.min,
            rect.min + Vec2::new(rect.width(), UiContext::WINDOW_HEADER_HEIGHT),
        )
    }

    pub fn header_text_rect(&self) -> Rect {
        let rect = self.full_rect();
        Rect::from_corners(
            rect.min,
            rect.min
                + Vec2::new(
                    UiContext::text_len(&self.label),
                    UiContext::WINDOW_HEADER_HEIGHT,
                ),
        )
    }

    pub fn new(label: String, rect: Rect, open: bool, docked: bool) -> Self {
        UiWindow {
            label,
            layer: 0,
            dock_rect: Rect {
                min: rect.min.round(),
                max: rect.max.round(),
            },
            docked,
            scroll: Scrollable {
                content_size: Vec2::ZERO,
                scroll: Vec2::ZERO,
            },
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
            top_verticies: Vec::new(),
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
            ds.on_top,
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
                        size.x
                            - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32),
                        rmb,
                    ),
                ),
                None,
                ds.color,
                viewport_size,
                clip_rect,
                ds.on_top,
            );
            self.rect(
                from_pos_size(
                    pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - rmb),
                    Vec2::new(
                        size.x
                            - rmb
                                * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32)
                                    as f32),
                        rmb,
                    ),
                ),
                None,
                ds.color,
                viewport_size,
                clip_rect,
                ds.on_top,
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
                ds.on_top,
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
                ds.on_top,
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
                    ds.on_top,
                );
                if r > b {
                    self.round_corner(
                        center,
                        r - b,
                        start_angle,
                        ds.color,
                        viewport_size,
                        clip_rect,
                        ds.on_top,
                    );
                }
            }
        }

        if let Some(border) = border {
            self.rect(
                from_pos_size(
                    pos + Vec2::new(rmb * ds.round_topleft as u32 as f32, 0.0),
                    Vec2::new(
                        size.x
                            - rmb * ((ds.round_topleft as u32 + ds.round_topright as u32) as f32),
                        b,
                    ),
                ),
                None,
                border.color_top,
                viewport_size,
                clip_rect,
                ds.on_top,
            );
            self.rect(
                from_pos_size(
                    pos + Vec2::new(rmb * ds.round_bottomleft as u32 as f32, size.y - b),
                    Vec2::new(
                        size.x
                            - rmb
                                * ((ds.round_bottomleft as u32 + ds.round_bottomright as u32)
                                    as f32),
                        b,
                    ),
                ),
                None,
                border.color_bottom,
                viewport_size,
                clip_rect,
                ds.on_top,
            );
            self.rect(
                from_pos_size(
                    pos + Vec2::new(0.0, rmb * ds.round_topleft as u32 as f32),
                    Vec2::new(
                        b,
                        size.y
                            - rmb * ((ds.round_topleft as u32 + ds.round_bottomleft as u32) as f32),
                    ),
                ),
                None,
                border.color_left,
                viewport_size,
                clip_rect,
                ds.on_top,
            );
            self.rect(
                from_pos_size(
                    pos + Vec2::new(size.x - b, rmb * ds.round_topright as u32 as f32),
                    Vec2::new(
                        b,
                        size.y
                            - rmb
                                * ((ds.round_topright as u32 + ds.round_bottomright as u32) as f32),
                    ),
                ),
                None,
                border.color_right,
                viewport_size,
                clip_rect,
                ds.on_top,
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
        let viewport_size = window.physical_size().as_vec2();

        let r = UiContext::WINDOW_ROUNDING as f32;
        let b = UiContext::BORDER as f32;
        let rmb = r.max(b);

        let header_h =
            (UiContext::ATLAS_CELL_SIZE.y as f32 + UiContext::WINDOW_PAD.y as f32 * 2.0).round();
        let focused = self.focused.is_some();
        let input = MultiInput::new(&window, &buttons, &touch);

        let content_area = Rect {
            min: self.full_rect().min + Vec2::new(0.0, header_h),
            max: self.full_rect().max,
        };

        let (_, mut scrollable) = self.scrollables.remove_entry(&id).unwrap_or((
            id,
            Scrollable {
                content_size: content_area.size(),
                scroll: Vec2::ZERO,
            },
        ));
        let cursor =
            (content_area.min + rmb + UiContext::WINDOW_PAD.as_vec2() - scrollable.scroll).round();

        if !self.open {
            return None;
        }
        let bar_size = UiContext::BAR_THICKNESS
            * Vec2::new(
                (scrollable.content_size.y > content_area.size().y) as u32 as f32,
                (scrollable.content_size.x > content_area.size().x) as u32 as f32,
            );
        let clip_rect = from_pos_size(
            content_area.min + b,
            content_area.size() - b * 2.0 - bar_size,
        );
        let max_width = (content_area.size().max(scrollable.content_size)).x
            - UiContext::WINDOW_PAD.x as f32
            - rmb
            - bar_size.x;

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
        scrollable.draw(
            NonZeroU64::new(id).unwrap(),
            content_area,
            self,
            viewport_size,
            input.cursor_pos,
            input.left_mouse_pressed,
            self.full_rect(),
        );
        self.scrollables.insert(id, scrollable);
        Some(r)
    }

    pub fn text(
        &mut self,
        pos: Vec2,
        color: Vec4,
        text: &str,
        viewport_size: Vec2,
        clip_rect: Rect,
        on_top: bool,
    ) -> Vec2 {
        let mut pen = pos;
        for char in text.chars() {
            if char == '\n' {
                pen.x = pos.x;
                pen.y += UiContext::ATLAS_CELL_SIZE.y as f32 + UiContext::LINE_SPACING as f32;
                continue;
            }
            let tpos = Vec2::new(pen.x, pen.y);

            let position = UiContext::char_to_atlas_pos(char);
            let uv = position.as_vec2() / UiContext::ATLAS_SIZE.as_vec2();
            let rect = Rect::from_corners(tpos, tpos + UiContext::ATLAS_CELL_SIZE.as_vec2());
            self.rect(
                rect,
                Some((uv, UiContext::UV_SIZE)),
                color,
                viewport_size,
                clip_rect,
                on_top,
            );
            pen.x +=
                UiContext::ATLAS_CELL_SIZE.x as f32 + UiContext::CHARACTER_ADVANCE_WIDTH as f32;
        }

        pen
    }

    pub fn text_direction(
        &mut self,
        pos: Vec2,
        color: Vec4,
        text: &str,
        viewport_size: Vec2,
        clip_rect: Rect,
        on_top: bool,
        direction: TextDirection,
    ) -> Vec2 {
        let clip_min = clip_rect.min;
        let clip_max = clip_rect.max;
        let half_vp = viewport_size / 2.0;

        let verticies = if on_top {
            &mut self.top_verticies
        } else {
            &mut self.verticies
        };
        let indicies = if on_top {
            &mut self.top_indicies
        } else {
            &mut self.indicies
        };

        let advance_dir: Vec2 = match direction {
            TextDirection::Right => Vec2::new(1.0, 0.0),
            TextDirection::Left => Vec2::new(-1.0, 0.0),
            TextDirection::Down => Vec2::new(0.0, 1.0),
            TextDirection::Up => Vec2::new(0.0, -1.0),
        };

        let ascent_dir: Vec2 = match direction {
            TextDirection::Right => Vec2::new(0.0, 1.0),
            TextDirection::Left => Vec2::new(0.0, -1.0),
            TextDirection::Down => Vec2::new(-1.0, 0.0),
            TextDirection::Up => Vec2::new(1.0, 0.0),
        };

        let mut pen = pos;

        for char in text.chars() {
            if char == '\n' {
                pen = pos
                    + ascent_dir
                        * (UiContext::ATLAS_CELL_SIZE.y as f32 + UiContext::LINE_SPACING as f32);
                continue;
            }

            let uv = UiContext::char_to_atlas_pos(char).as_vec2() / UiContext::ATLAS_SIZE.as_vec2();
            let uv_size = UiContext::UV_SIZE;
            let size = UiContext::ATLAS_CELL_SIZE.as_vec2();

            let p_tl = pen;
            let p_tr = pen + advance_dir * size.x;
            let p_br = pen + advance_dir * size.x + ascent_dir * size.y;
            let p_bl = pen + ascent_dir * size.y;

            let min_x = p_tl.x.min(p_tr.x).min(p_br.x).min(p_bl.x);
            let min_y = p_tl.y.min(p_tr.y).min(p_br.y).min(p_bl.y);
            let max_x = p_tl.x.max(p_tr.x).max(p_br.x).max(p_bl.x);
            let max_y = p_tl.y.max(p_tr.y).max(p_br.y).max(p_bl.y);

            if min_x >= clip_max.x
                || max_x <= clip_min.x
                || min_y >= clip_max.y
                || max_y <= clip_min.y
            {
                pen += advance_dir * UiContext::ATLAS_CELL_SIZE.as_vec2();
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
                TextDirection::Left => (1.0 - t_x_max, 1.0 - t_x_min, 1.0 - t_y_max, 1.0 - t_y_min),
                TextDirection::Down => (t_y_min, t_y_max, 1.0 - t_x_max, 1.0 - t_x_min),
                TextDirection::Up => (1.0 - t_y_max, 1.0 - t_y_min, t_x_min, t_x_max),
            };

            let uv_x_min = uv.x + t_adv_min * uv_size.x;
            let uv_x_max = uv.x + t_adv_max * uv_size.x;
            let uv_y_min = uv.y + t_asc_min * uv_size.y;
            let uv_y_max = uv.y + t_asc_max * uv_size.y;

            let to_ndc = |p: Vec2| (p / half_vp) - Vec2::splat(1.0);
            let vertex_id = verticies.len() as u32;

            let (c_tl, c_tr, c_br, c_bl) = match direction {
                TextDirection::Right => (
                    (
                        Vec2::new(clamped_min_x, clamped_min_y),
                        Vec2::new(uv_x_min, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_max_x, clamped_min_y),
                        Vec2::new(uv_x_max, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_max_x, clamped_max_y),
                        Vec2::new(uv_x_max, uv_y_max),
                    ),
                    (
                        Vec2::new(clamped_min_x, clamped_max_y),
                        Vec2::new(uv_x_min, uv_y_max),
                    ),
                ),
                TextDirection::Left => (
                    (
                        Vec2::new(clamped_max_x, clamped_max_y),
                        Vec2::new(uv_x_min, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_min_x, clamped_max_y),
                        Vec2::new(uv_x_max, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_min_x, clamped_min_y),
                        Vec2::new(uv_x_max, uv_y_max),
                    ),
                    (
                        Vec2::new(clamped_max_x, clamped_min_y),
                        Vec2::new(uv_x_min, uv_y_max),
                    ),
                ),
                TextDirection::Down => (
                    (
                        Vec2::new(clamped_max_x, clamped_min_y),
                        Vec2::new(uv_x_min, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_max_x, clamped_max_y),
                        Vec2::new(uv_x_max, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_min_x, clamped_max_y),
                        Vec2::new(uv_x_max, uv_y_max),
                    ),
                    (
                        Vec2::new(clamped_min_x, clamped_min_y),
                        Vec2::new(uv_x_min, uv_y_max),
                    ),
                ),
                TextDirection::Up => (
                    (
                        Vec2::new(clamped_min_x, clamped_max_y),
                        Vec2::new(uv_x_min, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_min_x, clamped_min_y),
                        Vec2::new(uv_x_max, uv_y_min),
                    ),
                    (
                        Vec2::new(clamped_max_x, clamped_min_y),
                        Vec2::new(uv_x_max, uv_y_max),
                    ),
                    (
                        Vec2::new(clamped_max_x, clamped_max_y),
                        Vec2::new(uv_x_min, uv_y_max),
                    ),
                ),
            };

            verticies.extend_from_slice(&[
                UIVertex {
                    color,
                    pos: to_ndc(c_tl.0),
                    uv: c_tl.1,
                },
                UIVertex {
                    color,
                    pos: to_ndc(c_tr.0),
                    uv: c_tr.1,
                },
                UIVertex {
                    color,
                    pos: to_ndc(c_br.0),
                    uv: c_br.1,
                },
                UIVertex {
                    color,
                    pos: to_ndc(c_bl.0),
                    uv: c_bl.1,
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

            pen += advance_dir
                * (UiContext::ATLAS_CELL_SIZE.as_vec2()
                    + UiContext::CHARACTER_ADVANCE_WIDTH as f32);
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
        } else {
            &mut self.verticies
        };

        let indicies = if on_top {
            &mut self.top_indicies
        } else {
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
        on_top: bool,
    ) {
        let segments = rounding.ceil() as u32;
        let half_vp = view_port_size / 2.0;
        let clip_min = clip_rect.min;
        let clip_max = clip_rect.max;

        let clamp_to_clip = |p: Vec2| p.clamp(clip_min, clip_max);
        let to_ndc = |p: Vec2| (p / half_vp) - Vec2::splat(1.0);

        let verticies = if on_top {
            &mut self.top_verticies
        } else {
            &mut self.verticies
        };

        let indicies = if on_top {
            &mut self.top_indicies
        } else {
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
                    indicies.extend_from_slice(&[center_vertex, prev_vertex, vertex]);
                }
            }

            prev_vertex = vertex;
        }
    }
}
