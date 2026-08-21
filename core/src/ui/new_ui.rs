use bevy::{
    input::{ButtonInput, touch::Touches},
    log,
    window::Window,
};
use fontdue::*;
use std::{collections::HashMap, fs, num::NonZeroU64, range::Range, sync::Mutex, vec::IntoIter};

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
        dock::{DockingNode, Siblings, Split},
        window::UiWindow,
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
        $crate::ui::new_ui::hash_location(file!(), line!(), column!())
    };
}

pub fn from_pos_size(pos: Vec2, size: Vec2) -> Rect {
    Rect::from_corners(pos, pos + size)
}

#[derive(Clone)]
pub struct FocusedState {
    pub is_being_draged: bool,
    pub draging: Option<NonZeroU64>,
    pub focused: Option<NonZeroU64>,
    pub cursor: TextCursor,
    pub selected: Range<usize>,
    pub offset: f32,
    pub darg_start: Vec2,
    pub drag_press_pos: Vec2,
    pub format_string: String,

    pub resize_top: bool,
    pub resize_bottom: bool,
    pub resize_left: bool,
    pub resize_right: bool,
}

impl Default for FocusedState {
    fn default() -> Self {
        Self {
            is_being_draged: false,
            draging: None,
            focused: None,
            cursor: TextCursor { byte_pos: 0 },
            selected: (0..0).into(),
            offset: 0.0,
            darg_start: Vec2::ZERO,
            drag_press_pos: Vec2::ZERO,
            format_string: String::new(),
            resize_top: false,
            resize_bottom: false,
            resize_left: false,
            resize_right: false,
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
    pub window_labels: HashMap<String, u32>,
    pub windows: Vec<Mutex<UiWindow>>,
}

impl UiWindows {
    pub fn by_layer(&self) -> std::vec::IntoIter<(usize, &std::sync::Mutex<UiWindow>)> {
        self.windows
            .iter()
            .enumerate()
            .sorted_by(|a, b| a.1.lock().unwrap().layer.cmp(&b.1.lock().unwrap().layer))
    }
}

#[derive(Resource)]
pub struct NUiResources {
    pub font_atlas: Image<format::R8Unorm, usage::Sampled>,
    pub verticies: [Buffer<UIVertex>; FRAMES_IN_FLIGHT],
    pub indicies: [Buffer<u32>; FRAMES_IN_FLIGHT],
    pub num_verticies: usize,
    pub num_indicies: usize,
    pub pending_verticies: Vec<UIVertex>,
    pub pending_indicies: Vec<u32>,
}

impl UiContext {
    pub const BG: Vec4 = Vec4::new(0.155, 0.155, 0.155, 1.0);
    pub const BG_DARK: Vec4 = Vec4::new(0.130, 0.130, 0.130, 1.0);
    pub const S0: Vec4 = Vec4::new(0.220, 0.220, 0.220, 1.0);
    pub const S1: Vec4 = Vec4::new(0.260, 0.260, 0.260, 1.0);
    pub const S2: Vec4 = Vec4::new(0.300, 0.300, 0.300, 1.0);
    pub const GRAB: Vec4 = Vec4::new(0.370, 0.370, 0.370, 1.0);
    pub const GRAB_HOT: Vec4 = Vec4::new(0.490, 0.490, 0.490, 1.0);
    pub const TEXT: Vec4 = Vec4::new(0.880, 0.880, 0.880, 1.0);
    pub const TEXT_DIM: Vec4 = Vec4::new(0.550, 0.550, 0.550, 1.0);

    pub const BLUE: Vec4 = Vec4::new(0.118, 0.565, 0.831, 1.0);
    pub const BLUE_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.6);

    pub const TRACE: Vec4 = Vec4::new(0.380, 0.380, 0.380, 1.0);
    pub const DEBUG: Vec4 = Vec4::new(0.400, 0.560, 0.700, 1.0);
    pub const INFO: Vec4 = Vec4::new(0.820, 0.820, 0.820, 1.0);
    pub const WARN: Vec4 = Vec4::new(0.980, 0.760, 0.110, 1.0);
    pub const ERROR: Vec4 = Vec4::new(0.950, 0.180, 0.180, 1.0);
    pub const BLUE_REALY_DIM: Vec4 = Vec4::new(0.118, 0.565, 0.831, 0.35);

    pub const DRAG_THRESHHOLD: f32 = 20.0;
    pub const FONT_SCALE: u32 = 6;
    pub const ATLAS_CELL_SIZE: UVec2 = UVec2::new(14, 30);
    pub const CHARACTER_ADVANCE_WIDTH: u32 = 0;
    pub const ATLAS_SIZE: UVec2 = UVec2::new(
        Self::ATLAS_CELL_SIZE.x * 256u32,
        Self::ATLAS_CELL_SIZE.y * 256u32,
    );
    pub const UV_SIZE: Vec2 = Vec2::splat(1.0 / 256.0);
    pub const LINE_SPACING: u32 = 1;

    pub const ELEMENT_GAP: UVec2 = UVec2::new(4, 2);
    pub const WINDOW_ROUNDING: u32 = 4;
    pub const ROUNDING: u32 = 2;
    pub const BORDER: u32 = 1;
    pub const CHILD_PAD: UVec2 = UVec2::new(2, 1);
    pub const INDENT: UVec2 = UVec2::new(40, 0);
    pub const WINDOW_PAD: UVec2 = UVec2::new(3, 2);
    pub const WINDOW_HEADER_HEIGHT: f32 =
        (UiContext::ATLAS_CELL_SIZE.y as f32 + UiContext::WINDOW_PAD.y as f32 * 2.0).round();

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
            window_labels,
            windows,
        } = ron::from_str(&fs::read_to_string("windows.ron").unwrap_or("".to_owned())).unwrap_or(
            SaveState {
                docking_nodes: DockingNode::Leaf {
                    siblings: Siblings {
                        members: SmallVec::new(),
                        active: 0,
                    },
                    root: true,
                },
                windows: Vec::new(),
                window_labels: HashMap::new(),
            },
        );
        let windows = UiWindows {
            add_windows: Mutex::new(SmallVec::new()),
            windows: windows
                .into_iter()
                .map(|w| Mutex::new(UiWindow::new(w.label, w.size, w.open, w.docked)))
                .collect(),
            window_labels,
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

    pub(crate) fn build_ui_resources(&mut self) -> Result<NUiResources> {
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

        Ok(NUiResources {
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

pub fn nwrite_ui_data(mut resources: ResMut<NUiResources>, frame: Res<FrameCount>) {
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
    res: Option<Res<NUiResources>>,
    mut world: ResMut<MainWorld>,
) {
    if res.is_some() {
        return;
    }
    let mut ctx = world.get_resource_mut::<UiContext>().unwrap();
    cmd.insert_resource(ctx.build_ui_resources().unwrap());
}

pub fn nextract_ui(mut res: If<ResMut<NUiResources>>, windows: Extract<Res<UiWindows>>) {
    for window in windows
        .windows
        .iter()
        .sorted_by_key(|a| a.lock().unwrap().layer)
    {
        let window = window.lock().unwrap();
        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies
            .extend(window.indicies.iter().map(|e| *e + vertex_offset as u32));
        res.pending_verticies.extend(window.verticies.iter());
        let vertex_offset = res.pending_verticies.len();
        res.pending_indicies.extend(
            window
                .top_indicies
                .iter()
                .map(|e| *e + vertex_offset as u32),
        );
        res.pending_verticies.extend(window.top_verticies.iter());
    }
}

#[derive(Clone, Copy)]
pub struct MultiInput {
    pub left_mouse_pressed: bool,
    pub left_mouse_pressing: bool,
    pub left_mouse_released: bool,
    pub cursor_pos: Option<Vec2>,
}

impl MultiInput {
    pub fn new(
        desktop_window: &Window,
        buttons: &ButtonInput<MouseButton>,
        touch: &Touches,
    ) -> Self {
        let mut this = Self {
            left_mouse_pressed: buttons.just_pressed(MouseButton::Left),
            left_mouse_pressing: buttons.pressed(MouseButton::Left),
            left_mouse_released: buttons.just_released(MouseButton::Left),
            cursor_pos: desktop_window.physical_cursor_position(),
        };

        if let Some(touch) = touch.iter().next() {
            this.cursor_pos = Some(touch.position() * desktop_window.scale_factor() as f32);
            this.left_mouse_pressing = true;
        }
        if let Some(touch) = touch.iter_just_pressed().next() {
            this.cursor_pos = Some(touch.position() * desktop_window.scale_factor() as f32);
            this.left_mouse_pressed = true;
        }
        if let Some(touch) = touch.iter_just_released().next() {
            this.cursor_pos = Some(touch.position() * desktop_window.scale_factor() as f32);
            this.left_mouse_released = true;
        }
        this
    }
}

#[derive(Serialize, Deserialize)]
struct SaveWindow {
    label: String,
    size: Rect,
    open: bool,
    docked: bool,
}

#[derive(Serialize, Deserialize)]
struct SaveState {
    docking_nodes: DockingNode,
    windows: Vec<SaveWindow>,
    window_labels: HashMap<String, u32>,
}

pub fn save_windows(
    events: MessageReader<AppExit>,
    windows: Res<UiWindows>,
    docking_nodes: Res<DockingNode>,
) {
    if events.is_empty() {
        return;
    }

    let save_state = SaveState {
        docking_nodes: docking_nodes.clone(),
        window_labels: windows.window_labels.clone(),
        windows: windows
            .windows
            .iter()
            .map(|w| {
                let window = w.lock().unwrap();
                SaveWindow {
                    label: window.label.clone(),
                    docked: window.docked,
                    open: window.open,
                    size: window.full_rect(),
                }
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
