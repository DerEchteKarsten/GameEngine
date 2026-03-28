// console_plugin/src/lib.rs
//
// Drop-in replacement for Bevy's LogPlugin.
// - Captures tracing/log events into a Bevy Resource (ConsoleLog)
// - Tags each entry with the schedule that was active when it was emitted
// - Renders an ImGui window via ResMut<UiBuilder>
// - Re-implements all the setup LogPlugin normally does
//   (tracy allocator, tracing subscriber, panic hook, etc.)

use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use bevy::app::{
    App, First, Last, Plugin, PostStartup, PostUpdate, PreStartup, PreUpdate, Startup, Update,
};
use bevy::ecs::prelude::*;
use bevy::ecs::schedule::{InternedScheduleLabel, ScheduleLabel};
use imgui::{ComboBoxFlags, SliderFlags};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;

use crate::render::{ExtractSchedule, Render, RenderStartup};
use crate::ui::UiBuilder;

#[cfg(feature = "trace_tracy_memory")]
#[global_allocator]
static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(std::alloc::System, 100);

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
    pub frame: u64,
}

type FrameCounter = Arc<std::sync::atomic::AtomicU64>;

#[derive(Clone)]
struct SharedBuffer(Arc<Mutex<Vec<LogEntry>>>);

struct ConsoleLayer {
    buffer: SharedBuffer,
    frame: FrameCounter,
}

impl<S> Layer<S> for ConsoleLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let message = {
            use tracing::field::{Field, Visit};
            struct Msg(String);
            impl Visit for Msg {
                fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
                    if f.name() == "message" {
                        self.0 = format!("{v:?}");
                    }
                }
                fn record_str(&mut self, f: &Field, v: &str) {
                    if f.name() == "message" {
                        self.0 = v.to_owned();
                    }
                }
            }
            let mut m = Msg(String::new());
            event.record(&mut m);
            m.0
        };

        let frame = self.frame.load(std::sync::atomic::Ordering::Relaxed);

        if let Ok(mut buf) = self.buffer.0.lock() {
            buf.push(LogEntry {
                level: *event.metadata().level(),
                target: event.metadata().target().to_owned(),
                message,
                frame,
            });
        }
    }
}

/// Cached per-span data stored in span extensions.
#[derive(Debug)]
struct SpanFields {
    name: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// ECS Resources
// ─────────────────────────────────────────────────────────────────────────────

/// All captured log entries.
#[derive(Resource, Default)]
pub struct ConsoleLog {
    pub entries: Vec<LogEntry>,
    frame_counter: u64,
}

/// UI state for the console window.
#[derive(Resource)]
pub struct ConsoleUiState {
    pub open: bool,
    pub filter_text: String,
    pub auto_scroll: bool,
    pub clear_requested: bool,
    pub scroll_to_bottom: bool,
    pub max_entries: usize,
    /// Minimum level index: 0=TRACE 1=DEBUG 2=INFO 3=WARN 4=ERROR
    pub min_level: u8,
}

impl Default for ConsoleUiState {
    fn default() -> Self {
        Self {
            open: true,
            filter_text: String::new(),
            auto_scroll: true,
            clear_requested: false,
            scroll_to_bottom: false,
            max_entries: 10_000,
            min_level: 2,
        }
    }
}

// Internal resource bridging shared state into ECS.
#[derive(Resource)]
struct ConsoleBuffer {
    buffer: SharedBuffer,
    frame: FrameCounter,
}

// ─────────────────────────────────────────────────────────────────────────────
// Plugin
// ─────────────────────────────────────────────────────────────────────────────

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

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        let buffer: SharedBuffer = SharedBuffer(Arc::new(Mutex::new(Vec::new())));
        let frame: FrameCounter = Arc::new(std::sync::atomic::AtomicU64::new(0));

        // ── Tracing subscriber ────────────────────────────────────────────────
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
                frame: frame.clone(),
            };

            // Registry is required — it is the subscriber implementation that
            // provides LookupSpan, which ConsoleLayer depends on.
            let subscriber = Registry::default().with(env_filter).with(console_layer);

            #[cfg(feature = "trace")]
            let subscriber = subscriber.with(tracing_tracy::TracyLayer::new());

            if self.also_log_to_stderr {
                let fmt = tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_target(true)
                    .with_ansi(true);
                let _ = tracing::subscriber::set_global_default(subscriber.with(fmt));
            } else {
                let _ = tracing::subscriber::set_global_default(subscriber);
            }

            let _ = tracing_log::LogTracer::init();
        }

        // ── Panic hook ────────────────────────────────────────────────────────
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

        // ── ECS ───────────────────────────────────────────────────────────────
        app.insert_resource(ConsoleBuffer { buffer, frame })
            .insert_resource(ConsoleLog::default())
            .insert_resource(ConsoleUiState::default())
            .add_systems(First, tick_frame_counter)
            .add_systems(
                PostUpdate,
                (flush_pending_logs, render_console_window).chain(),
            );
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Systems
// ─────────────────────────────────────────────────────────────────────────────

fn tick_frame_counter(mut log: ResMut<ConsoleLog>, cb: Res<ConsoleBuffer>) {
    log.frame_counter = log.frame_counter.wrapping_add(1);
    cb.frame
        .store(log.frame_counter, std::sync::atomic::Ordering::Relaxed);
}

fn flush_pending_logs(
    cb: Res<ConsoleBuffer>,
    mut log: ResMut<ConsoleLog>,
    mut state: ResMut<ConsoleUiState>,
) {
    if state.clear_requested {
        log.entries.clear();
        state.clear_requested = false;
        state.scroll_to_bottom = true;
    }

    let Ok(mut pending) = cb.buffer.0.lock() else {
        return;
    };
    let max = state.max_entries;
    log.entries.extend(pending.drain(..));

    if log.entries.len() > max {
        let excess = log.entries.len() - max;
        log.entries.drain(..excess);
    }
}

fn render_console_window(
    log: Res<ConsoleLog>,
    mut ui_state: ResMut<ConsoleUiState>,
    mut ui_builder: ResMut<UiBuilder>,
) {
    let Some(ui) = ui_builder.ui() else {
        return;
    };

    if !ui_state.open {
        return;
    }

    let level_names = ["TRACE", "DEBUG", "INFO", "WARN", "ERROR"];
    let level_colors: [[f32; 4]; 5] = [
        [0.6, 0.6, 0.6, 1.0], // trace – grey
        [0.4, 0.8, 1.0, 1.0], // debug – cyan
        [1.0, 1.0, 1.0, 1.0], // info  – white
        [1.0, 0.8, 0.2, 1.0], // warn  – yellow
        [1.0, 0.3, 0.3, 1.0], // error – red
    ];
    ui.window("Console").build(|| {
        // ── Toolbar ──────────────────────────────────────────────────────────

        // --- Level filter ---
        ui.text("Level:");
        ui.same_line();
        // -1.0 means "fill to end of available width" — but we need room for the rest,
        // so we calculate: push a fraction of the remaining space.
        // We have 4 stretchy widgets. Leave label widths as fixed, stretch the inputs.
        let available = ui.content_region_avail()[0];
        // Approximate fixed costs: "Level:" "Schedule:" "Clear" "Auto-scroll" "Clear log" + separators
        let fixed_cost = 420.0; // tune this to taste
        let n_stretchy = 3.0; // level slider, schedule combo, text filter
        let stretch_w = ((available - fixed_cost) / n_stretchy).max(40.0);

        ui.set_next_item_width(stretch_w);
        let mut min_level = ui_state.min_level as i32;
        if ui
            .slider_config("##level", 0, 4)
            .display_format(level_names[ui_state.min_level as usize])
            .build(&mut min_level)
        {
            ui_state.min_level = min_level as u8;
        }

        ui.same_line();
        ui.text("Filter:");

        // --- Text filter ---
        ui.same_line();
        ui.set_next_item_width(stretch_w);
        ui.input_text("##filter", &mut ui_state.filter_text).build();

        ui.same_line();
        if ui.button("Clear") {
            ui_state.filter_text.clear();
        }
        ui.same_line();
        ui.checkbox("Auto-scroll", &mut ui_state.auto_scroll);
        ui.same_line();
        if ui.button("Clear log") {
            ui_state.clear_requested = true;
        }

        ui.separator();

        // ── Log entries ───────────────────────────────────────────────────────
        let scroll_region_height = ui.content_region_avail()[1] - 4.0;
        ui.child_window("##scroll")
            .size([0.0, scroll_region_height])
            .horizontal_scrollbar(true)
            .build(|| {
                let filter_lc = ui_state.filter_text.to_lowercase();

                for entry in &log.entries {
                    // Level filter
                    let entry_level_idx = match entry.level {
                        tracing::Level::TRACE => 0u8,
                        tracing::Level::DEBUG => 1,
                        tracing::Level::INFO => 2,
                        tracing::Level::WARN => 3,
                        tracing::Level::ERROR => 4,
                    };
                    if entry_level_idx < ui_state.min_level {
                        continue;
                    }

                    // Text filter
                    if !filter_lc.is_empty() {
                        let haystack =
                            format!("{} {}", entry.target, entry.message,).to_lowercase();
                        if !haystack.contains(&filter_lc) {
                            continue;
                        }
                    }

                    let color = level_colors[entry_level_idx as usize];
                    let _col = ui.push_style_color(imgui::StyleColor::Text, color);

                    // [frame] [LEVEL] [Schedule] target: message
                    ui.text(&format!(
                        "[{:>6}] [{:<5}] {}: {}",
                        entry.frame,
                        level_names[entry_level_idx as usize],
                        entry.target,
                        entry.message,
                    ));
                }

                // Auto-scroll to bottom
                if ui_state.auto_scroll && ui.scroll_y() >= ui.scroll_max_y() - 4.0 {
                    ui.set_scroll_here_y_with_ratio(1.0);
                }
            });
    });
}
