//! Profile reports: a plain-text summary of frame times, GPU scopes, top CPU spans and GPU memory, and a Chrome trace of every span.
use std::{collections::HashMap, fmt::Write};

use lava::profiling::{MemoryReport, PipelineStats};
use serde_json::{Value, json};

use crate::profiler::FrameProfile;

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

/// A CPU span name summed over `frames`.
pub struct SpanTotals {
    pub name: &'static str,
    /// `None` when it ran on several threads.
    pub thread: Option<&'static str>,
    pub calls: usize,
    pub total_ns: u64,
    pub max_ns: u64,
}

/// The CPU spans of `frames` by name, longest total first. Nested spans count towards their
/// parents too.
pub fn cpu_spans(frames: &[FrameProfile]) -> Vec<SpanTotals> {
    let mut totals: HashMap<&str, SpanTotals> = HashMap::new();
    for span in frames.iter().flat_map(|f| &f.cpu) {
        let duration = span.end_ns - span.start_ns;
        let entry = totals.entry(span.name).or_insert(SpanTotals {
            name: span.name,
            thread: Some(span.thread),
            calls: 0,
            total_ns: 0,
            max_ns: 0,
        });
        if entry.thread != Some(span.thread) {
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

    writeln!(out, "\nFrame time      mean      p50      p95      max").unwrap();
    let cpu = distribution(frames.iter().map(|f| ms(f.cpu_ns())).collect());
    let gpu = distribution(with_gpu.iter().filter_map(|f| f.gpu_ns()).map(ms).collect());
    for (label, [mean, p50, p95, max]) in [("CPU", cpu), ("GPU", gpu)] {
        writeln!(
            out,
            "  {label:<8} {mean:>9.3} {p50:>8.3} {p95:>8.3} {max:>8.3}"
        )
        .unwrap();
    }
    if cpu[0] > 0.0 {
        writeln!(out, "  ({:.1} fps)", 1000.0 / cpu[0]).unwrap();
    }

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

    let spans = cpu_spans(frames);
    let n = frames.len().max(1) as f64;
    writeln!(
        out,
        "\nCPU spans (top {TOP_CPU_SPANS} by total time, inclusive)\n  {:<60} {:>9} {:>11} {:>9} {:>9}  thread",
        "name", "ms/frame", "calls/frame", "mean", "max"
    )
    .unwrap();
    for span in spans.iter().take(TOP_CPU_SPANS) {
        writeln!(
            out,
            "  {:<60} {:>9.3} {:>11.1} {:>9.3} {:>9.3}  {}",
            span.name,
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
        for span in &frame.cpu {
            let tid = match threads.iter().position(|t| *t == span.thread) {
                Some(tid) => tid,
                None => {
                    threads.push(span.thread);
                    threads.len() - 1
                }
            };
            events.push(json!({
                "name": span.name, "cat": "cpu", "ph": "X", "pid": 1, "tid": tid,
                "ts": us(span.start_ns), "dur": us(span.end_ns - span.start_ns),
            }));
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
    use crate::profiler::capture::CpuSpan;
    use lava::profiling::GpuScope;

    fn frame(frame: u64, cpu_ms: u64, gpu: &[(&'static str, u16, u64, u64)]) -> FrameProfile {
        let start_ns = frame * 100_000_000;
        FrameProfile {
            frame,
            start_ns,
            end_ns: start_ns + cpu_ms * 1_000_000,
            cpu: vec![CpuSpan {
                name: "update",
                thread: "main",
                depth: 0,
                start_ns,
                end_ns: start_ns + 1_000_000,
            }],
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
        }
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
            .map(|i| frame(i, 10 + i, &[("frame", 0, 0, 2), ("Raster", 1, 0, 1)]))
            .collect();
        let text = summary(&frames, &MemoryReport::default());
        assert!(text.starts_with("Profile of 4 frames (#0..#3), 4 with GPU timings."));
        let cpu = text
            .lines()
            .find(|l| l.trim_start().starts_with("CPU "))
            .unwrap();
        let numbers: Vec<f64> = cpu
            .split_whitespace()
            .skip(1)
            .map(|n| n.parse().unwrap())
            .collect();
        assert_eq!(numbers, [11.5, 12.0, 13.0, 13.0]);
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
        assert_eq!(rows, ["GPU", "main"]);
    }
}
