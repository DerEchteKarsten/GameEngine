use bevy::math::Rect;
use glam::Vec2;
use std::num::NonZeroU64;

use crate::ui::{
    new_ui::{UiContext, from_pos_size},
    window::{DrawSettings, UiWindow},
};

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
        self.scroll = self
            .scroll
            .clamp(Vec2::ZERO, (self.content_size - size).max(Vec2::ZERO));
    }

    fn draw_bar(
        &mut self,
        id: NonZeroU64,
        area: Rect,
        window: &mut UiWindow,
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

        window.rect(
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

        let id_scroll = id.saturating_add(1).saturating_add(!direction as u64);

        let dragging = window
            .focused
            .as_ref()
            .map(|f| f.draging == Some(id_scroll))
            .unwrap_or(false);

        let hovered = cursor_pos
            .map(|p| Rect::from_corners(thumb_pos, thumb_pos + thumb_size).contains(p))
            .unwrap_or(false);

        if left_mouse_pressed && hovered {
            if let Some(f) = &mut window.focused {
                let grab_offset = cursor_pos.map(|p| p - thumb).unwrap_or(Vec2::ZERO);
                f.draging = Some(id_scroll);
                f.darg_start =
                    grab_offset * Vec2::new(!direction as u32 as f32, direction as u32 as f32);
            }
        }

        if let Some(p) = cursor_pos
            && dragging
        {
            let grab_offset = window
                .focused
                .as_ref()
                .map(|f| f.darg_start)
                .unwrap_or(Vec2::ZERO);
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

    pub fn draw(
        &mut self,
        id: NonZeroU64,
        area: Rect,
        window: &mut UiWindow,
        viewport_size: Vec2,
        cursor_pos: Option<Vec2>,
        left_mouse_pressed: bool,
        clip_rect: Rect,
    ) {
        if self.content_size.y > area.size().y {
            self.draw_bar(
                id,
                area,
                window,
                true,
                viewport_size,
                cursor_pos,
                left_mouse_pressed,
                clip_rect,
            );
        }
        if self.content_size.x > area.size().x {
            self.draw_bar(
                id,
                area,
                window,
                false,
                viewport_size,
                cursor_pos,
                left_mouse_pressed,
                clip_rect,
            );
        }
    }
}
