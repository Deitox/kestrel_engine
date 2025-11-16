# Kestrel Engine - Performance & Architecture Remediation Plan

This plan captures the remediation work for the three most pressing issues surfaced in the recent review. The items are ordered so that stability comes first, reliable profiling second, and larger architectural refactors last.

## 1. Plugin Host Stability (High Priority)

**Problem statement**
- `App::with_plugin_runtime` (src/app/mod.rs:2935) moves the `PluginHost`/`PluginManager` out of `App`. If a plugin panics or returns early before `host.restore_manager` executes, the real host is dropped and the editor cannot recover.
- `IsolatedPluginProxy::new` (src/plugins.rs:1603) leaks plugin names/versions by calling `Box::leak`, so every reload grows the heap.

**Goals**
1. A plugin panic cannot bring down the editor; the offending plugin is disabled, metrics/logs capture the failure, and the runtime keeps running.
2. Isolated plugins clean up all allocations on unload and record watchdog events when they misbehave.

**Actions**
1. Replace the swap/restore pattern with an RAII guard that borrows the host/manager. Wrap plugin callbacks in `std::panic::catch_unwind` so we can log and disable panicking plugins safely.
2. Emit `PluginWatchdogEvent`s and transition plugins to `PluginState::Failed` when a panic occurs. Surface these events in the analytics plugin for visibility.
3. Store plugin names/versions as owned `Arc<str>` (or `String`) inside `IsolatedPluginProxy` to eliminate leaks when reloading.
4. Add regression tests:
   - Unit test in `app::plugin_host` that injects a plugin whose `update` panics and asserts the guard restores state while disabling the plugin.
   - Integration test that repeatedly loads/unloads a dummy isolated plugin to ensure no leaks and no dangling state.

**Validation**
- `cargo test app::plugin_host` and `cargo test plugins::isolated_proxy`.
- Manual soak test: trigger a deliberate plugin panic from the editor, verify the plugin is disabled, and confirm the rest of the UI remains responsive.

## 2. Frame-Time Allocation Budget (Medium-High)

**Problem statement**
- Each redraw clones large telemetry collections (frame timings, analytics plots, plugin history) inside `redraw_requested` (`src/app/mod.rs:4225-4340`). `FrameProfiler::samples` also copies its history every frame.
- Plugin UI helpers (`PluginManager::capability_metrics`, `asset_readback_metrics`, etc.) return fresh `HashMap`s each call, so simply opening the Plugins panel causes spikes in heap allocations.

**Goals**
1. Idle frames allocate essentially zero bytes so timing plots reflect real work.
2. Plugin inspection no longer duplicates large data structures every frame.

**Actions**
1. Update `FrameProfiler` to expose iterators/slices over its `VecDeque` so UI code consumes borrowed data instead of cloned vectors.
2. Cache derived collections (mesh keys, prefab lists, environment options) with dirty flags so they only rebuild when inputs change.
3. Change plugin manager accessors to return cached `Arc<HashMap<_>>` snapshots or streaming iterators instead of cloning per call.
4. Add lightweight instrumentation (e.g., `tracing` span with `allocator_api2::Global::stats()` or a custom counter behind a dev feature) to log per-frame allocation deltas during profiling sessions.

**Validation**
- Compare `update_ms`/`ui_ms` and allocation counters before/after via the analytics plots.
- Run a release build with the new allocation instrumentation to confirm idle frames remain flat while toggling editor panels.

## 3. App Decomposition & Ownership Boundaries (Medium)

**Problem statement**
- `App` (src/app/mod.rs:409) owns the renderer, ECS, input routing, analytics UI, prefab workflows, and plugin orchestration. The 5k-line module makes it difficult to reason about responsibilities or to unit-test subsystems in isolation.

**Goals**
1. Introduce clear module boundaries so rendering/loop logic, editor tooling, and plugin orchestration evolve independently.
2. Enable targeted testing (e.g., editor UI logic without requiring a real `Renderer`).

**Actions**
1. Extract a `RuntimeLoop` module that owns the renderer, ECS, and frame/fixed-step timing. `App` should depend on a trait (e.g., `RuntimeHost`) rather than concrete fields for these responsibilities.
2. Move egui/tooling state into an `EditorShell` module that consumes a runtime interface, keeping UI logic and caching localized.
3. Relocate plugin plumbing (`PluginHost`, `PluginManager`, `PluginContext`) into `app::plugin_runtime` with well-defined APIs so other modules interact through small interfaces.
4. Incrementally migrate subsystems (analytics, prefab shelf, mesh preview) to their own files/modules, each with focused tests.

**Validation**
- Introduce new unit or integration tests per module (e.g., `editor_shell::tests::prefab_history`) plus a smoke test ensuring `App` wires modules correctly.
- Update `docs/ARCHITECTURE.md` after each milestone to capture the new ownership diagram.

## Suggested Timeline
1. Weeks 1-2: Finish plugin host safety work, land tests, and run panic/reload soak tests.
2. Weeks 3-4: Implement allocation caching, add instrumentation, and document frame budget baselines.
3. Weeks 5+: Execute the decomposition plan module-by-module while monitoring CI timings and editor UX.

Following this order keeps the runtime stable before large-scale refactors and ensures performance data is trustworthy when tuning later milestones.
