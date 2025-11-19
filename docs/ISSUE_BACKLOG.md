# Stretch Goal Issue Backlog







This backlog converts every open stretch goal from `docs/MILESTONE_STATUS.md` into actionable work. Priorities follow a simple scheme:







- **P0** â€“ Stability/determinism tasks that guard core engine loops.



- **P1** â€“ Tooling and workflow improvements that unblock daily editing.



- **P2** â€“ Strategic feature bets for long-term differentiation.







Each issue lists its originating milestone plus crisp acceptance criteria so it can be copied directly into your issue tracker.







## P0 â€” Stability & Determinism

8. **[PerfGoal] Sprite timeline â‰¤0.300Â ms guardrail** â€” *Open.*

   - *Why:* The latest dev-profile bench run sits around 0.43Â ms for 10k animators. We already proved the release build can hit 0.288Â ms, but we need a concrete plan (bench procedure + kernel wins + CI guard) to stay under budget any time new sprite features land.

   - *Plan & acceptance:*
     1. **Release-profile baseline** â€” Add a `--profile bench-release` preset to `scripts/sprite_bench.py`, capture a fresh run (`perf/sprite_perf_guard_release.{txt,json}`) that demonstrates â‰¤0.300Â ms, and document the exact command in `docs/SPRITE_ANIMATION_PERF_PLAN.md`.
     2. **Guarded automation** â€” Wire the new `sprite_perf_guard` binary into CI so every PR runs `python scripts/sprite_bench.py --profile bench-release --runs 1` followed by `cargo run --bin sprite_perf_guard -- --report target/animation_targets_report.json`. The pipeline must fail if mean/max >0.300Â ms or `%slow > 1%`.
     3. **Const-dt SIMD bucket** â€” Implement the 8-lane SIMD kernel described in `docs/Sprite_Benchmark_Plan.md Â§2.2`, keep scalar fallbacks for tails, and add parity tests feeding randomized const-dt clips (including ping-pong flips). Acceptance: `cargo test --features sprite_anim_simd sprite_animation::simd_parity` passes and the release bench drops â‰¥10Â % in `sprite_timelines`.
     4. **Var-dt next-dt cache** â€” Introduce a `next_dt[]` cache/prefetch for var-dt animators and extend `tests/sprite_animation.rs` with a mixed-bucket regression that randomizes const/var clips. Acceptance: telemetry shows reduced `var_dt` cost, and the mixed-bucket test runs under `cargo test`.
     5. **Importer drift lint** â€” Finish the lint in `docs/SPRITE_ANIMATION_PERF_PLAN.md Â§4.1`: flag noisy â€œuniformâ€� timelines on import, persist severity, and add fixtures that assert lint output. Update `docs/animation_workflows.md` with a perf-capture checklist referencing the guard and lint expectations.
     6. **Final publication** â€” Re-run the release capture with SIMD + cache enabled, attach artifacts (or publish via CI), and update `README.md`/`docs/SPRITE_ANIMATION_PERF_PLAN.md` with the new figures plus the CI job link. Acceptance: `sprite_timelines` mean/max â‰¤0.300Â ms in the committed release run and the CI guard is green.







1. **[M1] Swapchain regression harness** â€” âœ… *Completed via headless render path + synthetic surface-error tests.*



   - *Highlights:* `Renderer::surface_tests::headless_render_recovers_from_surface_loss` now drives a fully headless render target, injects `SurfaceError::Lost`, and verifies that `render_frame` cleanly errors, triggers a resize, and then succeeds after recovery. Supporting hooks (`prepare_headless_render_target`, `inject_surface_error_for_test`) keep the harness deterministic.



2. **[M1] Configurable VSync toggle** â€” âœ… *Completed (runtime toggle + renderer reconfigure UI panel).*



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



6. **[M1 Perf] Sprite animation telemetry & HUD instrumentation** - *Completed via the SpriteAnimPerfTelemetry resource, Stats/HUD wiring, and expanded docs.*



   - *Highlights:* SpriteAnimPerfTelemetry now tracks zero-allocation counters for delta mix, ping-pong/event-heavy buckets, SIMD lane usage, modulo fallbacks, and emitted/coalesced events (`src/ecs/systems/animation.rs:633-750`). The Stats -> Sprite Animation Perf block and Sprite Stage Timings HUD expose those counters with warning colors plus Eval/Pack/Upload bars and palette upload timing splits fed by analytics (`src/app/editor_ui.rs:928-989`, `src/app/mod.rs:3935-4076`).



   - *Docs:* README + `docs/animation_workflows.md` walk through the Sprite Animation Perf panel/HUD and perf capture workflow, fulfilling the author guidance requirement (`README.md:69-75`, `docs/animation_workflows.md:19-65`, `docs/SPRITE_ANIMATION_PERF_PLAN.md:102`).



7. **[M1 Perf] Bench telemetry export & GPU timing split** - *Completed via the upgraded `animation_targets_measure` harness and perf artifact tooling.*



   - *Highlights:* The bench now emits sprite perf counters, percentile timing stats, and palette upload splits inside `target/animation_targets_report.json` (`tests/animation_targets.rs:90-210`, `tests/animation_targets.rs:514`), while analytics snapshots mirror the CPU/GPU timing separation used by the Stats panel (`src/analytics.rs:17-34`, `src/app/mod.rs:4026-4080`). CI helpers copy the JSON into perf artifacts and local scripts persist capture runs beneath `perf/` for comparison (`scripts/ci/run_animation_targets.ps1:24-44`, `scripts/capture_sprite_perf.py:10-90`, `scripts/sprite_bench.py:23-115`).



## P1 â€” Tooling & Workflow Enhancements







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



15. **[M11] Drag-and-drop prefab authoring** â€“ *Completed via the Prefab Shelf UI + viewport instancing pipeline.*



    - *Highlights:* The inspector exposes per-entity `PrefabDragPayload` handles (`src/app/editor_ui/entity_inspector.rs:1387`) that drop into the Prefab Shelf to name/save JSON or `.kscene` prefabs (`src/app/editor_ui.rs:2343`, `src/prefab.rs:1`); saved entries become drag sources that instantiate at 2D/3D drop targets inside the viewport (`src/app/editor_ui.rs:2556`, `src/app/mod.rs:950`, `src/app/mod.rs:1003`). Regression tests cover export/instantiate round-trips, drop offset alignment, and transform-clip retention (`tests/prefab_workflow.rs:10`, `tests/prefab_workflow.rs:42`, `tests/prefab_workflow.rs:82`).



    - *Acceptance:* Allow entities or hierarchies to be dragged from the inspector into a prefab shelf, saved as JSON/`kscene`, and instanced via drag/drop.



    - *Note:* Binary `.kscene` export requires launching with the `binary_scene` feature enabled; when absent the Prefab Shelf surfaces guidance to switch formats.







## P2 â€” Strategic Feature Bets







16. **[M12] Advanced shadow mapping & light culling** - *Partially complete (cascaded shadows landed; light culling + telemetry still open).*







    - *Status:* Directional cascaded shadows with adjustable splits/resolution are implemented (`src/renderer.rs:464-2052`), but Milestone 12 still lists GPU perf baselines and clustered/forward+ light culling as outstanding polish (`docs/MILESTONE_STATUS.md:22`, `docs/MILESTONE_STATUS.md:95`). We still need the light-culling path plus analytics counters to meet the acceptance criteria (`KESTREL_ENGINE_ROADMAP.md:202`).







17. **[M12] GPU performance baselines** - *Completed via timestamp query instrumentation + profiler export.*



    - *Highlights:* The renderer now wraps render passes with timestamp queries and resolves per-pass GPU durations (`src/renderer.rs:108`, `src/renderer.rs:1869`), the app records recent GPU timing history and provides a CSV exporter (`src/app/mod.rs:308`, `src/app/mod.rs:784`), and the stats panel surfaces latest/average GPU timings with an export control (`src/app/editor_ui.rs:1536`).



18. **[M13] Plugin sandbox for untrusted modules** - *Completed via the isolated plugin host, capability gating, and telemetry tests.*







    - *Highlights:* `kestrel_plugin_host` runs `trust = "isolated"` plugins out of process, `PluginManager` wires capability-scoped RPC plumbing plus watchdog timers (`src/bin/kestrel_plugin_host.rs:1`, `src/plugins.rs:1642-1875`), and `PluginContext` enforces capability declarations. Integration tests such as `cargo test --test plugins isolated_plugin_telemetry_pipeline` exercise the sandbox contract (`tests/plugins.rs:732-840`), while `docs/polish_targets.md:45-66` documents the compatibility matrix, analytics surfacing, and CI gates that keep the sandbox honest.







19. **[M9] Positional audio & falloff curves** - *Open.*







    - *Status:* Audio playback is still global/stereo via `EventBus` hooks (`src/audio.rs:17-143`), and Milestone 9 explicitly calls out positional audio/falloff work as pending polish (`docs/MILESTONE_STATUS.md:19`, `docs/MILESTONE_STATUS.md:80`, `KESTREL_ENGINE_ROADMAP.md:158`). There are no spatialization parameters or falloff controls in the audio panel yet.







20. **[Long-term] Force fields, attractors, and particle trails** - *Open.*



    - *Status:* Particle emitters only support standard burst/loop behaviors today (`src/ecs/systems/particles.rs:9-290`). Milestone 8 still lists force fields, attractors, and stretched trails as future experiments (`docs/MILESTONE_STATUS.md:18`, `docs/MILESTONE_STATUS.md:75`, `KESTREL_ENGINE_ROADMAP.md:145`), so none of the required components or inspector tooling exist yet.











Feel free to slice these issues finer when you import them into your tracker; the acceptance criteria can serve as the initial definition of done.




