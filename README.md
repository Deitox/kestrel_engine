# Kestrel Engine - Milestone 13

**Plugin system, scene/editor maturity, and a 2D/3D toolchain ready for extension**

## Highlights
- Hybrid transform graph - 2D sprites and 3D meshes share the same parent/child hierarchy so scene parenting stays consistent across spaces. A unified transform propagator keeps world matrices in sync for both billboards and meshes.
- Scene dependency tracker - Scene exports now record atlas and mesh requirements, and runtime reference counting retains and releases those assets automatically when scenes load or unload.
- Mesh metadata - Mesh entities carry material identifiers plus lighting flags (cast/receive shadows, emissive tint). The scene format and round-trip loader persist that data, paving the way for the Milestone 12 PBR work.
- HDR environment lighting - Load equirectangular HDR maps to drive diffuse irradiance, specular reflections, and a BRDF LUT so materials react to image-based lighting alongside the directional key light.
- Camera tooling - The mesh preview offers three modes (Disabled, Orbit, Free-fly). Free-fly introduces WASD/QE + Shift navigation with mouse look and roll, while orbit mode remains handy for turntable inspection.
- Perspective viewport editing - Ray-based picking, translate/rotate/scale gizmos, and a frame-selection helper keep mesh workflows aligned with the inspector.
- Plugin system - The new `EnginePlugin` trait, feature registry, and manifest-driven loader let subsystems (audio, scripting, analytics, future tooling) hook into init/update/fixed/event stages without modifying the core loop, paving the way for third-party extensions.
- Scene toolbar upgrades - Quick path history, dependency health readouts, and one-click retain buttons make Save/Load workflows safer.
- Scene I/O guardrails - Mesh-aware helpers (save_scene_to_path_with_mesh_source, load_scene_with_mesh) ensure custom assets keep their source paths and metadata during save/load workflows.
- Particle telemetry - The Stats panel now surfaces particle budget metrics (active count, spawn budget, emitter backlog) so runaway emitters are obvious without diving into the ECS.
- Animation workflow polish - Sprite timelines now support explicit loop modes (loop, ping-pong, once-hold, once-stop) plus per-frame events that surface through the `GameEvent` bus. A command-line Aseprite importer (`cargo run --bin aseprite_to_atlas`) converts authoring exports into engine-ready atlases, complete with optional loop overrides and timeline event metadata, and hot-reload keeps running scenes in sync with file edits.
- Animation monitoring - Transform clip/skeletal watchers reload assets instantly, validators log through the inspector + analytics queue, and the viewport HUD mirrors sprite/transform/skeletal budgets (including GPU palette uploads) so perf regressions are obvious without digging through logs.

## Core Systems
- Physics - Rapier2D simulates rigid bodies. ECS components (Transform, Velocity, RapierBody, RapierCollider) mirror state back into the world every fixed step.
- Rendering - A WGPU renderer performs depth-tested mesh draws, batched sprite passes, and egui compositing inside a single swapchain frame.
- Scripting - Rhai scripts hot-reload, queue gameplay commands, and surface log output through the debug UI.
- Assets - The asset manager loads texture atlases on demand, while the mesh registry keeps CPU/GPU copies of glTF data and now reference-counts scene dependencies so unused assets are released automatically.
- Audio - Lightweight rodio-backed cues highlight spawn/despawn/collision events.
- Scene management - JSON scenes capture the full entity graph (including materials/lighting) and can be saved/loaded from the UI or tests.

## Controls
- Space - spawn the configured burst count (remappable via `config/input.json`)
- B - spawn 5x as many sprites (minimum 1000)
- Right Mouse - pan the 2D camera (Disabled) / orbit preview (Orbit) / look around (Free-fly)
- Mouse Wheel - zoom the 2D camera (Disabled) / adjust orbit radius (Orbit) / tune fly speed or focus distance (Free-fly)
- M - cycle mesh preview camera mode (Disabled -> Orbit -> Free-fly)
- W, A, S, D, Q, E - move the preview camera in Free-fly
- Z, C - roll the preview camera in Free-fly
- L - toggle frustum lock for the preview camera
- Shift - boost movement speed in Free-fly
- Esc - quit

## Script Debugger & REPL
- Open the **Stats -> Scripts** section inside the left panel to toggle scripting, pause updates, step once while paused, or hot-reload the active Rhai file. Click **Open debugger** from that section (or press the same button inside the Scripts window) to pop out the dedicated console.
- The debugger window shows a scrollback console that mixes script logs, REPL input/output, and runtime errors. Use **Clear Console** to reset the log without touching the underlying script state.
- Type Rhai commands into the REPL field and press **Enter** or **Run**; commands execute against the live `World` just like the main script, so you can tweak emitters, spawn sprites, or inspect state at runtime.
- Arrow keys cycle through command history, and the History list lets you click to rehydrate older commands for editing. The input box auto-focuses whenever a script error occurs so you can fix issues quickly.
- Errors that occur during REPL execution or regular script updates automatically reopen the debugger and highlight the failure, keeping the workflow tight during iteration.

## Animation Tooling & Validation
- The viewport HUD (toggle from **Stats -> Viewport Overlays**) surfaces sprite/transform/skeletal/GPU palette budgets so perf regressions are visible at a glance.
- Asset watchers cover `assets/images/*.json`, `assets/animations/{clips,graphs}/**/*.json`, and `assets/animations/skeletal/**/*.gltf`. Saving any of these reloads the asset, reruns schema + semantic validators, and posts `AnimationValidationEvent` entries to the inspector banner and Stats sidebar.
- Skeleton reloads preserve playback state (active clip, time, playing flag, speed, group tags) so iteration never forces manual reseeding. Graph JSON files reimport immediately, keeping authored graphs validated even before the runtime consumes them.
- Run the same validators headlessly (and in CI) via `cargo run --bin animation_check -- assets/animations`; the CLI walks directories, filters supported extensions (`.json`, `.clip`, `.gltf`, `.glb`), prints Info/Warn/Error lines, and exits non-zero when blocking issues are detected.
- Keep sprite atlases on the current schema with `cargo run --bin migrate_atlas -- assets/images`. Append `--check` when you need a read-only verification (e.g., CI): the helper walks directories of JSON files, injects canonical `loop_mode` data, trims orphaned timeline events, clamps invalid durations, and bumps the file version so CI bots and local editors agree on the data they ingest.
- Load `assets/scenes/animation_showcase.json` (documented in `docs/animation_sample_content.md`) for a ready-to-edit scene that exercises the sprite timeline, transform clip, and palette upload counters used throughout the milestone tutorials.

## Scene Formats
- JSON scenes (`.json`) remain human-readable and are always supported.
- When the crate is built with the `binary_scene` feature, saving to a path that ends in `.kscene` writes a compressed binary payload (magic `KSCN`, versioned, LZ4-compressed bincode) that loads faster and takes less disk space.
- Use the scene tool to convert between formats without reauthoring content:  
  `cargo run --bin scene_tool --features binary_scene -- convert input.json output.kscene`
- Binary scenes cannot be opened without the feature flag; the loader emits a clear error if you try to open a `.kscene` from a build that lacks `binary_scene`.

## Build
`
cargo run
`

## Benchmarks
- `pwsh scripts/ci/run_animation_targets.ps1 [-OutputDirectory artifacts]` runs `cargo test --profile release-fat animation_targets_measure -- --ignored --exact --nocapture` (matching the CI configuration) and captures the results in `target/animation_targets_report.json` (copied to `artifacts` when provided). Each report now includes `{mean, median, p95, p99}` timing stats, `{warmup_frames, measured_frames, samples_per_case, dt, profile, lto_mode, rustc_version, target_cpu, feature_flags, commit_sha}` metadata, and a `sprite_perf` payload so CI can diff both budgets and slow-path mix.
- The report also embeds an `animation_budget` snapshot mirroring the in-editor HUD (sprite/transform/skeletal/palette metrics plus active counts) so CI trend tracking can compare analytics samples directly against the roadmap budgets.
- `python scripts/sprite_bench.py --label <my_label> --runs 3` wraps the release harness with the pinned env vars (no feature flags), aggregates three runs, and drops lightweight summaries in `perf/<label>.{txt,json}` (plus the metadata above). Pick a descriptive label (e.g. `before_phase0`, `after_phase1`) so it's obvious which results are being compared.
- Phase 2 sprite experiments (SoA/fixed-point/SIMD) are feature gated; enable them with `--features "sprite_anim_fixed_point,sprite_anim_simd"` (the helper script accepts `--features` and forwards the value to `cargo test`), but always compare back to the default run above.
- `python scripts/capture_sprite_perf.py --label after_phase1 --runs 3` wraps the sprite bench sweep plus `animation_profile_snapshot` (anim_stats-enabled). It emits `perf/<label>.txt/.json` for the averaged bench data and `perf/<label>_profile.{log,json}` for the per-step driver/apply stats so regressions can be compared apples-to-apples.
- The harness measures the roadmap checkpoints (10 000 sprite animators, 2 000 transform clips, 1 000 bones) and prints PASS/WARN summaries against the stated CPU budgets. Use the editor's **Stats -> Sprite Animation Perf** block to spot-check fast/slow bucket mix, delta-t ratios, modulo fallbacks, and Eval/Pack/Upload bars while iterating in real time.


## Plugins
- `pwsh scripts/build_plugins.ps1 [-Release]` builds every enabled entry from `config/plugins.json` by inferring the crate root from each artifact path.
- After rebuilding a plugin, open the Plugins panel in-app and click "Reload plugins" to rescan the manifest without restarting.

## Configuration
- Edit config/app.json to tweak window title, resolution, vsync, or fullscreen defaults.
- Override width/height/vsync from the CLI with `kestrel_engine --width 1920 --height 1080 --vsync off` (CLI overrides take precedence over config/app.json, which takes precedence over built-in defaults).
- Remap keyboard input by editing config/input.json (missing or invalid entries fall back to the built-in bindings with warnings).
- Toggle dynamic plugins via config/plugins.json (paths are resolved relative to that file; set `enabled` per entry).
- Disable built-in plugins by listing their names in `config/plugins.json` -> `disable_builtins`.
- The engine falls back to built-in defaults and logs a warning if the file is missing or malformed.
- If a dynamic plugin's path is missing or invalid, the loader logs it and automatically marks it disabled (the app will proceed without crashing).

## Documentation
- docs/ARCHITECTURE.md - subsystem responsibilities, frame flow, and notes on the hybrid transform pipeline.
- docs/DECISIONS.md - crate and technology choices (e.g., winit, wgpu, gltf, Rapier).
- docs/CODE_STYLE.md - formatting, linting, and error-handling guidelines.
- docs/PLUGINS.md - dynamic plugin manifest format, feature registry rules, and an example cdylib plugin.
- docs/animation_workflows.md - sprite timeline authoring, Aseprite importer usage, loop-mode tuning, and hot-reload troubleshooting tips.

## Experimental Keyframe Panel
The Milestone 5 keyframe editor is available behind the `animation_keyframe_panel` feature flag. To preview the current read-only state:

```powershell
cargo run --features animation_keyframe_panel
```

- Open the editor and select an entity driven by a sprite timeline or transform clip.
- In the Stats panel, click the "Open Keyframe Editor" toggle.
- The panel lists active tracks plus per-key metadata (index, time, value preview). Editing controls are coming in later milestones.

For details, see `docs/keyframe_editor_spec.md` and `docs/animation_milestone_5_plan.md`.
