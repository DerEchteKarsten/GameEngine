//! Editor log console: tracing/Vulkan validation capture layer, Tracy setup and console UI.
use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::app::{App, Last, Plugin, Update};
use bevy::ecs::prelude::*;
use bevy::math::{Rect, VectorSpace};
use glam::{Vec2, Vec4};
use lava::state::raw_vulkan::{DebugUtilsMessageSeverityFlagsEXT, DebugUtilsMessageTypeFlagsEXT};
use tracing::Level;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, fmt};

use crate::id;
use crate::ui::UiContext;
use crate::ui::builder::UiBuilder;
use crate::ui::window::{BorderSettings, DrawSettings};

// #[global_allocator]
// static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
//     tracy_client::ProfiledAllocator::new(std::alloc::System, 100);

#[derive(Debug, Clone, Default)]
pub struct LogEntry {
    pub buffer: Box<str>,
    pub message_type: MessageType,
    pub location: u16,
    pub frame: u64,
}

#[derive(Debug, Clone)]
pub enum MessageType {
    Validation {
        ty: DebugUtilsMessageTypeFlagsEXT,
        serverity: DebugUtilsMessageSeverityFlagsEXT,
    },
    Normal {
        level: u8,
        target: u16,
    },
}

impl Default for MessageType {
    fn default() -> Self {
        MessageType::Normal {
            level: 0,
            target: 0,
        }
    }
}

impl MessageType {
    pub fn to_str(&self, buffer: &str) -> String {
        match self {
            MessageType::Validation { ty, serverity } => {
                format!("VULKAN-{:?}-{:?}", serverity, ty)
            }
            MessageType::Normal { level, target } => {
                let target = *target as usize;
                if target == buffer.len() {
                    format!("{}", LogEntry::LEVEL_NAMES[*level as usize])
                } else {
                    format!(
                        "{}-[{}]",
                        LogEntry::LEVEL_NAMES[*level as usize],
                        &buffer[target..buffer.len()]
                    )
                }
            }
        }
    }

    pub fn color(&self) -> Vec4 {
        match self {
            MessageType::Validation { ty, serverity } => {
                if serverity.contains(DebugUtilsMessageSeverityFlagsEXT::ERROR) {
                    UiContext::ERROR * 1.25
                } else if serverity.contains(DebugUtilsMessageSeverityFlagsEXT::WARNING) {
                    UiContext::WARN * 1.25
                } else if serverity.contains(DebugUtilsMessageSeverityFlagsEXT::INFO) {
                    UiContext::INFO * 1.25
                } else if serverity.contains(DebugUtilsMessageSeverityFlagsEXT::VERBOSE) {
                    UiContext::TRACE * 1.25
                } else {
                    UiContext::ERROR * 1.25
                }
            }
            MessageType::Normal { level, .. } => LogEntry::LEVEL_COLORS[*level as usize],
        }
    }
}

impl LogEntry {
    const LEVEL_NAMES: [&str; 5] = ["TRACE", "DEBUG", "INFO", "WARNING", "ERROR"];
    const LEVEL_COLORS: [Vec4; 5] = [
        UiContext::TRACE,
        UiContext::DEBUG,
        UiContext::INFO,
        UiContext::WARN,
        UiContext::ERROR,
    ];

    fn format(&self) -> (String, Vec4) {
        let strg = format!(
            "[{:>6}] {} {}",
            self.frame,
            self.message_type.to_str(&self.buffer),
            &self.buffer[..self.location as usize],
        );
        (strg, self.message_type.color())
    }
    fn index_level(l: tracing::Level) -> u8 {
        match l {
            tracing::Level::TRACE => 0u8,
            tracing::Level::DEBUG => 1,
            tracing::Level::INFO => 2,
            tracing::Level::WARN => 3,
            tracing::Level::ERROR => 4,
        }
    }
    fn location_end(&self) -> usize {
        match self.message_type {
            MessageType::Normal { level: _, target } => target as usize,
            MessageType::Validation { .. } => self.buffer.len(),
        }
    }
}

struct SharedBuffer {
    buffer: std::cell::UnsafeCell<Box<[LogEntry; MAX_ENTIRES]>>,
    head: AtomicU64,
    frame: AtomicU64,
}

unsafe impl Send for SharedBuffer {}
unsafe impl Sync for SharedBuffer {}

struct ConsoleLayer {
    buffer: Arc<SharedBuffer>,
}

impl<S> Layer<S> for ConsoleLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let frame = self.buffer.frame.load(std::sync::atomic::Ordering::Relaxed);

        let entry = {
            use tracing::field::{Field, Visit};
            #[derive(Default)]
            struct Visitor {
                message: String,
                target: Option<String>,
                file: Option<String>,
                line: Option<u64>,
                location: Option<String>,
                flags: Option<DebugUtilsMessageSeverityFlagsEXT>,
                typ: Option<DebugUtilsMessageTypeFlagsEXT>,
            }
            impl Visit for Visitor {
                fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
                    match f.name() {
                        "message" => self.message = format!("{:?}", v),
                        _ => println!("Uncaptured Field: {}: {:?}", f.name(), v),
                    }
                }
                fn record_u64(&mut self, field: &tracing_core::Field, value: u64) {
                    match field.name() {
                        "message" => self.message = format!("{}", value),
                        "flags" => {
                            self.flags =
                                Some(DebugUtilsMessageSeverityFlagsEXT::from_raw(value as u32))
                        }
                        "typ" => {
                            self.typ = Some(DebugUtilsMessageTypeFlagsEXT::from_raw(value as u32))
                        }
                        "log.line" => self.line = Some(value),
                        _ => println!("Uncaptured Field: {}: {:?}", field.name(), value),
                    }
                }
                fn record_str(&mut self, field: &tracing_core::Field, value: &str) {
                    match field.name() {
                        "message" => self.message = format!("{}", value),
                        "log.target" => self.target = Some(value.to_owned()),
                        "log.file" => self.file = Some(value.to_owned()),
                        "validation_location" => self.location = Some(value.to_owned()),
                        "log.module_path" => {}
                        _ => println!("Uncaptured Field: {}: {:?}", field.name(), value),
                    }
                }
            }

            let mut m = Visitor::default();
            event.record(&mut m);

            let location = m.message.len();
            if let Some(location) = m.location {
                m.message.push_str(&location);
            } else if let Some(file) = m.file
                && let Some(line) = m.line
            {
                m.message.push_str(&format!("{}:{}", file, line));
            } else if let Some(file) = event.metadata().file()
                && let Some(line) = event.metadata().line()
            {
                m.message.push_str(&format!("{}:{}", file, line));
            } else {
                m.message.push_str(&"Unknown Location");
            };

            let target = m.message.len();

            if m.typ.is_none() {
                if let Some(target) = m.target {
                    m.message.push_str(&target);
                } else {
                    m.message.push_str(event.metadata().target());
                }
            }

            let buffer = m.message.into_boxed_str();
            LogEntry {
                buffer,
                message_type: m
                    .typ
                    .zip(m.flags)
                    .map(|(ty, serverity)| MessageType::Validation { ty, serverity })
                    .unwrap_or(MessageType::Normal {
                        level: LogEntry::index_level(*event.metadata().level()),
                        target: target as u16,
                    }),
                location: location as u16,
                frame,
            }
        };

        let idx = self
            .buffer
            .head
            .fetch_add(1, std::sync::atomic::Ordering::Acquire) as usize
            % MAX_ENTIRES;
        unsafe {
            self.buffer.buffer.get().as_mut().unwrap()[idx] = entry;
        };
    }
}

const MAX_ENTIRES: usize = 10_000;

#[derive(Resource)]
pub struct ConsoleUiState {
    pub filter_text: String,
    pub auto_scroll: bool,
    pub min_level: u8,
    pub filter_buf: Vec<u32>,
    pub filter_buf_head: usize,
    pub matches: usize,
    pub old_head: usize,
    pub inspecting: Option<(LogEntry, u32)>,
}

impl Default for ConsoleUiState {
    fn default() -> Self {
        Self {
            matches: 0,
            filter_buf_head: 0,
            old_head: 0,
            filter_buf: vec![0; MAX_ENTIRES],
            filter_text: String::new(),
            auto_scroll: false,
            min_level: 2,
            inspecting: None,
        }
    }
}

#[derive(Resource)]
struct ConsoleBuffer {
    buffer: Arc<SharedBuffer>,
}

pub struct ConsolePlugin {
    pub level: tracing::Level,
    pub also_log_to_stderr: bool,
    pub filter: String,
}

impl Default for ConsolePlugin {
    fn default() -> Self {
        Self {
            level: tracing::Level::INFO,
            also_log_to_stderr: cfg!(debug_assertions),
            filter: "wgpu=warn,naga=warn".into(),
        }
    }
}
#[derive(Default)]
struct TracyConfig(tracing_subscriber::fmt::format::DefaultFields);

impl tracing_tracy::Config for TracyConfig {
    type Formatter = tracing_subscriber::fmt::format::DefaultFields;
    fn format_fields_in_zone_name(&self) -> bool {
        true
    }
    fn formatter(&self) -> &Self::Formatter {
        &self.0
    }
    fn on_error(&self, client: &tracy_client::Client, error: &'static str) {
        client.color_message(error, 0xFF000000, 256);
    }
    fn stack_depth(&self, metadata: &tracing_core::Metadata<'_>) -> u16 {
        16
    }
}

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        let buffer = Arc::new(SharedBuffer {
            buffer: UnsafeCell::new(Box::new(std::array::from_fn(|_| LogEntry::default()))),
            head: AtomicU64::new(0),
            frame: AtomicU64::new(0),
        });

        {
            use tracing_subscriber::prelude::*;
            use tracing_subscriber::{EnvFilter, Registry};

            let filter_str = if self.filter.is_empty() {
                self.level.to_string()
            } else {
                format!("{},{}", self.level, self.filter)
            };
            let env_filter =
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&filter_str));

            let console_layer = ConsoleLayer {
                buffer: buffer.clone(),
            };

            let subscriber = Registry::default()
                .with(env_filter)
                .with(console_layer)
                .with(tracing_tracy::TracyLayer::new(TracyConfig::default()));

            if self.also_log_to_stderr {
                let fmt = tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(true)
                    .with_ansi(true);
                if tracing::subscriber::set_global_default(subscriber.with(fmt)).is_err() {
                    eprintln!(
                        "WARNING: global tracing subscriber already set — ConsolePlugin lost"
                    );
                }
            } else {
                if tracing::subscriber::set_global_default(subscriber).is_err() {
                    eprintln!(
                        "WARNING: global tracing subscriber already set — ConsolePlugin lost"
                    );
                }
            }

            let _ = tracing_log::LogTracer::init();
        }

        {
            let old = std::panic::take_hook();
            std::panic::set_hook(Box::new(move |info| {
                let loc = info
                    .location()
                    .map(|l| format!("{}:{}", l.file(), l.line()));
                tracing::error!(
                    target: "panic",
                    "PANIC at {}: {}",
                    loc.as_deref().unwrap_or("?"),
                    info
                );
                old(info);
            }));
        }

        app.insert_resource(ConsoleBuffer { buffer })
            .insert_resource(ConsoleUiState::default())
            .add_systems(Update, (console_window, console_inspector));

        app.add_systems(Last, frame_mark);
    }
}

fn console_window(
    log: Res<ConsoleBuffer>,
    mut ui_state: ResMut<ConsoleUiState>,
    mut ui_builder: UiBuilder,
) {
    log.buffer
        .frame
        .fetch_add(1, std::sync::atomic::Ordering::Release);

    ui_builder.build("Console", |ui| {
        ui.horizontal();
        ui.text("Level:");

        let prev = ui_state.min_level;
        ui_state.min_level =
            ui.dropdown(id!(), ui_state.min_level as usize, &LogEntry::LEVEL_NAMES) as u8;
        let min_level_changed = prev != ui_state.min_level;

        ui.text("Filter:");
        let filter_changed = ui.text_input(id!(), &mut ui_state.filter_text, 300.0);

        ui.text("Auto-scroll");
        ui_state.auto_scroll = ui.checkbox(ui_state.auto_scroll);

        let head = log.buffer.head.load(std::sync::atomic::Ordering::Relaxed) as usize;
        if ui.button("Clear log") {
            log.buffer.head.store(0, Ordering::Relaxed);
            ui_state.filter_buf_head = 0;
            ui_state.old_head = head.saturating_sub(MAX_ENTIRES);
            ui_state.matches = 0;
        }

        ui.vertical();

        let mut rect = ui.clip_rect;
        rect.min.y = ui.cursor.y;
        let size = rect.size()
            - (UiContext::WINDOW_PAD.as_vec2() + UiContext::ROUNDING.max(UiContext::BORDER) as f32)
                * 2.0;

        let filter_lc = ui_state.filter_text.to_lowercase();
        let min_level = ui_state.min_level;
        let filter = |entry: &LogEntry| {
            // let entry_level_idx = entry.level

            // let filter = entry.message.to_lowercase().contains(&filter_lc)
            //     || entry.target.to_lowercase().contains(&filter_lc);

            // let level = entry_level_idx >= min_level;

            // if filter && level {
            //     Some(entry.format())
            // } else {
            //     None
            // }
            // Some(entry.format())
            true
        };

        if filter_changed || min_level_changed {
            ui_state.filter_buf_head = 0;
            ui_state.old_head = head.saturating_sub(MAX_ENTIRES);
            ui_state.matches = 0;
        }

        let buffer = unsafe { log.buffer.buffer.get().as_ref().unwrap() };

        for k in ui_state.old_head..head {
            let idx = k % MAX_ENTIRES;

            if filter(&buffer[idx]) {
                let filter_head = ui_state.filter_buf_head;
                ui_state.filter_buf[filter_head] = idx as u32;
                ui_state.filter_buf_head = (ui_state.filter_buf_head + 1) % MAX_ENTIRES;
                ui_state.matches += 1;
            }
        }
        ui_state.old_head = head;
        let len = ui_state.matches.min(MAX_ENTIRES);
        ui.text_container(
            id!(),
            size,
            ui_state.auto_scroll,
            |ui, i| {
                let offset = if ui_state.matches < MAX_ENTIRES {
                    0
                } else {
                    ui_state.filter_buf_head
                };
                let idx = (offset + i) % MAX_ENTIRES;
                let entry_idx = &ui_state.filter_buf[idx];
                let entry = buffer[*entry_idx as usize].clone();
                let max_len = ((size.x
                    / (UiContext::ATLAS_CELL_SIZE.x + UiContext::CHARACTER_ADVANCE_WIDTH) as f32)
                    .floor() as usize)
                    .saturating_sub(3);
                let str;
                let (message, mut level_color) = entry.format();
                let message = if message.len() > max_len {
                    str = format!("{}...", &message[..message.ceil_char_boundary(max_len)]);
                    &str
                } else {
                    &message
                };
                let size = UiContext::text_size(message);
                let rect = Rect::from_corners(ui.cursor, ui.cursor + size);
                if ui.hoverd(rect)
                    || ui_state
                        .inspecting
                        .as_ref()
                        .is_some_and(|i| i.1 == *entry_idx)
                {
                    level_color *= 1.4;
                }
                ui.colored_text(message, level_color);
                if ui.prev_element_hoverd() && ui.ctx.input.primary_pressed {
                    ui_state.inspecting = Some((entry, *entry_idx));
                }
            },
            len,
        );
    });
}
fn console_inspector(mut ui: UiBuilder, ui_state: Res<ConsoleUiState>) {
    ui.build("Inspect Log Entry", |ui| {
        let Some((entry, _)) = &ui_state.inspecting else {
            ui.text("No entry selected");
            return;
        };

        ui.text(format!("Frame: {}", entry.frame));
        match entry.message_type {
            MessageType::Validation { ty, serverity } => {
                ui.text(format!("Type: {:?}", ty));
                ui.text(format!("Severity: {:?}", serverity));
            }
            MessageType::Normal { level, target } => {
                ui.text(format!("Level: {}", LogEntry::LEVEL_NAMES[level as usize]));
                ui.text(format!(
                    "Target: {}",
                    &entry.buffer[target as usize..entry.buffer.len()]
                ));
            }
        }

        let width = ui.ctx.window_rect.width() - UiContext::INDENT.x as f32 - 30.0;
        ui.collapsable(true, id!(), "Message", |ui| {
            ui.wrapping_text(
                &entry.buffer[..(entry.location as usize)],
                width,
                UiContext::TEXT,
            );
        });
        let location = &entry.buffer[entry.location as usize..entry.location_end()];
        ui.collapsable(true, id!(), "Location", |ui| {
            ui.wrapping_text(format!("{}", location,), width, UiContext::ACENT);
            if ui.prev_element_hoverd() && ui.ctx.input.primary_pressed && ui.ctx.ctrl {
                let mut cmd = std::process::Command::new("zeditor");
                cmd.arg(location);
                cmd.spawn().unwrap();
            }
        });
    });
}
fn frame_mark() {
    tracy_client::frame_mark();
}
