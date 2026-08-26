use std::{
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU64,
};

use bevy::{
    ecs::{
        message::MessageReader,
        system::{Res, Single, SystemParam, lifetimeless},
    },
    input::{
        ButtonInput,
        keyboard::{KeyCode, KeyboardInput},
        mouse::{AccumulatedMouseScroll, MouseButton},
        touch::Touches,
    },
    math::{Rect, VectorSpace},
    reflect::Reflect,
    window::Window,
};
use glam::{Vec2, Vec4};
use itertools::Itertools;

use crate::{
    bindings::UIVertex,
    ui::{
        Draggable, FocusedState, MultiInput, UiContext, UiWindows, from_pos_size,
        scrollable::Scrollable,
        window::{BorderSettings, DrawSettings, Drawable, Tab, TabState, TextDirection, UiWindow},
    },
};

#[derive(SystemParam)]
pub struct UiBuilder<'w, 's> {
    pub window: Single<'w, 's, lifetimeless::Read<Window>>,
    mouse: Res<'w, ButtonInput<MouseButton>>,
    touch: Res<'w, Touches>,
    scroll: Res<'w, AccumulatedMouseScroll>,
    windows: Res<'w, UiWindows>,
    keys: MessageReader<'w, 's, KeyboardInput>,
    keyspressed: Res<'w, ButtonInput<KeyCode>>,
}

impl<'s, 'w> UiBuilder<'w, 's> {
    pub fn build(
        &mut self,
        label: impl AsRef<str>,
        f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>),
    ) {
        let input = MultiInput::new(&self.window, &self.mouse, &self.touch);
        let mut hovered = input.cursor_pos.is_some();
        let mut window: Option<&UiWindow> = None;
        let mut tab: Option<&Tab> = None;
        for w in self.windows.windows.iter() {
            let Some(w) = w else { continue };
            for (i, t) in w.tabs.iter().enumerate() {
                if t.label.as_str() == label.as_ref() {
                    window = Some(w);
                    tab = Some(t);

                    if i != w.active_tab as usize {
                        return;
                    }
                    break;
                }
            }
            if w.rect.contains(input.cursor_pos.unwrap_or_default()) && window.is_none() {
                hovered = false;
            }
        }

        let Some(tab) = tab else {
            let Ok(mut add_windows) = self.windows.add_windows.lock() else {
                return;
            };
            add_windows.push(label.as_ref().to_string());
            return;
        };
        let window = window.unwrap();

        let scroll = Res::clone(&self.scroll);
        let shift = self.keyspressed.pressed(KeyCode::ShiftLeft)
            || self.keyspressed.pressed(KeyCode::ShiftRight);
        let ctrl = self.keyspressed.pressed(KeyCode::ControlLeft)
            || self.keyspressed.pressed(KeyCode::ControlRight);

        let Ok(mut state) = tab.state.lock() else {
            return;
        };
        let focused_state = unsafe {
            (&window.focused as *const _ as *mut Option<FocusedState>)
                .as_mut()
                .unwrap()
        }
        .as_mut();

        state.build(
            window,
            focused_state,
            &tab.label,
            input,
            &self.window,
            scroll,
            &mut self.keys,
            shift,
            ctrl,
            hovered,
            f,
        );
    }
}

#[derive(Copy, Clone, Reflect, Debug)]
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
        } else {
            text[(self.byte_pos - 1)..].chars().next()
        }
    }
}

pub struct UiWindowContext<'a, 'w, 's> {
    pub window: &'a mut TabState,
    pub focused: &'a mut Option<&'s mut FocusedState>,
    pub keys: &'a mut MessageReader<'w, 's, KeyboardInput>,
    pub window_id: u64,
    pub viewport_size: Vec2,
    pub input: MultiInput,
    pub max_width: f32,
    pub ctrl: bool,
    pub shift: bool,
    pub hovered: bool,
}

pub struct UiWindowBuilder<'a, 'w, 's> {
    pub ctx: UiWindowContext<'a, 'w, 's>,

    // Layout
    pub clip_rect: Rect,
    pub cursor: Vec2,
    pub cursor_origin: Vec2,
    pub prev_cursor: Vec2,
    pub prev_element: Rect,
    pub prev_element_hoverd: bool,
    pub line_height: f32,
    pub content_max: Vec2,
    pub direction: bool,

    // Interaction
    pub focuse_next: bool,
    pub scroll_delta: Vec2,
    pub scroll_consumed: bool,
    pub hovered_smth: bool,
    pub disabled: bool,
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

#[derive(Default)]
pub struct DraggableState {
    pub drag_started: bool,
    pub draging: bool,
    pub dropped: bool,
}

impl<'a, 'w, 's> UiWindowBuilder<'a, 'w, 's> {
    fn id(&self, h: &impl Hash) -> NonZeroU64 {
        let mut hash = DefaultHasher::new();
        h.hash(&mut hash);
        self.ctx.window_id.hash(&mut hash);
        NonZeroU64::new(hash.finish()).unwrap()
    }

    fn element_clicked(&self, rect: Rect) -> bool {
        self.hoverd(rect) && self.ctx.input.primary_pressed
    }

    fn hoverd(&self, rect: Rect) -> bool {
        Self::hoverdp(
            rect,
            self.clip_rect,
            self.ctx.input.cursor_pos,
            self.hovered_smth,
        ) && self.ctx.focused.is_some()
    }

    fn hoverdp(rect: Rect, clip: Rect, cursor_pos: Option<Vec2>, hovered_smth: bool) -> bool {
        cursor_pos
            .map(|pos| rect.intersect(clip).contains(pos) && !hovered_smth)
            .unwrap_or(false)
    }

    pub fn rect(&mut self, size: Vec2, ds: DrawSettings) {
        self.ctx.window.draw_box(
            from_pos_size(self.cursor, size),
            ds,
            self.ctx.viewport_size,
            self.clip_rect,
        );
        self.finish_element(size, false);
    }

    fn begin_element(&mut self, size: Vec2, consume_scroll: bool) -> bool {
        if (self.cursor.cmpgt(self.clip_rect.max)).any()
            || ((self.cursor + size).cmplt(self.clip_rect.min)).any()
        {
            self.finish_element(size, consume_scroll);
            true
        } else {
            false
        }
    }

    fn finish_element(&mut self, size: Vec2, consume_scroll: bool) {
        let size = size.round();
        let rect = from_pos_size(self.cursor, size);
        self.prev_element = rect;
        self.line_height = self.line_height.max(size.y);
        self.content_max = self.content_max.max(self.cursor + size);
        if self.hoverd(rect) {
            self.prev_element_hoverd = true;
            self.hovered_smth = true;
            self.scroll_consumed |= consume_scroll;
        } else {
            self.prev_element_hoverd = false;
        }
        if self.direction {
            self.cursor.x += size.x + UiContext::ELEMENT_GAP.x as f32;
        } else {
            self.cursor.y += size.y + UiContext::ELEMENT_GAP.y as f32;
        }
    }

    pub fn text(&mut self, label: impl AsRef<str>) {
        let size = UiContext::text_size(label.as_ref());
        if self.begin_element(size, false) {
            return;
        }
        self.ctx.window.draw_text(
            self.cursor,
            self.text_color(),
            label.as_ref(),
            self.ctx.viewport_size,
            self.clip_rect,
            false,
        );
        self.finish_element(size, false);
    }

    pub fn child_offset() -> Vec2 {
        UiContext::CHILD_PAD.as_vec2() + UiContext::BORDER.max(UiContext::ROUNDING) as f32
    }

    fn contain_size(size: Vec2) -> Vec2 {
        (size + Self::child_offset() * 2.0).round()
    }

    fn child_cursor(&self) -> Vec2 {
        (self.cursor + Self::child_offset()).round()
    }

    pub fn disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    pub fn button(&mut self, label: impl AsRef<str>) -> bool {
        let size = Self::contain_size(Vec2::new(
            UiContext::text_len(label.as_ref()),
            UiContext::ATLAS_CELL_SIZE.y as f32,
        ));
        if self.begin_element(size, false) {
            return false;
        }
        let hoverd = self.hoverd(Rect::from_corners(self.cursor, self.cursor + size));
        let clicked = self.ctx.input.primary_pressed && hoverd;

        self.ctx.window.draw_box(
            from_pos_size(self.cursor, size),
            DrawSettings::new(hoverd, clicked),
            self.ctx.viewport_size,
            self.clip_rect,
        );
        self.draw_text(self.child_cursor(), label, self.clip_rect, false);
        self.finish_element(size, false);
        clicked
    }

    pub fn droppable<S>(
        &mut self,
        state: &mut S,
        valid_drop_target: impl FnOnce() -> bool,
        children: impl FnOnce(&mut Self, &mut S),
        on_drop: impl FnOnce(&mut S),
    ) {
        let prev = self.cursor;
        let content_max = self.content_max;

        self.content_max = Vec2::ZERO;
        children(self, state);
        let rect = Rect::from_corners(prev, self.content_max);

        let ds = DrawSettings {
            border: Some(BorderSettings::uniform(UiContext::ACENT, 4)),
            color: Vec4::ZERO,
            rounding: 8,
            on_top: true,
            ..Default::default()
        };

        let hoverd = self.ctx.input.hovered(rect);
        if self.ctx.hovered && hoverd && valid_drop_target() {
            let rect = Rect::from_corners(
                rect.min - Vec2::new(4.0, 4.0),
                rect.max + Vec2::new(4.0, 4.0),
            );

            self.ctx
                .window
                .draw_box(rect, ds, self.ctx.viewport_size, self.clip_rect);

            if self.ctx.input.primary_released {
                on_drop(state);
            }
        }
        self.content_max = self.content_max.max(content_max);
    }

    pub fn draggable(
        &mut self,
        id: impl Hash,
        children: impl FnOnce(&mut Self, DraggableState),
        drag_icon: impl FnOnce(&mut Self),
    ) {
        let id = self.id(&id);

        let prev = self.cursor;
        let content_max = self.content_max;
        self.content_max = Vec2::ZERO;

        let mut drag_state = DraggableState::default();
        if let Some(focused) = self.ctx.focused {
            if focused.draging == Some(Draggable::Element(id))
                && (focused.drag_start - self.ctx.input.cursor_pos.unwrap()).length()
                    > UiContext::DRAG_THRESHHOLD
            {
                focused.draging = Some(Draggable::DragAndDrop(id));
                drag_state.drag_started = true;
            }
            drag_state.draging = focused.draging == Some(Draggable::DragAndDrop(id));
        }

        children(self, drag_state);
        let rect = Rect::from_corners(prev, self.content_max);

        if let Some(focused) = self.ctx.focused {
            let hoverd = self.ctx.input.hovered(rect);
            if hoverd && self.ctx.input.primary_pressed {
                focused.draging = Some(Draggable::Element(id));
                focused.drag_start = self.ctx.input.cursor_pos.unwrap();
            }

            if focused.draging == Some(Draggable::DragAndDrop(id)) {
                self.render_drag_icon(drag_icon);
            }
        }

        self.content_max = self.content_max.max(content_max);
    }

    fn render_drag_icon(&mut self, drag_icon: impl FnOnce(&mut Self)) {
        let verticies = std::mem::take(&mut self.ctx.window.verticies);
        let indicies = std::mem::take(&mut self.ctx.window.indicies);

        let cursor = self.cursor;
        self.cursor = self.ctx.input.cursor_pos.unwrap();
        let clip_rect = self.clip_rect;
        self.clip_rect = Rect::from_corners(Vec2::ZERO, self.ctx.viewport_size);

        let content_max = self.content_max;
        drag_icon(self);
        self.content_max = content_max;

        let offset = self.ctx.window.top_verticies.len();
        self.ctx
            .window
            .top_verticies
            .append(&mut self.ctx.window.verticies);
        self.ctx.window.top_indicies.extend(
            self.ctx
                .window
                .indicies
                .drain(..)
                .map(|i| i + offset as u32),
        );

        self.ctx.window.verticies = verticies;
        self.ctx.window.indicies = indicies;
        self.cursor = cursor;
        self.clip_rect = clip_rect;
    }

    pub fn slider(&mut self, id: impl Hash, min: f32, max: f32, width: f32, value: f32) -> f32 {
        let id = self.id(&id);
        let mut ds = DrawSettings::default();

        let line_size = UiContext::ATLAS_CELL_SIZE.y as f32;
        let slider_height = line_size / 3.0;

        let size = Vec2::new(width, slider_height);
        let slide_size = Vec2::new(16.0, line_size);

        if self.begin_element(Vec2::new(width, slide_size.y), false) {
            return value;
        }
        let slider_pos = self.cursor + Vec2::new(0.0, (line_size - slider_height) / 2.0);
        self.ctx.window.draw_box(
            from_pos_size(slider_pos, size),
            ds,
            self.ctx.viewport_size,
            self.clip_rect,
        );

        let slide_pos = if max != min {
            self.cursor
                + Vec2::new(
                    f32::clamp((value - min) / (max - min) * width, 0.0, width)
                        - slide_size.x * 0.5,
                    0.0,
                )
                .round()
        } else {
            (self.cursor + Vec2::new(width / 2.0, 0.0)).round()
        };

        let slide = from_pos_size(slide_pos, slide_size);

        if self.element_clicked(slide) {
            if let Some(f) = &mut self.ctx.focused {
                f.draging = Some(Draggable::Element(id.into()));
                f.drag_start = self.cursor;
            }
        }

        ds.color = UiContext::ACENT;
        ds.rounding = 4;

        self.ctx
            .window
            .draw_box(slide, ds, self.ctx.viewport_size, self.clip_rect);

        let mut ret = value;
        if let Some(f) = &self.ctx.focused
            && min != max
        {
            if let Some(Draggable::Element(element)) = f.draging
                && element == id
                && let Some(cursor) = self.ctx.input.cursor_pos
            {
                let val = (cursor - f.drag_start).project_onto(Vec2::new(1.0, 0.0)).x;
                ret = f32::clamp(val / width * (max - min) + min, min, max);
            }
        }

        self.finish_element(Vec2::new(width, slide_size.y), false);
        ret
    }

    const WORD_DELIMITER: [char; 6] = [' ', '.', ',', ':', '(', ')'];

    fn text_input_private(
        &mut self,
        id: impl Hash,
        width: f32,
        input_mode: InputMode,
    ) -> InputModeOutput {
        let id = self.id(&id);
        let inner_size = Vec2::new(width, UiContext::ATLAS_CELL_SIZE.y as f32);
        let size = Self::contain_size(inner_size);
        if self.begin_element(size, false) {
            return InputModeOutput::ChangDetection(false);
        }
        let text_color = self.text_color();
        let clicked = self.element_clicked(from_pos_size(self.cursor, size));
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
        let text_clip = from_pos_size(text_cursor, inner_size).intersect(self.clip_rect);

        let mut just_focused = false;
        let mut focused = if let Some(focused) = self.ctx.focused.as_mut() {
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
                focused.selected = (0..value.len()).into();
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
                        cursor.move_left(&value);
                        if self.ctx.ctrl {
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
                    if has_selection && !self.ctx.shift {
                        cursor.byte_pos = sel_max;
                    } else {
                        cursor.move_right(&value);
                        if self.ctx.ctrl {
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
                        changed = true;
                        cursor.byte_pos = sel_min;
                        selected.start = sel_min;
                        selected.end = sel_min;
                    } else {
                        cursor.delete_before(value);
                        changed = true;
                        if self.ctx.ctrl {
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
                        changed = true;
                        cursor.byte_pos = sel_min;
                        selected.start = sel_min;
                        selected.end = sel_min;
                    } else {
                        cursor.delete_after(value);
                        changed = true;
                        if self.ctx.ctrl {
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
                        self.hovered_smth,
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
            let right = (offset - width + 5.0).max(0.0);
            if right > **view {
                **view = right;
            }
        }
        let focused = focused.map(|(e1, e2, e3, _)| (*e1, *e2, e3.clone()));

        self.ctx.window.draw_box(
            from_pos_size(self.cursor, size),
            ds,
            self.ctx.viewport_size,
            self.clip_rect,
        );

        let p = text_cursor - Vec2::new(focused.as_ref().map(|e| e.1).unwrap_or(0.0), 0.0);
        self.ctx.window.draw_text(
            p,
            text_color,
            &value,
            self.ctx.viewport_size,
            text_clip,
            false,
        );

        if let Some((cursor, offset, selected)) = &focused {
            let x = UiContext::text_len(&value[..cursor.byte_pos]);
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
            self.ctx.window.draw_box(
                from_pos_size(
                    text_cursor + Vec2::new(x - *offset, 0.0),
                    Vec2::new(1.0, UiContext::ATLAS_CELL_SIZE.y as f32),
                ),
                ds,
                self.ctx.viewport_size,
                text_clip,
            );

            ds.color = UiContext::ACENT_DIM;
            let start = selected.start.min(selected.end);
            let end = selected.start.max(selected.end);

            let start = UiContext::text_len(&value[..start]);
            let end = UiContext::text_len(&value[..end]);

            self.ctx.window.draw_box(
                from_pos_size(
                    text_cursor + Vec2::new(start - offset, 0.0),
                    Vec2::new(end - start, UiContext::ATLAS_CELL_SIZE.y as f32),
                ),
                ds,
                self.ctx.viewport_size,
                text_clip,
            );
        }

        let ret = match input_mode {
            InputType::Float(f) => InputModeOutput::Float(value.parse().unwrap_or(f)),
            InputType::Int(i) => InputModeOutput::Int(value.parse().unwrap_or(i)),
            InputType::String => InputModeOutput::ChangDetection(changed),
        };

        self.finish_element(size, false);
        ret
    }

    pub fn text_input(&mut self, id: impl Hash, value: &mut String, width: f32) -> bool {
        let changed = if let InputModeOutput::ChangDetection(changed) =
            self.text_input_private(id, width, InputMode::String(value))
        {
            changed
        } else {
            false
        };
        changed
    }

    pub fn float_input(&mut self, id: impl Hash, value: f64, width: f32) -> f64 {
        if let InputModeOutput::Float(v) =
            self.text_input_private(id, width, InputMode::Float(value))
        {
            v
        } else {
            value
        }
    }

    pub fn int_input(&mut self, id: impl Hash, value: i64, width: f32) -> i64 {
        if let InputModeOutput::Int(v) = self.text_input_private(id, width, InputMode::Int(value)) {
            v
        } else {
            value
        }
    }

    pub fn checkbox(&mut self, mut value: bool) -> bool {
        let size = Self::contain_size(UiContext::ATLAS_CELL_SIZE.as_vec2());
        if self.begin_element(size, false) {
            return value;
        }
        let rect = from_pos_size(self.cursor, size);
        let hoverd = self.hoverd(rect);
        if self.ctx.input.primary_pressed && hoverd {
            value = !value;
        }
        let ds = DrawSettings::new(hoverd, false);
        self.ctx
            .window
            .draw_box(rect, ds, self.ctx.viewport_size, self.clip_rect);
        if value {
            self.ctx.window.draw_text(
                self.child_cursor(),
                UiContext::ACENT,
                "✓",
                self.ctx.viewport_size,
                self.clip_rect,
                false,
            );
        }
        self.finish_element(size, false);
        value
    }

    pub fn dropdown(&mut self, id: impl Hash, mut selected: usize, options: &[&str]) -> usize {
        let id = self.id(&id);

        let sizex = options
            .iter()
            .map(|o| UiContext::text_len(*o))
            .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Less))
            .unwrap_or(0.0);
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
        let hoverd = self.hoverd(rect);
        let open = self
            .ctx
            .focused
            .as_ref()
            .is_some_and(|e| e.focused == Some(id));

        let ds = DrawSettings {
            color: if hoverd { UiContext::S1 } else { UiContext::S0 },
            round_bottomleft: !open,
            round_bottomright: !open,
            ..Default::default()
        };

        self.ctx
            .window
            .draw_box(rect, ds, self.ctx.viewport_size, self.clip_rect);
        self.draw_text(
            self.child_cursor(),
            options[selected],
            self.clip_rect,
            false,
        );
        self.ctx.window.draw_text_direction(
            self.child_cursor()
                + Vec2::new(
                    sizex + UiContext::ELEMENT_GAP.x as f32,
                    if !open {
                        UiContext::ATLAS_CELL_SIZE.x as f32 * 1.5
                    } else {
                        0.0
                    },
                ),
            self.text_color(),
            "▼",
            self.ctx.viewport_size,
            self.clip_rect,
            if open {
                TextDirection::Right
            } else {
                TextDirection::Up
            },
        );
        if hoverd && self.ctx.input.primary_pressed {
            if let Some(f) = &mut self.ctx.focused {
                f.focused = if f.focused == Some(id) {
                    None
                } else {
                    Some(id)
                };
            }
        }
        if let Some(f) = &mut self.ctx.focused
            && f.focused == Some(id)
        {
            let mut cursor = self.cursor + Vec2::new(0.0, button_size.y);
            for (i, o) in options.iter().enumerate() {
                let rect = from_pos_size(cursor, button_size);
                let last = i + 1 == options.len();
                let hovered = self.hoverd(rect);
                if hovered {
                    self.hovered_smth = true;
                    if self.ctx.input.primary_pressed {
                        selected = i;
                        if let Some(f) = &mut self.ctx.focused {
                            f.focused = None;
                        }
                    }
                }
                let border = BorderSettings {
                    color_bottom: if last {
                        UiContext::S2
                    } else if hovered {
                        UiContext::S1
                    } else {
                        UiContext::S0
                    },
                    color_top: if hovered {
                        UiContext::S1
                    } else {
                        UiContext::S0
                    },
                    color_left: UiContext::S2,
                    color_right: UiContext::S2,
                    size: UiContext::BORDER,
                };
                let ds = DrawSettings {
                    color: if hovered {
                        UiContext::S1
                    } else {
                        UiContext::S0
                    },
                    border: Some(border),
                    round_bottomleft: last,
                    round_bottomright: last,
                    round_topleft: false,
                    round_topright: false,
                    rounding: 0,
                    on_top: true,
                };
                self.ctx
                    .window
                    .draw_box(rect, ds, self.ctx.viewport_size, self.clip_rect);
                self.draw_text(
                    (cursor + Self::child_offset()).round(),
                    o,
                    self.clip_rect,
                    true,
                );
                cursor.y += button_size.y;
            }
        }
        self.finish_element(button_size, false);
        selected
    }

    fn text_color(&self) -> Vec4 {
        if self.disabled {
            UiContext::TEXT_DIM
        } else {
            UiContext::TEXT
        }
    }

    fn draw_text(&mut self, pos: Vec2, text: impl AsRef<str>, clip: Rect, on_top: bool) {
        self.ctx.window.draw_text(
            pos,
            self.text_color(),
            text.as_ref(),
            self.ctx.viewport_size,
            clip,
            on_top,
        );
    }

    pub fn collapsable<R>(
        &mut self,
        label: impl Hash + AsRef<str>,
        children: impl FnOnce(&mut Self) -> R,
    ) -> Option<R> {
        let id = self.id(&label);

        let rmb = UiContext::ROUNDING.max(UiContext::BORDER) as f32;
        let size = Vec2::new(
            self.remaining_width() - (UiContext::CHILD_PAD.x as f32 + rmb),
            UiContext::ATLAS_CELL_SIZE.y as f32 + (UiContext::CHILD_PAD.y as f32 + rmb) * 2.0,
        )
        .floor();
        let rect = from_pos_size(self.cursor, size);

        let hoverd = self.hoverd(rect);

        let text_cursor = self.child_cursor();
        if hoverd && self.ctx.input.primary_pressed {
            let headers = &mut self.ctx.window.open_headers;
            if !headers.insert(id.into()) {
                headers.remove(&id.into());
            }
        }

        let open = !self.ctx.window.open_headers.contains(&id.into());

        if !open {
            if self.begin_element(size, false) {
                return None;
            }
        }

        self.ctx.window.draw_box(
            rect,
            DrawSettings::new(hoverd, false),
            self.ctx.viewport_size,
            self.clip_rect,
        );

        self.ctx.window.draw_text_direction(
            text_cursor
                + if !open {
                    Vec2::new(0.0, UiContext::ATLAS_CELL_SIZE.x as f32 * 1.5)
                } else {
                    Vec2::ZERO
                },
            self.text_color(),
            "▼",
            self.ctx.viewport_size,
            self.clip_rect.intersect(rect),
            if open {
                TextDirection::Right
            } else {
                TextDirection::Up
            },
        );

        self.draw_text(
            Vec2::new(
                text_cursor.x
                    + UiContext::ATLAS_CELL_SIZE.x as f32
                    + UiContext::ELEMENT_GAP.x as f32 * 2.0,
                text_cursor.y,
            ),
            label,
            self.clip_rect.intersect(rect),
            false,
        );

        self.finish_element(size, false);

        let prev = self.cursor;
        self.cursor += UiContext::INDENT.as_vec2();
        let res = if open { Some(children(self)) } else { None };
        let cursor = self.cursor;
        self.cursor = prev;
        if open {
            self.finish_element(Vec2::new(0.0, cursor.y - prev.y), false);
        }
        res
    }

    pub fn remaining_width(&self) -> f32 {
        (self.cursor_origin.x + self.ctx.max_width - self.cursor.x).max(0.0)
    }

    pub fn color_picker(&mut self, id: impl Hash, color: Vec4) -> Vec4 {
        let color = color.clamp(Vec4::ZERO, Vec4::ONE);
        let picker_size = 150.0f32;
        let bar_width = 14.0f32;
        let gap = UiContext::ELEMENT_GAP.x as f32;
        let rmb = UiContext::ROUNDING.max(UiContext::BORDER) as f32;

        let input_width = picker_size / 4.0 - gap + UiContext::BORDER as f32;
        let input_h =
            UiContext::ATLAS_CELL_SIZE.y as f32 + (UiContext::CHILD_PAD.as_vec2().y + rmb) * 2.0;
        let full_width = picker_size + gap + bar_width + gap + bar_width;

        let total_size = Vec2::new(full_width, picker_size + gap + bar_width + gap + input_h);

        if self.begin_element(total_size, false) {
            return color;
        }

        let id = self.id(&id);

        let sv_pos = self.cursor;
        let hue_pos = self.cursor + Vec2::new(picker_size + gap, 0.0);
        let alpha_pos = self.cursor + Vec2::new(picker_size + gap + bar_width + gap, 0.0);
        let preview_pos = self.cursor + Vec2::new(0.0, picker_size + gap);
        let preview_size = Vec2::new(full_width, bar_width);
        let inputs_pos = self.cursor + Vec2::new(0.0, picker_size + gap + bar_width + gap);

        let (mut h, mut s, mut v, mut a) = rgb_to_hsv(color);

        let id_sv = NonZeroU64::new(id.get()).unwrap();
        let id_hue = NonZeroU64::new(id.get() + 1).unwrap();
        let id_alpha = NonZeroU64::new(id.get() + 2).unwrap();

        if let Some(cursor_pos) = self.ctx.input.cursor_pos
            && self.ctx.input.primary_pressing
        {
            if let Some(f) = &mut self.ctx.focused {
                if self.ctx.input.primary_pressed {
                    let sv_rect = Rect::from_corners(sv_pos, sv_pos + Vec2::splat(picker_size));
                    let hue_rect =
                        Rect::from_corners(hue_pos, hue_pos + Vec2::new(bar_width, picker_size));
                    let alpha_rect = Rect::from_corners(
                        alpha_pos,
                        alpha_pos + Vec2::new(bar_width, picker_size),
                    );
                    if sv_rect.contains(cursor_pos) {
                        f.draging = Some(Draggable::Element(id_sv));
                    } else if hue_rect.contains(cursor_pos) {
                        f.draging = Some(Draggable::Element(id_hue));
                    } else if alpha_rect.contains(cursor_pos) {
                        f.draging = Some(Draggable::Element(id_alpha));
                    }

                    if f.draging == Some(Draggable::Element(id_sv)) {
                        s = ((cursor_pos.x - sv_pos.x) / picker_size).clamp(0.0, 1.0);
                        v = 1.0 - ((cursor_pos.y - sv_pos.y) / picker_size).clamp(0.0, 1.0);
                    } else if f.draging == Some(Draggable::Element(id_hue)) {
                        h = ((cursor_pos.y - hue_pos.y) / picker_size).clamp(0.0, 1.0);
                    } else if f.draging == Some(Draggable::Element(id_alpha)) {
                        a = 1.0 - ((cursor_pos.y - alpha_pos.y) / picker_size).clamp(0.0, 1.0);
                    }
                }
            }
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
                verts.extend_from_slice(&[
                    UIVertex {
                        pos: to_ndc(Vec2::new(cmin_x, cmin_y)),
                        color: bilerp(cmin_x, cmin_y),
                        uv: solid,
                    },
                    UIVertex {
                        pos: to_ndc(Vec2::new(cmax_x, cmin_y)),
                        color: bilerp(cmax_x, cmin_y),
                        uv: solid,
                    },
                    UIVertex {
                        pos: to_ndc(Vec2::new(cmax_x, cmax_y)),
                        color: bilerp(cmax_x, cmax_y),
                        uv: solid,
                    },
                    UIVertex {
                        pos: to_ndc(Vec2::new(cmin_x, cmax_y)),
                        color: bilerp(cmin_x, cmax_y),
                        uv: solid,
                    },
                ]);
                idxs.extend_from_slice(&[vi, vi + 1, vi + 2, vi, vi + 3, vi + 2]);
            };

        let checker = |win: &mut TabState, pos: Vec2, size: Vec2| {
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
                    win.draw_rect(
                        from_pos_size(p, s),
                        None,
                        c,
                        self.ctx.viewport_size,
                        self.clip_rect,
                        false,
                    );
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
                    (tl, Vec4::new(0.0, 0.0, 0.0, 0.0)),
                    (tr, Vec4::new(0.0, 0.0, 0.0, 0.0)),
                    (br, Vec4::new(0.0, 0.0, 0.0, 1.0)),
                    (bl, Vec4::new(0.0, 0.0, 0.0, 1.0)),
                ],
            );

            let cx = sv_pos + Vec2::new(s * picker_size, (1.0 - v) * picker_size);
            let cross = 4.0f32;
            self.ctx.window.draw_rect(
                from_pos_size(cx - Vec2::new(cross, 1.0), Vec2::new(cross * 2.0, 2.0)),
                None,
                Vec4::ONE,
                self.ctx.viewport_size,
                self.clip_rect,
                false,
            );
            self.ctx.window.draw_rect(
                from_pos_size(cx - Vec2::new(1.0, cross), Vec2::new(2.0, cross * 2.0)),
                None,
                Vec4::ONE,
                self.ctx.viewport_size,
                self.clip_rect,
                false,
            );
        }

        {
            let sextants: [(f32, f32); 6] = [
                (0.0 / 6.0, 1.0 / 6.0),
                (1.0 / 6.0, 2.0 / 6.0),
                (2.0 / 6.0, 3.0 / 6.0),
                (3.0 / 6.0, 4.0 / 6.0),
                (4.0 / 6.0, 5.0 / 6.0),
                (5.0 / 6.0, 6.0 / 6.0),
            ];
            for (t0, t1) in sextants {
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
            self.ctx.window.draw_rect(
                from_pos_size(Vec2::new(hue_pos.x, cy - 1.0), Vec2::new(bar_width, 2.0)),
                None,
                Vec4::ONE,
                self.ctx.viewport_size,
                self.clip_rect,
                false,
            );
        }

        {
            checker(
                &mut self.ctx.window,
                alpha_pos,
                Vec2::new(bar_width, picker_size),
            );
            let c_top = Vec4::new(new_color.x, new_color.y, new_color.z, 1.0);
            let c_bot = Vec4::new(new_color.x, new_color.y, new_color.z, 0.0);
            emit_quad(
                &mut self.ctx.window.verticies,
                &mut self.ctx.window.indicies,
                [
                    (alpha_pos, c_top),
                    (alpha_pos + Vec2::new(bar_width, 0.0), c_top),
                    (alpha_pos + Vec2::new(bar_width, picker_size), c_bot),
                    (alpha_pos + Vec2::new(0.0, picker_size), c_bot),
                ],
            );
            let cy = alpha_pos.y + (1.0 - a) * picker_size;
            self.ctx.window.draw_rect(
                from_pos_size(Vec2::new(alpha_pos.x, cy - 1.0), Vec2::new(bar_width, 2.0)),
                None,
                Vec4::ONE,
                self.ctx.viewport_size,
                self.clip_rect,
                false,
            );
        }

        {
            checker(&mut self.ctx.window, preview_pos, preview_size);
            self.ctx.window.draw_rect(
                from_pos_size(preview_pos, preview_size),
                None,
                new_color,
                self.ctx.viewport_size,
                self.clip_rect,
                false,
            );
        }

        {
            let saved_cursor = self.cursor;
            let saved_direction = self.direction;

            self.cursor = inputs_pos;
            self.direction = true;

            let r = self.float_input(id.get() + 10, new_color.x as f64, input_width) as f32;
            let g = self.float_input(id.get() + 11, new_color.y as f64, input_width) as f32;
            let b = self.float_input(id.get() + 12, new_color.z as f64, input_width) as f32;
            let a = self.float_input(id.get() + 13, new_color.w as f64, input_width) as f32;

            self.cursor = saved_cursor;
            self.direction = saved_direction;

            self.finish_element(total_size, false);

            Vec4::new(r, g, b, a).clamp(Vec4::ZERO, Vec4::ONE)
        }
    }

    pub fn container<R>(&mut self, id: impl Hash, size: Vec2, f: impl FnOnce(&mut Self) -> R) -> R {
        let id = self.id(&id);
        let scroll = self
            .ctx
            .window
            .scrollables
            .entry(id.into())
            .or_insert(Scrollable {
                content_size: size,
                scroll: Vec2::ZERO,
            })
            .scroll;

        let rect = from_pos_size(self.cursor, size);
        self.ctx.window.draw_box(
            rect,
            DrawSettings::default(),
            self.ctx.viewport_size,
            self.clip_rect,
        );

        let prev_cursor = self.cursor;
        let cr = self.clip_rect;

        let content_max = self.content_max;
        let hoverd = self.hovered_smth;

        self.clip_rect = self.clip_rect.intersect(rect);
        self.cursor = (self.cursor + UiContext::WINDOW_PAD.as_vec2() - scroll).round();
        let org = self.cursor;
        self.content_max = Vec2::ZERO;
        let r = f(self);

        self.hovered_smth = hoverd;

        let (_, mut scrollable) = self
            .ctx
            .window
            .scrollables
            .remove_entry(&id.into())
            .unwrap();
        scrollable.content_size = self.content_max - org;
        scrollable.update_and_draw(
            Draggable::Element(id),
            rect,
            self.ctx.window,
            &mut self.ctx.focused,
            self.ctx.viewport_size,
            self.ctx.input.cursor_pos,
            self.ctx.input.primary_pressed,
            self.clip_rect,
        );

        if self.hoverd(rect) && !self.scroll_consumed {
            scrollable.scroll(self.scroll_delta, size);
        }

        self.ctx.window.scrollables.insert(id.into(), scrollable);
        self.cursor = prev_cursor;
        self.content_max = content_max;
        self.finish_element(size, true);
        self.clip_rect = cr;
        r
    }

    pub fn histogram<'b>(
        &mut self,
        width: f32,
        height: f32,
        max: f32,
        min: f32,
        values: impl Iterator<Item = &'b f32>,
        len: usize,
    ) {
        let size = Self::contain_size(Vec2::new(width, height));
        if self.begin_element(size, false) {
            return;
        }

        let rect = from_pos_size(self.cursor, size);
        self.ctx.window.draw_box(
            rect,
            DrawSettings {
                rounding: 0,
                round_bottomleft: false,
                round_bottomright: false,
                round_topleft: false,
                round_topright: false,
                ..Default::default()
            },
            self.ctx.viewport_size,
            self.clip_rect,
        );

        let mut child_cursor = self.child_cursor() + Vec2::new(0.0, height);
        let value_width = width / len as f32;
        let values_per_pixel = (value_width.recip()).ceil() as usize;
        for values in &values.chunks(values_per_pixel) {
            let value: f32 = values.sum::<f32>() / values_per_pixel as f32;
            let t = (value + min) / (max - min);
            let value_height = 0.0.lerp(height, t.clamp(0.0, 1.0));
            let color = if t > 1.0 || t < 0.0 {
                UiContext::ERROR
            } else {
                UiContext::ACENT
            };
            let value_rect = Rect::from_corners(
                child_cursor,
                child_cursor - Vec2::new(-value_width, value_height),
            );
            self.ctx.window.draw_rect(
                value_rect,
                None,
                color,
                self.ctx.viewport_size,
                self.clip_rect,
                false,
            );
            if self.hoverd(value_rect) {
                self.tooltip_label(format!("{:.5}", value));
            }
            child_cursor.x += value_width;
        }

        self.finish_element(size, false);
    }

    fn tooltip_label(&mut self, label: impl AsRef<str>) {
        if let Some(cursor_pos) = self.ctx.input.cursor_pos {
            let cursor_pos = cursor_pos.round();
            let info_size = Self::contain_size(UiContext::text_size(label.as_ref()).round());
            let fullscreen_rect = Rect {
                min: Vec2::ZERO,
                max: self.ctx.viewport_size,
            };
            let info_rect = Rect {
                min: cursor_pos - info_size,
                max: cursor_pos,
            };
            self.ctx.window.draw_box(
                info_rect,
                DrawSettings {
                    on_top: true,
                    ..Default::default()
                },
                self.ctx.viewport_size,
                fullscreen_rect,
            );
            self.draw_text(
                info_rect.min + Self::child_offset(),
                label,
                fullscreen_rect,
                true,
            );
        }
    }

    pub fn tooltip(&mut self, label: impl AsRef<str>) {
        if self.prev_element_hoverd {
            self.tooltip_label(label);
        }
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

            self.cursor.y += self.line_height + UiContext::ELEMENT_GAP.y as f32;
        }
    }
}
