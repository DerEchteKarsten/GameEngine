//! Profiler: CPU spans and GPU pass timings per frame for the last frames, shown in the editor's Profiler tab and saved as a text summary plus a Chrome trace.
use std::{collections::VecDeque, path::Path, path::PathBuf};

use bevy::{
    app::{App, Last, Plugin},
    ecs::{resource::Resource, system::ResMut},
    log::{error, info},
};
use lava::profiling::{GpuScope, MemoryReport, memory_report};
use tracy_client::{GpuContext, GpuContextType, GpuSpan};

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

/// Everything recorded during one frame: the time between two ends of the main schedule.
#[derive(Clone, Debug, Default)]
pub struct FrameProfile {
    pub frame: u64,
    pub start_ns: u64,
    pub end_ns: u64,
    /// The spans that started in this frame, on any thread.
    pub cpu: Vec<CpuSpan>,
    /// The GPU work submitted in this frame, relative to `gpu_submit_ns`; it arrives a few
    /// frames later.
    pub gpu: Vec<GpuScope>,
    pub gpu_submit_ns: u64,
}

impl FrameProfile {
    pub fn cpu_ns(&self) -> u64 {
        self.end_ns - self.start_ns
    }

    /// Duration of the root scopes, `None` before the GPU timings arrived.
    pub fn gpu_ns(&self) -> Option<u64> {
        let roots = self.gpu.iter().filter(|s| s.depth == 0);
        (!self.gpu.is_empty()).then(|| roots.map(|s| s.end_ns - s.start_ns).sum())
    }
}

/// GPU scopes of one submission, read back once it finished.
pub struct GpuFrame {
    pub submit_ns: u64,
    pub scopes: Vec<GpuScope>,
}

#[derive(Resource)]
pub struct Profiler {
    pub frames: VecDeque<FrameProfile>,
    pub paused: bool,
    /// Index into `frames`, shown in the timeline. Selecting pauses, so it stays valid.
    pub selected: Option<usize>,
    pub memory: MemoryReport,
    capacity: usize,
    gpu_pending: Vec<GpuFrame>,
    frame_start: u64,
    next_frame: u64,
    memory_updated: Option<u64>,
    capture: Option<(usize, PathBuf)>,
    /// Tracy's GPU lane and the last timestamp uploaded to it.
    tracy: Option<(GpuContext, i64)>,
}

impl Profiler {
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        capture::set_enabled(!paused);
        if !paused {
            self.selected = None;
        }
    }

    /// Hands GPU timings to the frame they were submitted in.
    pub fn add_gpu(&mut self, frame: GpuFrame) {
        self.gpu_pending.push(frame);
    }

    /// Writes `profile.txt` (the summary) and `profile.json` (a Chrome trace for Perfetto or
    /// chrome://tracing) into `dir`.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let frames = self.frames.iter().cloned().collect::<Vec<_>>();
        std::fs::create_dir_all(dir)?;
        std::fs::write(
            dir.join("profile.txt"),
            report::summary(&frames, &self.memory),
        )?;
        std::fs::write(dir.join("profile.json"), report::chrome_trace(&frames))
    }
}

/// Render world: the GPU timings read back since the last extract. While it exists, the
/// frame slots are profiled.
#[derive(Resource, Default)]
pub struct GpuTimings {
    /// When each frame slot last submitted.
    pub submitted: [u64; FRAMES_IN_FLIGHT],
    pub frames: Vec<GpuFrame>,
}

fn extract_gpu_timings(mut timings: ResMut<GpuTimings>, mut main_world: ResMut<MainWorld>) {
    if let Some(mut profiler) = main_world.get_resource_mut::<Profiler>() {
        profiler.gpu_pending.append(&mut timings.frames);
    }
}

/// Shows the frame's GPU scopes on a Tracy GPU lane. Its clock is the profiler's, so the
/// scopes sit where they were submitted rather than where the GPU ran them.
fn upload_to_tracy(tracy: &mut Option<(GpuContext, i64)>, frame: &FrameProfile) {
    let Some(client) = tracy_client::Client::running() else {
        return;
    };
    if tracy.is_none() {
        let context =
            client.new_gpu_context(Some("GPU"), GpuContextType::Vulkan, now_ns() as i64, 1.0);
        *tracy = context.ok().map(|context| (context, 0));
    }
    let Some((context, last)) = tracy else { return };
    // Tracy wants the timestamps in increasing order, but frames may overlap on this clock.
    let mut timestamp = |ns: u64| {
        *last = (*last).max((frame.gpu_submit_ns + ns) as i64);
        *last
    };
    // Scopes are in pre-order: before a scope starts, the open ones at its depth or deeper end.
    let mut open: Vec<(GpuSpan, u16, u64)> = Vec::new();
    for scope in frame.gpu.iter().map(Some).chain([None]) {
        let depth = scope.map_or(0, |s| s.depth);
        while open.last().is_some_and(|(_, d, _)| *d >= depth) {
            let (mut span, _, end) = open.pop().unwrap();
            span.end_zone();
            span.upload_timestamp_end(timestamp(end));
        }
        let Some(scope) = scope else { break };
        let Ok(span) = context.span_alloc(scope.name, "", file!(), line!()) else {
            return;
        };
        span.upload_timestamp_start(timestamp(scope.start_ns));
        open.push((span, scope.depth, scope.end_ns));
    }
}

fn end_frame(mut profiler: ResMut<Profiler>) {
    tracy_client::frame_mark();
    let now = now_ns();
    let start = std::mem::replace(&mut profiler.frame_start, now);
    let spans = capture::drain();
    if profiler.paused {
        profiler.gpu_pending.clear();
        return;
    }
    let profiler = &mut *profiler;
    let frame = profiler.next_frame;
    profiler.next_frame += 1;
    profiler.frames.push_back(FrameProfile {
        frame,
        start_ns: start,
        end_ns: now,
        ..Default::default()
    });
    while profiler.frames.len() > profiler.capacity {
        profiler.frames.pop_front();
    }

    // The render thread's spans and the GPU timings belong to earlier frames.
    let frames = &mut profiler.frames;
    let containing = |frames: &VecDeque<FrameProfile>, ns: u64| {
        let i = frames.partition_point(|f| f.start_ns <= ns);
        (i > 0 && ns < frames[i - 1].end_ns).then(|| i - 1)
    };
    for span in spans {
        if let Some(i) = containing(frames, span.start_ns) {
            frames[i].cpu.push(span);
        }
    }
    profiler.gpu_pending.retain_mut(|gpu| {
        if gpu.submit_ns >= now {
            return true;
        }
        if let Some(i) = containing(frames, gpu.submit_ns) {
            frames[i].gpu = std::mem::take(&mut gpu.scopes);
            frames[i].gpu_submit_ns = gpu.submit_ns;
            upload_to_tracy(&mut profiler.tracy, &frames[i]);
        }
        false
    });

    if profiler
        .memory_updated
        .is_none_or(|updated| now - updated > 1_000_000_000)
    {
        profiler.memory = memory_report();
        profiler.memory_updated = Some(now);
    }

    if let Some((frames, dir)) = &profiler.capture
        && profiler.next_frame as usize == frames + GPU_LATENCY
    {
        match profiler.save(dir) {
            Ok(()) => info!("saved a profile of {frames} frames to {}", dir.display()),
            Err(err) => error!("failed to save the profile to {}: {err}", dir.display()),
        }
    }
}

/// Records every frame into [`Profiler`]. The spans come from [`capture::ProfileLayer`],
/// which has to be part of the tracing subscriber.
#[derive(Default)]
pub struct ProfilerPlugin {
    /// Saves a profile of the first this many frames into the directory (see
    /// [`Profiler::save`]), once their GPU timings arrived.
    pub capture: Option<(usize, PathBuf)>,
}

impl Plugin for ProfilerPlugin {
    fn build(&self, app: &mut App) {
        capture::set_enabled(true);
        app.insert_resource(Profiler {
            frames: VecDeque::new(),
            paused: false,
            selected: None,
            memory: MemoryReport::default(),
            capacity: MAX_FRAMES.max(self.capture.as_ref().map_or(0, |c| c.0 + GPU_LATENCY)),
            gpu_pending: Vec::new(),
            frame_start: now_ns(),
            next_frame: 0,
            memory_updated: None,
            capture: self.capture.clone(),
            tracy: None,
        })
        .add_systems(Last, end_frame);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<GpuTimings>()
                .add_systems(ExtractSchedule, extract_gpu_timings);
        }
    }
}
