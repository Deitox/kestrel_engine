# Animation System Roadmap — Review & Suggestions

**Summary:** This roadmap is excellent—coherent milestones, ECS-first, and it scales from flipbooks to skeletal rigs without overpromising. Below are targeted suggestions to tighten scope, de-risk tricky areas, and turn each milestone into shippable increments with clear exit criteria and perf budgets.

---

## High‑level praise
- **Layered growth:** Sprite → property tracks → skeletal → graphs → tooling is the right order.
- **Determinism stance:** Fixed‑step options + golden tests are called out—great for CI and replays.
- **Plugin & versioning hooks:** You’re thinking ahead about API stability and migration helpers.

---

## What I’d change or clarify

### 1) Define hard **performance budgets** up front
- **Sprite timelines:** ≤ **0.20 ms** CPU / 10k animators per frame on a mid‑tier desktop (release build).
- **Property tracks:** ≤ **0.40 ms** CPU / 2k clips (linear interpolation only at first).
- **Skeletal CPU evaluate:** ≤ **1.2 ms** for **1k bones** total; GPU upload ≤ **0.5 ms**.
- Track these in a tiny `animation_bench.rs` you can run in CI and print min/avg/max.

### 2) Tighten **Milestone 1** scope to “ship ready”
- **Importer MVP:** Pick **Aseprite JSON** first; defer LDtk to Milestone 1.1.
- **Loop modes order:** `OnceStop` (have), **`OnceHold`**, **`Loop`**, then **`PingPong`** (do last; it touches edge math).
- **Events:** Start with **frame index only** (no params). Add payloads later.
- **Hot‑reload:** Rebind by **frame name**, not index, so edits don’t scramble running anims.
- **Authoring doc:** include **atlas/timeline examples** + a “5‑minute” path from Aseprite → `atlas.json`.

### 3) Add a **“no‑alloc per frame”** rule
- All playback systems must avoid heap allocs in `update()`: preallocate frame arrays, cache UVs, and use interning for region names/IDs.

### 4) Property tracks: constrain **interpolation** in v1
- Support **Step** and **Linear** only. Defer **Cubic** until you have a clear need + profiler headroom.
- Merge tracks by **last writer wins** per component to keep evaluation O(N) and simple.

### 5) Skeletal: pick one **clip format** and one **skin path**
- Target **GLTF** exclusively at first (Spine later). GLTF gives you broad DCC support and stable tooling.
- Implement **CPU pose evaluation** + **GPU skinning** (palettes as uniform/storage buffers). Skip CPU skinning unless you need a fallback.

### 6) Graphs: start **stateless** then add parameters
- Milestone entry: **state machine** with time‑based transitions and simple condition flags.
- Then add **float params** and **1D blends**. Defer 2D blends/additive layers until skeletal is stable.

### 7) Testing and CI: make it **visible**
- Add a tiny **“Playback Viz”** test that renders N frames offscreen and hashes output to catch regressions.
- **Hot‑reload test**: modify a timeline during playback; assert the current frame name persists when possible.

### 8) Editor UX: make wins cheap
- Timeline **scrubber** in the inspector (no full dope sheet yet). Left/right **frame nudge** buttons.
- **Event preview** toggle that prints fired event names into the inspector status area.
- **Perf readout** in the Status bar: “Anim eval: X ms (Y animators, Z bones)”

### 9) Serialization/versioning
- Version `atlas.json` and track **schema version** in scenes/prefabs. Add a small **migrator** (`scripts/migrate_atlas`) that bumps old files.

### 10) Scripting surface (Rhai) guidelines
- Scripts **issue commands** (play/stop/set_fps/seek) but **never drive per‑frame** updates.
- Provide **async‑like helpers**: `await_anim_end(entity, "attack")` by polling in engine (not the script) and resuming the script callback.

---

## Reframed Milestones with Exit Criteria

### Milestone 1 — Productionize Sprite Timelines (tight)
**Scope:**
- Aseprite→timeline importer (CLI).
- Loop modes: OnceStop, OnceHold, Loop, PingPong.
- Events (frame index only), phase offset & random start.
- Hot‑reload by name; no per‑frame allocs; inspector scrubber & nudge.

**Exit Criteria:**
- Pass **golden playback** tests for all modes.
- `animation_bench` shows ≤ **0.20 ms** for **10k** animators (release).
- Hot‑reload test preserves current frame by name.

### Milestone 2 — Transform & Property Tracks (practical)
**Scope:** Step/Linear interpolation for translation/rotation/scale/tint; `ClipInstance`; inspector bindings.
**Exit Criteria:** ≤ **0.40 ms** for **2k** clips; golden end‑pose tests; scene/prefab round‑trip.

### Milestone 3 — Skeletal MVP (GLTF)
**Scope:** GLTF import (joints/weights), CPU pose, GPU skinning, skeleton inspector.
**Exit Criteria:** ≤ **1.2 ms** CPU for **1k bones**; ≤ **0.5 ms** GPU upload; pose validation tests pass.

### Milestone 4 — Animation Graph v0
**Scope:** State machine with flags/float params; 1D blend; script API for params; debug panel for active state & weights.
**Exit Criteria:** Deterministic graph tests; perf budget met with 5 characters x 3 layers.

### Milestone 5 — Tooling & Automation
**Scope:** Simple keyframe editor (step/linear), exporters/watchers, analytics integration.
**Exit Criteria:** Authoring “5‑minute path” doc; CI validates assets; playback viz hash stable.

---

## Concrete next PRs (one‑sitting tasks)
1. `animation_bench.rs` with entity‑count sweeps and CSV output.
2. Inspector: add scrubber + left/right frame nudge.
3. Hot‑reload by name; keep `frame_index` when names align, else find nearest.
4. Aseprite JSON → timeline CLI (MVP).

---

## API Additions (small, powerful)
```rust
// Control (already have core commands; add these)
fn seek_sprite_animation_frame(entity, usize) -> bool;
fn seek_sprite_animation_time(entity, f32) -> bool;
fn set_sprite_animation_phase_offset(entity, f32) -> bool;

// Query (handy for scripts/tools)
fn current_sprite_animation(entity) -> Option<&'static str>;
fn current_sprite_frame(entity) -> Option<usize>;
```

---

## Risks to watch
- **PingPong off‑by‑one** (duplicate end frames). Add tests that assert forward/backward edges.
- **Event storms** at high FPS. Throttle events to **frame boundaries** only.
- **Skinning buffer limits**. For WebGPU/DX12, pick a **max bone count** per draw and split batches if needed.

**Bottom line:** Your plan is strong; with these scope guards, budgets, and testable exit criteria, each milestone will land in a shippable state and keep performance predictable.
