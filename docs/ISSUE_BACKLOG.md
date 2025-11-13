# Stretch Goal Issue Backlog

This backlog converts every open stretch goal from `docs/MILESTONE_STATUS.md` into actionable work. Priorities follow a simple scheme:

- **P0** – Stability/determinism tasks that guard core engine loops.
- **P1** – Tooling and workflow improvements that unblock daily editing.
- **P2** – Strategic feature bets for long-term differentiation.

Each issue lists its originating milestone plus crisp acceptance criteria so it can be copied directly into your issue tracker.

## P0 — Stability & Determinism

1. **[M1] Swapchain regression harness** — ✅ *Completed via headless render path + synthetic surface-error tests.*
   - *Highlights:* `Renderer::surface_tests::headless_render_recovers_from_surface_loss` now drives a fully headless render target, injects `SurfaceError::Lost`, and verifies that `render_frame` cleanly errors, triggers a resize, and then succeeds after recovery. Supporting hooks (`prepare_headless_render_target_for_test`, `inject_surface_error_for_test`) keep the harness deterministic.
2. **[M1] Configurable VSync toggle** — ✅ *Completed (runtime toggle + renderer reconfigure UI panel).*
   - *Why:* Perf testing requires deterministic presentation modes.
   - *Acceptance:* Extend `config/app.json` plus the in-app UI so VSync can be toggled at runtime, with the renderer reconfiguring the surface immediately; log the active mode in analytics.
3. **[M3] Config-driven input remapping** - *Completed via `Input::from_config` loader + `InputBindings` overrides + regression test (`tests/input_bindings.rs:1`).*
   - *Highlights:* `InputBindings::load_or_default` consumes `config/input.json` with warnings for unknown keys, and `src/app/mod.rs:460` wires the config-driven bindings at startup.
   - *Why:* Hard-coded bindings block automated stress tests and accessibility.
   - *Acceptance:* Introduce `config/input.json` (or similar) and update `Input` so bindings load at startup with sensible fallbacks; emit warnings for unknown keys and add a regression test that remaps spawn controls.
4. **[M10] Scene round-trip regression suite** - *Completed via expanded `tests/scene_roundtrip.rs` coverage and dependency retention checks (`tests/scene_roundtrip.rs:530`).*
   - *Highlights:* The suite now serializes nested hierarchies, asserts parent/child IDs, round-trips JSON without diffs, and exercises retain/release flows for atlases, meshes, and environments (`tests/scene_roundtrip.rs:631`, `tests/scene_roundtrip.rs:700`).
   - *Why:* Complex scenes (meshes + materials + environments) need coverage beyond the existing happy-path test.
   - *Acceptance:* Expand `tests/scene_roundtrip.rs` (or add a new suite) to include nested hierarchies, dependency retain/release, and environment metadata. Tests should assert reference counts and serialized JSON diffs.
5. **[M11] Editor workflow regression tests** - *Completed via headless harness test (`src/app/gizmo_interaction.rs:576`).*
   - *Why:* Gizmo/selection bugs are common whenever egui changes land.
   - *Acceptance:* Build a headless egui test that simulates selecting an entity, switching gizmo modes, saving, and reloading. The test should confirm entity IDs remain stable and gizmo interaction state resets cleanly.
6. **[M1 Perf] Sprite animation telemetry & HUD instrumentation** - *In progress this sprint (see `docs/SPRITE_ANIMATION_PERF_PLAN.md`, Phase 2).*
   - *Why:* Hitting ≤0.200 ms @ 10k animators only holds if slow-path usage is visible to authors; we need per-frame counters and HUD surfacing to keep asset regressions obvious.
   - *Acceptance:* 
     1. Add zero-allocation per-frame counters (`const_dt`, `var_dt`, `ping_pong`, `events_heavy`, `%slow`) to the animation runtime and expose them via the profiler resource.
     2. Extend the Stats panel/HUD to display these counters with threshold coloring plus split CPU evaluation vs GPU palette upload timings.
     3. Document the workflow in `docs/animation_workflows.md` with screenshots so authors know how to read the new metrics.
7. **[M1 Perf] Bench telemetry export & GPU timing split** - *In progress this sprint (Plan Phase 2.3–2.4).*
   - *Why:* `animation_targets_measure` must prove the HUD counters match bench output, and palette uploads need distinct timing so we can budget CPU vs GPU separately.
   - *Acceptance:*
     1. Update `animation_targets_measure` to record and emit the new counters per run (stdout + `target/animation_targets_report.json`).
     2. Ensure GPU palette upload timings are reported independently of CPU evaluation both in the Stats panel and bench artifacts.
     3. Store the resulting CSV/JSON in `perf/` during perf captures so CI and docs can reference the exact numbers.

## P1 — Tooling & Workflow Enhancements

6. **[M2] Animated sprite timelines** - *Completed via atlas timeline metadata + ECS animation system + inspector controls.*
   - *Highlights:* Atlas JSON now carries timeline definitions (`assets/images/atlas.json:31`), runtime parsing streams animation frames into ECS components (`src/assets.rs:22`, `src/ecs/systems/animation.rs:1`, `src/ecs/world.rs:595`), and the inspector exposes playback/loop controls with timeline selection (`src/app/editor_ui.rs:885`). A regression test exercises parsing, playback, pause/resume, and reset behavior (`tests/sprite_animation.rs:1`).
7. **[M3.5] Quadtree fallback with perf telemetry** - *Completed via density-aware quadtree builder + analytics wiring.*
   - *Highlights:* `SpatialQuadtree` and adaptive metrics drive the fallback decision inside the physics systems (`src/ecs/physics.rs:160`, `src/ecs/systems/physics.rs:120`), analytics now records spatial occupancy snapshots so the stats panel can report mode/cell pressure plus expose a runtime toggle and threshold control (`src/app/editor_ui.rs:250`, `src/analytics.rs:13`). Regression tests cover the automatic activation path and metric reporting (`tests/spatial_index.rs:1`).
8. **[M4] CLI overrides for config values** - *Completed via CLI parser + AppConfigOverrides (runtime precedence logging + parser tests).*
   - *Acceptance:* Support `kestrel_engine --width 1280 --height 720 --vsync off` (or similar) with precedence rules logged at startup; include unit tests for argument parsing.
9. **[M5] Profiler & metrics panel with snapshot tests** - *Completed via runtime frame/system profiler + egui snapshot helpers.*
   - *Highlights:* Runtime frame timing history now records per-frame/update/fixed/render/UI durations (`src/app/mod.rs:1250`) and ECS systems report per-system timings through the new profiler resource (`src/ecs/profiler.rs:1`, `src/ecs/systems/*.rs`). The editor stats panel exposes a collapsible profiler table with frame summaries and ranked system timings plus quadtree metrics (`src/app/editor_ui.rs:250`). Snapshot-style tests cover the string summaries used in the panel to detect layout regressions (`src/app/editor_ui.rs:2098`).
10. **[M6] Multi-camera / follow-target tooling** - *Completed via camera bookmark selector + follow controls (`src/app/editor_ui.rs:490`).*
    - *Highlights:* The editor UI now manages bookmarks and follow targets, while scene metadata captures the active bookmark or followed entity (`src/app/mod.rs:1140`, `src/scene.rs:44`).
    - *Acceptance:* Allow the viewport to switch between multiple named cameras or follow a selected entity; persist the choice in scene metadata.
11. **[M7] Scripting debugger / REPL** - *Completed via debugger window + Rhai REPL plumbing (`src/scripts.rs:202`, `src/app/mod.rs:780`, `src/app/editor_ui.rs:860`).*
    - *Highlights:* `ScriptHost::eval_repl` shares scope state with runtime scripts and queues commands/logs for the engine (`src/scripts.rs:202`), the app tracks console/history state plus pause/step control wiring (`src/app/mod.rs:780`), and the egui debugger window exposes REPL input, command history, and auto-focus on errors (`src/app/editor_ui.rs:860`).
    - *Acceptance:* Embed a lightweight Rhai REPL with pause/step controls and command history; script errors should focus in the debugger panel.
12. **[M8] Particle budget analytics** - *Completed via ECS telemetry + Stats panel (`src/ecs/world.rs:322`, `src/app/editor_ui.rs:250`).*
    - *Acceptance:* Stream current particle counts, emitter backlog, and cap utilization into the analytics UI to spot runaway effects.
13. **[M9] Audio capability diagnostics** - *Completed via device metadata telemetry + analytics logging (`src/audio.rs:22`, `src/app/mod.rs:471`).*
    - *Acceptance:* Extend the new `AudioHealthSnapshot` telemetry with device name/sample rate info and route it into analytics/logs when initialization fails.
14. **[M10] Binary `.kscene` serializer** - *Completed via bincode+LZ4 pipeline with feature flag + tooling (`src/scene.rs:1100`, `src/bin/scene_tool.rs:10`).*
    - *Highlights:* Scenes saved with a `.kscene` extension now emit a versioned `KSCN` payload that bincode-serializes the normalized scene graph and compresses it with LZ4 (gated behind the `binary_scene` feature), while the loader auto-detects the magic header to decode or surface a helpful error if the feature is disabled.
    - *Tooling:* `scene_tool convert <input> <output>` converts between `.json` and `.kscene`, so existing content can migrate without manual edits.
    - *Acceptance:* Implement a binary encoder/decoder (with compression) behind a feature flag; provide migration tooling between JSON and binary formats.
15. **[M11] Drag-and-drop prefab authoring**
    - *Acceptance:* Allow entities or hierarchies to be dragged from the inspector into a prefab shelf, saved as JSON/`kscene`, and instanced via drag/drop.
    - *Note:* Binary `.kscene` export requires launching with the `binary_scene` feature enabled; when absent the Prefab Shelf surfaces guidance to switch formats.

## P2 — Strategic Feature Bets

16. **[M12] Advanced shadow mapping & light culling**
    - *Acceptance:* Introduce cascaded or clustered shadows plus light culling for 3D meshes, with performance counters exposed in analytics.
17. **[M12] GPU performance baselines** - *Completed via timestamp query instrumentation + profiler export.*
    - *Highlights:* The renderer now wraps render passes with timestamp queries and resolves per-pass GPU durations (`src/renderer.rs:108`, `src/renderer.rs:1869`), the app records recent GPU timing history and provides a CSV exporter (`src/app/mod.rs:308`, `src/app/mod.rs:784`), and the stats panel surfaces latest/average GPU timings with an export control (`src/app/editor_ui.rs:1536`).
18. **[M13] Plugin sandbox for untrusted modules**
    - *Acceptance:* Design an opt-in sandbox (capabilities/IPC or WASM) so dynamic plugins run without direct memory access; include a compatibility matrix and contract tests.
19. **[M9] Positional audio & falloff curves**
    - *Acceptance:* Integrate simple spatialization (pan + falloff) tied to entity positions; expose tuning sliders in the audio panel and verify via automated tests with deterministic triggers.
20. **[Long-term] Force fields, attractors, and particle trails**
    - *Acceptance:* Expand the particle system with force-field components, attractor/repulsor entities, and stretched billboard trails, all editable via the inspector.

Feel free to slice these issues finer when you import them into your tracker; the acceptance criteria can serve as the initial definition of done.
