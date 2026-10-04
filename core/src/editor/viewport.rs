//! Editor 3D viewport: the dockable tab showing the scene image, and window-to-viewport helpers.
use bevy::{
    ecs::{
        resource::Resource,
        system::{If, Res, ResMut, Single, SystemParam, lifetimeless},
    },
    input::{ButtonInput, mouse::MouseButton},
    math::Rect,
    window::Window,
};
use glam::{UVec2, Vec2, Vec4};
use lava::bindless::BindlessHandle;

use crate::render::headless::HeadlessSize;
use crate::ui::{UiWindows, builder::UiBuilder, window::Drawable};

#[derive(Resource, Debug)]
pub struct ViewPort {
    pub rect: Rect,
    pub focused: bool,
    pub hovered: bool,
    pub image: BindlessHandle,
    pub image_size: Vec2,
}

pub(crate) fn viewport_ui(
    mut ui: UiBuilder,
    mut vp: If<ResMut<ViewPort>>,
    windows: Res<UiWindows>,
    mouse: Res<ButtonInput<MouseButton>>,
) {
    let vp = &mut **vp;
    let window = windows.find_tab("Viewport").map(|(window, _)| window);
    let mut focused = false;
    vp.hovered = false;
    ui.build("Viewport", |ui| {
        let rect = ui.clip_rect;
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return;
        }
        vp.rect = rect;
        vp.hovered = ui
            .ctx
            .input
            .cursor_pos
            .is_some_and(|pos| rect.contains(pos) && windows.window_at(pos) == window);
        focused = ui.ctx.focused.is_some();

        ui.ctx.window.draw_rect(
            rect,
            Some((Vec2::ZERO, rect.size() / vp.image_size.max(Vec2::ONE))),
            Vec4::ONE,
            ui.ctx.viewport_size,
            rect,
            false,
            vp.image,
        );
    });

    vp.focused = focused || (vp.hovered && mouse.pressed(MouseButton::Right));
}

/// Size and cursor of whatever the scene is shown in: the viewport tab, else the headless
/// render target, else the whole window.
#[derive(SystemParam)]
pub struct ViewPortProxy<'s, 'w> {
    window: Option<Single<'w, 's, lifetimeless::Read<Window>>>,
    headless: Option<Res<'w, HeadlessSize>>,
    pub view_port: Option<Res<'w, ViewPort>>,
}

impl<'s, 'w> ViewPortProxy<'s, 'w> {
    pub fn width(&self) -> u32 {
        self.size().x
    }
    pub fn height(&self) -> u32 {
        self.size().y
    }
    pub fn size(&self) -> UVec2 {
        if let Some(vp) = &self.view_port {
            vp.rect.size().as_uvec2()
        } else if let Some(headless) = &self.headless {
            headless.0
        } else {
            self.window
                .as_ref()
                .map(|window| window.physical_size())
                .unwrap_or(UVec2::ONE)
        }
    }
    pub fn cursor_position(&self) -> Option<Vec2> {
        let cp = self.window.as_ref()?.cursor_position();
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
