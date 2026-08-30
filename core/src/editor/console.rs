use std::cell::UnsafeCell;
use std::fmt::Debug;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::app::{App, Last, Plugin, PostUpdate};
use bevy::ecs::prelude::*;
use glam::Vec4;
use tracing::Level;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, fmt};

use crate::id;
use crate::ui::UiContext;
use crate::ui::builder::UiBuilder;

// #[global_allocator]
// static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
//     tracy_client::ProfiledAllocator::new(std::alloc::System, 100);

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub buffer: Box<str>,
    pub message: u16,
    pub level: u8,
    pub message_type: MessageType,
    pub frame: u64,
}

#[derive(Debug, Clone)]
pub struct Serverity(pub u8);

impl Serverity {
    pub const VERBOSE: Self = Self(0);
    pub const INFO: Self = Self(1);
    pub const WARNING: Self = Self(2);
    pub const ERROR: Self = Self(4);
}

#[derive(Debug, Clone)]
pub struct ValidationMessageType(pub u8);

impl ValidationMessageType {
    pub const GENERAL: Self = Self(0);
    pub const VALIDATION: Self = Self(1);
    pub const PERFORMANCE: Self = Self(2);
}

#[derive(Debug, Clone)]
pub enum MessageType {
    Validation {
        ty: ValidationMessageType,
        serverity: Serverity,
    },
    Normal {
        target: u16,
        location: u16,
    },
}

impl LogEntry {
    const LEVEL_NAMES: [&str; 5] = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
    const LEVEL_COLORS: [Vec4; 5] = [
        UiContext::TRACE,
        UiContext::DEBUG,
        UiContext::INFO,
        UiContext::WARN,
        UiContext::ERROR,
    ];

    fn format(&self) -> (String, u32) {
        let entry_level_idx = Self::index_level(self.level);
        let strg = format!(
            "[{:>6}] [{:<5}] {}: {}",
            self.frame,
            Self::LEVEL_NAMES[entry_level_idx as usize],
            self.target,
            self.message,
        );
        (strg, entry_level_idx as u32)
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
        let message = {
            use tracing::field::{Field, Visit};

            impl Visit for LogEntry {
                fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
                    if f.name() == "message" {
                        self.message = format!("{v:?}");
                    }
                    if f.name() == "message" {
                        self.message = format!("{v:?}");
                    }
                    println!("Debug: {}: {:?}", f.name(), v);
                }
            }
            println!("{:#?}", event.metadata());
            let mut m = LogEntry::default();
            event.record(&mut m);
            println!("----------------------------------------");
            m.message
        };

        let frame = self.buffer.frame.load(std::sync::atomic::Ordering::Relaxed);

        let idx = self
            .buffer
            .head
            .fetch_add(1, std::sync::atomic::Ordering::Acquire) as usize
            % MAX_ENTIRES;
        unsafe {
            self.buffer.buffer.get().as_mut().unwrap()[idx] = LogEntry {
                level: *event.metadata().level(),
                target: event.metadata().target().to_owned(),
                message,
                frame,
            }
        };
    }
}

const MAX_ENTIRES: usize = 10_000;

#[derive(Resource)]
pub struct ConsoleUiState {
    pub filter_text: String,
    pub auto_scroll: bool,
    pub min_level: u8,
    pub filter_buf: Vec<(String, u32)>,
    pub filter_buf_head: usize,
    pub matches: usize,
    pub old_head: usize,
}

impl Default for ConsoleUiState {
    fn default() -> Self {
        Self {
            matches: 0,
            filter_buf_head: 0,
            old_head: 0,
            filter_buf: vec![(String::new(), 0); MAX_ENTIRES],
            filter_text: String::new(),
            auto_scroll: true,
            min_level: 2,
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
            buffer: UnsafeCell::new(Box::new(std::array::from_fn(|_| LogEntry {
                frame: 0,
                level: Level::TRACE,
                message: String::new(),
                target: String::new(),
            }))),
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
            .add_systems(PostUpdate, render_console_window.chain());

        app.add_systems(Last, frame_mark);
    }
}

fn render_console_window(
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
            let entry_level_idx = LogEntry::index_level(entry.level);

            let filter = entry.message.to_lowercase().contains(&filter_lc)
                || entry.target.to_lowercase().contains(&filter_lc);

            let level = entry_level_idx >= min_level;

            if filter && level {
                Some(entry.format())
            } else {
                None
            }
        };

        if filter_changed || min_level_changed {
            ui_state.filter_buf_head = 0;
            ui_state.old_head = head.saturating_sub(MAX_ENTIRES);
            ui_state.matches = 0;
        }

        let buffer = unsafe { log.buffer.buffer.get().as_ref().unwrap() };

        for k in ui_state.old_head..head {
            let idx = k % MAX_ENTIRES;

            if let Some(entry) = filter(&buffer[idx]) {
                let filter_head = ui_state.filter_buf_head;
                ui_state.filter_buf[filter_head] = entry;
                ui_state.filter_buf_head = (ui_state.filter_buf_head + 1) % MAX_ENTIRES;
                ui_state.matches += 1;
            }
        }
        ui_state.old_head = head;
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
                let entry = &ui_state.filter_buf[idx];
                ui.colored_text(&entry.0, LogEntry::LEVEL_COLORS[entry.1 as usize]);
            },
            ui_state.matches.min(MAX_ENTIRES),
        );
    });
}
fn frame_mark() {
    tracy_client::frame_mark();
}
