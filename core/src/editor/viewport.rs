use bevy::{
    ecs::{
        resource::Resource,
        system::{If, Res, ResMut, Single, SystemParam, lifetimeless},
    },
    math::{Rect, VectorSpace},
    window::Window,
};
use glam::{UVec2, Vec2};

use crate::ui::{builder::UiBuilder, dock::DockingNode};

#[derive(Resource, Debug, Copy, Clone)]
pub struct ViewPort {
    pub rect: Rect,
    pub visible_rect: Rect,
    pub focused: bool,
}

pub(crate) fn update_view_port(
    mut vp: If<ResMut<ViewPort>>,
    dock: Res<DockingNode>,
    window: Single<&Window>,
) {
    let size = window.physical_size();

    let dock_rect = dock
        .dock_info(u32::MAX, Rect::from_corners(Vec2::ZERO, size.as_vec2()))
        .unwrap();

    vp.rect = dock_rect;
    vp.visible_rect = vp
        .rect
        .intersect(Rect::from_corners(Vec2::ZERO, size.as_vec2()))
}

#[derive(SystemParam)]
pub struct ViewPortProxy<'s, 'w> {
    window: Single<'w, 's, lifetimeless::Read<Window>>,
    pub view_port: Option<Res<'w, ViewPort>>,
}

impl<'s, 'w> ViewPortProxy<'s, 'w> {
    pub fn width(&self) -> u32 {
        self.view_port
            .as_ref()
            .map(|vp| vp.rect.width() as u32)
            .unwrap_or(self.window.physical_width())
    }
    pub fn height(&self) -> u32 {
        self.view_port
            .as_ref()
            .map(|vp| vp.rect.height() as u32)
            .unwrap_or(self.window.physical_height())
    }
    pub fn size(&self) -> UVec2 {
        self.view_port
            .as_ref()
            .map(|vp| vp.rect.size().as_uvec2())
            .unwrap_or(self.window.physical_size())
    }
    pub fn cursor_position(&self) -> Option<Vec2> {
        let cp = self.window.cursor_position();
        cp.and_then(|pos| self.to_viewport_pos(pos))
    }

    pub fn to_viewport_pos(&self, pos: Vec2) -> Option<Vec2> {
        if let Some(vp) = &self.view_port {
            let position = pos - vp.rect.min;
            if position.cmpgt(vp.rect.size()).any() || position.cmple(Vec2::ZERO).any() {
                None
            } else {
                Some(position)
            }
        } else {
            Some(pos)
        }
    }

    pub fn focused(&self) -> bool {
        self.view_port.as_ref().map(|vp| vp.focused).unwrap_or(true)
    }
}
