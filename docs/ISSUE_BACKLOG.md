# Stretch Goal Issue Backlog

This backlog converts every open stretch goal from `docs/MILESTONE_STATUS.md` into actionable work. Priorities follow a simple scheme:

- **P0** – Stability/determinism tasks that guard core engine loops.
- **P1** – Tooling and workflow improvements that unblock daily editing.
- **P2** – Strategic feature bets for long-term differentiation.

Each issue lists its originating milestone plus crisp acceptance criteria so it can be copied directly into your issue tracker.

## P0 — Stability & Determinism

1. **[M1] Swapchain regression harness**
   - *Why:* Surface-loss handling is the riskiest WGPU code path today.
   - *Acceptance:* Add a headless integration test (mock surface or `wgpu` headless instance) that recreates the renderer after synthetic `SurfaceError::Lost/Outdated`. The test should fail if `Renderer::render_frame` ever panics after a simulated loss.
2. **[M1] Configurable VSync toggle** — ✅ *Completed (runtime toggle + renderer reconfigure UI panel).*
   - *Why:* Perf testing requires deterministic presentation modes.
   - *Acceptance:* Extend `config/app.json` plus the in-app UI so VSync can be toggled at runtime, with the renderer reconfiguring the surface immediately; log the active mode in analytics.
3. **[M3] Config-driven input remapping**
   - *Why:* Hard-coded bindings block automated stress tests and accessibility.
   - *Acceptance:* Introduce `config/input.json` (or similar) and update `Input` so bindings load at startup with sensible fallbacks; emit warnings for unknown keys and add a regression test that remaps spawn controls.
4. **[M10] Scene round-trip regression suite**
   - *Why:* Complex scenes (meshes + materials + environments) need coverage beyond the existing happy-path test.
   - *Acceptance:* Expand `tests/scene_roundtrip.rs` (or add a new suite) to include nested hierarchies, dependency retain/release, and environment metadata. Tests should assert reference counts and serialized JSON diffs.
5. **[M11] Editor workflow regression tests**
   - *Why:* Gizmo/selection bugs are common whenever egui changes land.
   - *Acceptance:* Build a headless egui test that simulates selecting an entity, switching gizmo modes, saving, and reloading. The test should confirm entity IDs remain stable and gizmo interaction state resets cleanly.

## P1 — Tooling & Workflow Enhancements

6. **[M2] Animated sprite timelines**
   - *Acceptance:* Add timeline data to atlas metadata, stream per-instance animation state through the ECS, and expose playback controls in the inspector.
7. **[M3.5] Quadtree fallback with perf telemetry**
   - *Acceptance:* Implement a density-aware quadtree fallback for the spatial hash, expose cell occupancy metrics to the analytics plugin, and add a toggle in the debug UI.
8. **[M4] CLI overrides for config values**
   - *Acceptance:* Support `kestrel_engine --width 1280 --height 720 --vsync off` (or similar) with precedence rules logged at startup; include unit tests for argument parsing.
9. **[M5] Profiler & metrics panel with snapshot tests**
   - *Acceptance:* Add a collapsible egui profiler panel (frame timings, ECS system timings) and snapshot tests that catch layout regressions.
10. **[M6] Multi-camera / follow-target tooling**
    - *Acceptance:* Allow the viewport to switch between multiple named cameras or follow a selected entity; persist the choice in scene metadata.
11. **[M7] Scripting debugger / REPL**
    - *Acceptance:* Embed a lightweight Rhai REPL with pause/step controls and command history; script errors should focus in the debugger panel.
12. **[M8] Particle budget analytics**
    - *Acceptance:* Stream current particle counts, emitter backlog, and cap utilization into the analytics UI to spot runaway effects.
13. **[M9] Audio capability diagnostics**
    - *Acceptance:* Extend the new `AudioHealthSnapshot` telemetry with device name/sample rate info and route it into analytics/logs when initialization fails.
14. **[M10] Binary `.kscene` serializer**
    - *Acceptance:* Implement a binary encoder/decoder (with compression) behind a feature flag; provide migration tooling between JSON and binary formats.
15. **[M11] Drag-and-drop prefab authoring**
    - *Acceptance:* Allow entities or hierarchies to be dragged from the inspector into a prefab shelf, saved as JSON/`kscene`, and instanced via drag/drop.

## P2 — Strategic Feature Bets

16. **[M12] Advanced shadow mapping & light culling**
    - *Acceptance:* Introduce cascaded or clustered shadows plus light culling for 3D meshes, with performance counters exposed in analytics.
17. **[M12] GPU performance baselines**
    - *Acceptance:* Capture GPU timing queries per pass and surface them in the profiler UI with CSV export for CI trend tracking.
18. **[M13] Plugin sandbox for untrusted modules**
    - *Acceptance:* Design an opt-in sandbox (capabilities/IPC or WASM) so dynamic plugins run without direct memory access; include a compatibility matrix and contract tests.
19. **[M9] Positional audio & falloff curves**
    - *Acceptance:* Integrate simple spatialization (pan + falloff) tied to entity positions; expose tuning sliders in the audio panel and verify via automated tests with deterministic triggers.
20. **[Long-term] Force fields, attractors, and particle trails**
    - *Acceptance:* Expand the particle system with force-field components, attractor/repulsor entities, and stretched billboard trails, all editable via the inspector.

Feel free to slice these issues finer when you import them into your tracker; the acceptance criteria can serve as the initial definition of done.
