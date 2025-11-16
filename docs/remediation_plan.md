# Kestrel Engine – Performance & Architecture Remediation Plan

This document tracks the staged remediation work. Each section calls out the goal, current status, and remaining tasks so progress stays visible as milestones land.

## 1. Plugin Runtime Failures (Weeks 1–2)

**Goal:** Plugin panics or reloads never destabilize the editor, watchdog metrics remain accurate, and isolated plugin metadata no longer leaks.

**Status:** **Partially Complete** – `App` now owns `PluginManager` directly (eliminating the swap/restore guard), panic isolation is covered by new regression tests, and plugin snapshots are cached. Remaining work focuses on isolated host cleanup and improved telemetry surfacing.

**Tasks**
- ✅ Replace the swap/restore pattern in `PluginRuntimeScope` with direct borrowing (manager now lives on `App`, so no guard is required).
- ✅ Add regression coverage (`plugin_panic_does_not_disrupt_other_plugins`, `plugin_status_snapshot_updates_on_change`) proving panics are isolated and cached snapshots refresh correctly.
- ✅ Ensure isolated plugin metadata is owned (proxy already stores `String` names/versions; no more `Box::leak` usage).
- ✅ Surface watchdog/capability failures through the analytics plugin so the UI can highlight which plugin was disabled.
- ✅ Add an integration test that repeatedly loads/unloads a dummy isolated plugin to verify no leaks or dangling state remain (`isolated_plugin_reload_cycle_does_not_accumulate_state`).
- ⬜ Run a manual soak test by triggering a deliberate plugin panic via the editor to confirm the offending plugin is disabled while the rest of the runtime stays responsive.

## 2. Frame-Time Allocation Budget (Weeks 2–4)

**Goal:** Idle frames allocate essentially zero bytes so frame-time plots reflect actual work, even when analytics panels are open.

**Status:** **In Progress** – Frame profiler samples, frame-time plots, plugin status data, prefab shelf entries, analytics recent events, scene dependency lists, GPU timings, and scripting tooling now reuse cached `Arc` snapshots, and per-frame allocation deltas are logged behind the `alloc_profiler` feature. Final step is validating the idle vs. panel-open measurements.

**Tasks**
- ✅ Rework `FrameProfiler`/analytics history so the editor consumes cached `Arc<[PlotPoint]>` data instead of cloning each frame.
- ✅ Cache telemetry-heavy collections (plugin statuses/capabilities, prefab entries, script console, analytics recent events, frame plots, scene history, retained atlas/mesh/clip lists, GPU timings).
- ✅ Apply the same snapshot approach to GPU timing history (`self.gpu_timings`) so egui reuses immutable data.
- ✅ Add lightweight allocation instrumentation (behind the `alloc_profiler` feature) that logs per-frame allocation deltas to validate improvements.
- ⬜ Compare `update_ms`, `ui_ms`, and allocation counters before/after to confirm idle-frame allocations stay flat even when toggling panels.

## 3. App Decomposition & Ownership Boundaries (Weeks 4–5+)

**Goal:** Separate responsibilities so runtime, editor UI, and plugin orchestration can evolve and be tested independently.

**Status:** **Not Started** – Refactor planning remains unchanged; implementation begins after the perf guardrails land.

**Tasks**
- Extract a `RuntimeLoop` module that owns the renderer, ECS, and fixed-step logic. Define a `RuntimeHost` trait so `App` depends on interfaces rather than concrete fields.
- Move plugin plumbing into `app::plugin_runtime`, with narrow APIs for loading, updating, and telemetry.
- Create an `EditorShell` module that owns egui state, telemetry caches, prefab workflows, script console, and animation tooling.
- Incrementally migrate subsystems (analytics UI, prefab shelf, mesh preview, REPL, file watchers) into focused modules, adding unit tests where practical.
- Update `docs/ARCHITECTURE.md` after each milestone to capture the new ownership diagram.

## 4. Renderer Pass Decomposition & Upload Efficiency (Weeks 5–6)

**Goal:** Reduce the 140 kB renderer monolith into manageable passes and eliminate avoidable CPU work per frame.

**Status:** **Not Started**

**Tasks**
- Split `Renderer` into pass-specific modules (swapchain/window management, sprite pass, mesh/shadow pass, light clusters, egui compositing).
- Convert frequently rebuilt temporaries into struct fields that reuse allocations by calling `Vec::clear()` instead of reallocating.
- Introduce a persistently mapped or ring-buffer-based instance upload path so large sprite batches no longer rewrite the full buffer each frame.
- Add pass-level tests/benchmarks (via the headless renderer hooks) to validate culling, light clustering, and GPU timing in isolation.

## 5. Telemetry & UI Snapshot Stabilization (Week 6)

**Goal:** All UI panels render from stable snapshots so toggling them on/off no longer impacts performance.

**Status:** **Partially Complete** – Plugin panels, prefab shelf, analytics recent events, frame plots, script console, REPL history, scene history, and retained asset lists consume cached `Arc` data. Remaining telemetry sources still need to adopt the same pattern and be validated through perf captures.

**Tasks**
- ✅ Extend `TelemetryCache`/runtime data to emit shared snapshots for prefab entries, plugin statuses, frame plots, analytics recent events, scene history, scripting tooling, and animation telemetry tables.
- ⬜ Measure editor responsiveness with all panels open to confirm allocation counters remain flat and frame-time variance stays low.

## Suggested Timeline Overview

| Week | Focus |
| --- | --- |
| 1–2 | Plugin runtime guardrails, panic handling, and leak fixes |
| 2–4 | Frame-time allocation work (profiler snapshots, UI caching, instrumentation) |
| 4–5+ | App decomposition into runtime/editor/plugin modules |
| 5–6 | Renderer pass separation and GPU upload efficiency |
| 6 | Telemetry snapshot stabilization and validation |

Following this order keeps the runtime stable before large-scale refactors and ensures performance instrumentation remains trustworthy as architecture evolves.
