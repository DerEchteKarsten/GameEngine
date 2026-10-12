# Known bugs

Found in a read-only review on 2026-10-11. **Verified** means the code was read and the bug confirmed; **plausible** means it follows from the code but wasn't reproduced. Line numbers are from that date. Delete an entry when it's fixed.

## lava: resources and device setup
- **Host image copy is enabled without a support check (verified).** The feature struct is always chained (`state.rs:476-484`), the extension only with rebar (`state.rs:380`), and support is never queried (`physical_device.rs:97-154`).
- `extended_dynamic_state3` is required and its features requested without a support query (`state.rs:377`, `:467-473`); the EDS2 logic-op feature is requested without its extension (`:474`). None of these states is used.
- Devices without mesh shaders aren't rejected; `Functions::mesh().unwrap()` panics later (`command_buffer.rs:1461`).
- `Buffer::raw` replaces the driver's alignment with the requested one instead of taking the max (`buffer/mod.rs:91-93`), which can lower it.
- Unsound safe API: `BufferSlice` is `Copy` but has `IndexMut`/`copy_from`/`as_slice` over mapped memory (`buffer/mod.rs:51-62`, `buffer/slice.rs:177-190`), so aliased `&mut` is possible; `cast` on images and views can retag usage (`image/slice.rs:61`).
- Plausible: `requiresDedicatedAllocation` is never queried (`image/mod.rs`).
- Plausible: bindless layout counts use limits that aren't the update-after-bind ones (`bindless.rs:188`, `:211`).
- Samplers 0 and 1 leave `max_lod` at 0, so they can't sample mips (`bindless.rs:135-163`).

## lava: swapchain
- **Acquire panics on `ERROR_OUT_OF_DATE` (verified).** `aquire_image(...).unwrap()` in core (`render/render.rs:151-153`); present handles it (`queue.rs:620`).
- Swapchain capabilities are stale and the extent isn't clamped: core never calls `refresh_capabilities` (`swapchain.rs:73-75`, `render/render.rs:156-197`), so a minimised (0x0) window panics.
- The global swapchain `FORMAT` is set only once (`swapchain.rs:23`, `:68`); swapchain images always get STORAGE usage without checking the surface's supported usages.
- `Swapchain::drop` calls `device_wait_idle` on every recreation (`swapchain.rs:204`).

## lava: ray tracing (none of it is used yet)
- **Entry names are rotated (verified).** `create_dispatch_pipeline` (`pipeline.rs`) makes the raygen stage from `ray_any`, the closest-hit stage from `ray_closest` and the miss stage from `ray_gen`, so pipeline creation fails and `.unwrap()` panics.
- **Shader binding table order mismatch (verified).** Groups are created raygen, hit, miss (`command_buffer.rs:430-441`), handles copied into regions laid out raygen, miss, hit (`rt_pipeline.rs:179-214`), and `cmd_trace_rays` gets the hit region as miss and vice versa (`command_buffer.rs:1809-1818`). The swaps cancel only with exactly one hit and one miss group.
- **Acceleration structure size is the whole buffer (verified).** `.size(buffer.size())` with a nonzero offset (`acceleration_structure.rs:47-51`); should be `size_info.acceleration_structure_size`. No `Drop` (leaks), scratch ignores `minAccelerationStructureScratchOffsetAlignment`.
- `build.rs:488` only accepts `["closest_hit","raygen"]` and `["any_hit","closest_hit","raygen"]`, so a pass with a miss shader panics; `stage_name` panics on intersection and callable stages (`build.rs:918-930`). Any-hit is parsed but never added to a hit group.

## lava: shader build and hot reload
- A shader the driver rejects on hot reload panics the watcher thread while it holds the `PipelineManager` locks (`create_module` and pipeline creation `.unwrap()` in `pipeline.rs`), which poisons them, so the render thread panics at its next pipeline lookup. Slang errors are handled.
- With `hot-reload`, the watcher `.unwrap()`s a compile-time absolute `SHADER_DIR` (`hot_reload.rs`) and the SPIR-V is read from `OUT_DIR` at start (`PassEntry::spirv`), so a binary moved off the build machine panics. Without the feature the SPIR-V is built into the binary.
- Plausible: a changed push-constant layout hot-reloads silently against the old Rust struct.
- `build.rs` writes a timestamp into `bindings.rs` every run and writes it unconditionally (`build.rs:769-771`, `:915`), so any shader edit rebuilds lava, core and game.

## lava: tests
- `pass_queries_count_passes_and_renderings` (`tests/gpu_headless.rs:441`) fails: pipeline statistics report 0 compute invocations where 4096 are expected. It fails without any local change too.

## core: renderer
- **GPU use-after-free on unload and hot reload (verified).** `GpuMesh.buffer` (`assets/mesh.rs:56-59`) and texture images are freed on drop, without `Frame::retire`, while frames in flight read them; `GpuMaterial::drop` returns its slot at once (`assets/material.rs:211-216`), so a pending frame can read another material. Rebaking with the editor open triggers it.
- **The outline pass clears the scene depth (verified)** (`render/render.rs:741`, `Some([0.0])`) whenever something is outlined, so nothing after it can use the scene depth.
- **Normals ignore non-uniform scale (verified).** `scene_raster.slang:46` uses `(float3x3)transform`, not the inverse transpose.
- **Mirrored instances are culled inside out.** Nothing checks for a negative determinant (`assets/bake.rs:277-282`, `render/world.rs:135`); the front face is always `CLOCKWISE` (`command_buffer.rs:738`).
- **BVH cull overflows are silent and can break the "filled once" invariant** (plausible). A group that leaves node processing because the queue is full (`bvh_cull.slang:356-359`) runs partial batches while others still produce (`:423-436`) and resets a counter that late increments make non-zero again, so stale or duplicate meshlets show until the next `fresh`. Subtrees past `max_nodes` (`:181`), candidates past capacity (`:231`) and draws past capacity (`:279-283`) are dropped without any report to the CPU.
- **Cross-group polling uses plain loads** (plausible). `queue.val[...]` (`bvh_cull.slang:367`), `node_count` and the batch counters (`:414-415`) aren't atomic loads; a stale 0 can end a group early (`:420`) and drop meshlets.
- `resize_swapchain` (`render/mod.rs:321`) and `extract_view_port` (`render/world.rs:181`) run unordered in `ExtractSchedule`; without the editor a shrink can leave `target.rect` at the old size, past the swapchain image (plausible).
- `extract_camera` uses `Single` (`render/render.rs:205`): with zero or two cameras `render` panics on the missing `RenderCamera` (plausible).
- A zero-size viewport tab gives a NaN aspect ratio (`scene/camera.rs:106-107`, no `max(1)`).
- `visible_meshlets()` reads `variables` on the CPU while the GPU may write it (`render/render.rs:557-560`).
- When `record_frame` grows the viewport target (`render/render.rs:804-813`) the tab draws one frame with the old handle and size.
- Plausible: `scene::update_camera` runs in `PostUpdate` without ordering against transform propagation (`scene/mod.rs:235`), so the camera matrices can be a frame stale.

## core: assets, scene, physics
- **The CPU TLAS is traversed far-first (verified).** `bvh.rs:316` sorts hits nearest-first and pushes them onto a LIFO stack. Results are correct, pruning is poor; the BLAS path (`bvh.rs:403`) is right.
- `NodeBuilder::total_aabb` merges unused child slots (`Aabb3d(ZERO, ZERO)`), so partly filled nodes grow to include the origin (`physics/bvh.rs:130-140`).
- `NodeView::child_count` uses `trailing_zeros` and always returns 8 (`physics/bvh.rs:156-159`); only the debug draw uses it.
- `Pod`/`Zeroable` for `HasBlas` holding an `Entity` is unsound (`physics/bvh.rs:41-42`); all-zero isn't a valid `Entity`.
- `ray_triangle_intersect` uses an absolute `1e-8` determinant epsilon (`physics/bvh.rs:438`) and misses tiny triangles (plausible).
- Instance indices follow the extract query's order (`render/world.rs:127`) and change on spawn, despawn and on selection (archetype move), so nothing per instance can persist across frames.
- The scene loader doesn't reject parent cycles or self-parents (`scene/file.rs:377-382`) (plausible).
- Cracks can open between separate primitives sharing an edge at LOD transitions; borders are only locked within one mesh (plausible).
- With attribute-aware simplification, meshopt's error includes normal and UV deviation (`assets/mesh.rs:624-635`), mixing units into the world error (conservative).
- Odd-sized mips drop their last row or column (`assets/texture.rs:377-381`); normal map mips aren't renormalised.
- File formats have no magic or version; `read_slice`/`read_compressed` allocate whatever length the file says (`assets/util.rs:30`, `:39-41`); the mesh loader trusts header offsets (`cull_data_offset - meshlet_offset` can underflow, `assets/mod.rs:71-97`), and corrupt mesh data becomes raw GPU pointers.
- The bake silently drops glTF samplers, `doubleSided`, tangents, vertex colours and every extension except lights, texture transforms and instancing, which warn (`assets/bake.rs`).

## core: editor and UI
- **The inspector rounds floats to 0.01 (verified).** An unfocused float field is formatted `{:.2}` (`ui/widgets.rs:230`) and parsed back (`:486`); `number_view`/`vector_view` (`editor/selected.rs:40-65`) apply it as an edit, so selecting an entity snaps its `Transform`, materials and settings.
- **Console ring buffer data race.** Writers `fetch_add` the head before writing the slot (`editor/console.rs:238-245`), and the UI clones `buffer[idx]` unsynchronised (`:411`, `:438`): it can read a half-written `LogEntry` or a freed `Box<str>`, and two writers can hit one slot after a wrap.
- The console filter and level are stubbed to `true` (`editor/console.rs:388-403`); "Clear log" hides new logs after more than 10k messages (`:379`); unknown event fields are `println!`ed (`:165`, `:179`, `:189`); ctrl-click `spawn().unwrap()`s `zeditor` (`:485`).
- UB in the UI builder: `&window.focused` is cast to `*mut` through a shared `Res<UiWindows>` (`ui/builder.rs:142`) while UI systems run in parallel.
- Hover is decided by window slot order, not layer (`ui/builder.rs:100-117`), so clicks can reach a window behind another (plausible).
- Gizmos aren't capped: more than `MAX_GIZZMOS` writes into the other frame slot or panics (`editor/gizzmos.rs:276-282`); the sphere hit test ignores t < 0, so spheres behind the camera can be clicked (`:149`).
- The Delete key despawns the selection whenever the Hierarchy tab is focused, also while typing a save path (`editor/picking.rs:92`).
- The editor camera overwrites its rotation every frame from a `Local` pitch/yaw (`editor/camera.rs:47`, `:90`), so inspector edits are lost and a camera spawned rotated snaps to yaw 0.
- `ViewPortProxy::cursor_position` mixes a logical cursor with a physical rect (`editor/viewport.rs:90`); unused.
- A `Set` is edited through a const-to-mut pointer cast (`editor/selected.rs:340-360`).
- Floating windows aren't clamped when the OS window shrinks (plausible).
- Font path is absolute and `windows.ron` relative to the working directory (`ui/mod.rs:451`, `:459`, `:744`).

## core: profiler
- With `stats == None` the pass table's counter values shift left under the wrong headers, and the task-invocation column is missing (`profiler/ui.rs:90-100`) (plausible).
- GPU scopes are placed at the CPU submit time without clock calibration (`profiler/report.rs:445`, `profiler/ui.rs:170`), and the windowed `submitted` time is taken after `present` (`render/render.rs:468`) (plausible).
- `capture` only fires on the exact frame number (`profiler/mod.rs:450`), so a pause at that frame skips the auto-save.

## game and tools
- `game/src/main.rs:158-161` prints a `cargo:rustc-env=…` line at runtime (build-script leftover); `:80-153` is dead code.
- `tools`: trait method signatures are never emitted because `FN_RE` needs `pub` (`tools/src/main.rs:37`, `:376`); enum variants aren't listed; multi-line `impl` headers lose their indentation; shader includes are followed one level deep.

## shaders (mostly dead includes, traps for GI work)
- `BrdfValue::invalid()`/`BrdfSample::invalid()` never set `value` (`brdf.slang:21-27`, `:37-45`).
- `luminance` uses Rec. 601 weights on linear Rec. 709 colour (`math.slang:126-129`).
- `direction_to_equirect_uv` is Y-up (`math.slang:12-18`); `composite.slang:42` has the Z-up version.
- SH: L1 has the Condon-Shortley sign, L2 M=±1 doesn't (`spherical_harmonics.slang:23-25` vs `:36-39`); self-consistent, wrong against external data or rotation.
- `hammersley` uses `i+1`, so x reaches 1 (`random.slang:26-28`); `r2_sequence` loses precision at large `i` (`:30-35`).
- `voxel_raytracing.slang:30` declares a `Material` that clashes with `datatypes.slang:119`.
- `old/spherical_harmonic_conversion.slang:26` weights an octahedral grid uniformly, which isn't equal-area.

## Performance bottlenecks
- No occlusion culling: every hidden fragment writes the 16-byte G-buffer.
- Submissions wait on the acquire semaphore at `ALL_COMMANDS` (`vkobjects/queue.rs:581-584`), so culling can't overlap the previous present; wait at `COLOR_ATTACHMENT_OUTPUT`.
- Pipelines are created synchronously on first use, under a write lock, with no `VkPipelineCache` (`command_buffer.rs:233-244`, `:405`, `:602-607`, `:768`).
- `PendingAccesses` is never pruned of destroyed resources; each command scans pending × accesses (`command_buffer.rs:1964-2020`).
- Every instance is re-extracted and rewritten each frame, static or not, reading each material from host-mapped memory (`render/world.rs:120-179`).
- `update_bvh` rebuilds the CPU TLAS every `PreUpdate` with fresh `Vec`s, and raycasts allocate a `Vec` per node (`physics/bvh.rs:306`, `:390`, `:563-608`). The builder groups Morton-sorted items without SAH.
- `draw_outline` makes up to about 113 loads per empty pixel at radius 6 (`draw_outline.slang:13-27`); JFA or a separable dilation would be cheaper.
- `meshlet_batches` holds 2M u32 but only 32k are indexed (`render/render.rs:512-518`, `bvh_cull.slang:409`); about 120 MB of fixed render buffers in total.
- Gizmo buffers reserve about 160 MB host-visible plus an 80 MB `Vec` (`editor/gizzmos.rs:33`, `:339-340`).
- `ProfileLayer::on_new_span` takes the global `NAMES` mutex for every new span, also with recording off (`profiler/spans.rs:207-222`); per-thread span rings leak for the process' life (`:150`).
- Per-frame allocations: the console (`editor/console.rs:438-440`), the inspector's `reflect_clone` and labels (`editor/selected.rs:745-777`), the hierarchy (`editor/picking.rs:83-166`, not virtualised), `UiWindows::by_layer` (`ui/mod.rs:225-239`), `present` and `BindlessWrites::submit` `Vec`s, a `Box` per `retire`.
- Hot reload recompiles every pass serially on any include change (`command_buffer.rs:535-546`).
- Mesh data sits in host-visible memory (fine on this iGPU, bad on a discrete GPU), with vertices duplicated per meshlet and LOD at 32 B unquantised.
