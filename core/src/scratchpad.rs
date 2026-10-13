//! API sketches for the "find solutions" entries of STATE.md that touch core (pseudo-Rust, not
//! a module, so it needn't compile). The reasoning is in the chat summary.

// ------------------------------------------------------------------------------------------
// Console ring buffer data race (editor/console.rs)
// ------------------------------------------------------------------------------------------
// Writers reserve a slot with `fetch_add`, write it, then publish it; the reader takes only
// published slots and copies them out under the slot's own seqlock-style marker. One writer
// per slot: the reservation counter is never reset ("Clear log" moves the reader's floor
// instead), so two writers get the same slot only after 2^64 messages.
struct LogRing {
    /// Next slot to write; the reader's `head` is `min(reserved, published floor)`.
    reserved: AtomicU64,
    slots: Box<[Slot; MAX_ENTRIES]>,
}
struct Slot {
    /// 0: empty, odd: being written, even and nonzero: `generation` of the entry inside.
    seq: AtomicU64,
    entry: UnsafeCell<MaybeUninit<LogEntry>>,
}
impl LogRing {
    fn push(&self, entry: LogEntry) {
        let n = self.reserved.fetch_add(1, Relaxed);
        let slot = &self.slots[n as usize % MAX_ENTRIES];
        // The writer of generation n takes the slot even if a slow reader is still copying
        // generation n - MAX_ENTRIES: the reader checks `seq` after its copy and retries.
        slot.seq.store(2 * n + 1, Release);
        unsafe { (*slot.entry.get()).write(entry) }; // drops the old `Box<str>`s: the reader
                                                     // never holds a reference, only a copy
        slot.seq.store(2 * n + 2, Release);
    }
    /// A copy of entry `n`, or `None` if it was overwritten meanwhile.
    fn get(&self, n: u64) -> Option<LogEntry> {
        let slot = &self.slots[n as usize % MAX_ENTRIES];
        loop {
            let seq = slot.seq.load(Acquire);
            if seq != 2 * n + 2 {
                return None; // not written yet, being written, or already recycled
            }
            // A copy of the `Box<str>`s is a clone: `LogEntry` must be `Clone` without
            // touching anything the writer may free: store the text inline (`ArrayString`)
            // or in an `Arc<str>` cloned here, and re-check `seq` after the clone.
            let entry = unsafe { (*slot.entry.get()).assume_init_ref().clone() };
            if slot.seq.load(Acquire) == seq {
                return Some(entry);
            }
        }
    }
}
// Alternative with less unsafe: a bounded MPSC channel (`crossbeam::ArrayQueue<LogEntry>`)
// per writer side and a `Vec` the UI drains it into on its thread every frame; the UI then
// owns the entries and the ring is a plain `VecDeque` there. Costs one copy per message.

//--------------- use the crossbeam varient if the performance is aceptable.


// ------------------------------------------------------------------------------------------
// Console filter, level, "Clear log", println!s, zeditor (editor/console.rs)
// ------------------------------------------------------------------------------------------
// - Filter and level: `LogEntry` keeps `level` and `target` as indices already; the stubbed
//   closure becomes
//     |e| e.level() >= min_level && (filter_lc.is_empty() || e.buffer.to_lowercase().contains(&filter_lc))
//   (lower-casing per entry allocates: keep a scratch `String` in `ConsoleUiState`, or match
//   case-insensitively with `eq_ignore_ascii_case` over windows).
// - "Clear log": don't reset `head`; set `ui_state.cleared_before = head` and start every
//   scan from `max(old_head, cleared_before)`. With the ring above, `head` is never reset.
// - Unknown event fields: `tracing::warn!` would recurse into the layer; drop them silently,
//   or count them in a `static AtomicUsize` the console shows.
// - ctrl-click: `Command::new(env::var("VISUAL").or(env::var("EDITOR")).unwrap_or("zeditor"))`,
//   and `if let Err(e) = cmd.spawn() { error!(..) }` instead of `unwrap`.

//------ Implement it like you suggested

// ------------------------------------------------------------------------------------------
// UB in the UI builder: `&window.focused` cast to `*mut` (ui/builder.rs:142)
// ------------------------------------------------------------------------------------------
// Only the window whose tab is being built mutates its focus, but every UI system holds
// `Res<UiWindows>` at once. Make the per-window mutable state interior-mutable the same way
// the tab state already is (`tab.state: Mutex<..>`):
pub struct UiWindow {
    pub focused: Mutex<Option<FocusedState>>, // or `parking_lot::Mutex`; locked per build
    // ...
}
// `update_windows` and `draw_windows` have `ResMut<UiWindows>` and use `get_mut()`, which
// doesn't lock. `build` locks it for the duration of the closure, like `tab.state`.

//------ Lets discuss this a bit further: theoreticaly, FocusedState can only ever be held by one tab, so it may even be better to just put it in a global resource with an UnsafeCell
// -----  keep it as is for now

// ------------------------------------------------------------------------------------------
// Hover decided by window slot order, not layer (ui/builder.rs:100-117)
// ------------------------------------------------------------------------------------------
// `build` walks `windows.windows` in slot order and unsets `hovered` when any *earlier* window
// contains the cursor. Use the layer: the tab is hovered iff its window is the topmost one
// under the cursor, which `UiWindows::window_at(pos)` already computes (the viewport uses it).
let hovered = input.cursor_pos.is_some_and(|pos| ui.windows.window_at(pos) == Some(window_index));

// --- looks good, implement it

// ------------------------------------------------------------------------------------------
// Delete despawns while typing a save path (editor/picking.rs:92)
// ------------------------------------------------------------------------------------------
// `ui.ctx.focused.is_some()` is true whenever the window is focused, text field or not. The
// focused state knows whether a text field has the keyboard: `focused.focused_element`.
if keys.just_pressed(KeyCode::Delete) && ui.ctx.focused.as_ref().is_some_and(|f| f.focused_element.is_none()) { .. }
// More generally: a `UiWindowBuilder::wants_keyboard()` helper, so every hotkey uses the
// same test (the text input already swallows keys it consumes; hotkeys could instead read
// `ui.ctx.keys`, which the input removes consumed keys from).

// ---- same here, implement it

// ------------------------------------------------------------------------------------------
// Performance: every instance re-extracted and rewritten each frame (render/world.rs)
// ------------------------------------------------------------------------------------------
// Persistent instance slots instead of a per-frame list, with bevy's change detection:
//   - `Instance` gets a render-world slot on insertion (a `SlotMap`, free list), freed on
//     `RemovedComponents<Instance>`; the slot index is stable, which also answers the
//     "nothing per instance can persist across frames" entry (intended for now).
//   - `extract` visits only `Changed<GlobalTransform>` / `Changed<Instance>` entities (the
//     editor's selection changes an `InstanceFlags` bit) and writes those slots.
//   - The GPU buffers stop being `frame_in_flight` rings: a changed slot is written to a
//     staging ring (the changes of the last FRAMES_IN_FLIGHT frames) and `update_buffer`ed,
//     or written in place when the frame that reads the old value is known to be done (one
//     ring per field only for the fields that change per frame: the transform).
//   - The material pointer and the OPAQUE flag come from the material: a `Changed<Handle>`
//     or `AssetEvent<GpuMaterial>` rewrites the slot; the per-frame `material.read()` goes.
// `instance_cull` then takes the slot count (holes are instances with a null BVH root and an
// empty AABB, skipped by the frustum test), or a compacted index list kept by the manager.

// --- leave it as for now, ill need to think this through some more

// ------------------------------------------------------------------------------------------
// Performance: `draw_outline` loads (draw_outline.slang)
// ------------------------------------------------------------------------------------------
// Separable dilation: two passes over the R8 mask that the outline pass now writes
// (`RenderResources.outline_mask`): horizontal max over [-r, r] into a second R8 image,
// vertical max over [-r, r] of that; 2(2r+1) loads per pixel instead of ~pi r^2, and the ring
// is `dilated && !mask`. A round outline needs a disc, which the two box passes approximate
// with an octagon when the horizontal pass also writes the distance to the nearest set
// texel (R8: 0..r) and the vertical pass tests `dx^2 + dy^2 <= r^2`. Cheaper still: the
// mask is a workgroup-local tile; load the 8x8 tile plus an r-wide apron into groupshared
// memory once (one load per thread plus the apron) and test the disc from there.

// --- yes implement the groupshared varient and cap the outline size at 8 to accomidate

// ------------------------------------------------------------------------------------------
// Performance: oversized fixed render buffers (render/render.rs:512-518)
// ------------------------------------------------------------------------------------------
// `meshlet_batches` is indexed by `candidate index / 64`, so it needs `candidate_meshlets.len() / 64`
// entries (32k u32 = 128 KB), not 2M (8 MB): size it from the candidate count. The others
// are worst cases of sponza (10k drawn meshlets of 2M capacity): make the sizes
// `RenderPlugin` fields with smaller defaults (256k meshlets, 256k candidates, 64k nodes,
// about 10 MB in all) and rely on the overflow report that now exists to raise them. Growing
// on overflow at runtime is possible too (recreate the buffer in `fit`, like the images),
// since the cull pass is restarted from scratch every frame.

// --- sounds good too, set these values as default and implement buffer resizing in the fit function. Cap this to some maximum and print a warning if its reached

// ------------------------------------------------------------------------------------------
// Performance: gizmo buffers (editor/gizzmos.rs)
// ------------------------------------------------------------------------------------------
// `MAX_GIZZMOS` of 1M x 80 B x 2 frames in flight = 160 MB host-visible, plus the pending
// `Vec`. The scene BVH debug draw is the only heavy user; 64k gizmos (10 MB) covers it, and
// a frame that draws more is truncated with the warning that now exists. Or allocate on
// demand: a `Vec<Gizzmo>` per frame in flight grown to the count actually drawn, and the GPU
// buffer recreated (retired with the frame) when it is too small, like the UI's vertex
// buffers in `write_ui_data`.

// --- Cap it at an even lower number of say 8k, but add a first gizzmo slider that offsets which 8k gizzmos get drawn if there are too many.

// ------------------------------------------------------------------------------------------
// Performance: `ProfileLayer::on_new_span` and the leaked span rings (profiler/spans.rs)
// ------------------------------------------------------------------------------------------
// - The `NAMES` mutex: `intern` is called for every new span to turn its name plus fields
//   into a `&'static str`. Spans whose name has no fields (`attrs.fields().is_empty()`, most
//   of bevy's) can skip the visitor and the intern: `attrs.metadata().name()` is already
//   `&'static str`. For the rest, a thread-local cache `HashMap<(callsite, fields hash), &'static str>`
//   in front of the global one makes the mutex a once-per-callsite-per-thread cost.
//   And when recording is off, `on_new_span` can skip the name entirely, storing nothing:
//   `on_enter` then names the span from `metadata().name()` (losing the fields of spans that
//   were created while recording was off, which is only the first frame after a resume).
// - The rings: a thread's ring is leaked because the `ThreadState` is dropped on thread
//   exit while readers may still iterate `THREADS`. Put the ring in an `Arc<Ring>` instead:
//   `THREADS: Mutex<Vec<Weak<Ring>>>`, pruned when a reader finds a dead `Weak`; the
//   thread-local holds the `Arc`, so the ring dies with its thread. Bevy's task pools keep
//   their threads for the process' life anyway, so the leak is only short-lived threads
//   (the asset loader's), at `SPANS_PER_THREAD` x 40 B each.

// --- Sounds good

// ------------------------------------------------------------------------------------------
// Performance: per-frame allocations (console, inspector, hierarchy, by_layer, present, retire)
// ------------------------------------------------------------------------------------------
// - Console (`entry.clone()` per visible row): with the ring above, the UI copies into a
//   scratch `LogEntry` kept in `ConsoleUiState` (`clone_from`), or the entry text lives
//   inline (`ArrayString<256>`), so a copy allocates nothing.
// - Inspector: `reflect_clone` of every component every frame is the design (CLAUDE.md,
//   "the tab works on a reflect_clone of the value every frame"); keep the clone only while
//   a field is being edited, and otherwise read the component through `reflect_ref` (no
//   clone, no allocation) — the edit path already handles writing back. Labels: a scratch
//   `String` in a `Local`, `write!`-ed instead of `format!`-ed.
// - Hierarchy: `draw_entity_node` recurses over every entity; a virtualised tree (like
//   `ui.list`, only the rows in view are laid out) needs the flattened visible row list,
//   which can be rebuilt only when the hierarchy changes (`Changed<ChildOf>`,
//   `RemovedComponents`, the expanded set) into a `Vec<(Entity, depth)>` kept in a `Local`.
// - `UiWindows::by_layer`: keep the sorted order in `UiWindows` (a `Vec<usize>` updated by
//   `reorder_layers`, which already walks every window), and iterate that.
// - `present`'s `Vec` of semaphore handles: a `SmallVec<[_; 2]>`, or take `&[vk::Semaphore]`
//   the caller keeps (core has one wait semaphore).
// - `BindlessWrites::submit`: a scratch `Vec` in the `BindlessWrites` reused across submits.
// - A `Box` per `retire`: retire takes `Box<dyn ..>` already; batch the frame's retired
//   values in an enum `Retired { Image(DynamicImage), Buffer(DynamicBuffer), Swapchain(..) }`
//   stored in a `Vec<Retired>` on the slot (no box), with `Other(Box<dyn ..>)` for the rest.

//--- Most of this makes sens, but for the editor in general there is no need to overly complicate things since again, its only a debug feature.
