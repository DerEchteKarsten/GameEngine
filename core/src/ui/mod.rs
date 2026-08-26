use bevy::{
    app::{App, PostUpdate, PreUpdate, Update},
    ecs::schedule::IntoScheduleConfigs,
    input::InputSystems,
};

use crate::{
    editor::viewport::ViewPort,
    render::{
        ExtractSchedule, Render, RenderApp,
        RenderSystems::{self},
    },
    ui::update_windows::{draw_windows, update_windows},
};

pub mod builder;
pub mod dock;
pub mod scrollable;
pub mod update_windows;
pub mod window;

// impl UiBuilder {
//     pub const BG: [f32; 4] = [0.155, 0.155, 0.155, 1.0]; // #272727 – slightly darker bg
//     pub const BG_DARK: [f32; 4] = [0.130, 0.130, 0.130, 1.0]; // #212121
//     pub const S0: [f32; 4] = [0.220, 0.220, 0.220, 1.0]; // #383838 – buttons/frames, bigger jump from bg
//     pub const S1: [f32; 4] = [0.260, 0.260, 0.260, 1.0]; // #424242 – hovered
//     pub const S2: [f32; 4] = [0.300, 0.300, 0.300, 1.0]; // #3c3c50 – active
//     pub const GRAB: [f32; 4] = [0.370, 0.370, 0.370, 1.0]; // #5e5e5e
//     pub const GRAB_HOT: [f32; 4] = [0.490, 0.490, 0.490, 1.0]; // #7d7d7d
//     pub const TEXT: [f32; 4] = [0.880, 0.880, 0.880, 1.0]; // #e0e0e0
//     pub const TEXT_DIM: [f32; 4] = [0.550, 0.550, 0.550, 1.0]; // #8c8c8c

//     pub const BLUE: [f32; 4] = [0.118, 0.565, 0.831, 1.0]; // #1e90d4 – UE blue
//     pub const BLUE_DIM: [f32; 4] = [0.118, 0.565, 0.831, 0.6]; // UE blue dimmed
//     pub const BLUE_REALY_DIM: [f32; 4] = [0.118, 0.565, 0.831, 0.35];

//     pub const TRACE: [f32; 4] = [0.380, 0.380, 0.380, 1.0]; // trace
//     pub const DEBUG: [f32; 4] = [0.400, 0.560, 0.700, 1.0]; // debug
//     pub const INFO: [f32; 4] = [0.820, 0.820, 0.820, 1.0]; // info
//     pub const WARN: [f32; 4] = [0.980, 0.760, 0.110, 1.0]; // warn
//     pub const ERROR: [f32; 4] = [0.950, 0.180, 0.180, 1.0]; // error
// }
use bevy::{
    input::{ButtonInput, touch::Touches},
    log,
    window::Window,
};
use fontdue::*;
use std::{
    collections::HashMap,
    fs,
    num::NonZeroU64,
    range::Range,
    sync::Mutex,
};

use anyhow::Result;
use bevy::{
    app::AppExit,
    ecs::{
        message::MessageReader,
        resource::Resource,
        system::{Commands, If, Res, ResMut},
    },
    input::mouse::MouseButton,
    math::Rect,
};
use futures::executor::block_on;
use glam::{UVec2, Vec2, Vec4};
use itertools::Itertools;
use lava::{
    buffer::Buffer,
    image::{Image, format, usage},
};
use ron::ser::PrettyConfig;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

use crate::{
    bindings::UIVertex,
    render::{
        FRAMES_IN_FLIGHT, MainWorld, extract_param::Extract, render::FrameCount, world::UploadQueue,
    },
    ui::{
        builder::TextCursor,
        dock::DockingNode,
        update_windows::ResizeEdges,
        window::{Tab, TabState, UiWindow},
    },
};

pub const fn hash_location(file: &str, line: u32, col: u32) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    let bytes = file.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000003b4c61);
        i += 1;
    }
    hash ^= line as u64;
    hash = hash.wrapping_mul(0x100000003b4c61);
    hash ^= col as u64;
    hash = hash.wrapping_mul(0x100000003b4c61);
    hash
}

#[macro_export]
macro_rules! id {
    () => {
        $crate::ui::hash_location(file!(), line!(), column!())
    };
}

pub fn from_pos_size(pos: Vec2, size: Vec2) -> Rect {
    Rect::from_corners(pos, pos + size)
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Draggable {
    ActiveTab,
    Window,
    WindowScrollHandle,
    TabScrollHandle,
    Element(NonZeroU64),
    DragAndDrop(NonZeroU64),
    ScrollHandle,
}

#[derive(Clone, Debug)]
pub struct FocusedState {
    pub draging: Option<Draggable>,
    pub focused: Option<NonZeroU64>,
    pub cursor: TextCursor,
    pub selected: Range<usize>,
    pub offset: f32,
    pub drag_start: Vec2,
    pub drag_press_pos: Vec2,
    pub format_string: String,
    pub edges: ResizeEdges,
}

impl Default for FocusedState {
    fn default() -> Self {
        Self {
            draging: None,
            focused: None,
            cursor: TextCursor { byte_pos: 0 },
            selected: (0..0).into(),
            offset: 0.0,
            drag_start: Vec2::ZERO,
            drag_press_pos: Vec2::ZERO,
            format_string: String::new(),
            edges: ResizeEdges::default(),
        }
    }
}

#[derive(Resource)]
pub struct UiContext {
    pub font: Option<fontdue::Font>,
    pub resize_path: u64,
    pub resize_depth: u32,
    pub drag_start: Vec2,
}

#[derive(Resource)]
pub struct UiWindows {
    pub add_windows: Mutex<SmallVec<[String; 4]>>,
    pub windows: Vec<Option<UiWindow>>,
    pub free_slots: Vec<usize>,
}

impl UiWindows {
    pub fn by_layer_mut(&mut self) -> impl DoubleEndedIterator<Item = (usize, &mut UiWindow)> {
        self.windows
            .iter_mut()
            .enumerate()
            .filter_map(|w| w.1.as_mut().map(|o| (w.0, o)))
            .sorted_by(|a, b| a.1.layer.cmp(&b.1.layer))
    }
    pub fn by_layer(&self) -> impl DoubleEndedIterator<Item = (usize, &UiWindow)> {
        self.windows
            .iter()
            .enumerate()
            .filter_map(|w| w.1.as_ref().map(|o| (w.0, o)))
            .sorted_by(|a, b| a.1.layer.cmp(&b.1.layer))
    }
    pub fn remove(&mut self, index: usize) -> UiWindow {
        let window = self.windows[index].take().unwrap();
        self.free_slots.push(index);
        window
    }

    pub fn append(&mut self, window: UiWindow) {
        if let Some(index) = self.free_slots.pop() {
            self.windows[index] = Some(window);
        } else {
            self.windows.push(Some(window));
        }
    }

    pub fn log(&self) {
        for (i, window) in self.windows.iter().enumerate() {
            if let Some(w) = window {
                log::info!(
                    "{i}: {:#?}",
                    w.tabs.iter().map(|t| &t.label).collect::<Vec<_>>()
                );
            } else {
                log::info!("{i}: Empty");
            }
        }
    }
}

#[derive(Resource)]
pub struct UiResources {
    pub font_atlas: Image<format::R8Unorm, usage::Sampled>,
    pub verticies: [Buffer<UIVertex>; FRAMES_IN_FLIGHT],
    pub indicies: [Buffer<u32>; FRAMES_IN_FLIGHT],
    pub num_verticies: usize,
    pub num_indicies: usize,
    pub pending_verticies: Vec<UIVertex>,
    pub pending_indicies: Vec<u32>,
}

impl UiContext {
    pub const BG: Vec4 = Vec4::new(0.196, 0.188, 0.184, 1.0); // dark0_soft #32302f (editor bg)
    pub const BG_DARK: Vec4 = Vec4::new(0.157, 0.157, 0.157, 1.0); // dark0 #282828 (recessed)
    pub const S0: Vec4 = Vec4::new(0.235, 0.220, 0.212, 1.0); // dark1 #3c3836 (sidebar)
    pub const S1: Vec4 = Vec4::new(0.275, 0.253, 0.241, 1.0); // dark1→dark2
    pub const S2: Vec4 = Vec4::new(0.314, 0.286, 0.271, 1.0); // dark2 #504945 (tab bar)
    pub const GRAB: Vec4 = Vec4::new(0.400, 0.361, 0.329, 1.0); // dark3 #665c54 (scrollbar thumb)
    pub const GRAB_HOT: Vec4 = Vec4::new(0.486, 0.435, 0.392, 1.0); // dark4 #7c6f64 (hover)
    pub const TEXT: Vec4 = Vec4::new(0.922, 0.859, 0.698, 1.0); // fg #ebdbb2 (cream)
    pub const TEXT_DIM: Vec4 = Vec4::new(0.573, 0.514, 0.455, 1.0); // gray #928374
    pub const ACENT: Vec4 = Vec4::new(0.118, 0.565, 0.831, 1.0);
    pub const ACENT_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.6);
    pub const TRACE: Vec4 = Vec4::new(0.420, 0.380, 0.340, 1.0); // dim warm gray
    pub const DEBUG: Vec4 = Vec4::new(0.514, 0.647, 0.596, 1.0); // blue #83a598
    pub const INFO: Vec4 = Vec4::new(0.820, 0.760, 0.620, 1.0); // dimmed fg
    pub const WARN: Vec4 = Vec4::new(0.980, 0.741, 0.184, 1.0); // yellow #fabd2f
    pub const ERROR: Vec4 = Vec4::new(0.984, 0.286, 0.204, 1.0); // red #fb4934
    pub const ACENT_REALY_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.35);
    pub const DRAG_THRESHHOLD: f32 = 50.0;
    pub const FONT_SCALE: u32 = 6;
    pub const ATLAS_CELL_SIZE: UVec2 = UVec2::new(14, 30);
    pub const CHARACTER_ADVANCE_WIDTH: u32 = 0;
    pub const ATLAS_SIZE: UVec2 = UVec2::new(
        Self::ATLAS_CELL_SIZE.x * 256u32,
        Self::ATLAS_CELL_SIZE.y * 256u32,
    );
    pub const UV_SIZE: Vec2 = Vec2::splat(1.0 / 256.0);
    pub const LINE_SPACING: u32 = 1;
    pub const ELEMENT_GAP: UVec2 = UVec2::new(8, 4);
    pub const WINDOW_ROUNDING: u32 = 4;
    pub const ROUNDING: u32 = 2;
    pub const BORDER: u32 = 1;
    pub const CHILD_PAD: UVec2 = UVec2::new(2, 1);
    pub const INDENT: UVec2 = UVec2::new(20, 0);
    pub const TAB_PAD: UVec2 = UVec2::new(6, 2);
    pub const TAB_GAP: UVec2 = UVec2::new(4, 2);
    pub const WINDOW_PAD: UVec2 = UVec2::new(3, 2);
    pub const WINDOW_HEADER_HEIGHT: f32 =
        (UiContext::ATLAS_CELL_SIZE.y as f32 + UiContext::WINDOW_PAD.y as f32 * 2.0).round();
    pub const RESIZE_THRESHOLD: f32 = 15.0f32;
    pub const BAR_THICKNESS: f32 = 6.0f32;
    pub const MIN_THUMB: f32 = 20.0f32;

    pub(crate) fn char_to_atlas_pos(c: char) -> UVec2 {
        let idx = c as u32;
        if idx > u16::MAX as u32 {
            return UVec2::ZERO;
        }
        let idx = idx as u16;
        let lower = idx & 0b1111_1111u16;
        let higher = (idx >> 8) & 0b1111_1111u16;
        UVec2::new(lower as u32, higher as u32) * Self::ATLAS_CELL_SIZE
    }

    pub(crate) fn new() -> Result<(Self, UiWindows, DockingNode)> {
        let bytes = fs::read("/home/karsten/code/GameEngine/editor_font.ttf")?;
        let font = Font::from_bytes(bytes, FontSettings::default()).unwrap();

        let SaveState {
            docking_nodes,
            windows,
        } = ron::from_str(&fs::read_to_string("windows.ron").unwrap_or("".to_owned())).unwrap_or(
            SaveState {
                docking_nodes: DockingNode::Leaf { window: u32::MAX },
                windows: Vec::new(),
            },
        );
        let windows = UiWindows {
            add_windows: Mutex::new(SmallVec::new()),
            windows: windows
                .iter()
                .map(|w| {
                    Some(UiWindow::new(
                        w.tabs
                            .iter()
                            .map(|t| Tab {
                                label: t.label.clone(),
                                state: Mutex::new(TabState::default()),
                            })
                            .collect(),
                        w.rect.clone(),
                        w.active_tab,
                    ))
                })
                .collect(),
            free_slots: Vec::new(),
        };

        Ok((
            Self {
                font: Some(font),
                resize_path: u64::MAX,
                resize_depth: 0,
                drag_start: Vec2::ZERO,
            },
            windows,
            docking_nodes,
        ))
    }

    pub(crate) fn build_ui_resources(&mut self) -> Result<UiResources> {
        let font = self.font.take().unwrap();
        let pixels = (Self::FONT_SCALE * 4) as f32;
        let font_metrics = font.horizontal_line_metrics(pixels).unwrap();
        log::info!("{:#?}, {:#?}", font_metrics, UiContext::ATLAS_CELL_SIZE);

        let mut atlas_data = vec![0u8; (Self::ATLAS_SIZE.x * Self::ATLAS_SIZE.y) as usize];
        for (c, _) in font.chars().iter() {
            let (metrics, data) = font.rasterize(*c, pixels);
            let pos = Self::char_to_atlas_pos(*c);
            for x in 0..metrics.width {
                for y in 0..metrics.height {
                    let atlas_y = (pos.y as i32 + font_metrics.ascent as i32
                        - metrics.height as i32
                        + y as i32
                        - metrics.ymin)
                        .saturating_cast::<usize>();
                    let atlas_x =
                        (pos.x as i32 + (x as i32 + metrics.xmin)).saturating_cast::<usize>();

                    atlas_data[atlas_y * UiContext::ATLAS_SIZE.x as usize + atlas_x] =
                        data[y * metrics.width + x];
                }
            }
        }
        atlas_data[0] = 255;
        let font_atlas = Image::new(Self::ATLAS_SIZE.x, Self::ATLAS_SIZE.y).unwrap();
        let future = UploadQueue::push_image(atlas_data, font_atlas);
        let font_atlas = block_on(future)?;

        Ok(UiResources {
            font_atlas,
            indicies: [Buffer::new(16 * 1024, true)?, Buffer::new(16 * 1024, true)?],
            verticies: [Buffer::new(16 * 1024, true)?, Buffer::new(16 * 1024, true)?],
            pending_indicies: Vec::with_capacity(16 * 1024),
            pending_verticies: Vec::with_capacity(16 * 1024),
            num_indicies: 0,
            num_verticies: 0,
        })
    }

    pub fn text_size(str: &str) -> Vec2 {
        let mut size = Vec2::new(0.0, 0.0);
        for line in str.lines() {
            size.x = size.x.max(
                (UiContext::ATLAS_CELL_SIZE.x as f32 + UiContext::CHARACTER_ADVANCE_WIDTH as f32)
                    * line.len() as f32,
            );
            size.y += UiContext::ATLAS_CELL_SIZE.y as f32;
        }

        size
    }

    pub fn text_len(str: &str) -> f32 {
        str.len() as f32
            * (UiContext::ATLAS_CELL_SIZE.x as f32 + UiContext::CHARACTER_ADVANCE_WIDTH as f32)
    }
}

pub fn write_ui_data(mut resources: ResMut<UiResources>, frame: Res<FrameCount>) {
    if resources.indicies[frame.frame_in_flight()].len() < resources.pending_indicies.len() {
        resources.indicies[frame.frame_in_flight()] =
            Buffer::new(resources.pending_indicies.len().next_power_of_two(), true).unwrap();
    }
    if resources.verticies[frame.frame_in_flight()].len() < resources.pending_verticies.len() {
        resources.verticies[frame.frame_in_flight()] =
            Buffer::new(resources.pending_verticies.len().next_power_of_two(), true).unwrap();
    }

    resources.verticies[frame.frame_in_flight()]
        .range(..)
        .copy_from(&resources.pending_verticies);
    resources.indicies[frame.frame_in_flight()]
        .range(..)
        .copy_from(&resources.pending_indicies);

    resources.num_indicies = resources.pending_indicies.len();
    resources.num_verticies = resources.pending_verticies.len();

    resources.pending_verticies.clear();
    resources.pending_indicies.clear();
}

pub fn create_ui_resources(
    mut cmd: Commands,
    res: Option<Res<UiResources>>,
    mut world: ResMut<MainWorld>,
) {
    if res.is_some() {
        return;
    }
    let mut ctx = world.get_resource_mut::<UiContext>().unwrap();
    cmd.insert_resource(ctx.build_ui_resources().unwrap());
}

pub fn extract_ui(mut res: If<ResMut<UiResources>>, windows: Extract<Res<UiWindows>>) {
    for (_, window) in windows.by_layer() {
        let tab = window.active_tab();
        let Ok(tab_state) = tab.state.lock() else {
            continue;
        };

        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies
            .extend(window.indicies.iter().map(|e| *e + vertex_offset as u32));
        res.pending_verticies.extend(window.verticies.iter());

        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies
            .extend(tab_state.indicies.iter().map(|e| *e + vertex_offset as u32));
        res.pending_verticies.extend(tab_state.verticies.iter());

        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies.extend(
            tab_state
                .top_indicies
                .iter()
                .map(|e| *e + vertex_offset as u32),
        );
        res.pending_verticies.extend(tab_state.top_verticies.iter());
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MultiInput {
    pub primary_pressed: bool,
    pub primary_pressing: bool,
    pub primary_released: bool,
    pub cursor_pos: Option<Vec2>,
}

impl MultiInput {
    pub fn new(
        desktop_window: &Window,
        buttons: &ButtonInput<MouseButton>,
        touch: &Touches,
    ) -> Self {
        let mut this = Self {
            primary_pressed: buttons.just_pressed(MouseButton::Left),
            primary_pressing: buttons.pressed(MouseButton::Left),
            primary_released: buttons.just_released(MouseButton::Left),
            cursor_pos: desktop_window.physical_cursor_position(),
        };

        if let Some(touch) = touch.iter().next() {
            this.cursor_pos = Some(touch.position() * desktop_window.scale_factor() as f32);
            this.primary_pressing = true;
        }
        if let Some(touch) = touch.iter_just_pressed().next() {
            this.cursor_pos = Some(touch.position() * desktop_window.scale_factor() as f32);
            this.primary_pressed = true;
        }
        if let Some(touch) = touch.iter_just_released().next() {
            this.cursor_pos = Some(touch.position() * desktop_window.scale_factor() as f32);
            this.primary_released = true;
        }
        this
    }

    pub fn to_viewport(self, view_port: &ViewPort) -> Self {
        let cursor_pos = self.cursor_pos.and_then(|cp| {
            let new_cp = cp - view_port.rect.min;
            if view_port.rect.contains(cp) {
                Some(new_cp)
            } else {
                None
            }
        });
        Self { cursor_pos, ..self }
    }

    pub fn clicked(&self, rect: Rect) -> bool {
        self.primary_pressed && self.hovered(rect)
    }
    pub fn hovered(&self, rect: Rect) -> bool {
        self.cursor_pos.is_some_and(|cp| rect.contains(cp))
    }
}

#[derive(Serialize, Deserialize)]
struct SaveWindow {
    rect: Rect,
    tabs: Vec<SaveTab>,
    active_tab: u32,
}

#[derive(Serialize, Deserialize)]
struct SaveTab {
    label: String,
}

#[derive(Serialize, Deserialize)]
struct SaveState {
    docking_nodes: DockingNode,
    windows: Vec<SaveWindow>,
}

pub fn save_windows(
    events: MessageReader<AppExit>,
    windows: Res<UiWindows>,
    docking_nodes: Res<DockingNode>,
) {
    if events.is_empty() {
        return;
    }

    let mut remap = HashMap::new();
    let mut new_idx = 0;
    for (i, w) in windows.windows.iter().enumerate() {
        if w.is_some() {
            remap.insert(i as u32, new_idx as u32);
            new_idx += 1;
        }
    }

    let save_state = SaveState {
        docking_nodes: docking_nodes.clone().remap(&remap),
        windows: windows
            .windows
            .iter()
            .filter_map(|w| w.as_ref())
            .map(|w| SaveWindow {
                tabs: w
                    .tabs
                    .iter()
                    .map(|t| SaveTab {
                        label: t.label.clone(),
                    })
                    .collect::<Vec<_>>(),
                rect: w.rect,
                active_tab: w.active_tab,
            })
            .collect::<Vec<_>>(),
    };

    let config = PrettyConfig::new();
    std::fs::write(
        "windows.ron",
        ron::ser::to_string_pretty(&save_state, config).unwrap(),
    )
    .unwrap();
}

#[allow(non_snake_case)]
pub fn UiPlugin(app: &mut App) {
    let sub_app = app.get_sub_app_mut(RenderApp).unwrap();
    sub_app
        .add_systems(Render, write_ui_data.in_set(RenderSystems::PreRender))
        .add_systems(ExtractSchedule, (extract_ui, create_ui_resources));
    let (ctx, windows, dock) = UiContext::new().unwrap();
    app.add_systems(PreUpdate, update_windows.after(InputSystems))
        .add_systems(Update, draw_windows)
        .insert_resource(ctx)
        .insert_resource(windows)
        .insert_resource(dock)
        .add_systems(PostUpdate, save_windows);
}
