use bevy::math::Rect;
use glam::Vec2;

use crate::ui::{
    Draggable, FocusedState, UiContext, from_pos_size,
    window::{DrawSettings, Drawable},
};

#[derive(Copy, Clone, Default, Debug)]
pub struct Scrollable {
    pub content_size: Vec2,
    pub scroll: Vec2,
}

impl Scrollable {
    pub fn cursor_pos(&self, content_pos: Vec2) -> Vec2 {
        (content_pos + UiContext::RMB as f32 + UiContext::WINDOW_PAD.as_vec2() - self.scroll)
            .round()
    }

    pub fn bar_size(&self, size: Vec2) -> Vec2 {
        UiContext::BAR_THICKNESS
            * Vec2::new(
                (self.content_size.y > size.y) as u32 as f32,
                (self.content_size.x > size.x) as u32 as f32,
            )
    }

    pub fn scroll(&mut self, delta: Vec2, size: Vec2) {
        let scrollbar_y = self.content_size.y > size.y;
        let scrollbar_x = self.content_size.x > size.x;

        self.scroll -= Vec2::new(
            scrollbar_x as u32 as f32 * delta.x,
            scrollbar_y as u32 as f32 * delta.y,
        );
    }

    pub fn clamp_scroll(&mut self, size: Vec2) {
        self.scroll = self
            .scroll
            .clamp(Vec2::ZERO, (self.content_size - size).max(Vec2::ZERO));
    }

    fn update_and_draw_bar(
        &mut self,
        draging: Draggable,
        area: Rect,
        window: &mut impl Drawable,
        focused: &mut Option<&mut FocusedState>,
        direction: bool,
        viewport_size: Vec2,
        cursor_pos: Option<Vec2>,
        left_mouse_pressed: bool,
        clip_rect: Rect,
    ) {
        let size = area.size();
        let pos = area.min;
        let b = UiContext::BORDER as f32;
        let track_pos = if direction {
            Vec2::new(pos.x + size.x - UiContext::BAR_THICKNESS - b, pos.y + b).round()
        } else {
            Vec2::new(pos.x + b, pos.y + size.y - UiContext::BAR_THICKNESS - b).round()
        };

        let track_size = if direction {
            Vec2::new(UiContext::BAR_THICKNESS, size.y - b * 2.0).round()
        } else {
            Vec2::new(size.x - b * 2.0, UiContext::BAR_THICKNESS).round()
        };

        window.draw_rect(
            Rect::from_corners(track_pos, track_pos + track_size),
            None,
            UiContext::S0,
            viewport_size,
            clip_rect,
            false,
        );

        let scroll_max = (self.content_size - size).max(Vec2::ONE);

        let ratio = (size / self.content_size).clamp(Vec2::ZERO, Vec2::ONE);
        let thumb_width = (track_size * ratio)
            .max(Vec2::splat(UiContext::MIN_THUMB))
            .round();
        let thumb_t = (self.scroll / scroll_max).clamp(Vec2::ZERO, Vec2::ONE);
        let thumb = (track_pos + thumb_t * (track_size - thumb_width)).round();
        let thumb_pos = if direction {
            Vec2::new(track_pos.x, thumb.y)
        } else {
            Vec2::new(thumb.x, track_pos.y)
        };
        let thumb_size = if direction {
            Vec2::new(UiContext::BAR_THICKNESS, thumb_width.y)
        } else {
            Vec2::new(thumb_width.x, UiContext::BAR_THICKNESS)
        };

        let dragging = focused
            .as_ref()
            .is_some_and(|d| d.draging.is_some_and(|d| d == draging));

        let hovered = cursor_pos
            .map(|p| Rect::from_corners(thumb_pos, thumb_pos + thumb_size).contains(p))
            .unwrap_or(false);

        if left_mouse_pressed
            && hovered
            && let Some(f) = focused
        {
            let grab_offset = cursor_pos.map(|p| p - thumb).unwrap_or(Vec2::ZERO);
            f.draging = Some(draging);
            f.drag_start =
                grab_offset * Vec2::new(!direction as u32 as f32, direction as u32 as f32);
        }

        if let Some(p) = cursor_pos
            && dragging
        {
            let grab_offset = focused.as_ref().map(|f| f.drag_start).unwrap_or(Vec2::ZERO);
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

        let thumb_color = if dragging || hovered {
            UiContext::GRAB_HOT
        } else {
            UiContext::GRAB
        };
        let ds = DrawSettings {
            color: thumb_color,
            ..Default::default()
        };
        window.draw_box(
            from_pos_size(thumb_pos, thumb_size),
            ds,
            viewport_size,
            clip_rect,
        );
    }

    pub fn update_and_draw(
        &mut self,
        dragging: Draggable,
        size: Rect,
        window: &mut impl Drawable,
        focused: &mut Option<&mut FocusedState>,
        viewport_size: Vec2,
        cursor_pos: Option<Vec2>,
        left_mouse_pressed: bool,
        clip_rect: Rect,
    ) {
        if self.content_size.y > size.size().y {
            self.update_and_draw_bar(
                dragging,
                size,
                window,
                focused,
                true,
                viewport_size,
                cursor_pos,
                left_mouse_pressed,
                clip_rect,
            );
        }
        if self.content_size.x > size.size().x {
            self.update_and_draw_bar(
                dragging,
                size,
                window,
                focused,
                false,
                viewport_size,
                cursor_pos,
                left_mouse_pressed,
                clip_rect,
            );
        }
    }
}
