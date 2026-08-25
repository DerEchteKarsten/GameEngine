use crate::ui::{
    Draggable,
    dock::DockingNode,
    window::{Drawable, Tab, TabState, UiWindow},
};
use bevy::{
    ecs::system::{Res, ResMut, Single},
    input::{
        ButtonInput,
        mouse::{AccumulatedMouseScroll, MouseButton},
        touch::Touches,
    },
    log,
    math::{Rect, VectorSpace},
    window::Window,
};
use glam::{Vec2, Vec4};
use itertools::Itertools;
use smallvec::SmallVec;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU64,
    sync::{Mutex, atomic::AtomicU32},
};

use crate::ui::{
    FocusedState, MultiInput, UiContext, UiWindows, from_pos_size,
    scrollable::Scrollable,
    window::{BorderSettings, DrawSettings, TextDirection},
};

struct FrameInfo {
    input: MultiInput,
    viewport_size: Vec2,
    full_screen_rect: Rect,
}

#[derive(Clone, Copy, Default, Debug)]
pub struct ResizeEdges {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl ResizeEdges {
    fn hoverd(&mut self, rect: Rect, input: &MultiInput, threshold: f32) {
        let (min, max, t) = (rect.min, rect.max, threshold);
        self.top = input.hovered(Rect::from_corners(
            Vec2::new(min.x - t, min.y - t),
            Vec2::new(max.x + t, min.y),
        ));
        self.bottom = input.hovered(Rect::from_corners(
            Vec2::new(min.x + t, max.y),
            Vec2::new(max.x + t, max.y + t),
        ));
        self.left = input.hovered(Rect::from_corners(
            Vec2::new(min.x - t, min.y - t),
            Vec2::new(min.x, max.y + t),
        ));
        self.right = input.hovered(Rect::from_corners(
            Vec2::new(max.x, min.y - t),
            Vec2::new(max.x + t, max.y + t),
        ));
    }
    fn any(&self) -> bool {
        self.top || self.bottom || self.left || self.right
    }
}

pub fn draw_windows(
    mut windows: ResMut<UiWindows>,
    desktop_window: Single<&Window>,
    buttons: Res<ButtonInput<MouseButton>>,
    touch: Res<Touches>,
    dock: Res<DockingNode>,
    scroll_delta: Res<AccumulatedMouseScroll>,
) {
    let viewport_size = desktop_window.physical_size().as_vec2();
    let frame = FrameInfo {
        input: MultiInput::new(&desktop_window, &buttons, &touch),
        viewport_size,
        full_screen_rect: Rect::from_corners(Vec2::ZERO, viewport_size),
    };
    for (i, window) in windows.by_layer_mut() {
        let mut cursor = window.rect.min;
        let header_rect = window.header_rect();
        draw_window(window, &frame, dock.contains(i as u32));

        let tabs = std::mem::take(&mut window.tabs);
        for (j, tab) in tabs.iter().enumerate() {
            let tab_rect = Rect::from_corners(
                cursor - window.tab_scroll.scroll,
                cursor - window.tab_scroll.scroll
                    + Vec2::new(
                        UiContext::text_len(&tab.label),
                        UiContext::WINDOW_HEADER_HEIGHT,
                    )
                    + UiContext::TAB_PAD.as_vec2() * 2.0,
            );
            let active = j == window.active_tab as usize;

            let hoverd = frame
                .input
                .cursor_pos
                .is_some_and(|cp| tab_rect.contains(cp));

            let ds = DrawSettings {
                border: Some(BorderSettings::uniform(UiContext::S1, 2)),
                color: if hoverd {
                    UiContext::S2
                } else if active {
                    UiContext::S1
                } else {
                    UiContext::BG
                },
                rounding: UiContext::ROUNDING,
                on_top: false,
                round_bottomleft: false,
                round_topright: false,
                ..Default::default()
            };
            window.draw_box(tab_rect, ds, frame.viewport_size, header_rect);

            window.draw_text(
                tab_rect.min + UiContext::TAB_PAD.as_vec2(),
                UiContext::TEXT,
                &tab.label,
                frame.viewport_size,
                header_rect,
                false,
            );
            cursor.x += tab_rect.width() + UiContext::TAB_GAP.x as f32;
        }
        window.tabs = tabs;
        let mut focused = window.focused.take();
        let mut scroll = window.tab_scroll;
        scroll.content_size = Rect::from_corners(
            window.rect.min,
            cursor + Vec2::new(0.0, UiContext::WINDOW_HEADER_HEIGHT),
        )
        .size();
        scroll.update_and_draw(
            Draggable::TabScrollHandle,
            header_rect,
            window,
            &mut focused.as_mut(),
            viewport_size,
            frame.input.cursor_pos,
            frame.input.primary_pressed,
            header_rect,
        );
        if focused.is_some() {
            scroll.scroll(scroll_delta.delta, header_rect.size());
        }
        window.tab_scroll = scroll;
        window.focused = focused;
    }
}

pub fn update_windows(
    mut windows: ResMut<UiWindows>,
    desktop_window: Single<&Window>,
    buttons: Res<ButtonInput<MouseButton>>,
    touch: Res<Touches>,
    mut ctx: ResMut<UiContext>,
    mut dock: ResMut<DockingNode>,
) {
    let new_windows = if let Ok(mut lock) = windows.add_windows.lock() {
        lock.drain(..).collect::<SmallVec<[String; 4]>>()
    } else {
        SmallVec::new()
    };
    for (i, label) in new_windows.into_iter().enumerate() {
        let pos = Vec2::new(500.0 * i as f32, 0.0);
        windows.append(UiWindow::new(
            vec![Tab {
                label,
                state: Mutex::new(TabState::default()),
            }],
            Rect::from_corners(pos, pos + Vec2::new(500.0, 500.0)),
            0,
        ));
    }

    let ctx: &mut UiContext = &mut ctx;
    let dock: &mut DockingNode = &mut dock;

    let viewport_size = desktop_window.physical_size().as_vec2();
    let frame = FrameInfo {
        input: MultiInput::new(&desktop_window, &buttons, &touch),
        viewport_size,
        full_screen_rect: Rect::from_corners(Vec2::ZERO, viewport_size),
    };

    let mut found_new_focused = false;
    let mut hovering_any_tab = false;
    let mut dragging = false;
    let mut resizing = false;
    let mut spawn_window = None;
    let mut drag_released = None;
    let mut preview_rect = None;

    for (i, window) in windows.by_layer_mut().rev() {
        window.indicies.clear();
        window.verticies.clear();

        let info = dock.dock_info(i as u32, frame.full_screen_rect);
        let docked = info.is_some();

        if let Some(info) = info {
            window.rect = info;
        }

        let hoverd = frame
            .input
            .hovered(window.rect.inflate(UiContext::RESIZE_THRESHOLD));

        if frame.input.primary_pressed {
            if !found_new_focused && hoverd {
                found_new_focused = true;
                if window.focused.is_none() {
                    window.focused = Some(FocusedState::default());
                }
            } else {
                window.focused = None;
            }
        }

        if let Some(focused) = window.focused.as_mut()
            && frame.input.primary_pressed
            && !docked
        {
            focused
                .edges
                .hoverd(window.rect, &frame.input, UiContext::RESIZE_THRESHOLD);
            resizing = focused.edges.any();
        }

        let mut hovering_tab = false;
        let mut detatch_tab = None;
        let mut cursor = window.rect.min;
        for (j, tab) in window.tabs.iter().enumerate() {
            if let Ok(mut tab_state) = tab.state.lock() {
                tab_state.verticies.clear();
                tab_state.indicies.clear();
                tab_state.top_verticies.clear();
                tab_state.top_indicies.clear();
            }
            let tab_rect = Rect::from_corners(
                cursor - window.tab_scroll.scroll,
                cursor - window.tab_scroll.scroll
                    + Vec2::new(
                        UiContext::text_len(&tab.label),
                        UiContext::WINDOW_HEADER_HEIGHT,
                    )
                    + UiContext::TAB_PAD.as_vec2(),
            );
            let hovering = frame.input.hovered(tab_rect);

            if hovering {
                hovering_any_tab = true;
                hovering_tab = true;
            }
            if let Some(focused) = window.focused.as_mut() {
                if hovering && frame.input.primary_pressed {
                    window.active_tab = j as u32;
                    if let Some(cursor_pos) = frame.input.cursor_pos
                        && !focused.edges.any()
                    {
                        focused.draging = if window.tabs.len() > 1 {
                            Some(Draggable::ActiveTab)
                        } else {
                            Some(Draggable::Window)
                        };
                        focused.drag_start = cursor_pos;
                        focused.drag_press_pos = window.rect.min - cursor_pos;
                    }
                }
                if window.active_tab == j as u32 {
                    if let Some(cp) = frame.input.cursor_pos
                        && focused.draging == Some(Draggable::ActiveTab)
                        && (focused.drag_start - cp).length() > UiContext::DRAG_THRESHHOLD
                    {
                        detatch_tab = Some(j as u32);
                    }
                }
            }
            cursor.x += tab_rect.width() + UiContext::TAB_GAP.x as f32;
        }

        if let Some(tab) = detatch_tab
            && window.tabs.len() > 1
        {
            let tab = window.tabs.remove(tab as usize);
            if window.active_tab != 0 {
                window.active_tab -= 1;
            }
            if let Some(focused) = &mut window.focused {
                focused.draging = Some(Draggable::Window);
            }
            spawn_window = Some(UiWindow {
                tab_scroll: Scrollable::default(),
                active_tab: 0,
                tabs: vec![tab],
                layer: window.layer,
                rect: window.rect,
                focused: window.focused.take(),
                verticies: Vec::new(),
                indicies: Vec::new(),
            })
        }

        let header_rect = window.header_rect();
        if let Some(focused) = &mut window.focused {
            if frame.input.primary_pressed {
                if frame.input.hovered(header_rect) && !hovering_tab && !focused.edges.any() {
                    focused.draging = Some(Draggable::Window);
                    let cp = frame.input.cursor_pos.unwrap();
                    focused.drag_start = cp;
                    focused.drag_press_pos = window.rect.min - cp;
                }
            }

            if frame.input.primary_released {
                if focused.draging == Some(Draggable::Window) && !docked {
                    drag_released = Some(i);
                }

                focused.edges = ResizeEdges::default();
                focused.draging = None;
            }

            if let Some(cursor_pos) = frame.input.cursor_pos
                && focused.draging == Some(Draggable::Window)
            {
                if docked {
                    if (focused.drag_start - cursor_pos).length() > UiContext::DRAG_THRESHHOLD {
                        dock.undock(i as u32);
                    }
                } else {
                    let size = window.rect.size();
                    window.rect.min = cursor_pos + focused.drag_press_pos;
                    window.rect.max = window.rect.min + size;

                    preview_rect = dock
                        .preview_dock(cursor_pos, frame.full_screen_rect)
                        .map(|r| (i, r));
                }
            }

            if let Some(cursor_pos) = frame.input.cursor_pos {
                let min_size = UiContext::WINDOW_HEADER_HEIGHT + 10.0;
                if focused.edges.top {
                    window.rect.min.y = cursor_pos.y.min(window.rect.max.y - min_size).round();
                }
                if focused.edges.bottom {
                    window.rect.max.y = cursor_pos.y.max(window.rect.min.y + min_size).round();
                }
                if focused.edges.left {
                    window.rect.min.x = cursor_pos.x.min(window.rect.max.x - 10.0).round();
                }
                if focused.edges.right {
                    window.rect.max.x = cursor_pos.x.max(window.rect.min.x + 10.0).round();
                }
            }

            dragging = focused.draging.is_some();
        }
    }

    if let Some((i, preview)) = preview_rect {
        windows.windows[i].as_mut().unwrap().draw_box(
            preview,
            DrawSettings {
                color: UiContext::ACENT_DIM,
                on_top: true,
                border: Some(BorderSettings::uniform(UiContext::ACENT, UiContext::BORDER)),
                ..Default::default()
            },
            frame.viewport_size,
            frame.full_screen_rect,
        );
    }

    if let Some(w) = drag_released
        && let Some(cp) = frame.input.cursor_pos
    {
        let dock_window = dock.dock(w as u32, cp, frame.full_screen_rect);
        let merge_window = dock_window.or_else(|| {
            windows
                .by_layer()
                .rev()
                .find(|(i, win)| *i != w && win.header_rect().contains(cp))
                .map(|(i, _)| i as u32)
        });
        if let Some(merge_window) = merge_window {
            let mut old = windows.remove(w);
            let focused = old.focused.take();
            let window = windows.windows[merge_window as usize].as_mut().unwrap();
            if let Some(focused) = focused {
                window.focused = Some(focused);
            }
            let tabs = window.tabs.len();
            window.tabs.append(&mut old.tabs);
            window.active_tab = tabs as u32;
        }
    };

    if let Some(spawn_window) = spawn_window {
        windows.append(spawn_window);
    }

    handle_dock_resizing(
        ctx,
        dock,
        &frame,
        !hovering_any_tab && !dragging && !resizing,
    );
    reorder_layers(&mut windows, dock);
}

fn draw_window(window: &mut UiWindow, frame: &FrameInfo, docked: bool) {
    let is_focused = window.focused.is_some();
    let edges = window.focused.as_ref().map(|f| f.edges).unwrap_or_default();

    let border_color = |active: bool| {
        if active {
            UiContext::ACENT
        } else {
            UiContext::S2
        }
    };

    let mut window_ds = DrawSettings {
        on_top: false,
        color: UiContext::BG,
        rounding: UiContext::WINDOW_ROUNDING,
        round_topleft: false,
        round_topright: false,
        round_bottomleft: false,
        round_bottomright: false,
        border: Some(BorderSettings {
            color_top: border_color(false),
            color_bottom: border_color(edges.bottom),
            color_left: border_color(edges.left),
            color_right: border_color(edges.right),
            size: UiContext::BORDER,
        }),
    };

    window.draw_box(
        window.content_rect(),
        window_ds,
        frame.viewport_size,
        frame.full_screen_rect,
    );

    let b = window_ds.border.as_mut().unwrap();
    b.color_top = border_color(edges.top);
    b.color_bottom = border_color(false);
    draw_header_bar(window, is_focused, window_ds, frame, docked);
}

fn draw_header_bar(
    window: &mut UiWindow,
    is_focused: bool,
    mut window_ds: DrawSettings,
    frame: &FrameInfo,
    docked: bool,
) {
    window_ds.round_topleft = !docked;
    window_ds.round_topright = !docked;
    window_ds.round_bottomleft = false;
    window_ds.round_bottomright = false;
    window_ds.color = if is_focused {
        UiContext::BG
    } else {
        UiContext::BG_DARK
    };
    window_ds.border.as_mut().unwrap().color_bottom = UiContext::S1;
    window.draw_box(
        window.header_rect(),
        window_ds,
        frame.viewport_size,
        frame.full_screen_rect,
    );
}

fn handle_dock_resizing(
    ctx: &mut UiContext,
    dock: &mut DockingNode,
    frame: &FrameInfo,
    click_valid: bool,
) {
    let input = &frame.input;

    if click_valid
        && let Some(cursor_pos) = input.cursor_pos
        && input.primary_pressed
    {
        let (path, depth, drag_start, _axis) =
            dock.find_resize(cursor_pos, frame.full_screen_rect, 0, 0);
        if path != u64::MAX {
            ctx.resize_path = path;
            ctx.resize_depth = depth;
            ctx.drag_start = drag_start;
        }
    }

    let is_resizing = ctx.resize_path != u64::MAX;

    if is_resizing
        && input.primary_pressing
        && let Some(cursor_pos) = input.cursor_pos
    {
        let delta = cursor_pos - ctx.drag_start;
        dock.resize(
            ctx.resize_path,
            ctx.resize_depth,
            0,
            delta,
            frame.full_screen_rect,
        );
    }

    if input.primary_released {
        ctx.resize_path = u64::MAX;
        ctx.resize_depth = 0;
    }
}

fn reorder_layers(windows: &mut UiWindows, dock: &mut DockingNode) {
    windows
        .windows
        .iter_mut()
        .enumerate()
        .filter_map(|w| w.1.as_mut().map(|o| (w.0, o)))
        .sorted_by_key(|w| {
            let tier: u8 = if dock.contains(w.0 as u32) {
                0
            } else if w.1.focused.is_some() {
                2
            } else {
                1
            };
            (tier, w.1.layer)
        })
        .enumerate()
        .for_each(|(layer, w)| {
            w.1.layer = layer as u32;
        });
}
