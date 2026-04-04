use bevy::{
    ecs::{
        resource::Resource,
        system::{Res, ResMut, Single, SystemParam, lifetimeless},
    },
    window::Window,
};
use glam::{IVec2, UVec2, Vec2};
use imgui::{StyleColor, WindowFlags};

use crate::ui::{UiBuilder, UiContext};

#[derive(Resource, Debug, Copy, Clone)]
pub struct ViewPort {
    pub view_pos: IVec2,
    pub scissor_pos: UVec2,
    pub scissor_size: UVec2,
    pub view_size: UVec2,
    pub focused: bool,
}

pub(crate) fn update_view_port(
    mut vp: ResMut<ViewPort>,
    window: Single<&Window>,
    mut ui: ResMut<UiBuilder>,
    mut ctx: ResMut<UiContext>,
) {
    let Some(ui) = ui.ui() else {
        vp.view_pos = IVec2::ZERO;
        vp.scissor_size = window.physical_size();
        vp.view_size = window.physical_size();
        vp.focused = true;
        return;
    };
    let padding_token = ui.push_style_var(imgui::StyleVar::WindowPadding([0.0, 0.0]));
    let border_token = ui.push_style_var(imgui::StyleVar::WindowBorderSize(0.0));

    if let Some(_window) = ui
        .window("Viewport##viewport")
        .flags(WindowFlags::NO_BACKGROUND)
        .draw_background(false)
        .bg_alpha(0.0)
        .collapsed(false, imgui::Condition::Always)
        .size(Vec2::new(500.0, 500.0), imgui::Condition::FirstUseEver)
        .begin()
    {
        vp.focused = ui.is_window_focused();
        let window_pos = Vec2::from_array(ui.window_pos());
        let content_min = Vec2::from_array(ui.window_content_region_min());
        let content_max = Vec2::from_array(ui.window_content_region_max());

        let abs_min = window_pos + content_min;
        let abs_max = window_pos + content_max;

        vp.view_pos = abs_min.as_ivec2();
        vp.view_size = (content_max - content_min).as_uvec2();

        let screen_size = window.size();
        let scissor_min = abs_min.max(Vec2::ZERO);
        let scissor_max = abs_max.min(screen_size);
        let scissor_size = (scissor_max - scissor_min).max(Vec2::ZERO);

        vp.scissor_pos = scissor_min.as_uvec2();
        vp.scissor_size = scissor_size.as_uvec2();
    } else {
        vp.view_pos = IVec2::ZERO;
        vp.scissor_pos = UVec2::ZERO;
        vp.scissor_size = window.physical_size();
        vp.view_size = window.physical_size();
        vp.focused = false;
    }
    padding_token.pop();
    border_token.pop();
    ctx.ctx.style_mut().colors[StyleColor::WindowBg as usize] = [0.155, 0.155, 0.155, 1.0];
}

#[derive(SystemParam)]
pub struct ViewPortProxy<'s, 'w> {
    window: Single<'w, 's, lifetimeless::Read<Window>>,
    view_port: Option<Res<'w, ViewPort>>,
}

impl<'s, 'w> ViewPortProxy<'s, 'w> {
    pub fn width(&self) -> u32 {
        self.view_port
            .as_ref()
            .map(|vp| vp.view_size.x)
            .unwrap_or(self.window.physical_width())
    }
    pub fn height(&self) -> u32 {
        self.view_port
            .as_ref()
            .map(|vp| vp.view_size.y)
            .unwrap_or(self.window.physical_height())
    }
    pub fn size(&self) -> UVec2 {
        self.view_port
            .as_ref()
            .map(|vp| vp.view_size)
            .unwrap_or(self.window.physical_size())
    }
    pub fn cursor_position(&self) -> Option<Vec2> {
        let cp = self.window.cursor_position()?;
        if let Some(vp) = &self.view_port {
            let position = cp - vp.view_pos.as_vec2();
            if position.cmpgt(vp.view_size.as_vec2()).any() || position.cmple(Vec2::ZERO).any() {
                None
            } else {
                Some(position)
            }
        } else {
            Some(cp)
        }
    }
    pub fn focused(&self) -> bool {
        self.view_port.as_ref().map(|vp| vp.focused).unwrap_or(true)
    }
}
