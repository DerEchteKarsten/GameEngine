use crate::ui::dock::{DockingNode, Siblings};
use bevy::{
    ecs::system::{Res, ResMut, Single},
    input::{ButtonInput, mouse::MouseButton, touch::Touches},
    math::{Rect, VectorSpace},
    window::Window,
};
use glam::{Vec2, Vec4};
use smallvec::SmallVec;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    num::NonZeroU64,
    sync::Mutex,
};
use tracing_log::log;

use crate::ui::{
    new_ui::{FocusedState, MultiInput, UiContext, UiWindows, from_pos_size},
    scrollable::Scrollable,
    window::{BorderSettings, DrawSettings, TextDirection, UiWindow},
};

struct FrameInfo {
    input: MultiInput,
    viewport_size: Vec2,
    full_screen_rect: Rect,
}

#[derive(Clone, Copy, Default)]
struct ResizeEdges {
    top: bool,
    bottom: bool,
    left: bool,
    right: bool,
}

impl ResizeEdges {
    fn hovered(rect: Rect, cursor_pos: Vec2, threshold: f32) -> Self {
        let (min, max, t) = (rect.min, rect.max, threshold);
        Self {
            top: Rect::from_corners(
                Vec2::new(min.x - t, min.y - t),
                Vec2::new(max.x + t, min.y + t),
            )
            .contains(cursor_pos),
            bottom: Rect::from_corners(
                Vec2::new(min.x - t, max.y - t),
                Vec2::new(max.x + t, max.y + t),
            )
            .contains(cursor_pos),
            left: Rect::from_corners(
                Vec2::new(min.x - t, min.y - t),
                Vec2::new(min.x + t, max.y + t),
            )
            .contains(cursor_pos),
            right: Rect::from_corners(
                Vec2::new(max.x - t, min.y - t),
                Vec2::new(max.x + t, max.y + t),
            )
            .contains(cursor_pos),
        }
    }

    fn store_in(self, focused: &mut FocusedState) {
        focused.resize_top = self.top;
        focused.resize_bottom = self.bottom;
        focused.resize_left = self.left;
        focused.resize_right = self.right;
    }
}

impl From<&FocusedState> for ResizeEdges {
    fn from(focused: &FocusedState) -> Self {
        Self {
            top: focused.resize_top,
            bottom: focused.resize_bottom,
            left: focused.resize_left,
            right: focused.resize_right,
        }
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
    for label in new_windows.into_iter() {
        let id = windows.windows.len() as u32;
        windows.windows.push(Mutex::new(UiWindow::new(
            label.clone(),
            Rect::from_corners(Vec2::ZERO, Vec2::splat(100.0)),
            true,
            false,
        )));
        windows.window_labels.insert(label, id);
    }

    let ctx: &mut UiContext = &mut ctx;
    let dock: &mut DockingNode = &mut dock;

    let viewport_size = desktop_window.physical_size().as_vec2();
    let frame = FrameInfo {
        input: MultiInput::new(&desktop_window, &buttons, &touch),
        viewport_size,
        full_screen_rect: Rect::from_corners(Vec2::ZERO, viewport_size),
    };

    let mut newly_focused: Option<usize> = None;
    let mut drag_release: Option<usize> = None;
    let mut header_hoverd: bool = false;

    for (i, window_cell) in windows.by_layer().rev() {
        let Ok(mut window) = window_cell.lock() else {
            continue;
        };

        let siblings = get_dock_info(&mut window, i as u32, dock, frame.full_screen_rect);

        let sibling_rects = siblings
            .as_ref()
            .map(|siblings| sibling_rects(&mut window, i, siblings, &windows.windows));

        let drag_rect = sibling_rects
            .as_ref()
            .map(|a| {
                Rect::from_corners(
                    a.0.min + window.full_rect().min,
                    a.0.max + window.full_rect().min,
                )
            })
            .unwrap_or(window.header_text_rect());

        if frame
            .input
            .cursor_pos
            .is_some_and(|cp| drag_rect.contains(cp))
        {
            header_hoverd = true;
        }

        window_focused(&mut window, i, drag_rect, dock, &frame, &mut newly_focused);

        let (preview_rect, drag_released) =
            handle_focused_input(&mut window, i, drag_rect, dock, &frame);

        if drag_released {
            drag_release = Some(i);
        }

        draw_window(
            &mut window,
            i,
            siblings,
            sibling_rects.map(|r| r.1),
            &windows.windows,
            &frame,
        );

        if let Some(preview) = preview_rect {
            draw_dock_preview(&mut window, preview, &frame);
        }
    }

    handle_dock_resizing(ctx, dock, &frame, header_hoverd);

    if let Some(window_idx) = drag_release
        && let Some(cursor_pos) = frame.input.cursor_pos
    {
        let docked = dock.dock(
            window_idx as u32,
            cursor_pos,
            frame.full_screen_rect,
            UiContext::WINDOW_HEADER_HEIGHT,
        );
        if let Ok(mut window) = windows.windows[window_idx].lock() {
            window.docked = docked;
        }
    }

    reorder_layers(&windows);
}

fn sibling_rects(
    window: &mut UiWindow,
    index: usize,
    siblings: &Siblings,
    all_windows: &[Mutex<UiWindow>],
) -> (Rect, Vec<Rect>) {
    let mut text_cursor = Vec2::new(UiContext::ELEMENT_GAP.x as f32, 0.0);
    let mut rects = Vec::with_capacity(siblings.members.len());
    let mut own_rect = None;

    for &sibling_id in siblings.members.iter() {
        let label_width = if index != sibling_id as usize {
            let Ok(sibling) = all_windows[sibling_id as usize].lock() else {
                continue;
            };
            UiContext::text_len(&sibling.label)
        } else {
            UiContext::text_len(&window.label)
        };

        let label_rect = from_pos_size(
            text_cursor + UiContext::WINDOW_PAD.as_vec2() + UiContext::ELEMENT_GAP.as_vec2(),
            Vec2::new(
                label_width + UiContext::WINDOW_PAD.x as f32 + UiContext::ELEMENT_GAP.x as f32,
                UiContext::WINDOW_HEADER_HEIGHT,
            ) - Vec2::new(
                0.0,
                UiContext::WINDOW_PAD.y as f32 + UiContext::ELEMENT_GAP.y as f32,
            ),
        );

        text_cursor += Vec2::new(label_rect.width() + UiContext::ELEMENT_GAP.as_vec2().x, 0.0);

        rects.push(label_rect);
        if sibling_id == index as u32 {
            own_rect = Some(label_rect);
        }
    }
    (
        own_rect.expect("Window isnt inside its own siblings?"),
        rects,
    )
}

fn window_focused(
    window: &mut UiWindow,
    index: usize,
    drag_rect: Rect,
    dock: &mut DockingNode,
    frame: &FrameInfo,
    newly_focused: &mut Option<usize>,
) {
    let input = &frame.input;

    if input.left_mouse_pressed {
        let clicked = input.cursor_pos.is_some_and(|p| drag_rect.contains(p));
        if clicked && newly_focused.is_none() {
            *newly_focused = Some(index);
            if window.docked {
                dock.set_active_tab(index as u32);
            }
            if window.focused.is_none() {
                window.focused = Some(FocusedState::default());
            }
        } else {
            window.focused = None;
        }
    }
}

fn get_dock_info(
    window: &mut UiWindow,
    index: u32,
    dock: &DockingNode,
    full_screen_rect: Rect,
) -> Option<Siblings> {
    let siblings = if window.docked {
        dock.dock_info(index, full_screen_rect)
            .map(|(rect, siblings)| {
                window.dock_rect = Rect {
                    min: rect.min.round(),
                    max: rect.max.round(),
                };
                siblings
            })
    } else {
        None
    };

    window.open = siblings
        .as_ref()
        .map(|siblings| siblings.active() == index)
        .unwrap_or(window.open);

    siblings
}

fn handle_focused_input(
    window: &mut UiWindow,
    index: usize,
    drag_rect: Rect,
    dock: &mut DockingNode,
    frame: &FrameInfo,
) -> (Option<Rect>, bool) {
    let input = &frame.input;
    let Some(mut focused) = window.focused.take() else {
        return (None, false);
    };

    let mut preview_rect = None;
    let mut drag_released = false;

    if let Some(cursor_pos) = input.cursor_pos {
        if drag_rect.contains(cursor_pos) {
            if input.left_mouse_pressed {
                focused.darg_start = window.full_rect().min - cursor_pos;
                focused.drag_press_pos = cursor_pos;
                focused.is_being_draged = true;
            }
        } else if !window.full_rect().contains(cursor_pos)
            && !window.docked
            && input.left_mouse_pressed
        {
            ResizeEdges::hovered(window.full_rect(), cursor_pos, UiContext::DRAG_THRESHHOLD)
                .store_in(&mut focused);
        }

        if focused.is_being_draged
            && window.docked
            && cursor_pos.distance(focused.drag_press_pos) > UiContext::DRAG_THRESHHOLD
        {
            window.docked = false;
            dock.undock(index as u32);
        }

        if input.left_mouse_pressing && !window.docked {
            apply_drag_and_resize(window, &focused, cursor_pos);
        }

        if focused.is_being_draged && !window.docked {
            preview_rect = dock.preview_dock(
                cursor_pos,
                frame.full_screen_rect,
                UiContext::WINDOW_HEADER_HEIGHT,
            );
        }
    }

    if input.left_mouse_released {
        drag_released = focused.is_being_draged && !window.docked;
        focused.draging = None;
        focused.darg_start = Vec2::ZERO;
        focused.is_being_draged = false;
        ResizeEdges::default().store_in(&mut focused);
    }

    window.focused = Some(focused);
    (preview_rect, drag_released)
}

fn apply_drag_and_resize(window: &mut UiWindow, focused: &FocusedState, cursor_pos: Vec2) {
    let size = window.rect.size();

    if focused.is_being_draged {
        let drag_pos = (cursor_pos + focused.darg_start).round();
        window.rect.min = drag_pos;
        window.rect.max = drag_pos + size;
    }

    let min_size = UiContext::WINDOW_HEADER_HEIGHT + 10.0;
    if focused.resize_top {
        window.rect.min.y = cursor_pos.y.min(window.rect.max.y - min_size).round();
    }
    if focused.resize_bottom {
        window.rect.max.y = cursor_pos.y.max(window.rect.min.y + min_size).round();
    }
    if focused.resize_left {
        window.rect.min.x = cursor_pos.x.min(window.rect.max.x - 10.0).round();
    }
    if focused.resize_right {
        window.rect.max.x = cursor_pos.x.max(window.rect.min.x + 10.0).round();
    }
}

fn draw_window(
    window: &mut UiWindow,
    index: usize,
    siblings: Option<Siblings>,
    sibling_rects: Option<Vec<Rect>>,
    all_windows: &[Mutex<UiWindow>],
    frame: &FrameInfo,
) {
    window.indicies.clear();
    window.verticies.clear();
    window.top_indicies.clear();
    window.top_verticies.clear();

    let is_focused = window.focused.is_some();
    let edges = window
        .focused
        .as_ref()
        .map(|f| ResizeEdges::from(f))
        .unwrap_or_default();

    let border_color = |active: bool| {
        if active {
            UiContext::BLUE
        } else {
            UiContext::S1
        }
    };

    let window_ds = DrawSettings {
        on_top: false,
        color: if window.docked {
            UiContext::BG_DARK
        } else {
            UiContext::BG
        },
        rounding: UiContext::WINDOW_ROUNDING,
        round_topleft: false,
        round_topright: false,
        round_bottomleft: !window.docked,
        round_bottomright: !window.docked,
        border: Some(BorderSettings {
            color_top: border_color(edges.top),
            color_bottom: border_color(edges.bottom),
            color_left: border_color(edges.left),
            color_right: border_color(edges.right),
            size: UiContext::BORDER,
        }),
    };

    if window.open {
        window.draw_box(
            window.content_rect(),
            window_ds,
            frame.viewport_size,
            frame.full_screen_rect,
        );
    }

    if let Some(siblings) = siblings
        && let Some(sibling_rects) = sibling_rects
    {
        if siblings.active() as usize == index {
            draw_header_bar(window, is_focused, window_ds, frame);
            draw_docked_tabs(window, index, siblings, sibling_rects, all_windows, frame);
        }
    } else {
        draw_header_bar(window, is_focused, window_ds, frame);
        draw_standalone_header(window, is_focused, edges, frame);
    }

    if window.open {
        draw_scrollable_content(window, window.content_rect(), frame);
    }
}

fn draw_header_bar(
    window: &mut UiWindow,
    is_focused: bool,
    mut window_ds: DrawSettings,
    frame: &FrameInfo,
) {
    window_ds.round_topleft = !window.docked;
    window_ds.round_topright = !window.docked;
    window_ds.round_bottomleft = false;
    window_ds.round_bottomright = false;
    log::info!("test");
    window_ds.color = if is_focused {
        UiContext::BG
    } else {
        UiContext::BG_DARK
    };
    window_ds.border.as_mut().unwrap().color_bottom = UiContext::S1;
    window.draw_box(
        Rect::from_corners(
            window.full_rect().min,
            window.full_rect().min
                + Vec2::new(window.full_rect().width(), UiContext::WINDOW_HEADER_HEIGHT),
        ),
        window_ds,
        frame.viewport_size,
        frame.full_screen_rect,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_docked_tabs(
    window: &mut UiWindow,
    index: usize,
    siblings: Siblings,
    sibling_rects: Vec<Rect>,
    windows: &[Mutex<UiWindow>],
    frame: &FrameInfo,
) {
    for (i, s) in siblings.members.iter().copied().enumerate() {
        let active = s == siblings.active;
        let rect = sibling_rects[i as usize];

        let rect = Rect::from_corners(
            rect.min + window.full_rect().min,
            rect.max + window.full_rect().min,
        );

        let hoverd = frame.input.cursor_pos.is_some_and(|cp| rect.contains(cp));

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
        window.draw_box(rect, ds, frame.viewport_size, frame.full_screen_rect);

        let label = if s as usize == index {
            window.label.clone()
        } else {
            windows[s as usize].lock().unwrap().label.clone()
        };
        window.text(
            rect.min + UiContext::WINDOW_PAD.as_vec2(),
            UiContext::TEXT,
            &label,
            frame.viewport_size,
            frame.full_screen_rect,
            false,
        );
    }
}
fn draw_standalone_header(
    window: &mut UiWindow,
    is_focused: bool,
    edges: ResizeEdges,
    frame: &FrameInfo,
) {
    window.text_direction(
        window.full_rect().min
            + if !window.open {
                Vec2::new(0.0, UiContext::ATLAS_CELL_SIZE.y as f32 + 2.0)
            } else {
                Vec2::ZERO
            },
        UiContext::TEXT,
        "▼",
        frame.viewport_size,
        frame.full_screen_rect,
        false,
        if window.open {
            TextDirection::Right
        } else {
            TextDirection::Up
        },
    );

    let arrow_size = Vec2::new(
        UiContext::text_len("▼"),
        UiContext::ATLAS_CELL_SIZE.y as f32,
    );

    let label = window.label.clone();
    window.text(
        window.full_rect().min + Vec2::new(UiContext::ELEMENT_GAP.x as f32 + arrow_size.x, 0.0),
        UiContext::TEXT,
        &label,
        frame.viewport_size,
        frame.full_screen_rect,
        false,
    );

    if let Some(cursor_pos) = frame.input.cursor_pos
        && Rect::from_center_half_size(
            window.full_rect().min + Vec2::new(UiContext::ELEMENT_GAP.x as f32 + arrow_size.x, 0.0),
            arrow_size,
        )
        .contains(cursor_pos)
        && frame.input.left_mouse_pressed
        && is_focused
        && !(edges.top || edges.left)
    {
        window.open = !window.open;
    }
}

fn draw_scrollable_content(window: &mut UiWindow, content_area: Rect, frame: &FrameInfo) {
    let input = &frame.input;

    let mut hasher = DefaultHasher::new();
    window.label.hash(&mut hasher);
    let id = hasher.finish();

    let (_, mut scrollable) = window.scrollables.remove_entry(&id).unwrap_or((
        id,
        Scrollable {
            content_size: content_area.size(),
            scroll: Vec2::ZERO,
        },
    ));
    let rect = window.full_rect();
    scrollable.draw(
        NonZeroU64::new(id).unwrap_or(NonZeroU64::MIN),
        content_area,
        window,
        frame.viewport_size,
        input.cursor_pos,
        input.left_mouse_pressed,
        rect,
    );
    window.scrollables.insert(id, scrollable);
}

fn draw_dock_preview(window: &mut UiWindow, preview: Rect, frame: &FrameInfo) {
    window.draw_box(
        preview,
        DrawSettings {
            color: UiContext::BLUE_DIM,
            on_top: true,
            border: Some(BorderSettings::uniform(UiContext::BLUE, UiContext::BORDER)),
            ..Default::default()
        },
        frame.viewport_size,
        frame.full_screen_rect,
    );
}

fn handle_dock_resizing(
    ctx: &mut UiContext,
    dock: &mut DockingNode,
    frame: &FrameInfo,
    click_over_window: bool,
) {
    let input = &frame.input;

    if !click_over_window
        && let Some(cursor_pos) = input.cursor_pos
        && input.left_mouse_pressed
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
        && input.left_mouse_pressing
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

    if input.left_mouse_released {
        ctx.resize_path = u64::MAX;
        ctx.resize_depth = 0;
    }
}

fn reorder_layers(windows: &UiWindows) {
    let mut order: Vec<usize> = (0..windows.windows.len()).collect();
    order.sort_by_key(|&i| {
        windows.windows[i]
            .lock()
            .map(|w| {
                let tier: u8 = if w.docked {
                    0
                } else if w.focused.is_some() {
                    2
                } else {
                    1
                };
                (tier, w.layer)
            })
            .unwrap_or((1, 0))
    });

    for (layer, index) in order.into_iter().enumerate() {
        if let Ok(mut window) = windows.windows[index].lock() {
            window.layer = layer as u32;
        }
    }
}
