//! The editor's Profiler tab: on/off switch, live frame/CPU/GPU time graphs, and for the frame clicked in them a zoomable timeline of its CPU spans and GPU scopes plus its GPU scope, shader time and CPU span tables (spans filterable by kind); GPU memory.
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use bevy::{
    ecs::system::{Local, ResMut},
    log::{error, info},
    math::Rect,
};
use glam::{Vec2, Vec4};
use lava::bindless::BindlessHandle;

use lava::profiling::MemoryReport;

use crate::{
    id,
    profiler::{
        FrameProfile, FrameTimes, Profiler, ThreadSpans,
        capture::SpanKind,
        report::{cpu_spans, gpu_scopes, shader_times},
    },
    ui::{
        UiContext,
        builder::{UiBuilder, UiWindowBuilder},
        from_pos_size,
        window::{DrawSettings, Drawable},
    },
};

/// Frames the averages cover.
const RECENT_FRAMES: usize = 120;
const ROW_HEIGHT: f32 = UiContext::ATLAS_CELL_SIZE.y as f32 + 2.0;
const CHAR_WIDTH: f32 = UiContext::ATLAS_CELL_SIZE.x as f32;

#[derive(Default)]
pub(crate) struct ProfilerView {
    /// The visible part of the timeline in ns since the frame started; `None` shows all of it.
    range: Option<(f64, f64)>,
    drag_from: Option<f32>,
    /// The selected frame's header and table rows, and its number.
    tables: Option<(u32, Tables)>,
    /// Index into `SPAN_FILTERS`.
    span_filter: usize,
}

const SPAN_FILTERS: [&str; 4] = ["All spans", "Systems", "Schedules", "Other spans"];
/// Characters of a span name the list shows; hovering it shows all of it.
const NAME_CHARS: usize = 56;

struct SpanRow {
    kind: SpanKind,
    name: &'static str,
    cells: [String; 6],
}

struct Tables {
    header: String,
    scopes: Vec<[String; 6]>,
    shaders: Vec<[String; 5]>,
    spans: Vec<SpanRow>,
}

fn tables(frame: &FrameProfile) -> Tables {
    let mut busy: Vec<_> = frame.threads.iter().map(|t| (t.thread, t.busy_ns)).collect();
    busy.sort_by_key(|(_, ns)| std::cmp::Reverse(*ns));
    let busy = busy.iter().take(3).map(|(t, ns)| format!("{t} {:.3}", ms(*ns)));
    let header = format!(
        "Frame #{}: {:.3} ms, CPU {} ms, GPU {}{}",
        frame.frame,
        ms(frame.frame_ns()),
        busy.collect::<Vec<_>>().join(" / "),
        frame
            .gpu_ns()
            .map_or("-".into(), |ns| format!("{:.3} ms", ms(ns))),
        if frame.complete {
            ""
        } else {
            " (some of its spans were overwritten)"
        }
    );
    let frames = std::slice::from_ref(frame);
    let gpu = frame.gpu_ns().map_or(0.0, ms);
    let scopes = gpu_scopes(frames).into_iter().map(|scope| {
        let scope_ms: f64 = scope.ms.iter().sum();
        let stats = scope.stats;
        [
            format!("{}{}", "  ".repeat(scope.depth as usize), scope.name),
            format!("{scope_ms:.3}"),
            format!("{:.1}", scope_ms / gpu.max(f64::EPSILON) * 100.0),
            stats.map_or(String::new(), |s| s.fragment.to_string()),
            stats.map_or(String::new(), |s| s.compute.to_string()),
            stats.map_or(String::new(), |s| s.mesh.to_string()),
        ]
    });
    let spans = cpu_spans(frames).into_iter().map(|span| SpanRow {
        kind: span.kind,
        name: span.name,
        cells: [
            span.name.chars().take(NAME_CHARS).collect(),
            span.kind.label().to_string(),
            format!("{:.3}", ms(span.total_ns)),
            span.calls.to_string(),
            format!("{:.3}", ms(span.max_ns)),
            span.thread.unwrap_or("(several)").to_string(),
        ],
    });
    let shaders = shader_times(frames).into_iter().map(|time| {
        [
            format!("{} {}", time.pass, time.stage),
            format!("{:.1}", time.share * 100.0),
            format!("{:.0}", time.subgroups),
            format!("{:.3}", time.ticks / 1e6),
            format!("{:.0}", time.ticks / time.subgroups.max(1.0)),
        ]
    });
    Tables {
        header,
        scopes: scopes.collect(),
        shaders: shaders.collect(),
        spans: spans.collect(),
    }
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

fn mean(values: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = values.fold((0.0, 0), |(sum, n), v| (sum + v, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn name_color(name: &str) -> Vec4 {
    let mut hash = DefaultHasher::new();
    name.hash(&mut hash);
    let [r, g, b, ..] = hash.finish().to_le_bytes();
    let channel = |c: u8| 0.35 + 0.5 * c as f32 / 255.0;
    Vec4::new(channel(r), channel(g), channel(b), 1.0)
}

/// Width left of the tab's visible area. Not `remaining_width`, which includes last frame's
/// content, so anything sized by it grows a little every frame.
fn visible_width(ui: &UiWindowBuilder) -> f32 {
    (ui.clip_rect.max.x - ui.cursor.x - 20.0).max(0.0)
}

/// A row of cells at fixed x offsets, in characters.
fn row(ui: &mut UiWindowBuilder, columns: &[usize], cells: &[String], color: Vec4) {
    ui.horizontal();
    let x = ui.cursor.x;
    for (column, cell) in columns.iter().zip(cells) {
        ui.cursor.x = x + *column as f32 * CHAR_WIDTH;
        ui.colored_text(cell, color);
    }
    ui.vertical();
}

struct Bar {
    name: &'static str,
    wait: bool,
    depth: u16,
    start_ns: u64,
    end_ns: u64,
}

fn cpu_bars(thread: &ThreadSpans) -> impl Iterator<Item = Bar> {
    thread.spans.iter().map(|span| Bar {
        name: span.name,
        wait: span.kind == SpanKind::Wait,
        depth: span.depth,
        start_ns: span.start_ns,
        end_ns: span.end_ns,
    })
}

fn gpu_bars(frame: &FrameProfile) -> impl Iterator<Item = Bar> {
    frame.gpu.iter().map(|scope| Bar {
        name: scope.name,
        wait: false,
        depth: scope.depth,
        start_ns: frame.gpu_submit_ns + scope.start_ns,
        end_ns: frame.gpu_submit_ns + scope.end_ns,
    })
}

/// Rows a lane takes: its name and one per span depth.
fn lane_rows(bars: impl Iterator<Item = Bar>) -> u16 {
    bars.map(|b| b.depth + 2).max().unwrap_or(1)
}

/// Draws the timeline's lanes one below the other.
struct Lanes {
    area: Rect,
    clip: Rect,
    cursor: Option<Vec2>,
    /// Timeline ns at the left edge and per pixel.
    start: f64,
    ns_per_px: f64,
    frame_start: u64,
    y: f32,
    hovered: Option<String>,
}

impl Lanes {
    fn draw(&mut self, ui: &mut UiWindowBuilder, lane: &str, bars: impl Iterator<Item = Bar>) {
        let viewport = ui.ctx.viewport_size;
        let area = self.area;
        ui.ctx.window.draw_text(
            Vec2::new(area.min.x, self.y),
            UiContext::TEXT_DIM,
            lane,
            viewport,
            self.clip,
            false,
        );
        self.y += ROW_HEIGHT;
        let x_of = |ns: u64| {
            area.min.x + ((ns as f64 - self.frame_start as f64 - self.start) / self.ns_per_px) as f32
        };
        let mut rows = 0;
        for bar in bars {
            rows = rows.max(bar.depth + 1);
            let x0 = x_of(bar.start_ns).max(area.min.x);
            let x1 = x_of(bar.end_ns).min(area.max.x).max(x0 + 1.0);
            if x0 > area.max.x || x1 < area.min.x {
                continue;
            }
            let top = self.y + bar.depth as f32 * ROW_HEIGHT;
            let rect = Rect::new(x0, top, x1, top + ROW_HEIGHT - 2.0);
            ui.ctx.window.draw_rect(
                rect,
                None,
                // Waits are idle time, so they stay in the background.
                if bar.wait {
                    UiContext::S2
                } else {
                    name_color(bar.name)
                },
                viewport,
                self.clip,
                false,
                BindlessHandle::default(),
            );
            if rect.width() > 3.0 * CHAR_WIDTH {
                ui.ctx.window.draw_text(
                    Vec2::new(rect.min.x + 2.0, rect.min.y + 1.0),
                    if bar.wait {
                        UiContext::TEXT_DIM
                    } else {
                        UiContext::BG_DARK
                    },
                    bar.name,
                    viewport,
                    rect.intersect(self.clip),
                    false,
                );
            }
            if self.cursor.is_some_and(|pos| rect.contains(pos)) {
                self.hovered = Some(format!(
                    "{} ({lane}): {:.3} ms",
                    bar.name,
                    ms(bar.end_ns - bar.start_ns)
                ));
            }
        }
        self.y += rows as f32 * ROW_HEIGHT;
    }
}

fn timeline(ui: &mut UiWindowBuilder, frame: &FrameProfile, view: &mut ProfilerView) {
    let threads = || frame.threads.iter().flat_map(cpu_bars);
    let end = threads()
        .chain(gpu_bars(frame))
        .map(|bar| bar.end_ns)
        .fold(frame.end_ns, u64::max);
    let full = (0.0, (end - frame.start_ns) as f64);
    let rows: u16 = frame.threads.iter().map(|t| lane_rows(cpu_bars(t))).sum::<u16>()
        + lane_rows(gpu_bars(frame));
    let height = rows as f32 * ROW_HEIGHT;
    let size = Vec2::new(visible_width(ui), (height + 20.0).min(600.0));

    ui.container(id!(), size, |ui| {
        let area = Rect::from_corners(ui.cursor, ui.cursor + Vec2::new(size.x - 30.0, height));
        let cursor = ui.ctx.input.cursor_pos.filter(|_| ui.hoverd(area));

        // Wheel zooms around the cursor, dragging pans.
        let (mut start, mut end) = view.range.unwrap_or(full);
        let ns_per_px = (end - start) / area.width() as f64;
        if let Some(pos) = cursor {
            let at = start + (pos.x - area.min.x) as f64 * ns_per_px;
            let delta = ui.ctx.scroll_delta.y;
            if delta != 0.0 {
                let zoom = if delta > 0.0 { 0.8 } else { 1.25 };
                start = at - (at - start) * zoom;
                end = at + (end - at) * zoom;
                view.range = Some((start, end));
            }
            ui.scroll_consumed = true;
        }
        match (cursor, ui.ctx.input.primary_pressing, view.drag_from) {
            (Some(pos), true, None) => view.drag_from = Some(pos.x),
            (_, true, Some(from)) => {
                let x = ui.ctx.input.cursor_pos.map_or(from, |p| p.x);
                let shift = (from - x) as f64 * ns_per_px;
                start += shift;
                end += shift;
                view.range = Some((start, end));
                view.drag_from = Some(x);
            }
            (_, false, _) => view.drag_from = None,
            _ => {}
        }

        let mut lanes = Lanes {
            area,
            clip: ui.clip_rect,
            cursor,
            start,
            ns_per_px: (end - start) / area.width() as f64,
            frame_start: frame.start_ns,
            y: area.min.y,
            hovered: None,
        };
        for thread in &frame.threads {
            lanes.draw(ui, thread.thread, cpu_bars(thread));
        }
        lanes.draw(ui, "GPU", gpu_bars(frame));
        // Gives the container its content size.
        ui.rect(
            area.size(),
            DrawSettings {
                color: Vec4::ZERO,
                border: None,
                ..Default::default()
            },
        );
        if let Some(label) = lanes.hovered {
            ui.tooltip_label(label);
        }
    });
}

pub(crate) fn profiler_ui(
    mut ui: UiBuilder,
    mut profiler: ResMut<Profiler>,
    mut view: Local<ProfilerView>,
) {
    ui.build("Profiler", |ui| {
        let (profiler, view) = (&mut *profiler, &mut *view);
        // While recording, the last frame is still running.
        let closed = profiler.times.len() - !profiler.paused as usize;
        let times = profiler.times.range(..closed);
        let recent = times.clone().skip(closed.saturating_sub(RECENT_FRAMES));
        let frame_time = mean(recent.clone().map(|t| ms(t.frame_ns())));
        let cpu = mean(recent.clone().filter(|t| t.cpu_ns > 0).map(|t| ms(t.cpu_ns)));
        let gpu = mean(recent.filter_map(|t| t.gpu_ns).map(ms));
        let width = visible_width(ui) - 10.0;
        let max = (frame_time * 2.0).max(1.0) as f32;

        ui.horizontal();
        ui.text("Profile");
        let enabled = ui.checkbox(profiler.enabled);
        if enabled != profiler.enabled {
            profiler.set_enabled(enabled);
            return;
        }
        if !profiler.enabled {
            ui.text(format!(
                "Frame {frame_time:.2} ms ({:.0} fps)",
                1000.0 / frame_time.max(f64::EPSILON)
            ));
            ui.vertical();
            let frame_ms = times.map(|t| ms(t.frame_ns()) as f32);
            ui.histogram(width, 40.0, max, 0.0, frame_ms, closed.max(1));
            return;
        }
        if ui.button(if profiler.paused { "Resume" } else { "Pause" }) {
            profiler.set_paused(!profiler.paused);
            return;
        }
        if ui.button("Save capture") {
            let time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let dir = PathBuf::from("profiles").join(time.to_string());
            match profiler.save(&dir) {
                Ok(()) => info!("saved the profile to {}", dir.display()),
                Err(err) => error!("failed to save the profile to {}: {err}", dir.display()),
            }
            return;
        }
        if ui.button("Fit timeline") {
            view.range = None;
        }
        ui.text(format!(
            "Frame {frame_time:.2} ms ({:.0} fps), CPU {cpu:.2} ms, GPU {gpu:.2} ms",
            1000.0 / frame_time.max(f64::EPSILON)
        ));
        ui.vertical();

        let mut clicked = None;
        let graphs: [(&str, fn(&FrameTimes) -> f32); 3] = [
            ("Frame time (click a frame to inspect it)", |t| {
                ms(t.frame_ns()) as f32
            }),
            ("CPU: busiest thread, without waits (exact while paused)", |t| {
                ms(t.cpu_ns) as f32
            }),
            ("GPU", |t| t.gpu_ns.map_or(0.0, ms) as f32),
        ];
        for (label, value) in graphs {
            ui.text(label);
            let values = times.clone().map(value);
            clicked = clicked.or(ui.histogram(width, 40.0, max, 0.0, values, closed.max(1)));
        }
        if let Some(i) = clicked {
            profiler.select(i);
            view.range = None;
        }

        let Some(frame) = &profiler.selected else {
            ui.text("Click a frame in the graphs to inspect it.");
            memory(ui, &profiler.memory, width);
            return;
        };
        if view.tables.as_ref().is_none_or(|(f, _)| *f != frame.frame) {
            view.tables = Some((frame.frame, tables(frame)));
        }
        ui.text(&view.tables.as_ref().unwrap().1.header);
        timeline(ui, frame, view);
        let Tables {
            scopes,
            shaders,
            spans,
            ..
        } = &view.tables.as_ref().unwrap().1;
        ui.collapsable(true, id!(), "GPU scopes", |ui| {
            let header = ["scope", "ms", "% GPU", "fragments", "compute", "mesh"];
            let columns = [0, 28, 38, 46, 58, 70];
            row(ui, &columns, &header.map(String::from), UiContext::TEXT_DIM);
            for cells in scopes {
                row(ui, &columns, cells, UiContext::TEXT);
            }
        });
        ui.collapsable(
            true,
            id!(),
            "Shader time (subgroup clocks)",
            |ui| {
                let header = [
                    "pass stage",
                    "share %",
                    "subgroups",
                    "Mticks",
                    "ticks/subgroup",
                ];
                let columns = [0, 32, 42, 54, 64];
                row(ui, &columns, &header.map(String::from), UiContext::TEXT_DIM);
                for cells in shaders {
                    row(ui, &columns, cells, UiContext::TEXT);
                }
            },
        );
        let span_filter = &mut view.span_filter;
        ui.collapsable(true, id!(), "CPU spans (started in this frame)", |ui| {
            *span_filter = ui.dropdown(id!(), *span_filter, &SPAN_FILTERS);
            let kind = [None, Some(SpanKind::System), Some(SpanKind::Schedule)]
                .get(*span_filter)
                .copied()
                .unwrap_or(Some(SpanKind::Other));
            let shown: Vec<&SpanRow> = spans
                .iter()
                .filter(|span| kind.is_none_or(|kind| span.kind == kind))
                .collect();

            let header = ["span", "kind", "ms", "calls", "max ms", "thread"];
            let columns = [
                0,
                NAME_CHARS + 2,
                NAME_CHARS + 11,
                NAME_CHARS + 21,
                NAME_CHARS + 29,
                NAME_CHARS + 39,
            ];
            row(ui, &columns, &header.map(String::from), UiContext::TEXT_DIM);
            let line = (UiContext::ATLAS_CELL_SIZE.y + UiContext::ELEMENT_GAP.y) as f32;
            let size = Vec2::new(
                visible_width(ui),
                (shown.len() as f32 * line + 10.0).min(600.0),
            );
            ui.text_container(
                id!(),
                size,
                false,
                |ui, i| {
                    let span = shown[i];
                    let name = from_pos_size(ui.cursor, UiContext::text_size(&span.cells[0]));
                    let hovered = span.name.len() > span.cells[0].len() && ui.hoverd(name);
                    row(ui, &columns, &span.cells, UiContext::TEXT);
                    if hovered {
                        ui.tooltip_label(span.name);
                    }
                },
                shown.len(),
            );
        });
        memory(ui, &profiler.memory, width);
    });
}

fn memory(ui: &mut UiWindowBuilder, memory: &MemoryReport, width: f32) {
    ui.collapsable(true, id!(), "GPU memory", |ui| {
        let mib = |b: u64| b as f64 / (1024.0 * 1024.0);
        ui.text(format!(
            "{:.1} MiB allocated in {} blocks, {:.1} MiB reserved",
            mib(memory.allocated),
            memory.blocks,
            mib(memory.reserved)
        ));
        for (i, heap) in memory.heaps.iter().enumerate() {
            let kind = if heap.device_local {
                "device local"
            } else {
                "host"
            };
            let usage = heap.usage.unwrap_or(0);
            ui.progress_bar(
                usage as f32 / heap.budget as f32,
                width,
                format!(
                    "heap {i} ({kind}): {:.0} / {:.0} MiB",
                    mib(usage),
                    mib(heap.budget)
                ),
            );
        }
    });
}
