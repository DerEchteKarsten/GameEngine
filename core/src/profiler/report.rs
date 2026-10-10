//! Profile reports: a plain-text summary of frame, CPU and GPU times, GPU scopes, hardware counters per pass, shader clock regions, top CPU spans and GPU memory, and a Chrome trace of every span.
use std::{collections::HashMap, fmt::Write};

use lava::profiling::{CounterUnit, MAX_COUNTERS, MemoryReport, PipelineStats};
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
    /// Per frame for amounts, averaged over the scope's time for rates (`CounterUnit::is_rate`),
    /// in the order of the frames' `counters`.
    pub counters: Option<[f64; MAX_COUNTERS]>,
    /// The scope time the rates are weighted by while summing.
    counted_ms: f64,
}

/// The GPU scopes of `frames` by depth and name, in tree order.
pub fn gpu_scopes(frames: &[FrameProfile]) -> Vec<ScopeTotals> {
    let units: Vec<CounterUnit> = frames
        .first()
        .map_or(Vec::new(), |f| f.counters.iter().map(|c| c.unit).collect());
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
            if let Some(values) = scope.counters {
                let sums = totals.counters.get_or_insert([0.0; MAX_COUNTERS]);
                for ((sum, value), unit) in sums.iter_mut().zip(values).zip(&units) {
                    *sum += if unit.is_rate() { value * duration } else { value };
                }
                totals.counted_ms += duration;
            }
        }
    }
    for scope in &mut scopes {
        let (frames, counted_ms) = (scope.ms.len() as f64, scope.counted_ms);
        if let Some(sums) = &mut scope.counters {
            for (sum, unit) in sums.iter_mut().zip(&units) {
                *sum /= if unit.is_rate() { counted_ms.max(f64::EPSILON) } else { frames };
            }
        }
    }
    scopes
}

/// A counter value with its unit, e.g. `37.2%`, `12.3 MB`, `1650 MHz`, `4.1M`.
pub fn format_counter(unit: CounterUnit, value: f64) -> String {
    let scaled = |value: f64, suffix: &str| {
        let (value, prefix) = match value.abs() {
            v if v >= 1e9 => (value / 1e9, "G"),
            v if v >= 1e6 => (value / 1e6, "M"),
            v if v >= 1e3 => (value / 1e3, "k"),
            _ => (value, ""),
        };
        format!("{value:.1}{prefix}{suffix}")
    };
    match unit {
        CounterUnit::Percentage => format!("{value:.1}%"),
        CounterUnit::Nanoseconds => format!("{:.3}ms", value / 1e6),
        CounterUnit::Bytes => scaled(value, "B"),
        CounterUnit::BytesPerSecond => scaled(value, "B/s"),
        CounterUnit::Hertz => format!("{:.0}MHz", value / 1e6),
        CounterUnit::Kelvin => format!("{value:.0}K"),
        CounterUnit::Watts => format!("{value:.1}W"),
        CounterUnit::Volts => format!("{value:.2}V"),
        CounterUnit::Amps => format!("{value:.2}A"),
        CounterUnit::Cycles | CounterUnit::Generic => scaled(value, ""),
    }
}

/// A pass's clock region over the frames that have region times.
pub struct RegionTotals {
    pub pass: &'static str,
    pub region: &'static str,
    /// Subgroup clock ticks per frame, summed over subgroups.
    pub ticks: f64,
    pub subgroups: f64,
    /// Of all regions' ticks.
    pub share: f64,
}

/// The shader clock regions of `frames` per pass and region, the largest first.
pub fn region_times(frames: &[FrameProfile]) -> Vec<RegionTotals> {
    let with_times = frames.iter().filter(|f| !f.regions.is_empty());
    let n = with_times.clone().count().max(1) as f64;
    let mut totals: Vec<RegionTotals> = Vec::new();
    for time in with_times.flat_map(|f| &f.regions) {
        let i = match totals
            .iter()
            .position(|t| t.pass == time.pass && t.region == time.region)
        {
            Some(i) => i,
            None => {
                totals.push(RegionTotals {
                    pass: time.pass,
                    region: time.region,
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

    let counters = frames.first().map(|f| f.counters.clone()).unwrap_or_default();
    if scopes.iter().any(|scope| scope.counters.is_some()) {
        writeln!(
            out,
            "\nGPU counters per pass (amounts per frame, rates averaged over the pass's time)"
        )
        .unwrap();
        for (i, counter) in counters.iter().enumerate() {
            writeln!(out, "  c{}: {} ({})", i + 1, counter.name, counter.category).unwrap();
        }
        write!(out, "  {:<24}", "scope").unwrap();
        for i in 0..counters.len() {
            write!(out, " {:>11}", format!("c{}", i + 1)).unwrap();
        }
        writeln!(out).unwrap();
        for scope in scopes.iter().filter(|scope| scope.counters.is_some()) {
            let name = format!("{}{}", "  ".repeat(scope.depth as usize), scope.name);
            write!(out, "  {name:<24}").unwrap();
            for (counter, value) in counters.iter().zip(scope.counters.unwrap()) {
                write!(out, " {:>11}", format_counter(counter.unit, value)).unwrap();
            }
            writeln!(out).unwrap();
        }
    }

    let regions = region_times(frames);
    if !regions.is_empty() {
        writeln!(
            out,
            "\nShader clock regions per frame: subgroup clock ticks from `CLOCK_REGION`, summed over subgroups\n(they run in parallel; the share says which regions keep the GPU busy, not the time)\n  {:<30} {:>8} {:>10} {:>12} {:>14}",
            "pass region", "share", "subgroups", "Mticks", "ticks/subgroup"
        )
        .unwrap();
        for time in &regions {
            writeln!(
                out,
                "  {:<30} {:>7.1}% {:>10.0} {:>12.3} {:>14.0}",
                format!("{} {}", time.pass, time.region),
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
            let mut args = json!({ "frame": frame.frame });
            if let Some(values) = scope.counters {
                for (counter, value) in frame.counters.iter().zip(values) {
                    args[&counter.name] = json!(value);
                }
            }
            events.push(json!({
                "name": scope.name, "cat": "gpu", "ph": "X", "pid": 1, "tid": 0,
                "ts": us(frame.gpu_submit_ns + scope.start_ns),
                "dur": us(scope.end_ns - scope.start_ns),
                "args": args,
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
    use lava::profiling::{GpuScope, RegionTime};

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
                    counters: None,
                })
                .collect(),
            gpu_submit_ns: start_ns,
            counters: Default::default(),
            regions: vec![
                RegionTime {
                    pass: "BvhCull",
                    region: "traverse",
                    ticks: 3000,
                    subgroups: 10,
                },
                RegionTime {
                    pass: "Composite",
                    region: "sky",
                    ticks: 1000,
                    subgroups: 20,
                },
            ],
        }
    }

    #[test]
    fn region_times_are_per_frame_and_sorted_by_share() {
        let frames = [frame(0, 10, &[]), frame(1, 10, &[])];
        let times = region_times(&frames);
        let rows: Vec<_> = times
            .iter()
            .map(|t| (t.pass, t.region, t.ticks, t.subgroups, t.share))
            .collect();
        assert_eq!(
            rows,
            [
                ("BvhCull", "traverse", 3000.0, 10.0, 0.75),
                ("Composite", "sky", 1000.0, 20.0, 0.25)
            ]
        );
    }

    #[test]
    fn counter_values_format_with_their_unit() {
        assert_eq!(format_counter(CounterUnit::Percentage, 37.24), "37.2%");
        assert_eq!(format_counter(CounterUnit::Bytes, 12_300_000.0), "12.3MB");
        assert_eq!(format_counter(CounterUnit::Hertz, 1.65e9), "1650MHz");
        assert_eq!(format_counter(CounterUnit::Generic, 4_100_000.0), "4.1M");
        assert_eq!(format_counter(CounterUnit::Nanoseconds, 250_000.0), "0.250ms");
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
