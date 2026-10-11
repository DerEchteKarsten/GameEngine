//! Immediate-mode UI builder core: windows, layout scopes, interaction, painting, scroll areas and drag-and-drop on the previous element.
use std::{
    any::Any,
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU64,
    sync::{Mutex, atomic::Ordering},
};

use bevy::{
    asset::{Asset, Handle},
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
    math::Rect,
    reflect::Reflect,
    window::Window,
};
use glam::{Vec2, Vec4};
use lava::bindless::BindlessHandle;

use crate::ui::{
    Draggable, FocusedState, MultiInput, UiContext, UiWindows, WindowRequest,
    dragdrop::{AssetDrag, DragDrop},
    from_pos_size,
    scrollable::Scrollable,
    window::{BorderSettings, DrawSettings, Drawable, Tab, TabState, UiOverlay, UiWindow},
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
    dnd: Res<'w, DragDrop>,
}

impl<'s, 'w> UiBuilder<'w, 's> {
    /// Shorthand for `self.window(label).build(f)`.
    pub fn build(
        &mut self,
        label: impl AsRef<str>,
        f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>),
    ) {
        self.window(label.as_ref()).build(f);
    }

    /// Starts a window that can be given an initial position and size before it is built.
    /// A window stays open only as long as it is built every frame.
    pub fn window<'a>(&'a mut self, label: &'a str) -> UiWindowConfig<'a, 'w, 's> {
        UiWindowConfig {
            ui: self,
            label,
            pos: None,
            size: None,
        }
    }
}

pub struct UiWindowConfig<'a, 'w, 's> {
    ui: &'a mut UiBuilder<'w, 's>,
    label: &'a str,
    pos: Option<Vec2>,
    size: Option<Vec2>,
}

impl<'a, 'w, 's> UiWindowConfig<'a, 'w, 's> {
    /// Initial position in physical pixels. Only applies when the window is first created.
    pub fn position(mut self, pos: Vec2) -> Self {
        self.pos = Some(pos);
        self
    }

    /// Initial size in physical pixels. Only applies when the window is first created.
    pub fn size(mut self, size: Vec2) -> Self {
        self.size = Some(size);
        self
    }

    pub fn build(self, f: impl FnOnce(&mut UiWindowBuilder<'_, 'w, 's>)) {
        let Self {
            ui,
            label,
            pos,
            size,
        } = self;
        let input = MultiInput::new(&ui.window, &ui.mouse, &ui.touch);
        let mut hovered = input.cursor_pos.is_some();
        let mut window: Option<&UiWindow> = None;
        let mut tab: Option<&Tab> = None;
        for w in ui.windows.windows.iter() {
            let Some(w) = w else { continue };
            for (i, t) in w.tabs.iter().enumerate() {
                if t.label.as_str() == label {
                    t.built.store(true, Ordering::Relaxed);
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
            if let Ok(mut requests) = ui.windows.requests.lock() {
                requests.push(WindowRequest {
                    label: label.to_string(),
                    pos,
                    size,
                });
            }
            return;
        };
        let window = window.unwrap();

        let scroll = Res::clone(&ui.scroll);
        let shift = ui.keyspressed.pressed(KeyCode::ShiftLeft)
            || ui.keyspressed.pressed(KeyCode::ShiftRight);
        let ctrl = ui.keyspressed.pressed(KeyCode::ControlLeft)
            || ui.keyspressed.pressed(KeyCode::ControlRight);

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
            &ui.window,
            scroll,
            &mut ui.keys,
            shift,
            ctrl,
            hovered,
            &ui.dnd,
            &ui.windows.overlay,
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
    pub scrollable: Scrollable,
    pub window_id: u64,
    pub window_rect: Rect,
    pub window_content_rect: Rect,
    pub origin: Vec2,
    pub viewport_size: Vec2,
    pub input: MultiInput,
    pub scroll_delta: Vec2,
    pub ctrl: bool,
    pub shift: bool,
    pub hovered: bool,
    pub dnd: &'a DragDrop,
    pub overlay: &'a Mutex<UiOverlay>,
}

/// Lays out elements one after another, downwards or, inside `row`, to the right. Every
/// element ends up in `prev_element`, which the `prev_*`, `tooltip`, `drag_source` and
/// `drop_target` calls after it act on. Widget widths are outer widths, frame included.
pub struct UiWindowBuilder<'a, 'w, 's> {
    pub ctx: UiWindowContext<'a, 'w, 's>,

    pub clip_rect: Rect,
    pub cursor: Vec2,
    pub prev_element: Rect,
    pub(super) content_max: Vec2,
    pub(super) horizontal: bool,

    pub(super) focuse_next: bool,
    pub(super) scroll_consumed: bool,
    pub(super) hovered_consumed: bool,
    pub(super) disable_all_input: bool,
}

/// A drag in progress: the cursor position and how far it moved since the last frame.
pub struct Drag {
    pub pos: Vec2,
    pub delta: Vec2,
}

impl<'a, 'w, 's> UiWindowBuilder<'a, 'w, 's> {
    pub(super) fn id(&self, h: &impl Hash) -> NonZeroU64 {
        let mut hash = DefaultHasher::new();
        h.hash(&mut hash);
        self.ctx.window_id.hash(&mut hash);
        NonZeroU64::new(hash.finish()).unwrap()
    }

    // Layout

    /// Finishes an element of `size` at once if it is outside the clip rect, returning true,
    /// so the caller can skip drawing it.
    pub(super) fn begin_element(&mut self, size: Vec2, consume_scroll: bool) -> bool {
        if (self.cursor.cmpgt(self.clip_rect.max)).any()
            || ((self.cursor + size).cmplt(self.clip_rect.min)).any()
        {
            self.finish_element(size, consume_scroll);
            true
        } else {
            false
        }
    }

    pub(super) fn finish_element(&mut self, size: Vec2, consume_scroll: bool) {
        let size = size.round();
        let rect = from_pos_size(self.cursor, size);
        self.prev_element = rect;
        self.content_max = self.content_max.max(self.cursor + size);
        if self.hovered(rect) {
            self.hovered_consumed = true;
            self.scroll_consumed |= consume_scroll;
        }
        if self.horizontal {
            self.cursor.x += size.x + UiContext::ELEMENT_GAP.x as f32;
        } else {
            self.cursor.y += size.y + UiContext::ELEMENT_GAP.y as f32;
        }
    }

    /// Space between a frame's edge and its content.
    pub fn child_offset() -> Vec2 {
        UiContext::CHILD_PAD.as_vec2() + UiContext::BORDER.max(UiContext::ROUNDING) as f32
    }

    pub(super) fn contain_size(size: Vec2) -> Vec2 {
        (size + Self::child_offset() * 2.0).round()
    }

    pub(super) fn child_cursor(&self) -> Vec2 {
        (self.cursor + Self::child_offset()).round()
    }

    /// Height of a framed widget (button, input, checkbox).
    pub fn widget_height() -> f32 {
        Self::contain_size(UiContext::ATLAS_CELL_SIZE.as_vec2()).y
    }

    /// Runs `f` at the cursor and returns the bounds of what it laid out. The cursor is back
    /// where it started.
    fn measure<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> (R, Rect) {
        let start = self.cursor;
        let content_max = std::mem::replace(&mut self.content_max, start);
        let r = f(self);
        let rect = Rect::from_corners(start, self.content_max.max(start));
        self.content_max = content_max.max(self.content_max);
        self.cursor = start;
        (r, rect)
    }

    fn scope<R>(&mut self, horizontal: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = std::mem::replace(&mut self.horizontal, horizontal);
        let (r, rect) = self.measure(f);
        self.horizontal = outer;
        self.finish_element(rect.size(), false);
        r
    }

    /// Lays out `f`'s elements left to right, as one element.
    pub fn row<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.scope(true, f)
    }

    /// Lays out `f`'s elements top to bottom, as one element.
    pub fn group<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.scope(false, f)
    }

    /// `group`, moved right by one indentation.
    pub fn indent<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.cursor.x += UiContext::INDENT.x as f32;
        let r = self.group(f);
        self.cursor.x -= UiContext::INDENT.x as f32;
        r
    }

    /// Space left between the cursor and the visible area's bottom right, inside the padding.
    pub fn available_size(&self) -> Vec2 {
        (self.clip_rect.max - Self::scroll_pad() - self.cursor).max(Vec2::ZERO)
    }

    /// An empty element of `size`.
    pub fn space(&mut self, size: Vec2) {
        self.finish_element(size, false);
    }

    /// Runs `f` with input to its widgets disabled (and their text dimmed) if `disabled`.
    pub fn disabled<R>(&mut self, disabled: bool, f: impl FnOnce(&mut Self) -> R) -> R {
        let outer = self.disable_all_input;
        self.disable_all_input |= disabled;
        let r = f(self);
        self.disable_all_input = outer;
        r
    }

    // Interaction

    /// The cursor is over the visible part of `rect`, no element drawn before took the hover,
    /// the window is focused and input isn't disabled.
    pub fn hovered(&self, rect: Rect) -> bool {
        Self::hoverdp(
            rect,
            self.clip_rect,
            self.ctx.input.cursor_pos,
            self.hovered_consumed,
        ) && self.ctx.focused.is_some()
            && !self.disable_all_input
    }

    pub(super) fn hoverdp(
        rect: Rect,
        clip: Rect,
        cursor_pos: Option<Vec2>,
        hovered_smth: bool,
    ) -> bool {
        cursor_pos
            .map(|pos| rect.intersect(clip).contains(pos) && !hovered_smth)
            .unwrap_or(false)
    }

    pub fn clicked(&self, rect: Rect) -> bool {
        self.hovered(rect) && self.ctx.input.primary_pressed
    }

    /// The cursor is over the previous element, also while input is disabled.
    pub fn prev_hovered(&self) -> bool {
        Self::hoverdp(
            self.prev_element,
            self.clip_rect,
            self.ctx.input.cursor_pos,
            false,
        ) && self.ctx.focused.is_some()
    }

    pub fn prev_clicked(&self) -> bool {
        self.prev_hovered() && self.ctx.input.primary_pressed && !self.disable_all_input
    }

    /// A drag that started with a press inside `rect`, until the button is released.
    pub fn drag(&mut self, id: impl Hash, rect: Rect) -> Option<Drag> {
        let id = Draggable::Element(self.id(&id));
        let pressed = self.clicked(rect);
        let pos = self.ctx.input.cursor_pos?;
        let focused = self.ctx.focused.as_mut()?;
        if pressed {
            focused.draging = Some(id);
            focused.drag_start = pos;
        }
        if focused.draging != Some(id) {
            return None;
        }
        let delta = pos - focused.drag_start;
        focused.drag_start = pos;
        Some(Drag { pos, delta })
    }

    /// The vertical wheel movement while the cursor is over `rect`, which then doesn't scroll
    /// the surrounding area.
    pub fn scroll_over(&mut self, rect: Rect) -> f32 {
        if !self.hovered(rect) {
            return 0.0;
        }
        self.scroll_consumed = true;
        self.ctx.scroll_delta.y
    }

    // Painting at absolute positions, clipped to the visible area, without layout.

    pub fn paint_rect(&mut self, rect: Rect, color: Vec4) {
        self.ctx.window.draw_rect(
            rect,
            None,
            color,
            self.ctx.viewport_size,
            self.clip_rect,
            false,
            BindlessHandle::default(),
        );
    }

    pub fn paint_image(&mut self, rect: Rect, image: BindlessHandle, uv: Rect) {
        self.ctx.window.draw_rect(
            rect,
            Some((uv.min, uv.size())),
            Vec4::ONE,
            self.ctx.viewport_size,
            self.clip_rect,
            false,
            image,
        );
    }

    pub fn paint_box(&mut self, rect: Rect, ds: DrawSettings) {
        self.ctx
            .window
            .draw_box(rect, ds, self.ctx.viewport_size, self.clip_rect);
    }

    /// Draws `text` from `pos`, clipped to `clip`, and returns where it ends.
    pub fn paint_text(&mut self, pos: Vec2, text: &str, color: Vec4, clip: Rect) -> Vec2 {
        self.ctx.window.draw_text(
            pos,
            color,
            text,
            self.ctx.viewport_size,
            clip.intersect(self.clip_rect),
            false,
        )
    }

    /// The text colour, dimmed while input is disabled.
    pub fn text_color(&self) -> Vec4 {
        if self.disable_all_input {
            UiContext::TEXT_DIM
        } else {
            UiContext::TEXT
        }
    }

    // Basic elements

    pub fn text(&mut self, label: impl AsRef<str>) {
        self.colored_text(label, self.text_color());
    }

    pub fn colored_text(&mut self, label: impl AsRef<str>, color: Vec4) {
        let size = UiContext::text_size(label.as_ref());
        if self.begin_element(size, false) {
            return;
        }
        self.paint_text(self.cursor, label.as_ref(), color, self.clip_rect);
        self.finish_element(size, false);
    }

    /// Text as tall as a framed widget, so it lines up with the ones beside it in a `row`.
    pub fn label(&mut self, text: impl AsRef<str>) {
        let size = Vec2::new(UiContext::text_len(text.as_ref()), Self::widget_height());
        if self.begin_element(size, false) {
            return;
        }
        let pos = Vec2::new(self.cursor.x, self.child_cursor().y);
        self.paint_text(pos, text.as_ref(), self.text_color(), self.clip_rect);
        self.finish_element(size, false);
    }

    pub fn wrapping_text(&mut self, text: impl AsRef<str>, max_width: f32, color: Vec4) {
        let line = UiContext::ATLAS_CELL_SIZE.y as f32 + UiContext::LINE_SPACING as f32;
        let advance =
            UiContext::ATLAS_CELL_SIZE.x as f32 + UiContext::CHARACTER_ADVANCE_WIDTH as f32;
        let mut size = Vec2::ZERO;
        let mut pen = self.cursor;
        let mut buf = [0; 4];
        for char in text.as_ref().chars() {
            if pen.x + advance > self.cursor.x + max_width || char == '\n' {
                pen.x = self.cursor.x;
                pen.y += line;
            }
            if char == '\n' {
                continue;
            }
            self.paint_text(pen, char.encode_utf8(&mut buf), color, self.clip_rect);
            pen.x += advance;
            size = size.max(pen + line - self.cursor);
        }
        self.finish_element(size, false);
    }

    /// Shows the part `uv` (0 to 1) of `image` at `size`.
    pub fn image(&mut self, image: BindlessHandle, size: Vec2, uv: Rect) {
        if self.begin_element(size, false) {
            return;
        }
        self.paint_image(from_pos_size(self.cursor, size), image, uv);
        self.finish_element(size, false);
    }

    /// A box of `size`.
    pub fn rect(&mut self, size: Vec2, ds: DrawSettings) {
        self.paint_box(from_pos_size(self.cursor, size), ds);
        self.finish_element(size, false);
    }

    // Scroll areas

    fn scroll_pad() -> Vec2 {
        UiContext::WINDOW_PAD.as_vec2() + UiContext::RMB as f32
    }

    /// A framed area of `size` whose content `f` lays out and scrolls. `f` returns the
    /// content's size without padding.
    fn scroll_area<R>(
        &mut self,
        id: NonZeroU64,
        size: Vec2,
        f: impl FnOnce(&mut Self, &Scrollable) -> (R, Vec2),
    ) -> R {
        let mut scrollable = self
            .ctx
            .window
            .scrollables
            .remove(&id.get())
            .unwrap_or_default();
        scrollable.clamp_scroll(size);

        let rect = from_pos_size(self.cursor, size);
        self.paint_box(rect, DrawSettings::default());

        let cursor = self.cursor;
        let clip = self.clip_rect;
        let content_max = self.content_max;
        let hovered = self.hovered_consumed;

        self.clip_rect = clip.intersect(Rect::from_corners(
            rect.min,
            rect.max - scrollable.bar_size(size),
        ));
        self.cursor = scrollable.cursor_pos(rect.min);
        self.content_max = self.cursor;
        let (r, content) = f(self, &scrollable);
        self.hovered_consumed = hovered;
        self.clip_rect = clip;

        scrollable.content_size = content + Self::scroll_pad() * 2.0;
        scrollable.update_and_draw(
            Draggable::Element(id),
            rect,
            self.ctx.window,
            self.ctx.focused,
            self.ctx.viewport_size,
            self.ctx.input.cursor_pos,
            self.ctx.input.primary_pressed,
            clip.intersect(rect),
        );
        if self.hovered(rect) && !self.scroll_consumed {
            scrollable.scroll(self.ctx.scroll_delta, size);
        }
        scrollable.clamp_scroll(size);
        self.ctx.window.scrollables.insert(id.get(), scrollable);

        self.cursor = cursor;
        self.content_max = content_max;
        self.finish_element(size, true);
        r
    }

    /// A framed, scrolling area of `size` around `f`'s elements.
    pub fn container<R>(&mut self, id: impl Hash, size: Vec2, f: impl FnOnce(&mut Self) -> R) -> R {
        let id = self.id(&id);
        self.scroll_area(id, size, |ui, _| {
            let origin = ui.cursor;
            let r = f(ui);
            (r, ui.content_max - origin)
        })
    }

    /// A framed, scrolling list of `len` rows one text line tall, at most `size.y` high, that
    /// only builds the visible rows. `auto_scroll` keeps it at the bottom.
    pub fn list(
        &mut self,
        id: impl Hash,
        size: Vec2,
        len: usize,
        auto_scroll: bool,
        mut row: impl FnMut(&mut Self, usize),
    ) {
        let id = self.id(&id);
        let line = (UiContext::ATLAS_CELL_SIZE.y + UiContext::ELEMENT_GAP.y) as f32;
        let pad = Self::scroll_pad().y;
        let size = Vec2::new(size.x, size.y.min(len as f32 * line + pad * 2.0));
        if auto_scroll && let Some(s) = self.ctx.window.scrollables.get_mut(&id.get()) {
            s.scroll.y = f32::MAX;
        }
        self.scroll_area(id, size, |ui, scrollable| {
            let top = scrollable.scroll.y - pad;
            let first = (top.max(0.0) / line) as usize;
            let end = (((top + size.y) / line).ceil().max(0.0) as usize).min(len);
            let origin = ui.cursor;
            for i in first..end {
                ui.cursor.y = origin.y + i as f32 * line;
                row(ui, i);
            }
            ((), Vec2::new(ui.content_max.x - origin.x, len as f32 * line))
        });
    }

    // Drag and drop

    /// Makes the previous element a drop target for payloads of type `T` that pass `accept`.
    /// While such a payload hovers it, it is outlined. Returns the payload on the frame it is
    /// released over the element.
    pub fn drop_target<T: Any>(&mut self, accept: impl Fn(&T) -> bool) -> Option<T> {
        let rect = self.prev_element;
        if !(self.ctx.hovered
            && self.ctx.input.hovered(rect.intersect(self.clip_rect))
            && self.ctx.dnd.accepts(&accept))
        {
            return None;
        }
        self.paint_box(
            rect.inflate(4.0),
            DrawSettings {
                border: Some(BorderSettings::uniform(UiContext::ACENT, 4)),
                color: Vec4::ZERO,
                rounding: 8,
                on_top: true,
                ..Default::default()
            },
        );
        if self.ctx.input.primary_released {
            self.ctx.dnd.take(&accept)
        } else {
            None
        }
    }

    /// `drop_target` that only accepts a dragged `A`, which is loaded when it is dropped.
    pub fn asset_drop_target<A: Asset>(&mut self) -> Option<Handle<A>> {
        self.drop_target(AssetDrag::is::<A>)
            .and_then(AssetDrag::load)
    }

    /// Makes the previous element draggable. `payload` is called once, when the drag threshold
    /// is crossed, and `drag_icon` is drawn at the cursor above all windows while dragging.
    pub fn drag_source<T: Any + Send + Sync>(
        &mut self,
        id: impl Hash,
        payload: impl FnOnce() -> T,
        drag_icon: impl FnOnce(&mut Self),
    ) {
        let id = self.id(&id);
        let rect = self.prev_element.intersect(self.clip_rect);
        let Some(cursor_pos) = self.ctx.input.cursor_pos else {
            return;
        };
        let Some(focused) = self.ctx.focused.as_mut() else {
            return;
        };
        if self.ctx.input.primary_pressed && rect.contains(cursor_pos) {
            focused.draging = Some(Draggable::Element(id));
            focused.drag_start = cursor_pos;
        }
        if focused.draging == Some(Draggable::Element(id))
            && (focused.drag_start - cursor_pos).length() > UiContext::DRAG_THRESHHOLD
        {
            focused.draging = Some(Draggable::DragAndDrop(id));
            self.ctx.dnd.begin(payload());
        }
        if focused.draging == Some(Draggable::DragAndDrop(id)) {
            self.render_drag_icon(cursor_pos, drag_icon);
        }
    }

    fn render_drag_icon(&mut self, cursor_pos: Vec2, drag_icon: impl FnOnce(&mut Self)) {
        let verticies = std::mem::take(&mut self.ctx.window.verticies);
        let indicies = std::mem::take(&mut self.ctx.window.indicies);

        let cursor = std::mem::replace(&mut self.cursor, cursor_pos);
        let clip_rect = std::mem::replace(
            &mut self.clip_rect,
            Rect::from_corners(Vec2::ZERO, self.ctx.viewport_size),
        );
        let content_max = self.content_max;
        let prev_element = self.prev_element;
        drag_icon(self);
        self.content_max = content_max;
        self.prev_element = prev_element;
        self.cursor = cursor;
        self.clip_rect = clip_rect;

        let icon_verticies = std::mem::replace(&mut self.ctx.window.verticies, verticies);
        let icon_indicies = std::mem::replace(&mut self.ctx.window.indicies, indicies);
        if let Ok(mut overlay) = self.ctx.overlay.lock() {
            let offset = overlay.verticies.len() as u32;
            overlay.verticies.extend(icon_verticies);
            overlay
                .indicies
                .extend(icon_indicies.into_iter().map(|i| i + offset));
        }
    }
}
