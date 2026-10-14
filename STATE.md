# State: known bugs, intended limitations and ideas

Bugs come from a read-only review on 2026-10-11 (**verified**: confirmed in the code; **plausible**: follows from the code but wasn't reproduced; line numbers are from that date). Anything under `--- fix ---` gets fixed right away; `--- intended ---` entries stay here so they aren't forgotten; `--- find solutions ---` entries get proposals without code changes; `--- future work ---` entries are agreed directions waiting for their time. `## Ideas` holds design proposals we don't want to lose.

## lava: ray tracing (none of it is used yet, and nothing tests it)
----intended----
- The bindless layout has no acceleration structure binding and no shader reaches one, so a ray tracing pass can't trace anything yet; `stage_name` panics on intersection and callable stages (`build.rs`).

## core: renderer
----intended----
- **GPU use-after-free on unload and hot reload (verified).** Texture images are destroyed on drop while frames in flight sample them through their bindless slots, and `GpuMaterial::drop` returns its slot at once (`assets/material.rs:211-216`), so a pending frame can read another material. Rebaking with the editor open triggers it. (Mesh buffers are kept alive by their frames since the keepalive slab, through `Frame::uses`.)
(This will be reworked when we move onto streaming geometry anyways)
- `visible_meshlets()` reads `variables` on the CPU while the GPU may write it (`render/render.rs:557-560`).
(only a debug feature anyways)
- When `record_frame` grows the viewport target (`render/render.rs:804-813`) the tab draws one frame with the old handle and size.
(same here)

## core: assets, scene, physics
--- find solutions ---
- Cracks can open between separate primitives sharing an edge at LOD transitions; borders are only locked within one mesh (plausible). (Goes away with "One mesh per glTF mesh" under Ideas.)
- With attribute-aware simplification, meshopt's error includes normal and UV deviation (`assets/mesh.rs:624-635`), mixing units into the world error (conservative).
(this was added to prevent big surfaces from changing uvs and introducing noticible texture diffrences)
- Odd-sized mips drop their last row or column (`assets/texture.rs:377-381`); normal map mips aren't renormalised.
- The bake silently drops glTF samplers, `doubleSided`, tangents, vertex colours and every extension except lights, texture transforms and instancing, which warn (`assets/bake.rs`).

---intended---
- `ray_triangle_intersect` uses an absolute `1e-8` determinant epsilon (`physics/bvh.rs:438`) and misses tiny triangles (plausible).
- `Pod`/`Zeroable` for `HasBlas` holding an `Entity` is unsound (`physics/bvh.rs:41-42`); all-zero isn't a valid `Entity`.
(Physics BVH is only for viewport picking for now, the whole system will be reworked in the future anyways)
- Instance indices follow the extract query's order (`render/world.rs:127`) and change on spawn, despawn and on selection (archetype move), so nothing per instance can persist across frames.
(Where would this even be an issue?)
- The scene loader doesn't reject parent cycles or self-parents (`scene/file.rs:377-382`) (plausible).
(we will trust the user not to pass in a malformated file)
- File formats have no magic or version; `read_slice`/`read_compressed` allocate whatever length the file says (`assets/util.rs:30`, `:39-41`); the mesh loader trusts header offsets (`cull_data_offset - meshlet_offset` can underflow, `assets/mod.rs:71-97`), and corrupt mesh data becomes raw GPU pointers. 
(Again, we trust the user to pass in well formated files)

## core: editor and UI
--- future work ---
- UB in the UI builder: `&window.focused` is cast to `*mut` through a shared `Res<UiWindows>` (`ui/builder.rs:142`) while UI systems run in parallel.
(Only one tab can ever hold the `FocusedState`, so it may be better off in a global resource with an `UnsafeCell` than as a `Mutex` per window; kept as is for now)

--- intended ----
- The editor camera overwrites its rotation every frame from a `Local` pitch/yaw (`editor/camera.rs:47`, `:90`), so inspector edits are lost and a camera spawned rotated snaps to yaw 0.
(currently just a testing tool, not realy nessairy to be correct)
- A `Set` is edited through a const-to-mut pointer cast (`editor/selected.rs:340-360`).
(The entity editor will need a rework at some point anyways, leave it be for now)

## Performance bottlenecks
--- intended ---
- No occlusion culling: every hidden fragment writes the 16-byte G-buffer.
(occulsion culling is on the roadmap, this is fine for now)

--- find solutions ----
- Pipelines are created synchronously on first use, under a write lock, with no `VkPipelineCache` (`command_buffer.rs:233-244`, `:405`, `:602-607`, `:768`). (Proposals in `lava/src/scratchpad.rs`: per-pass locks, a warm-up list, a cache file.)

--- future work ---
- Every instance is re-extracted and rewritten each frame, static or not, reading each material from host-mapped memory (`render/world.rs:120-179`).
(Persistent instance slots with change detection, sketched in `core/src/scratchpad.rs`; needs more thought first)
- Per-frame allocations in the editor: the inspector's `reflect_clone` and labels (`editor/selected.rs:745-777`), the hierarchy (`editor/picking.rs:83-166`, not virtualised), `UiWindows::by_layer` (`ui/mod.rs:225-239`).
(The editor is a debug feature: not worth complicating. Sketches in `core/src/scratchpad.rs`)
 
--- intended ---
- `update_bvh` rebuilds the CPU TLAS every `PreUpdate` with fresh `Vec`s, and raycasts allocate a `Vec` per node (`physics/bvh.rs:306`, `:390`, `:563-608`). The builder groups Morton-sorted items without SAH.
(as said before, only temporary for debug purposes)
- Hot reload recompiles every pass serially on any include change (`command_buffer.rs:535-546`).
(hot reload is a debug feature anyways)
- Mesh data sits in host-visible memory (fine on this iGPU, bad on a discrete GPU), with vertices duplicated per meshlet and LOD at 32 B unquantised.
(optimizing for dGPUs is still todo)

## Ideas

### One mesh per glTF mesh, materials per meshlet (2026-10-13)
Survey: Unity and Godot treat a model file as a scene (prefab / `PackedScene`) whose meshes are sub-resources you reference or extract; neither merges at import. Unreal's Content Browser import makes one StaticMesh per mesh node ("Combine Static Meshes" forces one), "Import Into Level" and Datasmith keep the node tree as actors, and a StaticMesh is the glTF *mesh object* with its primitives as material sections: Nanite builds one cluster DAG per StaticMesh across sections and resolves the material per cluster and triangle through a table. Godot adds per-node name suffixes (`-col`, `-noimp`, ...) set in the DCC.

Proposal, no import flag:
1. The mesh unit becomes the glTF mesh, not the primitive: all primitives bake into one `.mesh` with one simplification DAG (the primitive seam becomes an interior edge, which removes the cross-primitive LOD cracks), meshlets built per primitive so none mixes materials, a material slot index per meshlet, `Instance` with one material handle per slot, the material looked up per meshlet in `scene_raster.slang` and opaque/blended decided per meshlet in `bvh_cull.slang`. The "one child entity per primitive" case leaves the bake and the scene files.
2. A `.mesh` carries its default material slots (paths relative to the file, like a material's textures), so a `.mesh` dropped from the browser spawns complete and the scene only overrides slots.
3. The file stays a scene; a glTF of one mesh node is a scene of one entity (the browser can show it with a mesh icon and drop it like a mesh).
4. Merging is the author's choice: a node suffix like `-merge` makes the bake weld that subtree into one `.mesh` with the children's transforms baked in; later an editor "merge selected" for already baked assets.
Steps 1 and 2 change the format (`--force` bake) and fit the streaming geometry rework; 4 is independent.
