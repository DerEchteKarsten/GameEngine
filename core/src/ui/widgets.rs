//! UI widgets on `UiWindowBuilder`: text fitting, buttons, checkboxes, text and number inputs, property rows, sliders, dropdowns, headers, selectable rows, table rows, colour picker, progress bar, histogram and tooltips.
use std::{borrow::Cow, hash::Hash};

use bevy::{input::keyboard::KeyCode, math::Rect};
use glam::{Vec2, Vec4};
use itertools::Itertools;
use lava::{bindings::UIVertex, bindless::BindlessHandle};

use crate::ui::{
    UiContext,
    builder::{TextCursor, UiWindowBuilder},
    from_pos_size,
    window::{BorderSettings, DrawSettings, Drawable, TextDirection},
};

/// Which end of a text that doesn't fit is replaced by "..".
#[derive(Copy, Clone)]
pub enum Ellipsis {
    Start,
    End,
}

/// `text` cut to `width`, with ".." where it was cut.
pub fn fit_text(text: &str, width: f32, ellipsis: Ellipsis) -> Cow<'_, str> {
    let max = (width / UiContext::text_len("x")).max(0.0) as usize;
    let chars = text.chars().count();
    if chars <= max {
        return Cow::Borrowed(text);
    }
    let keep = max.saturating_sub(2);
    Cow::Owned(match ellipsis {
        Ellipsis::Start => format!("..{}", text.chars().skip(chars - keep).collect::<String>()),
        Ellipsis::End => format!("{}..", text.chars().take(keep).collect::<String>()),
    })
}

enum InputMode<'a> {
    String(&'a mut String),
    Float(f64),
    Int(i64),
}

enum InputModeOutput {
    Float(f64),
    Int(i64),
    ChangDetection(bool),
}

fn rgb_to_hsv(c: Vec4) -> (f32, f32, f32, f32) {
    let r = c.x;
    let g = c.y;
    let b = c.z;
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
    let i = h6.floor() as i32;
    let f = h6 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
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
    /// Text cut to `width` with "..", showing all of it as a tooltip when hovered.
    pub fn text_fit(&mut self, text: &str, width: f32, color: Vec4, ellipsis: Ellipsis) {
        let fitted = fit_text(text, width, ellipsis);
        self.colored_text(&fitted, color);
        if let Cow::Owned(_) = fitted {
            self.tooltip(text);
        }
    }

    pub fn button(&mut self, label: impl AsRef<str>) -> bool {
        let size = Self::contain_size(UiContext::text_size(label.as_ref()));
        if self.begin_element(size, false) {
            return false;
        }
        let rect = from_pos_size(self.cursor, size);
        let hovered = self.hovered(rect);
        let clicked = self.clicked(rect);
        self.paint_box(rect, DrawSettings::new(hovered, clicked));
        self.paint_text(
            self.child_cursor(),
            label.as_ref(),
            self.text_color(),
            self.clip_rect,
        );
        self.finish_element(size, false);
        clicked
    }

    /// A box ticked while `value` is true, followed by `label` (may be empty). Clicking
    /// either toggles it.
    pub fn checkbox(&mut self, label: &str, mut value: bool) -> bool {
        let box_size = Self::contain_size(UiContext::ATLAS_CELL_SIZE.as_vec2());
        let label_x = box_size.x + UiContext::ELEMENT_GAP.x as f32;
        let size = if label.is_empty() {
            box_size
        } else {
            box_size + Vec2::new(label_x + UiContext::text_len(label), 0.0)
        };
        if self.begin_element(size, false) {
            return value;
        }
        let rect = from_pos_size(self.cursor, size);
        let hovered = self.hovered(rect);
        if self.clicked(rect) {
            value = !value;
        }
        self.paint_box(
            from_pos_size(self.cursor, box_size),
            DrawSettings::new(hovered, false),
        );
        let text_y = self.child_cursor().y;
        if value {
            let color = if self.disable_all_input {
                UiContext::ACENT_DIM
            } else {
                UiContext::ACENT
            };
            self.paint_text(self.child_cursor(), "✓", color, self.clip_rect);
        }
        self.paint_text(
            Vec2::new(self.cursor.x + label_x, text_y),
            label,
            self.text_color(),
            self.clip_rect,
        );
        self.finish_element(size, false);
        value
    }

    pub fn slider(&mut self, id: impl Hash, min: f32, max: f32, width: f32, value: f32) -> f32 {
        let line = UiContext::ATLAS_CELL_SIZE.y as f32;
        let size = Vec2::new(width, line);
        if self.begin_element(size, false) {
            return value;
        }
        let rect = from_pos_size(self.cursor, size);
        let mut value = value;
        if let Some(drag) = self.drag(id, rect)
            && max != min
        {
            value = (min + (drag.pos.x - rect.min.x) / width * (max - min)).clamp(min, max);
        }

        self.paint_box(
            from_pos_size(
                self.cursor + Vec2::new(0.0, line / 3.0),
                Vec2::new(width, line / 3.0),
            ),
            DrawSettings::default(),
        );
        let t = if max != min {
            ((value - min) / (max - min)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let knob = Vec2::new(16.0, line);
        self.paint_box(
            from_pos_size(
                (self.cursor + Vec2::new(t * width - knob.x * 0.5, 0.0)).round(),
                knob,
            ),
            DrawSettings {
                color: UiContext::ACENT,
                rounding: 4,
                ..Default::default()
            },
        );
        self.finish_element(size, false);
        value
    }

    const WORD_DELIMITER: [char; 6] = [' ', '.', ',', ':', '(', ')'];

    fn text_input_private(
        &mut self,
        id: impl Hash,
        width: f32,
        input_mode: InputMode,
    ) -> InputModeOutput {
        let id = self.id(&id);
        let size = Vec2::new(width, Self::widget_height());
        let inner_size = Vec2::new(
            (width - Self::child_offset().x * 2.0).max(0.0),
            UiContext::ATLAS_CELL_SIZE.y as f32,
        );
        let inner_width = inner_size.x;
        if self.begin_element(size, false) {
            return InputModeOutput::ChangDetection(false);
        }
        let text_color = self.text_color();
        let clicked = self.clicked(from_pos_size(self.cursor, size));
        let text_cursor = self.child_cursor();

        enum InputType {
            String,
            Float(f64),
            Int(i64),
        }
        let (mut value, need_format_string, input_mode) = match input_mode {
            InputMode::String(s) => (s, false, InputType::String),
            InputMode::Float(f) => (&mut format!("{:.2}", f), true, InputType::Float(f)),
            InputMode::Int(i) => (&mut format!("{}", i), true, InputType::Int(i)),
        };
        let text_clip = from_pos_size(text_cursor, inner_size);

        let mut just_focused = false;
        let mut focused = if let Some(focused) = self.ctx.focused.as_mut() {
            if (clicked && focused.focused_element != Some(id)) || self.focuse_next {
                focused.focused_element = Some(id);
                self.focuse_next = false;
                if need_format_string {
                    focused.format_string = value.clone();
                }
                focused.cursor = TextCursor {
                    byte_pos: value.len(),
                };
                focused.offset = 0.0;
                focused.selected = (0..value.len()).into();
                just_focused = true;
            }
            if focused.focused_element == Some(id) {
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
                    &mut focused.focused_element,
                ))
            } else {
                None
            }
        } else {
            None
        };

        let mut changed = false;
        let mut ds = DrawSettings::default();
        if let Some((cursor, view, selected, focused)) = &mut focused {
            ds = ds.border_color(UiContext::ACENT);
            for key in self.ctx.keys.read() {
                let has_selection = selected.start != selected.end;
                let sel_min = selected.start.min(selected.end);
                let sel_max = selected.start.max(selected.end);
                if !(key.repeat || key.state.is_pressed()) {
                    continue;
                }

                let mut navigation = false;
                if key.key_code == KeyCode::ArrowLeft {
                    navigation = true;
                    if has_selection && !self.ctx.shift {
                        cursor.byte_pos = sel_min;
                    } else {
                        cursor.move_left(value);
                        if self.ctx.ctrl {
                            while let Some(char) = cursor.ch_before(value)
                                && !Self::WORD_DELIMITER.contains(&char)
                                && cursor.byte_pos != 0
                            {
                                cursor.move_left(value);
                            }
                        }
                    }
                } else if key.key_code == KeyCode::ArrowRight {
                    navigation = true;
                    if has_selection && !self.ctx.shift {
                        cursor.byte_pos = sel_max;
                    } else {
                        cursor.move_right(value);
                        if self.ctx.ctrl {
                            while let Some(char) = cursor.ch(value)
                                && !Self::WORD_DELIMITER.contains(&char)
                                && cursor.byte_pos != value.len()
                            {
                                cursor.move_right(value);
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
                        changed = true;
                        cursor.byte_pos = sel_min;
                        selected.start = sel_min;
                        selected.end = sel_min;
                    } else {
                        cursor.delete_before(value);
                        changed = true;
                        if self.ctx.ctrl {
                            while let Some(char) = cursor.ch_before(value)
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
                        changed = true;
                        cursor.byte_pos = sel_min;
                        selected.start = sel_min;
                        selected.end = sel_min;
                    } else {
                        cursor.delete_after(value);
                        changed = true;
                        if self.ctx.ctrl {
                            while let Some(char) = cursor.ch(value)
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
                } else if self.ctx.ctrl && key.key_code == KeyCode::KeyA {
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
                    if has_selection {
                        value.drain(sel_min..sel_max);
                        cursor.byte_pos = sel_min;
                        selected.start = cursor.byte_pos;
                        selected.end = cursor.byte_pos;
                    }
                    cursor.insert(value, str);
                    changed = true;
                }
                if !self.ctx.shift {
                    selected.start = cursor.byte_pos;
                    selected.end = cursor.byte_pos;
                } else if navigation {
                    selected.end = cursor.byte_pos;
                }
            }

            let mut pos = -**view;
            let mut any_clicked = false;
            for (i, _c) in value.char_indices() {
                if self.ctx.input.primary_pressing
                    && Self::hoverdp(
                        from_pos_size(
                            text_cursor + Vec2::new(pos, 0.0),
                            UiContext::ATLAS_CELL_SIZE.as_vec2(),
                        ),
                        self.clip_rect,
                        self.ctx.input.cursor_pos,
                        self.hovered_consumed,
                    )
                    && !just_focused
                {
                    any_clicked = true;
                    cursor.byte_pos = i;
                    if self.ctx.shift {
                        selected.end = i;
                    } else {
                        selected.start = i;
                        selected.end = i;
                    }
                }
                pos += UiContext::ATLAS_CELL_SIZE.x as f32;
            }
            if !any_clicked && clicked && !just_focused {
                let end = value.len();
                cursor.byte_pos = end;
                if self.ctx.shift {
                    selected.end = end;
                } else {
                    selected.start = end;
                    selected.end = end;
                }
            }

            let offset = UiContext::text_len(&value[..cursor.byte_pos]).round();
            let left = (offset - 5.0).max(0.0);
            if left < **view {
                **view = left;
            }
            let right = (offset - inner_width + 5.0).max(0.0);
            if right > **view {
                **view = right;
            }
        }
        let focused = focused.map(|(e1, e2, e3, _)| (*e1, *e2, *e3));

        // `value` may borrow the focused state, so this draws through the window directly.
        let window = &mut *self.ctx.window;
        let viewport = self.ctx.viewport_size;
        let text_clip = text_clip.intersect(self.clip_rect);
        window.draw_box(
            from_pos_size(self.cursor, size),
            ds,
            viewport,
            self.clip_rect,
        );
        let offset = focused.as_ref().map_or(0.0, |e| e.1);
        window.draw_text(
            text_cursor - Vec2::new(offset, 0.0),
            text_color,
            value,
            viewport,
            text_clip,
            false,
        );
        if let Some((cursor, offset, selected)) = &focused {
            let x = UiContext::text_len(&value[..cursor.byte_pos]);
            let start = UiContext::text_len(&value[..selected.start.min(selected.end)]);
            let end = UiContext::text_len(&value[..selected.start.max(selected.end)]);
            let height = UiContext::ATLAS_CELL_SIZE.y as f32;
            for (x, width, color) in [
                (x, 1.0, Vec4::ONE),
                (start, end - start, UiContext::ACENT_DIM),
            ] {
                window.draw_rect(
                    from_pos_size(
                        text_cursor + Vec2::new(x - offset, 0.0),
                        Vec2::new(width, height),
                    ),
                    None,
                    color,
                    viewport,
                    text_clip,
                    false,
                    BindlessHandle::default(),
                );
            }
        }

        let ret = match input_mode {
            InputType::Float(f) => InputModeOutput::Float(value.parse().unwrap_or(f)),
            InputType::Int(i) => InputModeOutput::Int(value.parse().unwrap_or(i)),
            InputType::String => InputModeOutput::ChangDetection(changed),
        };

        self.finish_element(size, false);
        ret
    }

    /// Returns whether `value` changed.
    pub fn text_input(&mut self, id: impl Hash, value: &mut String, width: f32) -> bool {
        matches!(
            self.text_input_private(id, width, InputMode::String(value)),
            InputModeOutput::ChangDetection(true)
        )
    }

    pub fn float_input(&mut self, id: impl Hash, value: f64, width: f32) -> f64 {
        match self.text_input_private(id, width, InputMode::Float(value)) {
            InputModeOutput::Float(v) => v,
            _ => value,
        }
    }

    pub fn int_input(&mut self, id: impl Hash, value: i64, width: f32) -> i64 {
        match self.text_input_private(id, width, InputMode::Int(value)) {
            InputModeOutput::Int(v) => v,
            _ => value,
        }
    }

    /// `N` number fields side by side, `width` wide together.
    pub fn float_inputs<const N: usize>(
        &mut self,
        id: impl Hash,
        mut values: [f64; N],
        width: f32,
    ) -> [f64; N] {
        let gap = UiContext::ELEMENT_GAP.x as f32;
        let field = ((width - gap * (N as f32 - 1.0)) / N as f32).floor();
        self.row(|ui| {
            for (i, value) in values.iter_mut().enumerate() {
                *value = ui.float_input((&id, i), *value, field);
            }
        });
        values
    }

    /// A row with `label` on the left and `widget` in a slot of `PROPERTY_WIDTH` at the right
    /// edge; `widget` gets the slot's width.
    pub fn property<R>(&mut self, label: &str, widget: impl FnOnce(&mut Self, f32) -> R) -> R {
        const PROPERTY_WIDTH: f32 = 300.0;
        self.row(|ui| {
            let slot = ui.cursor.x + ui.available_size().x - PROPERTY_WIDTH;
            ui.label(label);
            ui.cursor.x = ui.cursor.x.max(slot);
            widget(ui, PROPERTY_WIDTH)
        })
    }

    /// A bar filled to `fraction` (clamped to 0..=1) with `label` centered on top. Pass an
    /// empty label for a plain bar.
    pub fn progress_bar(&mut self, fraction: f32, width: f32, label: impl AsRef<str>) {
        let size = Vec2::new(width, Self::widget_height());
        if self.begin_element(size, false) {
            return;
        }
        let fraction = if fraction.is_nan() {
            0.0
        } else {
            fraction.clamp(0.0, 1.0)
        };

        let rect = from_pos_size(self.cursor, size);
        self.paint_box(
            rect,
            DrawSettings {
                color: UiContext::BG_DARK,
                ..Default::default()
            },
        );
        let track = rect.inflate(-(UiContext::BORDER.max(UiContext::ROUNDING) as f32));
        let fill_width = (track.width() * fraction).round();
        if fill_width > 0.0 {
            let color = if self.disable_all_input {
                UiContext::ACENT_DIM
            } else {
                UiContext::ACENT
            };
            self.paint_rect(
                from_pos_size(track.min, Vec2::new(fill_width, track.height())),
                color,
            );
        }

        let label = label.as_ref();
        if !label.is_empty() {
            let pos = Vec2::new(
                rect.center().x - UiContext::text_len(label) / 2.0,
                self.child_cursor().y,
            )
            .round();
            self.paint_text(pos, label, self.text_color(), track);
        }
        self.finish_element(size, false);
    }

    pub fn dropdown(&mut self, id: impl Hash, mut selected: usize, options: &[&str]) -> usize {
        let id = self.id(&id);

        let sizex = options
            .iter()
            .map(|o| UiContext::text_len(o))
            .fold(0.0, f32::max);
        let arrow_size =
            UiContext::ATLAS_CELL_SIZE.x as f32 + UiContext::ELEMENT_GAP.x as f32 * 2.0;
        let button_size = Self::contain_size(Vec2::new(
            sizex + arrow_size,
            UiContext::ATLAS_CELL_SIZE.y as f32,
        ));
        if self.begin_element(button_size, false) {
            return selected;
        }

        let rect = from_pos_size(self.cursor, button_size);
        let hovered = self.hovered(rect);
        let open = self
            .ctx
            .focused
            .as_ref()
            .is_some_and(|e| e.focused_element == Some(id));

        self.paint_box(
            rect,
            DrawSettings {
                color: if hovered {
                    UiContext::S1
                } else {
                    UiContext::S0
                },
                round_bottomleft: !open,
                round_bottomright: !open,
                ..Default::default()
            },
        );
        self.paint_text(
            self.child_cursor(),
            options[selected],
            self.text_color(),
            self.clip_rect,
        );
        self.arrow(
            self.child_cursor() + Vec2::new(sizex + UiContext::ELEMENT_GAP.x as f32, 0.0),
            open,
            self.clip_rect,
        );
        if self.clicked(rect)
            && let Some(f) = &mut self.ctx.focused
        {
            f.focused_element = if open { None } else { Some(id) };
        }
        if open {
            let mut cursor = self.cursor + Vec2::new(0.0, button_size.y);
            for (i, o) in options.iter().enumerate() {
                let rect = from_pos_size(cursor, button_size);
                let last = i + 1 == options.len();
                let hovered = self.hovered(rect);
                if hovered {
                    self.hovered_consumed = true;
                    if self.ctx.input.primary_pressed {
                        selected = i;
                        if let Some(f) = &mut self.ctx.focused {
                            f.focused_element = None;
                        }
                    }
                }
                let fill = if hovered {
                    UiContext::S1
                } else {
                    UiContext::S0
                };
                let border = BorderSettings {
                    color_bottom: if last { UiContext::S2 } else { fill },
                    color_top: fill,
                    color_left: UiContext::S2,
                    color_right: UiContext::S2,
                    size: UiContext::BORDER,
                };
                let ds = DrawSettings {
                    color: fill,
                    border: Some(border),
                    round_bottomleft: last,
                    round_bottomright: last,
                    round_topleft: false,
                    round_topright: false,
                    rounding: 0,
                    on_top: true,
                };
                self.paint_box(rect, ds);
                let color = self.text_color();
                self.ctx.window.draw_text(
                    (cursor + Self::child_offset()).round(),
                    color,
                    o,
                    self.ctx.viewport_size,
                    self.clip_rect,
                    true,
                );
                cursor.y += button_size.y;
            }
        }
        self.finish_element(button_size, false);
        selected
    }

    /// The "▼" of dropdowns and headers at `pos`, pointing right while closed.
    fn arrow(&mut self, pos: Vec2, open: bool, clip: Rect) {
        let (offset, direction) = if open {
            (Vec2::ZERO, TextDirection::Right)
        } else {
            (
                Vec2::new(0.0, UiContext::ATLAS_CELL_SIZE.x as f32 * 1.5),
                TextDirection::Up,
            )
        };
        let color = self.text_color();
        self.ctx.window.draw_text_direction(
            pos + offset,
            color,
            "▼",
            self.ctx.viewport_size,
            clip,
            direction,
        );
    }

    /// A full-width header that opens and closes on click and returns whether it is open.
    /// `selected` highlights it.
    pub fn header(
        &mut self,
        id: impl Hash,
        label: impl AsRef<str>,
        default_open: bool,
        selected: bool,
    ) -> bool {
        let id: u64 = self.id(&id).into();
        let size = Vec2::new(
            self.available_size().x,
            UiContext::ATLAS_CELL_SIZE.y as f32 + UiContext::LINE_SPACING as f32,
        )
        .floor();
        let rect = from_pos_size(self.cursor, size);
        if self.clicked(rect) && !self.ctx.window.open_headers.insert(id) {
            self.ctx.window.open_headers.remove(&id);
        }
        let open = self.ctx.window.open_headers.contains(&id) ^ default_open;
        if self.begin_element(size, false) {
            return open;
        }

        let mut ds = DrawSettings::new(self.hovered(rect), false);
        if selected {
            ds.color = UiContext::ACENT_REALY_DIM;
        }
        self.paint_box(rect, ds);
        let text = self.cursor + Vec2::new(UiContext::CHILD_PAD.x as f32, 0.0);
        let clip = self.clip_rect.intersect(rect);
        self.arrow(text, open, clip);
        let label_x = UiContext::ATLAS_CELL_SIZE.x as f32 + UiContext::ELEMENT_GAP.x as f32 * 2.0;
        self.paint_text(
            text + Vec2::new(label_x, 0.0),
            label.as_ref(),
            self.text_color(),
            clip,
        );
        self.finish_element(size, false);
        open
    }

    /// A `header` with `children` indented below it while open.
    pub fn collapsable<R>(
        &mut self,
        default_open: bool,
        id: impl Hash,
        label: impl AsRef<str>,
        children: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        self.header(id, label, default_open, false)
            .then(|| self.indent(children))
    }

    /// A full-width row of `text` that is filled while hovered or `selected`. Returns whether
    /// it was clicked.
    pub fn selectable(&mut self, text: &str, color: Vec4, selected: bool) -> bool {
        let size = Vec2::new(
            self.available_size().x.max(UiContext::text_len(text)),
            UiContext::ATLAS_CELL_SIZE.y as f32,
        );
        if self.begin_element(size, false) {
            return false;
        }
        let rect = from_pos_size(self.cursor, size);
        if selected {
            self.paint_rect(rect, UiContext::ACENT_REALY_DIM);
        } else if self.hovered(rect) {
            self.paint_rect(rect, UiContext::S1);
        }
        self.paint_text(self.cursor, text, color, rect);
        let clicked = self.clicked(rect);
        self.finish_element(size, false);
        clicked
    }

    pub fn table<'b>(
        &mut self,
        header_labels: impl Iterator<Item = impl AsRef<str>>,
        contents: impl Iterator<Item = impl Iterator<Item = impl AsRef<str>>>,
    ) {
        let size = (self.available_size().x
            / (header_labels.try_len().unwrap_or(10) as f32 * UiContext::ATLAS_CELL_SIZE.x as f32))
            as usize;

        self.table_row(header_labels.map(|i| (i, Some(size))), UiContext::TEXT_DIM);
        for row in contents {
            self.table_row(row.map(|s| (s, Some(size))), UiContext::TEXT);
        }
    }

    /// A row of `cells`, each cut to its column's width in characters
    pub fn table_row<'b>(
        &mut self,
        cells: impl Iterator<Item = (impl AsRef<str>, Option<usize>)>,
        color: Vec4,
    ) {
        let char_width = UiContext::text_len("x");
        self.row(|ui| {
            let mut x = ui.cursor.x;
            for (cell, width) in cells {
                ui.cursor.x = x;
                let text_width = match width {
                    Some(w) => w.saturating_sub(1) as f32 * char_width,
                    None => ui.available_size().x,
                };
                ui.text_fit(cell.as_ref(), text_width, color, Ellipsis::End);
                x += width.unwrap_or(0) as f32 * char_width;
            }
        });
    }

    pub fn color_picker(&mut self, id: impl Hash, color: Vec4) -> Vec4 {
        let color = color.clamp(Vec4::ZERO, Vec4::ONE);
        let picker_size = 150.0f32;
        let bar_width = 14.0f32;
        let gap = UiContext::ELEMENT_GAP.x as f32;

        let full_width = picker_size + gap + bar_width + gap + bar_width;
        let total_size = Vec2::new(
            full_width,
            picker_size + gap + bar_width + gap + Self::widget_height(),
        );

        if self.begin_element(total_size, false) {
            return color;
        }

        let sv_pos = self.cursor;
        let hue_pos = self.cursor + Vec2::new(picker_size + gap, 0.0);
        let alpha_pos = self.cursor + Vec2::new(picker_size + gap + bar_width + gap, 0.0);
        let preview_pos = self.cursor + Vec2::new(0.0, picker_size + gap);
        let preview_size = Vec2::new(full_width, bar_width);
        let inputs_pos = self.cursor + Vec2::new(0.0, picker_size + gap + bar_width + gap);

        let (mut h, mut s, mut v, mut a) = rgb_to_hsv(color);
        let fraction = |pos: f32, start: f32| ((pos - start) / picker_size).clamp(0.0, 1.0);
        let bar = Vec2::new(bar_width, picker_size);
        if let Some(drag) = self.drag((&id, 0), from_pos_size(sv_pos, Vec2::splat(picker_size))) {
            s = fraction(drag.pos.x, sv_pos.x);
            v = 1.0 - fraction(drag.pos.y, sv_pos.y);
        }
        if let Some(drag) = self.drag((&id, 1), from_pos_size(hue_pos, bar)) {
            h = fraction(drag.pos.y, hue_pos.y);
        }
        if let Some(drag) = self.drag((&id, 2), from_pos_size(alpha_pos, bar)) {
            a = 1.0 - fraction(drag.pos.y, alpha_pos.y);
        }

        let new_color = hsv_to_rgb(h, s, v, a);
        let pure_hue = hsv_to_rgb(h, 1.0, 1.0, 1.0);
        let half_vp = self.ctx.viewport_size / 2.0;
        let clip_min = self.clip_rect.min;
        let clip_max = self.clip_rect.max;
        let solid = Vec2::splat(20.0);

        let to_ndc = |p: Vec2| (p / half_vp) - Vec2::splat(1.0);

        let emit_quad =
            |verts: &mut Vec<UIVertex>, idxs: &mut Vec<u32>, corners: [(Vec2, Vec4); 4]| {
                let min_x = corners.iter().fold(f32::MAX, |acc, (p, _)| acc.min(p.x));
                let min_y = corners.iter().fold(f32::MAX, |acc, (p, _)| acc.min(p.y));
                let max_x = corners.iter().fold(f32::MIN, |acc, (p, _)| acc.max(p.x));
                let max_y = corners.iter().fold(f32::MIN, |acc, (p, _)| acc.max(p.y));

                let cmin_x = min_x.max(clip_min.x);
                let cmin_y = min_y.max(clip_min.y);
                let cmax_x = max_x.min(clip_max.x);
                let cmax_y = max_y.min(clip_max.y);

                if cmin_x >= cmax_x || cmin_y >= cmax_y {
                    return;
                }

                let x_range = max_x - min_x;
                let y_range = max_y - min_y;

                let bilerp = |px: f32, py: f32| -> Vec4 {
                    let tx = if x_range > 0.0 {
                        (px - min_x) / x_range
                    } else {
                        0.0
                    };
                    let ty = if y_range > 0.0 {
                        (py - min_y) / y_range
                    } else {
                        0.0
                    };
                    let top = corners[0].1.lerp(corners[1].1, tx);
                    let bottom = corners[3].1.lerp(corners[2].1, tx);
                    top.lerp(bottom, ty)
                };

                let vi = verts.len() as u32;
                for (x, y) in [
                    (cmin_x, cmin_y),
                    (cmax_x, cmin_y),
                    (cmax_x, cmax_y),
                    (cmin_x, cmax_y),
                ] {
                    verts.push(UIVertex {
                        pos: to_ndc(Vec2::new(x, y)),
                        color: bilerp(x, y),
                        uv: solid,
                        ..Default::default()
                    });
                }
                idxs.extend_from_slice(&[vi, vi + 1, vi + 2, vi, vi + 3, vi + 2]);
            };

        let checker = |ui: &mut Self, pos: Vec2, size: Vec2| {
            let check = 3.0f32;
            let dark = Vec4::new(0.4, 0.4, 0.4, 1.0);
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
                    ui.paint_rect(from_pos_size(p, s), c);
                }
            }
        };

        {
            let tl = sv_pos;
            let tr = sv_pos + Vec2::new(picker_size, 0.0);
            let br = sv_pos + Vec2::splat(picker_size);
            let bl = sv_pos + Vec2::new(0.0, picker_size);

            emit_quad(
                &mut self.ctx.window.verticies,
                &mut self.ctx.window.indicies,
                [
                    (tl, Vec4::ONE),
                    (tr, pure_hue),
                    (br, pure_hue),
                    (bl, Vec4::ONE),
                ],
            );
            emit_quad(
                &mut self.ctx.window.verticies,
                &mut self.ctx.window.indicies,
                [
                    (tl, Vec4::ZERO),
                    (tr, Vec4::ZERO),
                    (br, Vec4::W),
                    (bl, Vec4::W),
                ],
            );

            let cx = sv_pos + Vec2::new(s * picker_size, (1.0 - v) * picker_size);
            let cross = 4.0f32;
            self.paint_rect(
                from_pos_size(cx - Vec2::new(cross, 1.0), Vec2::new(cross * 2.0, 2.0)),
                Vec4::ONE,
            );
            self.paint_rect(
                from_pos_size(cx - Vec2::new(1.0, cross), Vec2::new(2.0, cross * 2.0)),
                Vec4::ONE,
            );
        }

        for sextant in 0..6 {
            let (t0, t1) = (sextant as f32 / 6.0, (sextant + 1) as f32 / 6.0);
            let y0 = hue_pos.y + t0 * picker_size;
            let y1 = hue_pos.y + t1 * picker_size;
            let c0 = hsv_to_rgb(t0, 1.0, 1.0, 1.0);
            let c1 = hsv_to_rgb(t1, 1.0, 1.0, 1.0);
            emit_quad(
                &mut self.ctx.window.verticies,
                &mut self.ctx.window.indicies,
                [
                    (Vec2::new(hue_pos.x, y0), c0),
                    (Vec2::new(hue_pos.x + bar_width, y0), c0),
                    (Vec2::new(hue_pos.x + bar_width, y1), c1),
                    (Vec2::new(hue_pos.x, y1), c1),
                ],
            );
        }
        let cy = hue_pos.y + h * picker_size;
        self.paint_rect(
            from_pos_size(Vec2::new(hue_pos.x, cy - 1.0), Vec2::new(bar_width, 2.0)),
            Vec4::ONE,
        );

        checker(self, alpha_pos, bar);
        let c_top = new_color.with_w(1.0);
        let c_bot = new_color.with_w(0.0);
        emit_quad(
            &mut self.ctx.window.verticies,
            &mut self.ctx.window.indicies,
            [
                (alpha_pos, c_top),
                (alpha_pos + Vec2::new(bar_width, 0.0), c_top),
                (alpha_pos + bar, c_bot),
                (alpha_pos + Vec2::new(0.0, picker_size), c_bot),
            ],
        );
        let cy = alpha_pos.y + (1.0 - a) * picker_size;
        self.paint_rect(
            from_pos_size(Vec2::new(alpha_pos.x, cy - 1.0), Vec2::new(bar_width, 2.0)),
            Vec4::ONE,
        );

        checker(self, preview_pos, preview_size);
        self.paint_rect(from_pos_size(preview_pos, preview_size), new_color);

        let cursor = std::mem::replace(&mut self.cursor, inputs_pos);
        let rgba = self.float_inputs((&id, 3), new_color.as_dvec4().to_array(), full_width);
        self.cursor = cursor;
        self.finish_element(total_size, false);
        Vec4::from_array(rgba.map(|c| c as f32)).clamp(Vec4::ZERO, Vec4::ONE)
    }

    /// Draws `len` values as bars from `min` to `max` (averaged where there are more values
    /// than pixels) and returns the index of the first value of the clicked bar.
    pub fn histogram(
        &mut self,
        width: f32,
        height: f32,
        max: f32,
        min: f32,
        values: impl Iterator<Item = f32>,
        len: usize,
    ) -> Option<usize> {
        let size = Vec2::new(width, height);
        if self.begin_element(size, false) {
            return None;
        }

        let rect = from_pos_size(self.cursor, size);
        self.paint_box(
            rect,
            DrawSettings {
                rounding: 0,
                ..Default::default()
            },
        );

        let inner = rect.inflate(-Self::child_offset().x);
        let mut x = inner.min.x;
        let value_width = inner.width() / len as f32;
        let values_per_pixel = (value_width.recip()).ceil() as usize;
        let mut clicked = None;
        let mut index = 0;
        for values in &values.chunks(values_per_pixel) {
            let (sum, count) = values.fold((0.0, 0), |(sum, count), v| (sum + v, count + 1));
            let value: f32 = sum / count as f32;
            let t = (value - min) / (max - min);
            let color = if !(0.0..=1.0).contains(&t) {
                UiContext::ERROR
            } else {
                UiContext::ACENT
            };
            let bar_width = value_width * count as f32;
            let bar_height = inner.height() * t.clamp(0.0, 1.0);
            self.paint_rect(
                Rect::new(x, inner.max.y - bar_height, x + bar_width, inner.max.y),
                color,
            );
            let column = Rect::new(x, inner.min.y, x + bar_width, inner.max.y);
            if self.hovered(column) {
                self.tooltip_label(format!("{:.5}", value));
                if self.ctx.input.primary_pressed {
                    clicked = Some(index);
                }
            }
            x += bar_width;
            index += count;
        }

        self.finish_element(size, false);
        clicked
    }

    /// Draws `label` in a box at the cursor, on top of everything.
    pub fn tooltip_label(&mut self, label: impl AsRef<str>) {
        let Some(cursor_pos) = self.ctx.input.cursor_pos else {
            return;
        };
        let cursor_pos = cursor_pos.round();
        let info_size = Self::contain_size(UiContext::text_size(label.as_ref()));
        let screen = Rect::from_corners(Vec2::ZERO, self.ctx.viewport_size);
        let info_rect = Rect::from_corners(cursor_pos - info_size, cursor_pos);
        self.ctx.window.draw_box(
            info_rect,
            DrawSettings {
                on_top: true,
                ..Default::default()
            },
            self.ctx.viewport_size,
            screen,
        );
        let color = self.text_color();
        self.ctx.window.draw_text(
            info_rect.min + Self::child_offset(),
            color,
            label.as_ref(),
            self.ctx.viewport_size,
            screen,
            true,
        );
    }

    /// Shows `label` while the previous element is hovered.
    pub fn tooltip(&mut self, label: impl AsRef<str>) {
        if self.prev_hovered() {
            self.tooltip_label(label);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut() {
        let width = |chars: usize| chars as f32 * UiContext::text_len("x");
        assert_eq!(fit_text("box.glb", width(12), Ellipsis::End), "box.glb");
        let bunny = "stanford_bunny.glb";
        assert_eq!(fit_text(bunny, width(12), Ellipsis::End), "stanford_b..");
        assert_eq!(fit_text(bunny, width(12), Ellipsis::Start), ".._bunny.glb");
        assert_eq!(fit_text("äöüäöü", width(4), Ellipsis::End), "äö..");
    }
}
