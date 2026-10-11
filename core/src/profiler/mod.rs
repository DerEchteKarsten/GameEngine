//! Profiler: the frame timeline (frame numbers, wall and GPU times) that four independent systems tag their data with, CPU spans, GPU timestamps, pass queries (pipeline statistics and hardware counters) and shader clocks, each with its own switch, plus the one frame profile built when a frame is selected; shown in the editor's Profiler tab and saved as a text summary plus a Chrome trace.
use std::{
    collections::VecDeque,
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Instant,
};

use bevy::{
    app::{App, Last, Plugin},
    ecs::{resource::Resource, system::ResMut},
    log::{error, info, warn},
};
use lava::{
    profiling::{
        Counter, GpuScope, MAX_COUNTERS, MemoryReport, PassCounters, PassResult, RegionTime,
        clock, counters, memory_report,
    },
    vkobjects::queue::Frame,
};

use crate::{
    profiler::spans::CpuSpan,
    render::{ExtractSchedule, FRAMES_IN_FLIGHT, MainWorld, RenderApp},
};

pub mod report;
pub mod spans;
pub mod ui;

/// Frames the profiler keeps.
pub const MAX_FRAMES: usize = 300;
/// How many frames after its submission a frame's GPU results arrive at most.
const GPU_LATENCY: usize = FRAMES_IN_FLIGHT + 1;
/// GPU scopes, pass results and shader clock regions kept per frame.
const GPU_ENTRIES_PER_FRAME: usize = 64;
/// The counters `ProfilerPlugin` selects by default: clock, vector engine use and L1 traffic on
/// Intel GPUs, which fit one pass. ANV reads 0 per pass for some counters on the B390 (GPU
/// Busy, GPU Memory Byte Read/Write, L3 Read/Miss), so none of those. Other devices name their
/// counters differently, so this selects nothing there.
pub const DEFAULT_COUNTERS: [&str; 8] = [
    "AVG GPU Core Frequency",
    "XVE Active",
    "XVE Stall",
    "XVE Threads Occupancy All",
    "XVE Inst Executed ALU0 All",
    "Load Store Cache Byte Read",
    "Load Store Cache Byte Write",
    "Load Store Cache Hit",
];

static FRAME: AtomicU32 = AtomicU32::new(0);
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);

/// The clock of all profiler times.
pub fn now_ns() -> u64 {
    EPOCH.elapsed().as_nanos() as u64
}

/// The running frame, which spans and submissions are tagged with.
pub fn frame() -> u32 {
    FRAME.load(Ordering::Relaxed)
}

/// One frame's times: the time between two ends of the main schedule.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTimes {
    pub frame: u32,
    pub start_ns: u64,
    /// 0 while the frame runs.
    pub end_ns: u64,
    /// Duration of the root GPU scopes, once they arrived.
    pub gpu_ns: Option<u64>,
}

impl FrameTimes {
    /// Wall time, which includes waiting for the GPU.
    pub fn frame_ns(&self) -> u64 {
        self.end_ns.saturating_sub(self.start_ns)
    }
}

/// One thread's spans that started in a frame.
#[derive(Clone, Debug)]
pub struct ThreadSpans {
    pub thread: &'static str,
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
    /// The pass queries of the work submitted in this frame.
    pub passes: Vec<PassResult>,
    /// The counters the passes' values belong to.
    pub counters: Arc<[&'static Counter]>,
    /// The subgroup clock ticks of every clock region (`CLOCK_REGION` in `profile.slang`).
    pub regions: Vec<RegionTime>,
    /// False if a thread's ring overwrote spans of this frame.
    pub complete: bool,
}

impl FrameProfile {
    pub fn frame_ns(&self) -> u64 {
        self.end_ns - self.start_ns
    }

    /// Duration of the root scopes, `None` without GPU timestamps.
    pub fn gpu_ns(&self) -> Option<u64> {
        (!self.gpu.is_empty()).then(|| root_ns(&self.gpu))
    }
}

fn root_ns(scopes: &[GpuScope]) -> u64 {
    let roots = scopes.iter().filter(|s| s.depth == 0);
    roots.map(|s| s.end_ns - s.start_ns).sum()
}

/// Appends to a ring of `capacity` entries, dropping the oldest when it is full.
fn push<T>(ring: &mut VecDeque<T>, capacity: usize, entry: T) {
    if ring.len() == capacity {
        ring.pop_front();
    }
    ring.push_back(entry);
}

#[derive(Resource)]
pub struct Profiler {
    /// The last frames, oldest first. While recording, the last one is the running frame.
    pub times: VecDeque<FrameTimes>,
    /// The switches of the four systems. Off, a system records nothing new.
    pub spans: bool,
    pub timestamps: bool,
    pub counters: bool,
    pub clocks: bool,
    /// Paused, no frames are added and no spans recorded.
    pub paused: bool,
    /// The frame picked in the graphs, built once by `select`.
    pub selected: Option<FrameProfile>,
    pub memory: MemoryReport,
    capacity: usize,
    /// The hardware counters the pass queries record.
    pub selection: Arc<[&'static Counter]>,
    /// Per counter of `lava::profiling::counters::counters()`: whether it is selected or can
    /// be added to the selection. Computed when the selection changes.
    pub compatible: Vec<bool>,
    /// Counter names to select once lava is initialised.
    pending_counters: Option<Vec<String>>,
    /// Every GPU scope of the last frames with its frame and submit time.
    gpu: VecDeque<(u32, u64, GpuScope)>,
    passes: VecDeque<(u32, PassResult)>,
    regions: VecDeque<(u32, RegionTime)>,
    memory_updated: Option<u64>,
    capture: Option<(usize, PathBuf)>,
}

impl Profiler {
    /// Pausing stops recording and drops the running frame; resuming drops the selection.
    pub fn set_paused(&mut self, paused: bool) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        spans::set_enabled(self.spans && !paused);
        if paused {
            self.times.pop_back();
        } else {
            self.selected = None;
            self.start_frame(now_ns());
        }
    }

    /// Pauses and builds the profile of `times[i]`.
    pub fn select(&mut self, i: usize) {
        self.set_paused(true);
        self.selected = self.profiles(i..i + 1).pop();
    }

    /// Selects the counters the pass queries record. A selection of more than `MAX_COUNTERS`
    /// or of more than one pass is refused with an error in the log.
    pub fn set_counters(&mut self, selection: &[&'static Counter]) {
        let fits = |selection: &[&Counter]| {
            selection.len() <= MAX_COUNTERS
                && (selection.is_empty() || counters::passes(selection) == 1)
        };
        if !fits(selection) {
            error!("can't select these counters: they don't fit one pass");
            return;
        }
        self.selection = selection.into();
        // Values recorded with the old selection would be read as the new one's.
        self.passes.iter_mut().for_each(|(_, pass)| pass.values = None);
        self.compatible = counters::counters()
            .iter()
            .map(|counter| {
                let mut with = selection.to_vec();
                with.push(counter);
                selection.iter().any(|s| std::ptr::eq(*s, counter)) || fits(&with)
            })
            .collect();
    }

    /// Selects the counters with these names, in order, skipping those that don't exist or
    /// don't fit.
    fn select_counters_by_name(&mut self, names: &[String]) {
        let mut selection: Vec<&'static Counter> = Vec::new();
        let mut skipped = Vec::new();
        for name in names {
            let counter = counters::counters().iter().find(|c| c.name == *name);
            match counter {
                Some(counter) if selection.len() < MAX_COUNTERS => {
                    selection.push(counter);
                    if counters::passes(&selection) != 1 {
                        selection.pop();
                        skipped.push(name.as_str());
                    }
                }
                _ => skipped.push(name.as_str()),
            }
        }
        if !skipped.is_empty() && !counters::counters().is_empty() {
            warn!(
                "counters not recorded (unknown, or not in one pass with the others): {skipped:?}"
            );
        }
        self.set_counters(&selection);
    }

    /// Hands the GPU scopes of the work submitted in `frame` (at `submit_ns`) to it.
    pub fn add_timestamps(&mut self, frame: u32, submit_ns: u64, scopes: &[GpuScope]) {
        let Some(i) = self.index(frame) else { return };
        self.times[i].gpu_ns = Some(root_ns(scopes));
        let capacity = self.capacity * GPU_ENTRIES_PER_FRAME;
        for scope in scopes {
            push(&mut self.gpu, capacity, (frame, submit_ns, scope.clone()));
        }
    }

    /// Hands the pass queries of the work submitted in `frame` to it.
    pub fn add_counters(&mut self, frame: u32, counters: &PassCounters) {
        if self.index(frame).is_none() {
            return;
        }
        // Frames in flight across a new selection have the old one's counters.
        let current = Arc::ptr_eq(&counters.counters, &self.selection);
        let capacity = self.capacity * GPU_ENTRIES_PER_FRAME;
        for pass in &counters.passes {
            let mut pass = pass.clone();
            if !current {
                pass.values = None;
            }
            push(&mut self.passes, capacity, (frame, pass));
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
            frame: frame(),
            start_ns: now,
            ..Default::default()
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
        spans::for_each_thread(|thread, complete_from, spans| {
            for frame in &mut frames {
                frame.complete &= frame.start_ns >= complete_from;
                frame.threads.push(ThreadSpans {
                    thread,
                    spans: Vec::new(),
                });
            }
            for span in spans.iter().flat_map(|spans| spans.iter()) {
                let i = times.partition_point(|t| t.frame < span.frame);
                if range.contains(&i) && times[i].frame == span.frame {
                    let frame = &mut frames[i - range.start];
                    frame.threads.last_mut().unwrap().spans.push(*span);
                }
            }
            for frame in &mut frames {
                if frame.threads.last().unwrap().spans.is_empty() {
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
            let passes = self.passes.iter().filter(|(f, _)| *f == frame.frame);
            frame.passes = passes.map(|(_, pass)| pass.clone()).collect();
            frame.counters = self.selection.clone();
            let regions = self.regions.iter().filter(|(f, _)| *f == frame.frame);
            frame.regions = regions.map(|(_, time)| time.clone()).collect();
        }
        frames
    }
}

/// Render world: the GPU switches, and per frame slot the frame number and time of its last
/// submission and the GPU results it read back of the one before, until extract hands them
/// over.
#[derive(Resource, Default)]
pub struct GpuTimings {
    /// `Profiler::timestamps`.
    pub timestamps: bool,
    /// `Profiler::selection` while `Profiler::counters` is on.
    pub counters: Option<Arc<[&'static Counter]>>,
    pub slots: [GpuSlot; FRAMES_IN_FLIGHT],
}

#[derive(Default)]
pub struct GpuSlot {
    pub submitted: (u32, u64),
    /// The frame and submit time of `scopes` and `passes`.
    pub read: (u32, u64),
    pub scopes: Vec<GpuScope>,
    pub passes: PassCounters,
    pub ready: bool,
}

impl GpuSlot {
    /// Keeps the results of the slot's last submission, reusing their buffers.
    pub fn read_back(&mut self, frame: &Frame) {
        self.read = self.submitted;
        self.scopes.clear();
        self.scopes
            .extend_from_slice(frame.last_timestamps().unwrap_or_default());
        self.passes.passes.clear();
        if let Some(passes) = frame.last_counters() {
            self.passes.counters.clone_from(&passes.counters);
            self.passes.passes.clone_from(&passes.passes);
        }
        self.ready = !self.scopes.is_empty() || !self.passes.passes.is_empty();
    }
}

fn extract_gpu_timings(mut timings: ResMut<GpuTimings>, mut main_world: ResMut<MainWorld>) {
    let mut profiler = main_world.get_resource_mut::<Profiler>();
    timings.timestamps = profiler.as_ref().is_some_and(|p| p.timestamps);
    timings.counters = (profiler.as_ref())
        .filter(|p| p.counters)
        .map(|p| p.selection.clone());
    for slot in timings.slots.iter_mut().filter(|slot| slot.ready) {
        slot.ready = false;
        if let Some(profiler) = &mut profiler {
            let (frame, submit_ns) = slot.read;
            if !slot.scopes.is_empty() {
                profiler.add_timestamps(frame, submit_ns, &slot.scopes);
            }
            profiler.add_counters(frame, &slot.passes);
        }
    }
}

fn end_frame(mut profiler: ResMut<Profiler>) {
    let now = now_ns();
    let frame = FRAME.fetch_add(1, Ordering::Relaxed);
    let profiler = &mut *profiler;
    if let Some(names) = profiler.pending_counters.take() {
        profiler.select_counters_by_name(&names);
    }
    spans::set_enabled(profiler.spans && !profiler.paused);
    clock::set_enabled(profiler.clocks);
    if profiler.clocks {
        let capacity = profiler.capacity * GPU_ENTRIES_PER_FRAME;
        let (paused, regions) = (profiler.paused, &mut profiler.regions);
        clock::read(|time| {
            if !paused {
                push(regions, capacity, (frame, time));
            }
        });
    }
    if profiler
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

/// Records every frame into [`Profiler`]. The spans come from [`spans::ProfileLayer`], which
/// has to be part of the tracing subscriber.
pub struct ProfilerPlugin {
    /// The switches the systems start with (see `Profiler`).
    pub spans: bool,
    pub timestamps: bool,
    pub clocks: bool,
    /// The hardware counters the pass queries record, by name (`Profiler::set_counters`
    /// changes them later); default `DEFAULT_COUNTERS`. `None` starts without pass queries.
    pub counters: Option<Vec<String>>,
    /// Saves a profile of the first this many frames into the directory (see
    /// [`Profiler::save`]), once their GPU results arrived.
    pub capture: Option<(usize, PathBuf)>,
}

impl Default for ProfilerPlugin {
    fn default() -> Self {
        Self {
            spans: true,
            timestamps: true,
            clocks: true,
            counters: Some(DEFAULT_COUNTERS.map(String::from).to_vec()),
            capture: None,
        }
    }
}

impl Plugin for ProfilerPlugin {
    fn build(&self, app: &mut App) {
        let capacity = MAX_FRAMES.max(self.capture.as_ref().map_or(0, |c| c.0 + GPU_LATENCY + 1));
        let mut profiler = Profiler {
            times: VecDeque::with_capacity(capacity + 1),
            spans: self.spans,
            timestamps: self.timestamps,
            counters: self.counters.is_some(),
            clocks: self.clocks,
            paused: false,
            selected: None,
            memory: MemoryReport::default(),
            capacity,
            selection: Arc::new([]),
            compatible: Vec::new(),
            pending_counters: self.counters.clone(),
            gpu: VecDeque::with_capacity(capacity * GPU_ENTRIES_PER_FRAME),
            passes: VecDeque::with_capacity(capacity * GPU_ENTRIES_PER_FRAME),
            regions: VecDeque::with_capacity(capacity * GPU_ENTRIES_PER_FRAME),
            memory_updated: None,
            capture: self.capture.clone(),
        };
        spans::set_enabled(self.spans);
        profiler.start_frame(now_ns());
        app.insert_resource(profiler).add_systems(Last, end_frame);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app
                .init_resource::<GpuTimings>()
                .add_systems(ExtractSchedule, extract_gpu_timings);
        }
    }
}
