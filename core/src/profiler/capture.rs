//! CPU span capture: a tracing layer writing every span entered while recording into a fixed ring on its own thread, plus per-thread busy-time counters; the rings are read only while recording is off, and `busy_signs` tells which spans make up a thread's busy time.
use std::{
    cell::RefCell,
    collections::HashSet,
    fmt::Write,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering},
    },
    time::Instant,
};

use tracing::{
    Subscriber,
    field::{Field, Visit},
    span,
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

/// A span's time on one thread, in ns since the process started.
#[derive(Clone, Copy, Debug)]
pub struct CpuSpan {
    pub name: &'static str,
    pub start_ns: u64,
    pub end_ns: u64,
    /// The frame it started in.
    pub frame: u32,
    /// Its open ancestors when it ended, counting ones that never end and so aren't recorded.
    pub depth: u16,
    pub kind: SpanKind,
}

/// Bevy's spans of systems (and run conditions) and of schedules, spans with a `wait = true`
/// field, in which the thread blocks on the GPU or another thread, and all the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SpanKind {
    System,
    Schedule,
    Wait,
    Other,
}

impl SpanKind {
    pub fn label(self) -> &'static str {
        match self {
            SpanKind::System => "system",
            SpanKind::Schedule => "schedule",
            SpanKind::Wait => "wait",
            SpanKind::Other => "other",
        }
    }
}

/// Spans each thread keeps; older ones are overwritten.
pub const SPANS_PER_THREAD: usize = 1 << 16;
/// Deeper spans aren't recorded. Once recording is off, a thread can still finish this many
/// open spans, so readers leave that many slots alone.
const MAX_DEPTH: usize = 64;
/// Frames the busy-time counters hold at once.
const BUSY_FRAMES: usize = 4;

static ENABLED: AtomicBool = AtomicBool::new(false);
static FRAME: AtomicU32 = AtomicU32::new(0);
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
static NAMES: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Default::default);
/// The ring of every thread that recorded a span. Locked only to add a thread or read them.
static THREADS: Mutex<Vec<&'static Ring>> = Mutex::new(Vec::new());

/// One thread's spans. Only that thread writes them.
struct Ring {
    thread: &'static str,
    /// Spans written so far; slot `i % SPANS_PER_THREAD` holds span `i`.
    head: AtomicUsize,
    spans: *mut CpuSpan,
    /// Busy ns per frame (modulo `BUSY_FRAMES`): the time of the spans that started in it,
    /// without their children's and without waits.
    busy: [AtomicU64; BUSY_FRAMES],
}

unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

impl Ring {
    fn push(&self, span: CpuSpan) {
        let head = self.head.load(Ordering::Relaxed);
        unsafe { self.spans.add(head % SPANS_PER_THREAD).write(span) };
        self.head.store(head + 1, Ordering::Release);
    }

    /// The spans a reader may look at, oldest first, and the end of the oldest one if older
    /// ones are gone (else 0). Writes that happen while it reads land outside them, as long
    /// as recording is off or the caller is the ring's own thread.
    fn read(&self) -> (u64, [&[CpuSpan]; 2]) {
        let head = self.head.load(Ordering::Acquire);
        let start = head.saturating_sub(SPANS_PER_THREAD - MAX_DEPTH);
        let (a, b) = (start % SPANS_PER_THREAD, head % SPANS_PER_THREAD);
        let slice = |from: usize, to: usize| unsafe {
            std::slice::from_raw_parts(self.spans.add(from), to - from)
        };
        let spans = match (start == head, a < b) {
            (true, _) => [&[][..], &[]],
            (false, true) => [slice(a, b), &[]],
            (false, false) => [slice(a, SPANS_PER_THREAD), slice(0, b)],
        };
        let complete_from = if start > 0 { spans[0][0].end_ns } else { 0 };
        (complete_from, spans)
    }
}

/// The clock of all profiler times.
pub fn now_ns() -> u64 {
    EPOCH.elapsed().as_nanos() as u64
}

/// Starts or stops recording; spans entered while it is off are not recorded.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// The frame spans entered from now on belong to.
pub fn set_frame(frame: u32) {
    FRAME.store(frame, Ordering::Relaxed);
}

pub fn frame() -> u32 {
    FRAME.load(Ordering::Relaxed)
}

/// The busiest thread's time in `frame` by the counters, which then start over for the frame
/// `BUSY_FRAMES` later. Each span's own time counts towards the frame it started in.
pub fn take_busy(frame: u32) -> u64 {
    let slot = frame as usize % BUSY_FRAMES;
    let threads = THREADS.lock().unwrap();
    let busy = threads.iter().map(|ring| ring.busy[slot].swap(0, Ordering::Relaxed));
    busy.max().unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq)]
enum Counted {
    Yes,
    InWait,
}

/// Calls `f` with every span of one thread (newest first) and its share of the thread's busy
/// time: 1 for a span that isn't a wait and has no counted parent, -1 for a wait whose
/// parent counts, else 0. The thread was busy for the signed sum of the durations. A parent
/// that was never recorded (an ancestor that never ends, like bevy's `bevy_app`) doesn't count.
pub fn busy_signs<'a>(spans: [&'a [CpuSpan]; 2], mut f: impl FnMut(&'a CpuSpan, i64)) {
    // The newest span seen at each depth; walking backwards, a parent comes before its children.
    let mut parents: [Option<(u64, u64, Counted)>; MAX_DEPTH] = [None; MAX_DEPTH];
    for span in spans.into_iter().rev().flat_map(|spans| spans.iter().rev()) {
        let depth = span.depth as usize;
        let parent = depth.checked_sub(1).and_then(|d| parents[d]);
        let parent = parent
            .filter(|(start, end, _)| *start <= span.start_ns && span.end_ns <= *end)
            .map(|(.., counted)| counted);
        let wait = span.kind == SpanKind::Wait;
        let (sign, counted) = match (parent, wait) {
            (Some(Counted::InWait), _) => (0, Counted::InWait),
            (Some(Counted::Yes), true) => (-1, Counted::InWait),
            (Some(Counted::Yes), false) => (0, Counted::Yes),
            (None, true) => (0, Counted::InWait),
            (None, false) => (1, Counted::Yes),
        };
        parents[depth] = Some((span.start_ns, span.end_ns, counted));
        f(span, sign);
    }
}

/// Calls `f` with every thread's name, the end of its oldest span if older ones were
/// overwritten (else 0), and its spans, oldest first. Only while recording is off.
pub fn for_each_thread(mut f: impl FnMut(&'static str, u64, [&[CpuSpan]; 2])) {
    assert!(
        !ENABLED.load(Ordering::Relaxed),
        "spans are only read while recording is off"
    );
    for ring in THREADS.lock().unwrap().iter() {
        let (complete_from, spans) = ring.read();
        f(ring.thread, complete_from, spans);
    }
}

/// Names live as long as the process, so a span can carry its name around for free.
pub(super) fn intern(name: &str) -> &'static str {
    let mut names = NAMES.lock().unwrap();
    if let Some(name) = names.get(name) {
        return name;
    }
    let name: &'static str = Box::leak(name.into());
    names.insert(name);
    name
}

struct Open {
    id: u64,
    name: SpanName,
    start_ns: u64,
    frame: u32,
    /// Time of its children that ended so far.
    children_ns: u64,
}

struct ThreadState {
    ring: &'static Ring,
    open: Vec<Open>,
}

thread_local! {
    static THREAD: RefCell<ThreadState> = RefCell::new({
        let current = std::thread::current();
        let thread = match current.name() {
            Some(name) => intern(name),
            None => intern(&format!("{:?}", current.id())),
        };
        // Uninitialised, so pages the thread never writes stay uncommitted.
        let spans = Box::leak(Box::<[CpuSpan]>::new_uninit_slice(SPANS_PER_THREAD));
        let ring: &'static Ring = Box::leak(Box::new(Ring {
            thread,
            head: AtomicUsize::new(0),
            spans: spans.as_mut_ptr().cast(),
            busy: Default::default(),
        }));
        THREADS.lock().unwrap().push(ring);
        ThreadState {
            ring,
            open: Vec::with_capacity(MAX_DEPTH),
        }
    });
    /// Where a new span's name is put together.
    static NAME: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The profiler's name of a span. Bevy's "system" and "schedule" spans are named by their
/// `name` field; any other span by its own name followed by its `name` and `pass` fields
/// (bevy's "system_commands", lava's recording spans).
#[derive(Clone, Copy)]
struct SpanName(&'static str, SpanKind);

struct NameVisitor<'a> {
    name: &'a mut String,
    kind: SpanKind,
}

impl NameVisitor<'_> {
    /// Makes room for the field's value if it is part of the name.
    fn names(&mut self, field: &Field) -> bool {
        match (field.name(), self.kind) {
            ("name", SpanKind::System | SpanKind::Schedule) => self.name.clear(),
            ("name" | "pass", _) => self.name.push(' '),
            _ => return false,
        }
        true
    }
}

impl Visit for NameVisitor<'_> {
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "wait" && value {
            self.kind = SpanKind::Wait;
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if self.names(field) {
            self.name.push_str(value);
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if self.names(field) {
            let _ = write!(self.name, "{value:?}");
        }
    }
}

/// Records the spans every thread enters while the profiler is enabled. Add it to the
/// subscriber; it costs an atomic load per span while disabled.
pub struct ProfileLayer;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for ProfileLayer {
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let name = attrs.metadata().name();
        let kind = match name {
            "system" => SpanKind::System,
            "schedule" => SpanKind::Schedule,
            _ => SpanKind::Other,
        };
        // Without a name (threads tearing down), the span keeps its plain name.
        let _ = NAME.try_with(|buffer| {
            let buffer = &mut *buffer.borrow_mut();
            buffer.clear();
            buffer.push_str(name);
            let mut visitor = NameVisitor { name: buffer, kind };
            attrs.record(&mut visitor);
            let kind = visitor.kind;
            span.extensions_mut().insert(SpanName(intern(buffer), kind));
        });
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(span) = ctx.span(id) else { return };
        let name = span
            .extensions()
            .get::<SpanName>()
            .copied()
            .unwrap_or(SpanName(span.name(), SpanKind::Other));
        let open = Open {
            id: id.into_u64(),
            name,
            start_ns: now_ns(),
            frame: FRAME.load(Ordering::Relaxed),
            children_ns: 0,
        };
        // `try_with`: threads still enter and exit spans while their locals are destroyed.
        let _ = THREAD.try_with(|thread| {
            let open_spans = &mut thread.borrow_mut().open;
            if open_spans.len() < MAX_DEPTH {
                open_spans.push(open);
            }
        });
    }

    fn on_exit(&self, id: &span::Id, _ctx: Context<'_, S>) {
        let end_ns = now_ns();
        let _ = THREAD.try_with(|thread| {
            let thread = &mut *thread.borrow_mut();
            // Only spans entered while the profiler was enabled are open here; they are
            // recorded even if it was disabled since, as they belong to a recorded frame.
            let id = id.into_u64();
            let Some(depth) = thread.open.iter().rposition(|open| open.id == id) else {
                return;
            };
            let open = thread.open.remove(depth);
            let SpanName(name, kind) = open.name;
            let duration = end_ns - open.start_ns;
            let ancestors = &mut thread.open[..depth];
            if kind != SpanKind::Wait && ancestors.iter().all(|a| a.name.1 != SpanKind::Wait) {
                let own = duration.saturating_sub(open.children_ns);
                let slot = open.frame as usize % BUSY_FRAMES;
                thread.ring.busy[slot].fetch_add(own, Ordering::Relaxed);
            }
            if let Some(parent) = ancestors.last_mut() {
                parent.children_ns += duration;
            }
            thread.ring.push(CpuSpan {
                name,
                start_ns: open.start_ns,
                end_ns,
                frame: open.frame,
                depth: depth as u16,
                kind,
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    /// Runs `f` on a fresh thread with only the profile layer installed and returns the spans
    /// that thread recorded. Reads the thread's own ring, so recording stays on for other
    /// tests.
    fn capture(f: impl FnOnce() + Send + 'static) -> Vec<CpuSpan> {
        capture_with_busy(f).0
    }

    /// Also returns the sum of the thread's busy counters.
    fn capture_with_busy(f: impl FnOnce() + Send + 'static) -> (Vec<CpuSpan>, u64) {
        std::thread::Builder::new()
            .name("capture test".into())
            .spawn(move || {
                let subscriber = tracing_subscriber::registry().with(ProfileLayer);
                set_enabled(true);
                tracing::subscriber::with_default(subscriber, f);
                let ring = THREAD.with_borrow(|t| t.ring);
                assert_eq!(ring.thread, "capture test");
                let (_, spans) = ring.read();
                let busy = ring.busy.iter().map(|b| b.load(Ordering::Relaxed)).sum();
                (spans.concat(), busy)
            })
            .unwrap()
            .join()
            .unwrap()
    }

    #[test]
    fn nested_spans_record_names_depths_and_intervals() {
        let spans = capture(|| {
            let _outer = tracing::info_span!("outer").entered();
            for _ in 0..2 {
                let _inner = tracing::info_span!("inner").entered();
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        });
        // Recorded when they end: both inner spans before the outer one.
        let names: Vec<_> = spans.iter().map(|s| (s.name, s.depth)).collect();
        assert_eq!(names, [("inner", 1), ("inner", 1), ("outer", 0)]);
        let outer = spans[2];
        for inner in &spans[..2] {
            assert!(outer.start_ns <= inner.start_ns && inner.end_ns <= outer.end_ns);
            assert!(inner.end_ns - inner.start_ns >= 1_000_000);
        }
        assert!(spans[0].end_ns <= spans[1].start_ns);
    }

    #[test]
    fn spans_are_named_and_classified_by_their_fields() {
        use SpanKind::*;
        #[derive(Debug)]
        struct Update;
        let spans = capture(|| {
            let _schedule = tracing::info_span!("schedule", name = ?Update).entered();
            let system = tracing::info_span!("system", name = "game::update".to_string());
            system.in_scope(|| {
                let _pass = tracing::info_span!("compute", pass = "BvhCull").entered();
            });
            // Entering the same span again records it again.
            system.in_scope(|| {});
            let _commands = tracing::info_span!("system_commands", name = "game::update").entered();
            let _wait = tracing::info_span!("wait for frame slot", wait = true).entered();
        });
        let names: Vec<_> = spans.iter().map(|s| (s.name, s.kind, s.depth)).collect();
        assert_eq!(
            names,
            [
                ("compute BvhCull", Other, 2),
                ("game::update", System, 1),
                ("game::update", System, 1),
                ("wait for frame slot", Wait, 2),
                ("system_commands game::update", Other, 1),
                ("Update", Schedule, 0),
            ]
        );
    }

    #[test]
    fn busy_time_is_roots_minus_their_outermost_waits() {
        let (spans, counted) = capture_with_busy(|| {
            // Never ends, like bevy's `bevy_app`, so its children are the roots.
            std::mem::forget(tracing::info_span!("forever").entered());
            {
                let _root = tracing::info_span!("root").entered();
                let _work = tracing::info_span!("work").entered();
                drop(_work);
                let _wait = tracing::info_span!("wait", wait = true).entered();
                let _inner = tracing::info_span!("inner wait", wait = true).entered();
            }
            let _idle = tracing::info_span!("idle", wait = true).entered();
            let _work = tracing::info_span!("work in idle").entered();
        });
        let mut signs = Vec::new();
        busy_signs([&spans, &[]], |span, sign| signs.push((span.name, sign)));
        signs.reverse();
        assert_eq!(
            signs,
            [
                ("work", 0),
                ("inner wait", 0),
                ("wait", -1),
                ("root", 1),
                ("work in idle", 0),
                ("idle", 0)
            ]
        );
        let ns = |name| {
            let span = spans.iter().find(|s| s.name == name).unwrap();
            span.end_ns - span.start_ns
        };
        assert_eq!(counted, ns("root") - ns("wait"));
    }

    #[test]
    fn a_full_ring_keeps_the_newest_spans() {
        let total = SPANS_PER_THREAD + 10;
        let spans = capture(move || {
            for _ in 0..total {
                let _span = tracing::info_span!("span").entered();
            }
        });
        assert_eq!(spans.len(), SPANS_PER_THREAD - MAX_DEPTH);
        assert!(spans.is_sorted_by_key(|s| s.end_ns));
    }

    #[test]
    fn interned_names_are_shared() {
        let a = intern(&String::from("same name"));
        let b = intern("same name");
        assert!(std::ptr::eq(a, b));
    }
}
