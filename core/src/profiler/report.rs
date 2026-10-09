//! Profile reports: a plain-text summary of frame, CPU and GPU times, GPU scopes, shader clock time per pass, top CPU spans and GPU memory, and a Chrome trace of every span.
use std::{collections::HashMap, fmt::Write};

use lava::profiling::{MemoryReport, PipelineStats};
use serde_json::{Value, json};

use crate::profiler::{FrameProfile, capture::SpanKind};

const TOP_CPU_SPANS: usize = 25;

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

/// Mean, median, 95th percentile and maximum.
fn distribution(mut values: Vec<f64>) -> [f64; 4] {
    if values.is_empty() {
        return [0.0; 4];
    }
    values.sort_by(f64::total_cmp);
    let at = |p: f64| values[((values.len() - 1) as f64 * p).round() as usize];
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    [mean, at(0.5), at(0.95), values[values.len() - 1]]
}

/// A GPU scope summed over each frame it appears in.
#[derive(Default)]
pub struct ScopeTotals {
    pub name: &'static str,
    pub depth: u16,
    /// Milliseconds per frame.
    pub ms: Vec<f64>,
    pub stats: Option<PipelineStats>,
}

/// The GPU scopes of `frames` by depth and name, in tree order.
pub fn gpu_scopes(frames: &[FrameProfile]) -> Vec<ScopeTotals> {
    let mut scopes: Vec<ScopeTotals> = Vec::new();
    for frame in frames.iter().filter(|f| !f.gpu.is_empty()) {
        let mut in_frame: Vec<usize> = Vec::new();
        let mut cursor = 0;
        for scope in &frame.gpu {
            let i = match scopes
                .iter()
                .position(|s| s.name == scope.name && s.depth == scope.depth)
            {
                Some(i) => i,
                None => {
                    // After the scope recorded before it, so conditional scopes keep their place.
                    scopes.insert(
                        cursor,
                        ScopeTotals {
                            name: scope.name,
                            depth: scope.depth,
                            ..Default::default()
                        },
                    );
                    in_frame
                        .iter_mut()
                        .filter(|i| **i >= cursor)
                        .for_each(|i| *i += 1);
                    cursor
                }
            };
            cursor = i + 1;
            let totals = &mut scopes[i];
            let duration = ms(scope.end_ns - scope.start_ns);
            if in_frame.contains(&i) {
                *totals.ms.last_mut().unwrap() += duration;
            } else {
                in_frame.push(i);
                totals.ms.push(duration);
            }
            if let Some(stats) = scope.stats {
                let sum = totals.stats.get_or_insert_default();
                sum.task += stats.task;
                sum.mesh += stats.mesh;
                sum.fragment += stats.fragment;
                sum.compute += stats.compute;
            }
        }
    }
    scopes
}

/// A pass's shader stage over the frames that have shader clock times.
pub struct ShaderTotals {
    pub pass: &'static str,
    pub stage: &'static str,
    /// Subgroup clock ticks per frame, summed over subgroups.
    pub ticks: f64,
    pub subgroups: f64,
    /// Of all instrumented shaders' ticks.
    pub share: f64,
}

/// The shader clock times of `frames` per pass and stage, the largest first.
pub fn shader_times(frames: &[FrameProfile]) -> Vec<ShaderTotals> {
    let with_times = frames.iter().filter(|f| !f.shaders.is_empty());
    let n = with_times.clone().count().max(1) as f64;
    let mut totals: Vec<ShaderTotals> = Vec::new();
    for time in with_times.flat_map(|f| &f.shaders) {
        let i = match totals
            .iter()
            .position(|t| t.pass == time.pass && t.stage == time.stage)
        {
            Some(i) => i,
            None => {
                totals.push(ShaderTotals {
                    pass: time.pass,
                    stage: time.stage,
                    ticks: 0.0,
                    subgroups: 0.0,
                    share: 0.0,
                });
                totals.len() - 1
            }
        };
        totals[i].ticks += time.ticks as f64 / n;
        totals[i].subgroups += time.subgroups as f64 / n;
    }
    let all: f64 = totals.iter().map(|t| t.ticks).sum();
    for total in &mut totals {
        total.share = total.ticks / all.max(1.0);
    }
    totals.sort_by(|a, b| b.ticks.total_cmp(&a.ticks));
    totals
}

/// A CPU span name summed over `frames`.
pub struct SpanTotals {
    pub name: &'static str,
    pub kind: SpanKind,
    /// `None` when it ran on several threads.
    pub thread: Option<&'static str>,
    pub calls: usize,
    pub total_ns: u64,
    pub max_ns: u64,
}

/// The CPU spans of `frames` by name and kind, longest total first. Nested spans count
/// towards their parents too.
pub fn cpu_spans(frames: &[FrameProfile]) -> Vec<SpanTotals> {
    let mut totals: HashMap<(&str, SpanKind), SpanTotals> = HashMap::new();
    let threads = frames.iter().flat_map(|f| &f.threads);
    for (thread, span) in threads.flat_map(|t| t.spans.iter().map(|s| (t.thread, s))) {
        let duration = span.end_ns - span.start_ns;
        let entry = totals.entry((span.name, span.kind)).or_insert(SpanTotals {
            name: span.name,
            kind: span.kind,
            thread: Some(thread),
            calls: 0,
            total_ns: 0,
            max_ns: 0,
        });
        if entry.thread != Some(thread) {
            entry.thread = None;
        }
        entry.calls += 1;
        entry.total_ns += duration;
        entry.max_ns = entry.max_ns.max(duration);
    }
    let mut totals: Vec<_> = totals.into_values().collect();
    totals.sort_by(|a, b| b.total_ns.cmp(&a.total_ns));
    totals
}

fn bytes(b: u64) -> String {
    let mib = b as f64 / (1024.0 * 1024.0);
    if mib >= 1024.0 {
        format!("{:.2} GiB", mib / 1024.0)
    } else {
        format!("{mib:.1} MiB")
    }
}

/// A plain-text report of `frames`, for a person or an AI to read.
pub fn summary(frames: &[FrameProfile], memory: &MemoryReport) -> String {
    let mut out = String::new();
    let with_gpu: Vec<&FrameProfile> = frames.iter().filter(|f| f.gpu_ns().is_some()).collect();
    match (frames.first(), frames.last()) {
        (Some(first), Some(last)) => writeln!(
            out,
            "Profile of {} frames (#{}..#{}), {} with GPU timings. Times in ms.",
            frames.len(),
            first.frame,
            last.frame,
            with_gpu.len()
        ),
        _ => writeln!(out, "Profile of 0 frames."),
    }
    .unwrap();

    // CPU is the busiest thread's time without waits (see `ThreadSpans::busy_ns`), GPU the
    // time the GPU executed the frame's commands.
    let frame = distribution(frames.iter().map(|f| ms(f.frame_ns())).collect());
    let cpu = distribution(frames.iter().map(|f| ms(f.cpu_ns())).collect());
    let gpu = distribution(with_gpu.iter().filter_map(|f| f.gpu_ns()).map(ms).collect());
    // Over every frame, so a thread that works only now and then doesn't look busy.
    let mut threads: Vec<(&str, [f64; 4])> = Vec::new();
    for thread in frames.iter().flat_map(|f| &f.threads).map(|t| t.thread) {
        if threads.iter().all(|(t, _)| *t != thread) {
            let busy = frames.iter().map(|f| {
                let busy = f.threads.iter().find(|t| t.thread == thread);
                busy.map_or(0.0, |t| ms(t.busy_ns))
            });
            threads.push((thread, distribution(busy.collect())));
        }
    }
    threads.sort_by(|a, b| b.1[0].total_cmp(&a.1[0]));
    let bound = match threads.first() {
        Some((thread, _)) if cpu[0] > gpu[0] => format!("CPU-bound ({thread})"),
        _ => "GPU-bound".to_string(),
    };
    writeln!(
        out,
        "\nFrame time                  mean      p50      p95      max   ({:.1} fps, {bound})",
        1000.0 / frame[0].max(f64::EPSILON)
    )
    .unwrap();
    let rows = [("Frame (wall)".to_string(), frame), ("CPU (busiest thread)".into(), cpu)]
        .into_iter()
        .chain(threads.iter().take(3).map(|(t, d)| (format!("  {t}"), *d)))
        .chain([("GPU".into(), gpu)]);
    for (label, [mean, p50, p95, max]) in rows {
        writeln!(
            out,
            "  {label:<22} {mean:>9.3} {p50:>8.3} {p95:>8.3} {max:>8.3}"
        )
        .unwrap();
    }
    writeln!(
        out,
        "  (CPU: time in spans minus wait spans, per thread; GPU: from timestamps)"
    )
    .unwrap();

    let scopes = gpu_scopes(frames);
    let gpu_mean = gpu[0].max(f64::EPSILON);
    writeln!(
        out,
        "\nGPU scopes per frame         mean      p95   % GPU   | invocations per frame: task mesh fragment compute"
    )
    .unwrap();
    for scope in &scopes {
        let [mean, _, p95, _] = distribution(scope.ms.clone());
        let name = format!("{}{}", "  ".repeat(scope.depth as usize), scope.name);
        write!(
            out,
            "  {name:<24} {mean:>8.3} {p95:>8.3} {:>6.1}%",
            mean / gpu_mean * 100.0
        )
        .unwrap();
        if let Some(stats) = scope.stats {
            let n = scope.ms.len() as u64;
            write!(
                out,
                "  | {} {} {} {}",
                stats.task / n,
                stats.mesh / n,
                stats.fragment / n,
                stats.compute / n
            )
            .unwrap();
        }
        writeln!(out).unwrap();
    }

    let shaders = shader_times(frames);
    if !shaders.is_empty() {
        writeln!(
            out,
            "\nShader time per frame: subgroup clock ticks from `PROFILE`, summed over subgroups\n(they run in parallel; the share says which shaders keep the GPU busy, not the time)\n  {:<30} {:>8} {:>10} {:>12} {:>14}",
            "pass stage", "share", "subgroups", "Mticks", "ticks/subgroup"
        )
        .unwrap();
        for time in &shaders {
            writeln!(
                out,
                "  {:<30} {:>7.1}% {:>10.0} {:>12.3} {:>14.0}",
                format!("{} {}", time.pass, time.stage),
                time.share * 100.0,
                time.subgroups,
                time.ticks / 1e6,
                time.ticks / time.subgroups.max(1.0)
            )
            .unwrap();
        }
    }

    let spans = cpu_spans(frames);
    let n = frames.len().max(1) as f64;
    writeln!(
        out,
        "\nCPU spans (top {TOP_CPU_SPANS} by total time, inclusive)\n  {:<60} {:<8} {:>9} {:>11} {:>9} {:>9}  thread",
        "name", "kind", "ms/frame", "calls/frame", "mean", "max"
    )
    .unwrap();
    for span in spans.iter().take(TOP_CPU_SPANS) {
        writeln!(
            out,
            "  {:<60} {:<8} {:>9.3} {:>11.1} {:>9.3} {:>9.3}  {}",
            span.name,
            span.kind.label(),
            ms(span.total_ns) / n,
            span.calls as f64 / n,
            ms(span.total_ns) / span.calls as f64,
            ms(span.max_ns),
            span.thread.unwrap_or("(several)")
        )
        .unwrap();
    }

    writeln!(
        out,
        "\nGPU memory: {} allocated in {} blocks, {} reserved",
        bytes(memory.allocated),
        memory.blocks,
        bytes(memory.reserved)
    )
    .unwrap();
    for (i, heap) in memory.heaps.iter().enumerate() {
        let kind = if heap.device_local {
            "device local"
        } else {
            "host"
        };
        match heap.usage {
            Some(usage) => writeln!(
                out,
                "  heap {i} ({kind}, {}): {} used of {} budget",
                bytes(heap.size),
                bytes(usage),
                bytes(heap.budget)
            ),
            None => writeln!(out, "  heap {i} ({kind}): {}", bytes(heap.size)),
        }
        .unwrap();
    }
    out
}

/// The spans of `frames` in the Trace Event format: one row per thread and one for the GPU.
pub fn chrome_trace(frames: &[FrameProfile]) -> String {
    let us = |ns: u64| ns as f64 / 1000.0;
    let mut threads: Vec<&str> = vec!["GPU"];
    let mut events: Vec<Value> = Vec::new();
    for frame in frames {
        for thread in &frame.threads {
            let tid = match threads.iter().position(|t| *t == thread.thread) {
                Some(tid) => tid,
                None => {
                    threads.push(thread.thread);
                    threads.len() - 1
                }
            };
            for span in &thread.spans {
                events.push(json!({
                    "name": span.name, "cat": "cpu", "ph": "X", "pid": 1, "tid": tid,
                    "ts": us(span.start_ns), "dur": us(span.end_ns - span.start_ns),
                }));
            }
        }
        for scope in &frame.gpu {
            events.push(json!({
                "name": scope.name, "cat": "gpu", "ph": "X", "pid": 1, "tid": 0,
                "ts": us(frame.gpu_submit_ns + scope.start_ns),
                "dur": us(scope.end_ns - scope.start_ns),
                "args": { "frame": frame.frame },
            }));
        }
    }
    for (tid, name) in threads.iter().enumerate() {
        events.push(json!({
            "name": "thread_name", "ph": "M", "pid": 1, "tid": tid, "args": { "name": name },
        }));
    }
    json!({ "traceEvents": events, "displayTimeUnit": "ms" }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiler::{ThreadSpans, capture::CpuSpan};
    use lava::profiling::{GpuScope, ShaderTime};

    fn frame(frame: u32, frame_ms: u64, gpu: &[(&'static str, u16, u64, u64)]) -> FrameProfile {
        let start_ns = frame as u64 * 100_000_000;
        FrameProfile {
            frame,
            start_ns,
            end_ns: start_ns + frame_ms * 1_000_000,
            threads: vec![
                ThreadSpans {
                    thread: "main",
                    busy_ns: frame_ms * 500_000,
                    spans: vec![CpuSpan {
                        name: "update",
                        start_ns,
                        end_ns: start_ns + 1_000_000,
                        frame,
                        depth: 0,
                        kind: SpanKind::Other,
                    }],
                },
                ThreadSpans {
                    thread: "render thread",
                    busy_ns: 1_000_000,
                    spans: Vec::new(),
                },
            ],
            complete: true,
            gpu: gpu
                .iter()
                .map(|&(name, depth, start, end)| GpuScope {
                    name,
                    depth,
                    start_ns: start * 1_000_000,
                    end_ns: end * 1_000_000,
                    stats: (depth > 0).then_some(PipelineStats {
                        fragment: 10,
                        ..Default::default()
                    }),
                })
                .collect(),
            gpu_submit_ns: start_ns,
            shaders: vec![
                ShaderTime {
                    pass: "Raster",
                    stage: "fragment",
                    ticks: 3000,
                    subgroups: 10,
                },
                ShaderTime {
                    pass: "Tonemap",
                    stage: "compute",
                    ticks: 1000,
                    subgroups: 20,
                },
            ],
        }
    }

    #[test]
    fn shader_times_are_per_frame_and_sorted_by_share() {
        let frames = [frame(0, 10, &[]), frame(1, 10, &[])];
        let times = shader_times(&frames);
        let rows: Vec<_> = times
            .iter()
            .map(|t| (t.pass, t.stage, t.ticks, t.subgroups, t.share))
            .collect();
        assert_eq!(
            rows,
            [
                ("Raster", "fragment", 3000.0, 10.0, 0.75),
                ("Tonemap", "compute", 1000.0, 20.0, 0.25)
            ]
        );
    }

    #[test]
    fn distribution_has_mean_percentiles_and_max() {
        let [mean, p50, p95, max] = distribution((1..=100).map(f64::from).collect());
        assert_eq!(mean, 50.5);
        assert_eq!((p50, p95, max), (51.0, 95.0, 100.0));
        assert_eq!(distribution(Vec::new()), [0.0; 4]);
    }

    #[test]
    fn gpu_scopes_keep_tree_order_and_sum_repeats() {
        let frames = [
            frame(0, 10, &[("frame", 0, 0, 4), ("A", 1, 0, 1), ("C", 1, 2, 4)]),
            // B appears only here, between A and C; A runs twice.
            frame(
                1,
                10,
                &[
                    ("frame", 0, 0, 6),
                    ("A", 1, 0, 1),
                    ("B", 1, 1, 2),
                    ("A", 1, 2, 3),
                    ("C", 1, 3, 6),
                ],
            ),
            frame(2, 10, &[]),
        ];
        let scopes = gpu_scopes(&frames);
        let names: Vec<_> = scopes.iter().map(|s| (s.name, s.depth)).collect();
        assert_eq!(names, [("frame", 0), ("A", 1), ("B", 1), ("C", 1)]);
        assert_eq!(scopes[1].ms, [1.0, 2.0]);
        assert_eq!(scopes[2].ms, [1.0]);
        assert_eq!(scopes[1].stats.unwrap().fragment, 30);
        assert!(scopes[0].stats.is_none());
    }

    #[test]
    fn summary_reports_frame_times_and_scopes() {
        let frames: Vec<_> = (0..4)
            .map(|i| frame(i, 10 + i as u64, &[("frame", 0, 0, 2), ("Raster", 1, 0, 1)]))
            .collect();
        let text = summary(&frames, &MemoryReport::default());
        assert!(text.starts_with("Profile of 4 frames (#0..#3), 4 with GPU timings."));
        let numbers = |label: &str| -> Vec<f64> {
            let line = text.lines().find(|l| l.trim_start().starts_with(label)).unwrap();
            let numbers = line[line.find(label).unwrap() + label.len()..].split_whitespace();
            numbers.map(|n| n.parse().unwrap()).collect()
        };
        assert_eq!(numbers("Frame (wall)"), [11.5, 12.0, 13.0, 13.0]);
        // Main is busy half of each frame, the render thread 1 ms.
        assert_eq!(numbers("CPU (busiest thread)"), [5.75, 6.0, 6.5, 6.5]);
        assert_eq!(numbers("main"), [5.75, 6.0, 6.5, 6.5]);
        assert_eq!(numbers("render thread"), [1.0; 4]);
        assert_eq!(numbers("GPU "), [2.0; 4]);
        assert!(text.contains("CPU-bound (main)"));
        let raster = text.lines().find(|l| l.contains("Raster")).unwrap();
        assert!(
            raster.contains("1.000") && raster.contains("50.0%"),
            "{raster}"
        );
        assert!(text.contains("update"));
    }

    #[test]
    fn chrome_trace_is_valid_json_with_a_row_per_thread() {
        let frames = [frame(0, 10, &[("frame", 0, 0, 2), ("Raster", 1, 0, 1)])];
        let trace: Value = serde_json::from_str(&chrome_trace(&frames)).unwrap();
        let events = trace["traceEvents"].as_array().unwrap();
        let spans: Vec<_> = events.iter().filter(|e| e["ph"] == "X").collect();
        assert_eq!(spans.len(), 3);
        let raster = spans.iter().find(|e| e["name"] == "Raster").unwrap();
        assert_eq!(
            (raster["tid"].as_u64(), raster["dur"].as_f64()),
            (Some(0), Some(1000.0))
        );
        let rows: Vec<_> = events
            .iter()
            .filter(|e| e["ph"] == "M")
            .map(|e| e["args"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(rows, ["GPU", "main", "render thread"]);
    }
}
