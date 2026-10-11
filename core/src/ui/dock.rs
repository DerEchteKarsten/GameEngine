//! Docking layout tree: splitting, docking/undocking and resizing of UI windows.
use std::collections::HashMap;

use bevy::{ecs::resource::Resource, math::Rect};
use glam::{Vec2, Vec2Swizzles};
use serde::{Deserialize, Serialize};

use crate::ui::{UiContext, from_pos_size};

#[derive(Serialize, Deserialize, Clone)]
pub enum Split {
    Horizontal,
    Vertical,
}

impl Split {
    pub fn direction_vec(&self) -> Vec2 {
        match self {
            Self::Horizontal => Vec2::new(1.0, 0.0),
            Self::Vertical => Vec2::new(0.0, 1.0),
        }
    }
    pub fn to_bytes(&self) -> u8 {
        match self {
            Self::Horizontal => 0,
            Self::Vertical => 1,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Resource)]
pub enum DockingNode {
    Leaf {
        window: u32,
    },
    Node {
        split: Split,
        extend: f32,
        left: Box<DockingNode>,
        right: Box<DockingNode>,
    },
}

impl DockingNode {
    pub fn contains(&self, window: u32) -> bool {
        match self {
            DockingNode::Leaf { window: w, .. } => *w == window,
            DockingNode::Node { left, right, .. } => {
                left.contains(window) || right.contains(window)
            }
        }
    }

    fn split_area(area: Rect, split: Split, extend: f32) -> (Rect, Rect) {
        let size = (1.0 - extend) * area.size();
        let left_area = Rect {
            min: area.min,
            max: area.max - split.direction_vec() * size,
        };
        let size = extend * area.size();
        let right_area = Rect {
            min: area.min + split.direction_vec() * size,
            max: area.max,
        };
        (left_area, right_area)
    }

    /// Puts `window` into the first empty leaf. Returns false if there is none.
    pub fn fill_empty(&mut self, window: u32) -> bool {
        match self {
            DockingNode::Leaf { window: w } => {
                let empty = *w == u32::MAX;
                if empty {
                    *w = window;
                }
                empty
            }
            DockingNode::Node { left, right, .. } => {
                left.fill_empty(window) || right.fill_empty(window)
            }
        }
    }

    pub fn dock(&mut self, window: u32, cursor_pos: Vec2, area: Rect) -> Option<u32> {
        match self {
            DockingNode::Leaf { window: w, .. } => {
                if *w == u32::MAX {
                    // An empty leaf is filled completely instead of being split.
                    if area.contains(cursor_pos) {
                        *w = window;
                    }
                    return None;
                }
                if header_strip(area).contains(cursor_pos) {
                    return Some(*w);
                }
                if let Some((split, after)) = edge_split(area, cursor_pos) {
                    let new = Box::new(DockingNode::Leaf { window });
                    let old = Box::new(std::mem::take(self));
                    let (left, right) = if after { (old, new) } else { (new, old) };
                    *self = DockingNode::Node {
                        split,
                        extend: 0.5,
                        left,
                        right,
                    };
                }
                None
            }
            DockingNode::Node {
                split,
                extend,
                left,
                right,
            } => {
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);
                if left_area.contains(cursor_pos) {
                    Self::dock(left, window, cursor_pos, left_area)
                } else if right_area.contains(cursor_pos) {
                    Self::dock(right, window, cursor_pos, right_area)
                } else {
                    None
                }
            }
        }
    }

    pub fn preview_dock(&self, cursor_pos: Vec2, area: Rect) -> Option<Rect> {
        match self {
            DockingNode::Leaf { window } => {
                if *window == u32::MAX {
                    return area.contains(cursor_pos).then_some(area);
                }
                if header_strip(area).contains(cursor_pos) {
                    return Some(area);
                }
                let (split, after) = edge_split(area, cursor_pos)?;
                let (left, right) = Self::split_area(area, split, 0.5);
                Some(if after { right } else { left })
            }
            DockingNode::Node {
                split,
                extend,
                left,
                right,
            } => {
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);
                if left_area.contains(cursor_pos) {
                    left.preview_dock(cursor_pos, left_area)
                } else if right_area.contains(cursor_pos) {
                    right.preview_dock(cursor_pos, right_area)
                } else {
                    None
                }
            }
        }
    }

    pub fn undock(&mut self, window: u32) -> bool {
        match self {
            DockingNode::Leaf { window: w, .. } => *w == window,
            DockingNode::Node {
                split: _,
                extend: _,
                left,
                right,
            } => {
                let left_empty = left.undock(window);
                let right_empty = right.undock(window);
                if left_empty && right_empty {
                    return true;
                }
                if left_empty {
                    let root = std::mem::take(right);
                    *self = *root;
                } else if right_empty {
                    let root = std::mem::take(left);
                    *self = *root;
                }
                false
            }
        }
    }

    pub fn find_resize(
        &self,
        cursor_pos: Vec2,
        area: Rect,
        path: u64,
        depth: usize,
    ) -> (u64, u32, Vec2, Option<Split>) {
        match self {
            DockingNode::Leaf { .. } => (u64::MAX, 0, Vec2::ZERO, None),
            DockingNode::Node {
                split,
                extend,
                left,
                right,
            } => {
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);
                let thickness = UiContext::RESIZE_THRESHOLD;
                let d = split.direction_vec();
                let perp = d.yx();

                let split_point = area.min + d * (area.size() * *extend);
                let divider = Rect {
                    min: split_point - d * (thickness / 2.0),
                    max: split_point + d * (thickness / 2.0) + perp * area.size(),
                };

                if divider.contains(cursor_pos) {
                    (path, depth as u32, area.min, Some(split.clone()))
                } else if left_area.contains(cursor_pos) {
                    left.find_resize(cursor_pos, left_area, path | (1u64 << depth), depth + 1)
                } else if right_area.contains(cursor_pos) {
                    right.find_resize(cursor_pos, right_area, path, depth + 1)
                } else {
                    (u64::MAX, 0, Vec2::ZERO, None)
                }
            }
        }
    }

    pub fn resize(&mut self, path: u64, max_depth: u32, depth: u32, delta: Vec2, area: Rect) {
        match self {
            DockingNode::Node {
                split,
                extend,
                left,
                right,
            } => {
                if depth == max_depth {
                    *extend = (delta.project_onto(split.clone().direction_vec()) / area.size())
                        .length()
                        .clamp(0.1, 0.9);
                }
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);

                if ((path >> depth) & 1) > 0 {
                    left.as_mut()
                        .resize(path, max_depth, depth + 1, delta, left_area);
                } else {
                    right
                        .as_mut()
                        .resize(path, max_depth, depth + 1, delta, right_area);
                }
            }
            DockingNode::Leaf { .. } => {}
        }
    }

    pub fn dock_info(&self, window: u32, area: Rect) -> Option<Rect> {
        match self {
            DockingNode::Leaf { window: w, .. } => {
                if *w == window {
                    Some(area)
                } else {
                    None
                }
            }
            DockingNode::Node {
                split,
                extend,
                left,
                right,
            } => {
                let (left_area, right_area) = Self::split_area(area, split.clone(), *extend);

                let left = left.dock_info(window, left_area);
                if left.is_some() {
                    left
                } else {
                    right.dock_info(window, right_area)
                }
            }
        }
    }

    pub fn remap(self, map: &HashMap<u32, u32>) -> Self {
        match self {
            DockingNode::Leaf { window: w } => {
                if let Some(new_w) = map.get(&w) {
                    DockingNode::Leaf { window: *new_w }
                } else {
                    self
                }
            }
            DockingNode::Node {
                split,
                extend,
                left,
                right,
            } => DockingNode::Node {
                split,
                extend,
                left: Box::new(left.remap(map)),
                right: Box::new(right.remap(map)),
            },
        }
    }
}

impl Default for DockingNode {
    fn default() -> Self {
        DockingNode::Leaf { window: u32::MAX }
    }
}

/// The strip of `area` a window's header covers; dropping a window there adds it as a tab.
fn header_strip(area: Rect) -> Rect {
    from_pos_size(
        area.min,
        Vec2::new(area.width(), UiContext::WINDOW_HEADER_HEIGHT),
    )
}

/// How a window dropped at `pos` near an edge of `area` splits it, and whether the window
/// goes after (right of or below) the old content.
fn edge_split(area: Rect, pos: Vec2) -> Option<(Split, bool)> {
    const THICKNESS: f32 = 40.0;
    if !area.contains(pos) {
        return None;
    }
    let near = |d: f32| d <= THICKNESS;
    let (top, bottom) = (near(pos.y - area.min.y), near(area.max.y - pos.y));
    if top || bottom {
        Some((Split::Vertical, bottom))
    } else if near(pos.x - area.min.x) || near(area.max.x - pos.x) {
        Some((Split::Horizontal, near(area.max.x - pos.x)))
    } else {
        None
    }
}
