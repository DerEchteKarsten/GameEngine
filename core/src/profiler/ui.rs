//! The editor's Profiler tab: frame-time graph, a zoomable timeline of the selected frame's CPU spans and GPU scopes, the slowest passes and spans, and GPU memory.
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

use crate::{
    id,
    profiler::{
        FrameProfile, Profiler,
        report::{cpu_spans, gpu_scopes},
    },
    ui::{
        UiContext,
        builder::{UiBuilder, UiWindowBuilder},
        window::{DrawSettings, Drawable},
    },
};

/// Frames the averages and tables cover.
const RECENT_FRAMES: usize = 120;
const ROW_HEIGHT: f32 = UiContext::ATLAS_CELL_SIZE.y as f32 + 2.0;
const CHAR_WIDTH: f32 = UiContext::ATLAS_CELL_SIZE.x as f32;

/// Frames between two updates of the tables.
const TABLE_INTERVAL: u64 = 30;

#[derive(Default)]
pub(crate) struct ProfilerView {
    /// The visible part of the timeline in ns since the frame started; `None` shows all of it.
    range: Option<(f64, f64)>,
    drag_from: Option<f32>,
    /// The rows of the GPU scope and CPU span tables, and the newest frame they include.
    tables: Option<(u64, Vec<[String; 6]>, Vec<[String; 5]>)>,
}

fn tables(recent: &[FrameProfile], gpu_mean: f64) -> (Vec<[String; 6]>, Vec<[String; 5]>) {
    let scopes = gpu_scopes(recent).into_iter().map(|scope| {
        let n = scope.ms.len() as u64;
        let scope_mean = mean(scope.ms.iter().copied());
        let max = scope.ms.iter().copied().fold(0.0, f64::max);
        let stats = scope.stats.map(|s| (s.fragment / n, s.compute / n));
        [
            format!("{}{}", "  ".repeat(scope.depth as usize), scope.name),
            format!("{scope_mean:.3}"),
            format!("{max:.3}"),
            format!("{:.1}", scope_mean / gpu_mean.max(f64::EPSILON) * 100.0),
            stats.map_or(String::new(), |s| s.0.to_string()),
            stats.map_or(String::new(), |s| s.1.to_string()),
        ]
    });
    let n = recent.len().max(1) as f64;
    let spans = cpu_spans(recent).into_iter().take(25).map(|span| {
        [
            span.name.chars().take(48).collect(),
            format!("{:.3}", ms(span.total_ns) / n),
            format!("{:.1}", span.calls as f64 / n),
            format!("{:.3}", ms(span.max_ns)),
            span.thread.unwrap_or("(several)").to_string(),
        ]
    });
    (scopes.collect(), spans.collect())
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
    depth: u16,
    start_ns: u64,
    end_ns: u64,
}

/// One lane per thread and one for the GPU, each a row per span depth.
fn lanes(frame: &FrameProfile) -> Vec<(&'static str, Vec<Bar>)> {
    let mut lanes: Vec<(&'static str, Vec<Bar>)> = Vec::new();
    for span in &frame.cpu {
        let bar = Bar {
            name: span.name,
            depth: span.depth,
            start_ns: span.start_ns,
            end_ns: span.end_ns,
        };
        match lanes.iter_mut().find(|(thread, _)| *thread == span.thread) {
            Some((_, bars)) => bars.push(bar),
            None => lanes.push((span.thread, vec![bar])),
        }
    }
    lanes.sort_by_key(|(thread, _)| (*thread != "main", *thread != "render thread", *thread));
    let gpu = frame.gpu.iter().map(|scope| Bar {
        name: scope.name,
        depth: scope.depth,
        start_ns: frame.gpu_submit_ns + scope.start_ns,
        end_ns: frame.gpu_submit_ns + scope.end_ns,
    });
    lanes.push(("GPU", gpu.collect()));
    lanes
}

fn timeline(ui: &mut UiWindowBuilder, frame: &FrameProfile, view: &mut ProfilerView) {
    let lanes = lanes(frame);
    let end = lanes
        .iter()
        .flat_map(|(_, bars)| bars)
        .map(|bar| bar.end_ns)
        .max()
        .unwrap_or(frame.end_ns)
        .max(frame.end_ns);
    let full = (0.0, (end - frame.start_ns) as f64);
    let height: f32 = lanes
        .iter()
        .map(|(_, bars)| bars.iter().map(|b| b.depth + 2).max().unwrap_or(1) as f32 * ROW_HEIGHT)
        .sum();
    let size = Vec2::new(ui.remaining_width() - 10.0, (height + 20.0).min(600.0));

    ui.container(id!(), size, |ui| {
        let area = Rect::from_corners(ui.cursor, ui.cursor + Vec2::new(size.x - 30.0, height));
        let clip = ui.clip_rect;
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
        let ns_per_px = (end - start) / area.width() as f64;
        let x_of =
            |ns: u64| area.min.x + ((ns as f64 - frame.start_ns as f64 - start) / ns_per_px) as f32;

        let viewport = ui.ctx.viewport_size;
        let mut hovered = None;
        let mut y = area.min.y;
        for (thread, bars) in &lanes {
            ui.ctx.window.draw_text(
                Vec2::new(area.min.x, y),
                UiContext::TEXT_DIM,
                thread,
                viewport,
                clip,
                false,
            );
            y += ROW_HEIGHT;
            for bar in bars {
                let x0 = x_of(bar.start_ns).max(area.min.x);
                let x1 = x_of(bar.end_ns).min(area.max.x).max(x0 + 1.0);
                if x0 > area.max.x || x1 < area.min.x {
                    continue;
                }
                let top = y + bar.depth as f32 * ROW_HEIGHT;
                let rect = Rect::new(x0, top, x1, top + ROW_HEIGHT - 2.0);
                ui.ctx.window.draw_rect(
                    rect,
                    None,
                    name_color(bar.name),
                    viewport,
                    clip,
                    false,
                    BindlessHandle::default(),
                );
                if rect.width() > 3.0 * CHAR_WIDTH {
                    ui.ctx.window.draw_text(
                        Vec2::new(rect.min.x + 2.0, rect.min.y + 1.0),
                        UiContext::BG_DARK,
                        bar.name,
                        viewport,
                        rect.intersect(clip),
                        false,
                    );
                }
                if cursor.is_some_and(|pos| rect.contains(pos)) {
                    hovered = Some(format!(
                        "{} ({thread}): {:.3} ms",
                        bar.name,
                        ms(bar.end_ns - bar.start_ns)
                    ));
                }
            }
            y += bars.iter().map(|b| b.depth + 1).max().unwrap_or(0) as f32 * ROW_HEIGHT;
        }
        // Gives the container its content size.
        ui.rect(
            area.size(),
            DrawSettings {
                color: Vec4::ZERO,
                border: None,
                ..Default::default()
            },
        );
        if let Some(label) = hovered {
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
        let profiler = &mut *profiler;
        let frames = profiler.frames.make_contiguous();
        let recent = &frames[frames.len().saturating_sub(RECENT_FRAMES)..];
        let cpu = mean(recent.iter().map(|f| ms(f.cpu_ns())));
        let gpu = mean(recent.iter().filter_map(|f| f.gpu_ns()).map(ms));
        let newest = recent.last().map_or(0, |f| f.frame);
        if view
            .tables
            .as_ref()
            .is_none_or(|(frame, ..)| newest.abs_diff(*frame) >= TABLE_INTERVAL)
        {
            let (scopes, spans) = tables(recent, gpu);
            view.tables = Some((newest, scopes, spans));
        }

        ui.horizontal();
        if ui.button(if profiler.paused { "Resume" } else { "Pause" }) {
            profiler.set_paused(!profiler.paused);
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
        }
        if ui.button("Fit timeline") {
            view.range = None;
        }
        ui.text(format!(
            "CPU {cpu:.2} ms ({:.0} fps), GPU {gpu:.2} ms",
            1000.0 / cpu.max(f64::EPSILON)
        ));
        ui.vertical();

        let width = ui.remaining_width() - 20.0;
        let len = profiler.frames.len().max(1);
        let cpu_ms: Vec<f32> = profiler
            .frames
            .iter()
            .map(|f| ms(f.cpu_ns()) as f32)
            .collect();
        let gpu_ms: Vec<f32> = profiler
            .frames
            .iter()
            .map(|f| f.gpu_ns().map_or(0.0, ms) as f32)
            .collect();
        let max = (cpu.max(gpu) * 2.0).max(1.0) as f32;
        ui.text("CPU frame time (click a frame to inspect it)");
        let clicked_cpu = ui.histogram(width, 40.0, max, 0.0, cpu_ms.iter(), len);
        ui.text("GPU frame time");
        let clicked_gpu = ui.histogram(width, 40.0, max, 0.0, gpu_ms.iter(), len);
        if let Some(i) = clicked_cpu.or(clicked_gpu) {
            profiler.set_paused(true);
            profiler.selected = Some(i);
            view.range = None;
        }

        // Without a selection, the newest frame whose GPU timings arrived.
        let shown = profiler.selected.or_else(|| {
            let frames = &profiler.frames;
            frames
                .iter()
                .rposition(|f| !f.gpu.is_empty())
                .or(frames.len().checked_sub(1))
        });
        if let Some(frame) = shown.and_then(|i| profiler.frames.get(i)) {
            ui.text(format!(
                "Frame #{}: CPU {:.3} ms, GPU {}",
                frame.frame,
                ms(frame.cpu_ns()),
                frame
                    .gpu_ns()
                    .map_or("-".into(), |ns| format!("{:.3} ms", ms(ns)))
            ));
            timeline(ui, frame, &mut view);
        }

        let (_, scopes, spans) = view.tables.as_ref().unwrap();
        ui.collapsable(true, id!(), "GPU scopes (last 120 frames)", |ui| {
            let header = [
                "scope",
                "mean ms",
                "max ms",
                "% GPU",
                "fragments",
                "compute",
            ];
            let columns = [0, 28, 38, 48, 56, 70];
            row(ui, &columns, &header.map(String::from), UiContext::TEXT_DIM);
            for cells in scopes {
                row(ui, &columns, cells, UiContext::TEXT);
            }
        });
        ui.collapsable(true, id!(), "CPU spans (last 120 frames)", |ui| {
            let header = ["span", "ms/frame", "calls", "max ms", "thread"];
            let columns = [0, 50, 60, 70, 80];
            row(ui, &columns, &header.map(String::from), UiContext::TEXT_DIM);
            for cells in spans {
                row(ui, &columns, cells, UiContext::TEXT);
            }
        });
        ui.collapsable(true, id!(), "GPU memory", |ui| {
            let memory = &profiler.memory;
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
    });
}
