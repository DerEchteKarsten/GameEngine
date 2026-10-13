//! Editor log console and the tracing subscriber: the console's capture layer (logs and Vulkan validation), with `profiling` also the profiler's span layer, plus the console UI.
use bevy::{ecs::reflect::ReflectResource, reflect::Reflect};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};

use bevy::app::{App, Plugin, Update};
use bevy::ecs::prelude::*;
use glam::{Vec2, Vec4};
use lava::state::raw_vulkan::{DebugUtilsMessageSeverityFlagsEXT, DebugUtilsMessageTypeFlagsEXT};
use tracing::Level;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{Layer, fmt};

use crate::id;
use crate::ui::UiContext;
use crate::ui::builder::UiBuilder;
use crate::ui::widgets::{Ellipsis, fit_text};
use crate::ui::window::{BorderSettings, DrawSettings};

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
    /// The level the console filters by: a validation message's severity as a level.
    fn level(&self) -> u8 {
        match self.message_type {
            MessageType::Normal { level, .. } => level,
            MessageType::Validation { serverity, .. } => {
                if serverity.contains(DebugUtilsMessageSeverityFlagsEXT::ERROR) {
                    4
                } else if serverity.contains(DebugUtilsMessageSeverityFlagsEXT::WARNING) {
                    3
                } else if serverity.contains(DebugUtilsMessageSeverityFlagsEXT::INFO) {
                    2
                } else {
                    0
                }
            }
        }
    }
    /// Level at least `min_level`, and `filter` somewhere in the text, case-insensitively.
    fn passes(&self, min_level: u8, filter: &str) -> bool {
        self.level() >= min_level
            && (filter.is_empty()
                || self
                    .buffer
                    .as_bytes()
                    .windows(filter.len())
                    .any(|window| window.eq_ignore_ascii_case(filter.as_bytes())))
    }
}

/// The frame the console counts, stamped on every entry.
static FRAME: AtomicU64 = AtomicU64::new(0);

/// Sends every event to the console; the UI thread owns the entries (`ConsoleLog`). The
/// channel holds `MAX_ENTRIES`; when the console doesn't keep up, events are dropped from the
/// console only (stderr still gets them).
struct ConsoleLayer {
    sender: SyncSender<LogEntry>,
}

impl<S> Layer<S> for ConsoleLayer
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let frame = FRAME.load(Ordering::Relaxed);

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
            // Fields the console doesn't show are dropped: logging about them from inside
            // the layer would recurse.
            impl Visit for Visitor {
                fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
                    if f.name() == "message" {
                        self.message = format!("{:?}", v);
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
                        _ => {}
                    }
                }
                fn record_str(&mut self, field: &tracing_core::Field, value: &str) {
                    match field.name() {
                        "message" => self.message = format!("{}", value),
                        "log.target" => self.target = Some(value.to_owned()),
                        "log.file" => self.file = Some(value.to_owned()),
                        "validation_location" => self.location = Some(value.to_owned()),
                        _ => {}
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
                m.message.push_str("Unknown Location");
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

        let _ = self.sender.try_send(entry);
    }
}

/// Entries the console keeps; older ones are dropped.
const MAX_ENTRIES: usize = 10_000;

#[derive(Resource, Reflect)]
#[reflect(Resource)]
pub struct ConsoleUiState {
    pub filter_text: String,
    pub auto_scroll: bool,
    pub min_level: u8,
    /// Numbers (`ConsoleLog::number`) of the entries that pass the filter, oldest first.
    #[reflect(ignore, clone)]
    pub matches: VecDeque<usize>,
    /// Entries up to this number have been filtered.
    pub filtered: usize,
    #[reflect(ignore, clone)]
    pub inspecting: Option<(LogEntry, usize)>,
}

impl Default for ConsoleUiState {
    fn default() -> Self {
        Self {
            matches: VecDeque::new(),
            filtered: 0,
            filter_text: String::new(),
            auto_scroll: false,
            min_level: 2,
            inspecting: None,
        }
    }
}

/// The console's entries, drained from the layer's channel every frame. Entry `n` (its
/// number, counting every entry ever received) is `entries[n - first]`.
#[derive(Resource)]
pub struct ConsoleLog {
    /// Locked only by `console_window`; a `Receiver` isn't `Sync` on its own.
    receiver: Mutex<Receiver<LogEntry>>,
    entries: VecDeque<LogEntry>,
    /// The number of `entries[0]`.
    first: usize,
    /// Entries before this number were cleared.
    cleared: usize,
}

impl ConsoleLog {
    fn get(&self, number: usize) -> Option<&LogEntry> {
        self.entries.get(number.checked_sub(self.first)?)
    }
    fn end(&self) -> usize {
        self.first + self.entries.len()
    }
}

pub struct ConsolePlugin;

/// The receiving end of the console layer [`register_tracing`] installs, until
/// [`ConsolePlugin`] takes it.
static LOG: OnceLock<Mutex<Option<Receiver<LogEntry>>>> = OnceLock::new();

/// Installs the global tracing subscriber: logs go to the console and stderr, spans (with the
/// `profiling` feature) to the profiler. `RUST_LOG` overrides the level. Call it
/// first thing in `main`: bevy creates a system's span when the system is added, and a span
/// created before there is a subscriber never records. [`ConsolePlugin`] calls it if `main` didn't; later calls do nothing.
pub fn register_tracing() {
    LOG.get_or_init(|| {
        use tracing_subscriber::prelude::*;
        use tracing_subscriber::{EnvFilter, Registry};

        let (sender, receiver) = sync_channel(MAX_ENTRIES);
        let env_filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("debug,wgpu=warn,naga=warn"));
        let fmt = tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_target(true)
            .with_ansi(true);
        let subscriber = Registry::default()
            .with(env_filter)
            .with(ConsoleLayer { sender })
            .with(fmt);
        #[cfg(feature = "profiling")]
        let subscriber = subscriber.with(crate::profiler::spans::ProfileLayer);
        if tracing::subscriber::set_global_default(subscriber).is_err() {
            eprintln!("WARNING: global tracing subscriber already set — the console stays empty");
        }
        let _ = tracing_log::LogTracer::init();
        Mutex::new(Some(receiver))
    });
}

impl Plugin for ConsolePlugin {
    fn build(&self, app: &mut App) {
        register_tracing();
        let receiver = LOG
            .get()
            .expect("registered above")
            .lock()
            .unwrap()
            .take()
            .expect("the console plugin is added once");

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

        app.insert_resource(ConsoleLog {
            receiver: Mutex::new(receiver),
            entries: VecDeque::with_capacity(MAX_ENTRIES),
            first: 0,
            cleared: 0,
        })
        .insert_resource(ConsoleUiState::default())
        .add_systems(Update, (console_window, console_inspector));
    }
}

fn console_window(
    mut log: ResMut<ConsoleLog>,
    mut ui_state: ResMut<ConsoleUiState>,
    mut ui_builder: UiBuilder,
) {
    FRAME.fetch_add(1, Ordering::Relaxed);
    let log = &mut *log;
    while let Ok(entry) = log.receiver.get_mut().unwrap().try_recv() {
        if log.entries.len() == MAX_ENTRIES {
            log.entries.pop_front();
            log.first += 1;
        }
        log.entries.push_back(entry);
    }

    ui_builder.build("Console", |ui| {
        let (min_level_changed, filter_changed) = ui.row(|ui| {
            ui.label("Level:");
            let prev = ui_state.min_level;
            ui_state.min_level =
                ui.dropdown(id!(), ui_state.min_level as usize, &LogEntry::LEVEL_NAMES) as u8;

            ui.label("Filter:");
            let filter_changed = ui.text_input(id!(), &mut ui_state.filter_text, 300.0);

            ui_state.auto_scroll = ui.checkbox("Auto-scroll", ui_state.auto_scroll);

            if ui.button("Clear log") {
                log.cleared = log.end();
            }
            (prev != ui_state.min_level, filter_changed)
        });
        let size = ui.available_size();

        let ui_state = &mut *ui_state;
        let from = log.first.max(log.cleared);
        if filter_changed || min_level_changed {
            ui_state.matches.clear();
            ui_state.filtered = from;
        }
        // Matches of entries that are gone, and new entries.
        while ui_state.matches.front().is_some_and(|&n| n < from) {
            ui_state.matches.pop_front();
        }
        for number in ui_state.filtered.max(from)..log.end() {
            let entry = log.get(number).unwrap();
            if entry.passes(ui_state.min_level, &ui_state.filter_text) {
                ui_state.matches.push_back(number);
            }
        }
        ui_state.filtered = log.end();

        ui.list(
            id!(),
            size,
            ui_state.matches.len(),
            ui_state.auto_scroll,
            |ui, i| {
                let number = ui_state.matches[i];
                let entry = log.get(number).unwrap();
                let (message, color) = entry.format();
                let message = fit_text(&message, ui.available_size().x, Ellipsis::End);
                let inspected = ui_state
                    .inspecting
                    .as_ref()
                    .is_some_and(|i| i.1 == number);
                if ui.selectable(&message, color, inspected) {
                    ui_state.inspecting = Some((entry.clone(), number));
                }
            },
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

        ui.collapsable(true, id!(), "Message", |ui| {
            let width = ui.available_size().x;
            ui.wrapping_text(&entry.buffer[..(entry.location as usize)], width, UiContext::TEXT);
        });
        let location = &entry.buffer[entry.location as usize..entry.location_end()];
        ui.collapsable(true, id!(), "Location", |ui| {
            let width = ui.available_size().x;
            ui.wrapping_text(location, width, UiContext::ACENT);
            // Ctrl-click opens the location in `$VISUAL`, else `$EDITOR`, else zeditor.
            if ui.prev_clicked() && ui.ctx.ctrl {
                let editor = std::env::var("VISUAL")
                    .or_else(|_| std::env::var("EDITOR"))
                    .unwrap_or_else(|_| "zeditor".to_string());
                if let Err(err) = std::process::Command::new(&editor).arg(location).spawn() {
                    tracing::error!("failed to open {location} in {editor}: {err}");
                }
            }
        });
    });
}
