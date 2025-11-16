# Kestrel Engine – Performance & Architecture Remediation Plan

This document captures the staged plan for addressing the performance and architectural issues highlighted in the recent review. Each section lists the motivation, the concrete work items, and the target timeline so that changes can land incrementally without destabilizing the editor.

## 1. Plugin Runtime Failures (Weeks 1–2)

**Goal:** Plugin panics or reloads never destabilize the editor, watchdog metrics remain accurate, and isolated plugin metadata no longer leaks.

**Tasks**
- Replace the swap/restore pattern in `PluginRuntimeScope` with an RAII guard that only borrows the host/manager and automatically restores them even if a callback panics (`src/app/mod.rs:3038-3084`).
- Wrap every plugin callback (`PluginManager::update`, `fixed_update`, `handle_events`, `shutdown`) in `std::panic::catch_unwind`, emitting `PluginWatchdogEvent`s and marking failing plugins as `PluginState::Failed` in place (`src/plugins.rs:1326-1465`).
- Update `IsolatedPluginProxy` so plugin names/versions are stored as owned `Arc<str>`/`String` values instead of using `Box::leak`, ensuring reloads free memory.
- Surface watchdog/capability failures through the analytics plugin so the UI can highlight which plugin was disabled.
- Add regression tests: (1) a unit test proving the runtime guard restores state after a panic, and (2) an integration test that repeatedly loads/unloads a dummy isolated plugin to verify no leaks or dangling state remain.
- Run a manual soak test by triggering a deliberate plugin panic via the editor to confirm the offending plugin is disabled while the rest of the runtime stays responsive.

## 2. Frame-Time Allocation Budget (Weeks 2–4)

**Goal:** Idle frames allocate essentially zero bytes so frame-time plots reflect actual work, even when analytics panels are open.

**Tasks**
- Rework `FrameProfiler` to expose slices/iterators over its `VecDeque` rather than cloning into a new `Vec` every frame. Apply the same approach to GPU timing history so both the editor and analytics share snapshots via `Arc<[Sample]>` instead of cloning (`src/app/mod.rs:362-4448`).
- Cache plugin status/capability data: plugin manager helpers (`capability_metrics`, `asset_readback_metrics`, `ecs_query_history`, `watchdog_events`) should return cached `Arc<HashMap<…>>` snapshots that only rebuild when inputs change (`src/plugins.rs:820-1165`).
- Refactor editor panels (plugin list, script console, REPL history, prefab shelf) to consume cached telemetry snapshots rather than cloning `Vec`s each frame (`src/app/editor_ui.rs:2434-2609`, `src/app/mod.rs:4516-4543`).
- Add lightweight allocation instrumentation (behind a dev feature) that logs per-frame allocation deltas so profiling sessions can verify improvements.
- Compare `update_ms`, `ui_ms`, and allocation counters before/after to confirm idle-frame allocations remain flat even when toggling the analytics panels.

## 3. App Decomposition & Ownership Boundaries (Weeks 4–5+)

**Goal:** Separate responsibilities so runtime, editor UI, and plugin orchestration can evolve and be tested independently.

**Tasks**
- Extract a `RuntimeLoop` (or similar) module that owns the renderer, ECS, and fixed-step logic. Define a `RuntimeHost` trait so `App` only depends on the trait instead of concrete internals.
- Move plugin plumbing (`PluginHost`, `PluginRuntimeScope`, analytics hooks) into `app::plugin_runtime`, with narrow APIs for loading, updating, and telemetry so other modules interact through well-defined boundaries.
- Create an `EditorShell` module that owns egui state, telemetry caches, prefab workflows, script console, and animation tooling. `EditorShell` should communicate with the runtime through explicit interfaces, shrinking `App` dramatically (`src/app/mod.rs` currently ~230kB).
- Incrementally migrate subsystems (analytics UI, prefab shelf, mesh preview, REPL, file watchers) into focused modules, adding unit tests where practical (e.g., `editor_shell::tests::prefab_history`).
- Update `docs/ARCHITECTURE.md` after each milestone to reflect the new ownership diagram and interfaces.

## 4. Renderer Pass Decomposition & Upload Efficiency (Weeks 5–6)

**Goal:** Reduce the 140 kB renderer monolith into manageable passes and eliminate avoidable CPU work per frame.

**Tasks**
- Split `Renderer` into pass-specific modules: swapchain/window management, sprite pass, mesh/shadow pass, light clusters, and egui compositing. Each pass should own its pipelines, scratch buffers, and configuration (`src/renderer.rs:1-3300`).
- Convert frequently rebuilt temporaries (`sprite_bind_groups`, culled mesh draw lists, light-cluster scratch buffers) into struct fields that reuse allocations by calling `Vec::clear()` each frame instead of re-allocating.
- Introduce a persistently mapped or ring-buffer-based instance upload path so large sprite batches no longer overwrite the full buffer every frame.
- Add pass-level tests/benchmarks (using the existing headless renderer hooks) to validate culling, light clustering, and GPU timing in isolation.

## 5. Telemetry & UI Snapshot Stabilization (Week 6)

**Goal:** All UI panels render from stable snapshots so toggling them on/off no longer impacts performance.

**Tasks**
- Extend `TelemetryCache` to produce shared snapshots for plugin statuses, capability metrics, script console entries, REPL history, prefab entries, and animation telemetry. Snapshots should refresh only when inputs change.
- Update egui panels to consume these immutable snapshots (iterators over `Arc<[T]>`) instead of rebuilding `Vec`s on every `frame`.
- Measure editor responsiveness with all panels open to confirm allocation counters remain flat and frame-time variance stays low.

## Suggested Timeline Overview

| Week | Focus |
| --- | --- |
| 1–2 | Plugin runtime guardrails, panic handling, and leak fixes |
| 2–4 | Frame-time allocation work (profiler snapshots, UI caching, instrumentation) |
| 4–5+ | App decomposition into runtime/editor/plugin modules |
| 5–6 | Renderer pass separation and GPU upload efficiency |
| 6 | Telemetry snapshot stabilization and validation |

Following this order ensures stability work lands before large-scale refactors, and that performance instrumentation remains trustworthy as the architecture evolves.
