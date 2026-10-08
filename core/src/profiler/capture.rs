//! CPU span capture: a tracing layer recording every entered span into per-thread buffers while the profiler runs.
use std::{
    cell::RefCell,
    collections::HashSet,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
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
    pub thread: &'static str,
    pub depth: u16,
    pub start_ns: u64,
    pub end_ns: u64,
}

static ENABLED: AtomicBool = AtomicBool::new(false);
static EPOCH: LazyLock<Instant> = LazyLock::new(Instant::now);
static NAMES: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Default::default);
/// Every thread that recorded a span, with its buffer.
static THREADS: Mutex<Vec<Arc<Mutex<Vec<CpuSpan>>>>> = Mutex::new(Vec::new());

/// The clock of all profiler times.
pub fn now_ns() -> u64 {
    EPOCH.elapsed().as_nanos() as u64
}

/// Starts or stops recording; spans entered while it is off are not recorded.
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Takes every span that ended since the last call.
pub fn drain() -> Vec<CpuSpan> {
    let threads = THREADS.lock().unwrap();
    let mut spans = Vec::new();
    for buffer in threads.iter() {
        spans.append(&mut buffer.lock().unwrap());
    }
    spans
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

struct ThreadState {
    thread: &'static str,
    buffer: Arc<Mutex<Vec<CpuSpan>>>,
    open: Vec<(span::Id, &'static str, u64)>,
}

thread_local! {
    static THREAD: RefCell<ThreadState> = RefCell::new({
        let current = std::thread::current();
        let thread = match current.name() {
            Some(name) => intern(name),
            None => intern(&format!("{:?}", current.id())),
        };
        let buffer = Arc::new(Mutex::new(Vec::new()));
        THREADS.lock().unwrap().push(buffer.clone());
        ThreadState {
            thread,
            buffer,
            open: Vec::new(),
        }
    });
}

/// The profiler's name of a span: its `name` field if it has one (bevy names its system spans
/// "system"), followed by its `pass` field (lava's recording spans).
struct SpanName(&'static str);

struct NameVisitor {
    name: String,
}

impl NameVisitor {
    fn record(&mut self, field: &Field, value: &str) {
        match field.name() {
            "name" => self.name = value.to_owned(),
            "pass" => {
                self.name.push(' ');
                self.name.push_str(value);
            }
            _ => {}
        }
    }
}

impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, value);
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.record(field, &format!("{value:?}"));
    }
}

/// Records the spans every thread enters while the profiler is enabled. Add it to the
/// subscriber; it costs an atomic load per span while disabled.
pub struct ProfileLayer;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for ProfileLayer {
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let mut visitor = NameVisitor {
            name: attrs.metadata().name().to_owned(),
        };
        attrs.record(&mut visitor);
        span.extensions_mut()
            .insert(SpanName(intern(&visitor.name)));
    }

    fn on_enter(&self, id: &span::Id, ctx: Context<'_, S>) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let Some(span) = ctx.span(id) else { return };
        let name = span
            .extensions()
            .get::<SpanName>()
            .map_or(span.name(), |n| n.0);
        let start = now_ns();
        // `try_with`: threads still exit spans while their locals are destroyed.
        let _ = THREAD.try_with(|thread| thread.borrow_mut().open.push((id.clone(), name, start)));
    }

    fn on_exit(&self, id: &span::Id, _ctx: Context<'_, S>) {
        let end = now_ns();
        let _ = THREAD.try_with(|thread| {
            let thread = &mut *thread.borrow_mut();
            // Spans entered before the profiler was enabled aren't open here.
            let Some(depth) = thread.open.iter().rposition(|(open, ..)| open == id) else {
                return;
            };
            let (_, name, start_ns) = thread.open.remove(depth);
            if ENABLED.load(Ordering::Relaxed) {
                let span = CpuSpan {
                    name,
                    thread: thread.thread,
                    depth: depth as u16,
                    start_ns,
                    end_ns: end,
                };
                thread.buffer.lock().unwrap().push(span);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    /// Runs `f` on a fresh thread with only the profile layer installed and returns the spans
    /// that thread recorded.
    fn capture(f: impl FnOnce() + Send + 'static) -> Vec<CpuSpan> {
        std::thread::Builder::new()
            .name("capture test".into())
            .spawn(move || {
                let subscriber = tracing_subscriber::registry().with(ProfileLayer);
                set_enabled(true);
                tracing::subscriber::with_default(subscriber, f);
                let thread = THREAD.with_borrow(|t| t.buffer.clone());
                let spans = std::mem::take(&mut *thread.lock().unwrap());
                spans
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
        assert!(spans.iter().all(|s| s.thread == "capture test"));
        let outer = spans[2];
        for inner in &spans[..2] {
            assert!(outer.start_ns <= inner.start_ns && inner.end_ns <= outer.end_ns);
            assert!(inner.end_ns - inner.start_ns >= 1_000_000);
        }
        assert!(spans[0].end_ns <= spans[1].start_ns);
    }

    #[test]
    fn spans_are_named_by_their_name_and_pass_fields() {
        let spans = capture(|| {
            let system = tracing::info_span!("system", name = "game::update".to_string());
            system.in_scope(|| {
                let _pass = tracing::info_span!("compute", pass = "BvhCull").entered();
            });
            // Entering the same span again records it again.
            system.in_scope(|| {});
        });
        let names: Vec<_> = spans.iter().map(|s| (s.name, s.depth)).collect();
        assert_eq!(
            names,
            [
                ("compute BvhCull", 1),
                ("game::update", 0),
                ("game::update", 0)
            ]
        );
    }

    #[test]
    fn interned_names_are_shared() {
        let a = intern(&String::from("same name"));
        let b = intern("same name");
        assert!(std::ptr::eq(a, b));
    }
}
