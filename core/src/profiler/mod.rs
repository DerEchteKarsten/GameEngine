//! Profiler: per-frame wall, CPU and GPU times plus rings of GPU scopes and shader clocks, and the one frame profile (CPU spans from the threads' rings, GPU scopes) built when a frame is selected; shown in the editor's Profiler tab and saved as a text summary plus a Chrome trace.
use std::{collections::VecDeque, ops::Range, path::Path, path::PathBuf};

use bevy::{
    app::{App, Last, Plugin},
    ecs::{resource::Resource, system::ResMut},
    log::{error, info, warn},
};
use lava::profiling::{FrameTimings, GpuScope, MemoryReport, ShaderTime, memory_report};

use crate::{
    profiler::capture::{CpuSpan, now_ns},
    render::{ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, RenderApp},
};

pub mod capture;
pub mod report;
pub mod ui;

/// Frames the profiler keeps.
pub const MAX_FRAMES: usize = 300;
/// How many frames after its submission a frame's GPU timings arrive at most.
const GPU_LATENCY: usize = FRAMES_IN_FLIGHT + 1;
/// GPU scopes and shader times kept per frame.
const GPU_ENTRIES_PER_FRAME: usize = 64;
/// How many frames after it ended a frame's busy-time counters are read: the root spans that
/// started in it (main's update, the render thread's frame) end only after `end_frame`.
const BUSY_DELAY: u32 = 2;

/// One frame's times: the time between two ends of the main schedule.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    pub frame: u32,
    pub start_ns: u64,
    /// 0 while the frame runs.
    pub end_ns: u64,
    /// The busiest thread's time. While recording it counts the spans that started in the
    /// frame; pausing replaces it by the exact time in the frame's window.
    pub cpu_ns: u64,
    /// Duration of the root GPU scopes, once they arrived.
    pub gpu_ns: Option<u64>,
}

impl FrameTimes {
    /// Wall time, which includes waiting for the GPU.
    pub fn frame_ns(&self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns)
    }
}

/// One thread's spans of a frame.
#[derive(Clone, Debug)]
pub struct ThreadSpans {
    pub thread: &'static str,
    /// The time of the frame's window the thread spent in spans, but not in wait spans.
    pub busy_ns: u64,
    /// The spans that started in the frame.
    pub spans: Vec<CpuSpan>,
}

/// Everything recorded during one frame.
#[derive(Clone, Debug, Default)]
pub struct FrameProfile {
    pub frame: u32,
    pub start_ns: u64,
    pub end_ns: u64,
    /// Main, render thread, then the others by name.
    pub threads: Vec<ThreadSpans>,
    /// The GPU work submitted in this frame, relative to `gpu_submit_ns`.
    pub gpu: Vec<GpuScope>,
    pub gpu_submit_ns: u64,
    /// The subgroup clock ticks of every instrumented pass and stage (`profile.slang`).
    pub shaders: Vec<ShaderTime>,
    /// False if a thread's ring overwrote spans of this frame.
    pub complete: bool,
}

impl FrameProfile {
    pub fn frame_ns(&self) -> u64 {
        self.end_ns - self.start_ns
    }

    /// Busy time of the busiest thread: main and render thread work in parallel, so the
    /// frame can't be shorter.
    pub fn cpu_ns(&self) -> u64 {
        self.threads.iter().map(|t| t.busy_ns).max().unwrap_or(0)
    }

    /// Duration of the root scopes, `None` without GPU timings.
    pub fn gpu_ns(&self) -> Option<u64> {
        (!self.gpu.is_empty()).then(|| root_ns(&self.gpu))
    }
}

fn root_ns(scopes: &[GpuScope]) -> u64 {
    let roots = scopes.iter().filter(|s| s.depth == 0);
    roots.map(|s| s.end_ns - s.start_ns).sum()
}

/// Calls `add` with the index of every frame `span` overlaps and `sign` times its time in it.
fn busy_per_frame(
    times: &VecDeque<FrameTimes>,
    span: &CpuSpan,
    sign: i64,
    mut add: impl FnMut(usize, i64),
) {
    let first = times.partition_point(|t| t.frame < span.frame);
    for (i, frame) in times.range(first..).enumerate() {
        if frame.start_ns >= span.end_ns {
            break;
        }
        let ns = span.end_ns.min(frame.end_ns).saturating_sub(span.start_ns.max(frame.start_ns));
        add(first + i, sign * ns as i64);
    }
}

#[derive(Resource)]
pub struct Profiler {
    /// The last frames, oldest first. While recording, the last one is the running frame.
    pub times: VecDeque<FrameTimes>,
    /// Off, frames only have their wall time: no spans, GPU queries or shader clocks.
    pub enabled: bool,
    /// Paused, no frames are added and no spans recorded.
    pub paused: bool,
    /// The frame picked in the graphs, built once by `select`.
    pub selected: Option<FrameProfile>,
    pub memory: MemoryReport,
    capacity: usize,
    /// Every GPU scope of the last frames with its frame and submit time.
    gpu: VecDeque<(u32, u64, GpuScope)>,
    shaders: VecDeque<(u32, ShaderTime)>,
    memory_updated: Option<u64>,
    capture: Option<(usize, PathBuf)>,
}

impl Profiler {
    /// Pausing stops recording, drops the running frame and replaces the CPU times by the
    /// exact ones; resuming drops the selection.
    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        capture::set_enabled(self.enabled && !paused);
        if paused {
            self.times.pop_back();
            if self.enabled {
                self.exact_cpu_times();
            }
        } else {
            self.selected = None;
            self.start_frame(now_ns());
        }
    }

    /// Starts over, since frames of both kinds don't mix.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.paused = false;
        self.selected = None;
        self.times.clear();
        self.gpu.clear();
        self.shaders.clear();
        capture::set_enabled(enabled);
        self.start_frame(now_ns());
    }

    /// Pauses and builds the profile of `times[i]`.
    pub fn select(&mut self, i: usize) {
        self.set_paused(true);
        self.selected = self.profiles(i..i + 1).pop();
    }

    /// Hands the GPU timings of the work submitted in `frame` (at `submit_ns`) to it.
    pub fn add_gpu(&mut self, frame: u32, submit_ns: u64, timings: &FrameTimings) {
        let Some(i) = self.index(frame) else { return };
        self.times[i].gpu_ns = Some(root_ns(&timings.scopes));
        let capacity = self.capacity * GPU_ENTRIES_PER_FRAME;
        for scope in &timings.scopes {
            if self.gpu.len() == capacity {
                self.gpu.pop_front();
            }
            self.gpu.push_back((frame, submit_ns, scope.clone()));
        }
        for time in &timings.shaders {
            if self.shaders.len() == capacity {
                self.shaders.pop_front();
            }
            self.shaders.push_back((frame, time.clone()));
        }
    }

    /// Pauses and writes `profile.txt` (the summary) and `profile.json` (a Chrome trace for
    /// Perfetto or chrome://tracing) of the frames whose spans are all still there into `dir`.
    pub fn save(&mut self, dir: &Path) -> std::io::Result<()> {
        self.set_paused(true);
        let mut frames = self.profiles(0..self.times.len());
        let all = frames.len();
        frames.retain(|f| f.complete);
        if frames.len() < all {
            warn!(
                "{} of {all} frames lost spans to full rings and are left out of the profile",
                all - frames.len()
            );
        }
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("profile.txt"),
            report::summary(&frames, &self.memory),
        )?;
        std::fs::write(dir.join("profile.json"), report::chrome_trace(&frames))
    }

    fn index(&self, frame: u32) -> Option<usize> {
        let i = self.times.partition_point(|t| t.frame < frame);
        (self.times.get(i)?.frame == frame).then_some(i)
    }

    fn start_frame(&mut self, now: u64) {
        // `capacity` frames plus the running one.
        if self.times.len() > self.capacity {
            self.times.pop_front();
        }
        self.times.push_back(FrameTimes {
            frame: capture::frame(),
            start_ns: now,
            ..Default::default()
        });
    }

    /// The busiest thread's time in each frame's window, from every recorded span.
    fn exact_cpu_times(&mut self) {
        let times = &mut self.times;
        times.iter_mut().for_each(|t| t.cpu_ns = 0);
        let mut busy = vec![0i64; times.len()];
        capture::for_each_thread(|_, _, spans| {
            busy.fill(0);
            capture::busy_signs(spans, |span, sign| {
                if sign != 0 {
                    busy_per_frame(times, span, sign, |i, ns| busy[i] += ns);
                }
            });
            for (frame, busy) in times.iter_mut().zip(&busy) {
                frame.cpu_ns = frame.cpu_ns.max((*busy).max(0) as u64);
            }
        });
    }

    /// The profiles of `times[range]`, from one pass over every span. Only while paused.
    fn profiles(&self, range: Range<usize>) -> Vec<FrameProfile> {
        let times = &self.times;
        let mut frames: Vec<FrameProfile> = times
            .range(range.clone())
            .map(|t| FrameProfile {
                frame: t.frame,
                start_ns: t.start_ns,
                end_ns: t.end_ns,
                complete: true,
                ..Default::default()
            })
            .collect();
        let mut busy = vec![0i64; frames.len()];
        capture::for_each_thread(|thread, complete_from, spans| {
            busy.fill(0);
            for frame in &mut frames {
                frame.complete &= frame.start_ns >= complete_from;
                frame.threads.push(ThreadSpans {
                    thread,
                    busy_ns: 0,
                    spans: Vec::new(),
                });
            }
            capture::busy_signs(spans, |span, sign| {
                if sign != 0 {
                    busy_per_frame(times, span, sign, |i, ns| {
                        if range.contains(&i) {
                            busy[i - range.start] += ns;
                        }
                    });
                }
                let first = times.partition_point(|t| t.frame < span.frame);
                if range.contains(&first) && times[first].frame == span.frame {
                    let frame = &mut frames[first - range.start];
                    frame.threads.last_mut().unwrap().spans.push(*span);
                }
            });
            for (frame, busy) in frames.iter_mut().zip(&busy) {
                let thread = frame.threads.last_mut().unwrap();
                thread.busy_ns = (*busy).max(0) as u64;
                if thread.spans.is_empty() && thread.busy_ns == 0 {
                    frame.threads.pop();
                }
            }
        });
        for frame in &mut frames {
            frame
                .threads
                .sort_by_key(|t| (t.thread != "main", t.thread != "render thread", t.thread));
            for (_, submit_ns, scope) in self.gpu.iter().filter(|(f, ..)| *f == frame.frame) {
                frame.gpu_submit_ns = *submit_ns;
                frame.gpu.push(scope.clone());
            }
            let shaders = self.shaders.iter().filter(|(f, _)| *f == frame.frame);
            frame.shaders = shaders.map(|(_, time)| time.clone()).collect();
        }
        frames
    }
}

/// Render world: per frame slot, the frame number and time of its last submission, and the
/// GPU timings it read back of the one before, until extract hands them over.
#[derive(Resource, Default)]
pub struct GpuTimings {
    /// Whether the frame slots are profiled: `Profiler::enabled`.
    pub enabled: bool,
    pub slots: [GpuSlot; FRAMES_IN_FLIGHT],
}

#[derive(Default)]
pub struct GpuSlot {
    pub submitted: (u32, u64),
    /// The frame and submit time of `timings`.
    pub read: (u32, u64),
    pub timings: FrameTimings,
    pub ready: bool,
}

impl GpuSlot {
    /// Keeps the timings of the slot's last submission, reusing their buffers.
    pub fn read_back(&mut self, timings: &FrameTimings) {
        self.read = self.submitted;
        self.timings.scopes.clone_from(&timings.scopes);
        self.timings.shaders.clone_from(&timings.shaders);
        self.ready = true;
    }
}

fn extract_gpu_timings(mut timings: ResMut<GpuTimings>, mut main_world: ResMut<MainWorld>) {
    let mut profiler = main_world.get_resource_mut::<Profiler>();
    timings.enabled = profiler.as_ref().is_some_and(|p| p.enabled);
    for slot in timings.slots.iter_mut().filter(|slot| slot.ready) {
        slot.ready = false;
        if let Some(profiler) = &mut profiler {
            profiler.add_gpu(slot.read.0, slot.read.1, &slot.timings);
        }
    }
}

fn end_frame(mut profiler: ResMut<Profiler>) {
    let now = now_ns();
    let frame = capture::frame();
    capture::set_frame(frame + 1);
    let profiler = &mut *profiler;
    if profiler.enabled
        && profiler
            .memory_updated
            .is_none_or(|updated| now - updated > 1_000_000_000)
    {
        profiler.memory = memory_report();
        profiler.memory_updated = Some(now);
    }
    if profiler.paused {
        return;
    }
    if let Some(running) = profiler.times.back_mut() {
        running.end_ns = now;
    }
    profiler.start_frame(now);
    if profiler.enabled
        && let Some(done) = frame.checked_sub(BUSY_DELAY)
    {
        let busy = capture::take_busy(done);
        if let Some(i) = profiler.index(done) {
            profiler.times[i].cpu_ns = busy;
        }
    }

    if profiler
        .capture
        .as_ref()
        .is_some_and(|(frames, _)| frame as usize == frames + GPU_LATENCY)
    {
        let (frames, dir) = profiler.capture.take().unwrap();
        match profiler.save(&dir) {
            Ok(()) => info!("saved a profile of {frames} frames to {}", dir.display()),
            Err(err) => error!("failed to save the profile to {}: {err}", dir.display()),
        }
        profiler.set_paused(false);
    }
}

/// Records every frame into [`Profiler`]. The spans come from [`capture::ProfileLayer`],
/// which has to be part of the tracing subscriber.
pub struct ProfilerPlugin {
    /// Whether to start profiling; `Profiler::set_enabled` switches it later. A capture
    /// enables it.
    pub enabled: bool,
    /// Saves a profile of the first this many frames into the directory (see
    /// [`Profiler::save`]), once their GPU timings arrived.
    pub capture: Option<(usize, PathBuf)>,
}

impl Default for ProfilerPlugin {
    fn default() -> Self {
        Self {
            enabled: true,
            capture: None,
        }
    }
}

impl Plugin for ProfilerPlugin {
    fn build(&self, app: &mut App) {
        let enabled = self.enabled || self.capture.is_some();
        let capacity = MAX_FRAMES.max(self.capture.as_ref().map_or(0, |c| c.0 + GPU_LATENCY + 1));
        let mut profiler = Profiler {
            times: VecDeque::with_capacity(capacity + 1),
            enabled,
            paused: false,
            selected: None,
            memory: MemoryReport::default(),
            capacity,
            gpu: VecDeque::with_capacity(capacity * GPU_ENTRIES_PER_FRAME),
            shaders: VecDeque::with_capacity(capacity * GPU_ENTRIES_PER_FRAME),
            memory_updated: None,
            capture: self.capture.clone(),
        };
        capture::set_enabled(enabled);
        profiler.start_frame(now_ns());
        app.insert_resource(profiler).add_systems(Last, end_frame);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<GpuTimings>()
                .add_systems(ExtractSchedule, extract_gpu_timings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiler::capture::SpanKind;

    fn span(frame: u32, start_ns: u64, end_ns: u64) -> CpuSpan {
        CpuSpan {
            name: "span",
            start_ns,
            end_ns,
            frame,
            depth: 0,
            kind: SpanKind::Other,
        }
    }

    #[test]
    fn busy_time_is_clipped_to_each_frame_window() {
        let times: VecDeque<_> = [(0, 0, 10), (1, 10, 20), (3, 25, 40)]
            .map(|(frame, start_ns, end_ns)| FrameTimes {
                frame,
                start_ns,
                end_ns,
                ..Default::default()
            })
            .into();
        let mut busy = [0; 3];
        // A root reaching from frame 0 into frame 1 and past it, a wait inside it.
        for (span, sign) in [(span(0, 5, 30), 1), (span(1, 12, 15), -1)] {
            busy_per_frame(&times, &span, sign, |i, ns| busy[i] += ns);
        }
        assert_eq!(busy, [5, 7, 5]);
        // A span of a frame that wasn't recorded counts from the next one on.
        let mut busy = [0; 3];
        busy_per_frame(&times, &span(2, 22, 30), 1, |i, ns| busy[i] += ns);
        assert_eq!(busy, [0, 0, 5]);
    }
}
