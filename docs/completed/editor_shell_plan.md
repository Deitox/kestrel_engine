# Editor Shell Extraction Plan

1. **Catalog Dependencies**
   - Enumerate the existing `App` fields that only serve the editor UI (prefab inputs, script console, animation keyframe panel, telemetry caches, egui handles).
   - Note the types they depend on (`ClipEditRecord`, `AnimationValidationEvent`, `ScriptConsoleEntry`, etc.) so the new module exposes them via re-exports or wrapper APIs.

2. **Introduce `EditorUiState`** _(Complete - 2025-11-17)_
   - Define a struct in `app::editor_shell` that holds the `// UI State` fields currently on `App`, along with helper methods for default construction.
   - Keep the existing structs (`ClipEditRecord`, etc.) near their current definitions for now and reference them from the new state struct.

## Step 2 Progress _(Complete - 2025-11-17)_
- `EditorUiState` now owns the selection stack (`selected_entity`, inspector details, `gizmo_mode`, and active `gizmo_interaction`) and exposes borrow helpers so gizmo tooling and plugin plumbing can read/update state without poking at `App` internals.
- The frame profiler and GPU timing history moved into the shell as well: `App` records samples through `record_frame_timing_sample`/`update_gpu_timing_snapshots`, while egui panels consume immutable snapshots via the shell.
- `EditorUiOutput` grew a `gizmo_interaction` field so the viewport logic can queue interaction resets without mutating `App` mid-frame, which keeps `egui_ctx.run` borrow-safe now that the data lives inside the shell.
- Camera bookmark data (list, active selection, text input) now lives in `EditorUiState`, so bookmark apply/save/delete flows mutate the shell-owned list and `SceneMetadata` serialization pulls from there rather than `App`.
- Inspector entity info/bounds are snapped into `EditorUiState` each frame and inspector controls now emit queued actions that `App` applies after `egui`, so the viewport/inspector flows no longer borrow the ECS during UI building.
- Prefab shelf helpers and the skin-mesh inspector now consume `EditorUiParams` snapshots (clip/atlas/skeleton catalogs, material+mesh subset lists, and skeleton-entity bindings), so the entire inspector panel renders from shell-owned copies while all prefab/skin toggles emit deferred `InspectorAction`s. This eliminates the last `App::ecs`/`AssetManager` borrows during `egui_ctx.run` and keeps the inspector borrow-safe.
- Script debugger availability/path/enabled/paused/error snapshots now live in `EditorUiState`, and `EditorUiParams` consumes that immutable snapshot so egui never touches `self.script_plugin()` directly. The snapshot also carries the active script handle list, so both the sidebar panel and the debugger window render a stable mapping of handles to scene entities without borrowing the plugin mid-egui.

3. **Refactor App Construction** _(Complete - 2025-11-18)_
   - Update `App::new` to initialize `EditorUiState` through the shell and remove the duplicated fields from `App`.
   - Provide accessors on `EditorShell` (e.g., `ui_state_mut()`) so `App` can read/write state without exposing internals.

4. **Port Call Sites Incrementally** _(Complete - 2025-11-19)_
   - Replace direct `self.ui_*` and related references with `self.editor_shell.ui.*` in small sections (e.g., Stats panel data, prefab workflows, script console) and run `cargo check` after each batch.

5. **Document & Test** _(Complete - 2025-11-20)_
   - Once the migration is stable, update `docs/completed/remediation_plan.md` to note progress on the EditorShell milestone.
   - Run `cargo check`/targeted tests to ensure the UI behaviour remains unchanged.

Following these steps keeps the extraction incremental and reduces risk compared to a single giant refactor.

## Dependency Catalog _(Step 1 – 2025-11-16)_

### Editor host & instrumentation
- `editor_shell: EditorShell` wraps `egui::Context`, `egui_winit::State`, and `egui_wgpu::{Renderer, ScreenDescriptor}` so any new module that owns UI state must re-export those handles.
- `frame_profiler: FrameProfiler` and `FrameTimingSample` (defined in `src/app/mod.rs`) collect the timings rendered in the Stats + Frame Budget panels.
- `frame_budget_idle_snapshot`, `frame_budget_panel_snapshot`, `frame_budget_status` store `FrameBudgetSnapshot` structs (also in `src/app/mod.rs`) plus the active status string. These snapshots include allocation deltas when the `alloc_profiler` feature is enabled and are consumed by `editor_ui::frame_budget`.
- `frame_plot_points: Arc<[eplot::PlotPoint]>` and `frame_plot_revision: u64` cache the plotted frame history (requires access to the analytics plugin for data).
- `gpu_timings: Arc<[GpuPassTiming]>`, `gpu_timing_history: VecDeque<GpuTimingFrame>`, `gpu_timing_history_capacity: usize`, `gpu_frame_counter: u64`, `gpu_metrics_status: Option<String>` feed the GPU metrics panel (`GpuPassTiming` lives in `src/renderer.rs`, `GpuTimingFrame` beside `App`).
- `telemetry_cache: TelemetryCache` memoizes mesh/environment lists and prefab shelf entries via `VersionedTelemetry<T>` plus `editor_ui::PrefabShelfEntry`.
- `id_lookup_input: String` and `id_lookup_active: bool` drive the inspector search dialog (`editor_ui::entity_inspector` expects mutable references to both).

### Particle, lighting, and guard-rail sliders
- Particle spawn controls: `ui_spawn_per_press: i32`, `ui_auto_spawn_rate: f32`, `ui_cell_size: f32`, `ui_spatial_use_quadtree: bool`, `ui_spatial_density_threshold: f32`.
- Emitter tweakers: `ui_root_spin`, `ui_emitter_rate`, `ui_emitter_spread`, `ui_emitter_speed`, `ui_emitter_lifetime`, `ui_emitter_start_size`, `ui_emitter_end_size`, `ui_emitter_start_color`, `ui_emitter_end_color`, `ui_particle_max_spawn_per_frame`, `ui_particle_max_total`, `ui_particle_max_emitter_backlog`.
- Lighting/environment sliders: `ui_light_direction: Vec3`, `ui_light_color: Vec3`, `ui_light_ambient: Vec3`, `ui_light_exposure: f32`, `ui_environment_intensity: f32`.
- Shadow tuning: `ui_shadow_distance`, `ui_shadow_bias`, `ui_shadow_strength`, `ui_shadow_cascade_count`, `ui_shadow_resolution`, `ui_shadow_split_lambda`, `ui_shadow_pcf_radius`.
- Viewport tuneables: `ui_camera_zoom_min`, `ui_camera_zoom_max`, `ui_sprite_guard_pixels`, `ui_sprite_guard_mode: SpriteGuardrailMode` (from `src/config.rs`), `ui_scale: f32`.

### Scene/prefab workflows & history
- Scene picker and status: `ui_scene_path: String`, `ui_scene_status: Option<String>`, plus `camera_bookmark_input: String`.
- Prefab authoring: `prefab_name_input: String`, `prefab_format: PrefabFormat`, `prefab_status: Option<PrefabStatusMessage>` (`PrefabFormat`/`PrefabStatusMessage` defined in `src/prefab.rs` to describe serialization + validation state).
- Animation group importers: `animation_group_input: String`, `animation_group_scale_input: f32`.
- Dependency snapshots: `scene_dependencies: Option<SceneDependencies>` and `scene_dependency_fingerprints: Option<SceneDependencyFingerprints>` (`src/scene.rs`) are updated when scenes load so the editor can show dependency reports; App core also reads them to retain/release assets.
- Scene navigation history: `scene_history: VecDeque<String>` plus `scene_history_snapshot: Option<Arc<[String]>>` keep the recent file list shown in the UI.
- Inspector status messaging: `inspector_status: Option<String>`.

### Inspector toggles & visibility helpers
- `debug_show_spatial_hash: bool` and `debug_show_colliders: bool` toggle debug overlays in the viewport and inspector.
- `script_debugger_open: bool` and `script_focus_repl: bool` gate whether the script debugger panel and REPL input are visible/focused.

### Script console & REPL buffers
- REPL text + history: `script_repl_input: String`, `script_repl_history: VecDeque<String>`, `script_repl_history_index: Option<usize>`, `script_repl_history_snapshot: Option<Arc<[String]>>`.
- Console log buffers: `script_console: VecDeque<ScriptConsoleEntry>` and `script_console_snapshot: Option<Arc<[ScriptConsoleEntry]>>` where `ScriptConsoleEntry`/`ScriptConsoleKind` are defined near `App`.
- Error tracking: `last_reported_script_error: Option<String>` keeps the banner in sync with runtime errors.

### Animation editing & validation
- `animation_keyframe_panel: AnimationKeyframePanel` (from `src/app/animation_keyframe_panel.rs`) encapsulates the keyframe editor UI state and helper caches.
- Undo/redo + dirty tracking: `clip_dirty: HashSet<String>`, `clip_edit_history: Vec<ClipEditRecord>`, `clip_edit_redo: Vec<ClipEditRecord>` (`ClipEditRecord` struct also near `App`).
- Status + overrides: `animation_clip_status: Option<String>` plus `clip_edit_overrides: HashMap<String, Arc<AnimationClip>>` (clip data from `src/assets.rs`).
- Validator surface: `pending_animation_validation_events: Vec<AnimationValidationEvent>` (`src/animation_validation.rs`) and `suppressed_validation_paths: HashSet<PathBuf>` hold pending results + suppression state for the inspector/tooltips.

### Telemetry & stats caches
- Prefab/scene telemetry: `scene_history_snapshot`, `script_repl_history_snapshot`, and `script_console_snapshot` already mentioned above; the stats panel further relies on cached `scene_history_arc()`, `scene_atlas_refs_arc()`, and `scene_mesh_refs_arc()` builders using `Arc<[T]>` to avoid repeated allocations.
- Additional caches from analytics plugins: `frame_plot_points`, `gpu_timings`, and `frame_budget_*` (see instrumentation section) rely on `Arc` snapshots so `EditorShell` should expose immutable views (e.g., `Arc<[eplot::PlotPoint]>`, `Arc<[GpuPassTiming]>`) to callers without cloning the full vectors on every frame.

This catalog covers every `App` field currently used exclusively by the editor UI, along with the custom types/modules they depend on. The next step is to hang these off a dedicated `EditorUiState` owned by `EditorShell` so that `App` only interacts with them through accessor methods.

## Editor State Skeleton _(Step 2 – 2025-11-16)_

- Added `EditorUiState` to `src/app/editor_shell.rs` alongside helper types (`EditorUiStateParams`, `EmitterUiDefaults`). The struct mirrors the entire `// UI State` block from `App` so the migration is a mechanical field move.
- `EditorShell` now owns an optional `ui_state` plus accessor helpers (`install_ui_state`, `ui_state`, `ui_state_mut`). They’re currently gated with `#[allow(dead_code)]` until `App` hands off ownership.
- `EditorUiState::new` centralizes the default construction logic using `ParticleConfig`, `SceneLightingState`, and the editor config. This lets `App::new` build the UI state with a single call once we wire it in.

Step 3 is now complete: App::new hands the emitter/lighting/editor config into EditorUiState::new, which seeds the scene path/history, inspector strings, and script console defaults internally so the constructor never touches UI-only fields again.

## App Wiring Progress _(Step 3 — 2025-11-16)_

- `EditorShell::new` now requires an `EditorUiState` at construction, so `App::new` installs the UI data exactly once and the borrow helpers never juggle an `Option` or late `install_ui_state()` call again.
- `App::editor_ui_state()` / `editor_ui_state_mut()` now simply delegate to the shell accessors, keeping all `RefCell` bookkeeping inside `EditorShell`.
- `App::new` now constructs `EditorUiState` using `EditorUiStateParams`/`EmitterUiDefaults` and installs it on `EditorShell` right after the struct is created. This keeps the egui handles + UI state packaged together.
- First migration batch: all script debugger / REPL data (`script_debugger_open`, focus flag, REPL input/history buffers, console logs, last reported error) now live exclusively inside `EditorUiState`. The associated helpers (`push_script_console`, `append_script_history`, `script_repl_history_arc`, `sync_script_error_state`, etc.) were updated to read/write the shell state, and the egui `EditorUiParams`/`EditorUiOutput` plumbing was rewired accordingly.
- Second migration batch: scene/prefab/inspector fields (`ui_scene_path`, `ui_scene_status`, `prefab_*` inputs, animation group overrides, inspector status, scene history) moved into `EditorUiState`. `EditorUiParams`/`EditorUiOutput` were extended so the UI mutates local copies and App rehydrates state after each frame, avoiding borrow conflicts with `egui_ctx`. Actions such as clearing the scene history are now communicated via the output payload instead of mutating App fields directly during UI rendering.
- Third migration batch: particle + emitter sliders (`ui_spawn_per_press`, `ui_auto_spawn_rate`, spatial cell/quadtree toggles, emitter knobs, particle caps, `ui_root_spin`, and the UI copy of the environment intensity) now live solely on `EditorUiState`. Auto-spawn loops, particle hotkeys, script commands, and ECS synchronization paths all route through the shell, and `App` dropped the corresponding struct fields entirely. `set_active_environment` now mirrors intensity edits into the shell so the UI reflects external environment switches.
- Fourth migration batch: lighting/shadow controls and the camera guardrail sliders moved behind the shell. `EditorUiParams`/`EditorUiOutput` now carry the light vectors, shadow tuning, zoom limits, and sprite guardrail mode/pixels, while `apply_editor_camera_settings` and the scene-metadata import path clamp/write through `EditorUiState`. The egui layer raises an `editor_settings_dirty` flag instead of poking `App` directly, so camera guardrail updates remain borrow-free and `App` applies the sanitized values (and persists config) once per frame.
- Fifth migration batch: animation tooling and debug overlays are fully shell-owned. The keyframe panel's open state, clip edit overrides/history, dirty flags, validation queues, inspector status, sprite guardrail warnings, GPU timing export status, and the UI-only scene atlas/mesh/clip snapshots now travel through `EditorUiState`, and egui just reports the desired panel open flag so `App` can toggle after the frame. Spatial hash/collider gizmos read/write through the shell too, so no UI-only booleans remain on `App`.
- Sixth migration batch: runtime animation tooling (keyframe panel rendering, command dispatch, and the `TrackEditOperation` helpers) moved into `app::animation_tooling` so `App` no longer hosts thousands of lines of keyframe logic. `App` now just calls the helper methods, keeping analytics hooks in place while giving the tooling its own module boundary.
- Seventh migration batch: prefab workflows now live in `app::prefab_tooling`. Saving/instantiating prefabs, prefab status messaging, and the supporting helpers were pulled out of `App`, so the runtime just stitches responses together while the focused module owns the editor-only prefab plumbing.
- Eighth migration batch: mesh preview tooling moved into `app::mesh_preview_tooling`. Status messaging, control-mode/frustum toggles, camera resets, preview mesh selection, and spawn helpers now sit behind a dedicated module so `App` only issues high-level requests.
- Ninth migration batch: analytics/plugin telemetry snapshots now live on `EditorUiState`. `App::refresh_editor_analytics_state()` captures GPU pass metrics, capability logs/events, asset readbacks, watchdog alerts, animation validation/budget data, light-cluster overlays, and keyframe usage logs so egui renders purely from cached `Arc` slices.
- Tenth migration batch: the script console helpers (`push_script_console`, REPL history snapshots, command execution, error mirroring) now live in `app::script_console`, and inspector utilities (status messaging, scene/asset snapshot builders, `focus_selection`) sit in `app::inspector_tooling`. `docs/ARCHITECTURE.md` was updated to describe the new module boundaries so future contributors can find these helpers quickly.
- Eleventh migration batch: file watcher plumbing moved into `app::asset_watch_tooling` (atlas hot-reload sync, animation watch roots, queue helpers) and the remaining telemetry caches + frame budget utilities live in `app::telemetry_tooling`. With `TelemetryCache`, `FrameProfiler`, and the frame-budget helpers centralized, `App` now just orchestrates state transitions while the tooling modules manage editor-only caching.
- Final constructor cleanup (2025-11-18): `EditorUiStateParams` now carries only renderer/editor config data, while `EditorUiState::new` seeds the default scene path/history (shared through `SCENE_HISTORY_CAPACITY`) plus the inspector/script defaults. `App::new` only passes the runtime dependencies, so new UI knobs automatically originate in the shell.

Step 3 is wrapped and Step 4 landed on 2025-11-19, so the remaining work now focuses on Step 5’s documentation pass and follow-up validation runs.

## Call-Site Porting Plan _(Step 4 - Complete)_

- **Analytics + stats panels**: Port `editor_ui::stats`/frame-budget widgets first so every telemetry panel reads immutable snapshots from `EditorUiState`. After each section compiles, run `cargo check` and flip the Stats, Frame Budget, and GPU Metrics panels in the editor build to verify no regressions.
- **Prefab/inspector workflows**: Tackle the prefab shelf, scene picker, dependency reports, and inspector tooling next. These modules already expose helpers, so the migration is swapping raw `self.ui_*` fields for `EditorUiState` data plus extending `EditorUiOutput` when the UI needs to push mutations (e.g., clearing scene history). Smoke test by loading/saving prefabs and bouncing through the dependency dialogs.
- **Viewport + gizmo plumbing**: Update viewport interaction handling so selection, gizmo state, guard-rail sliders, and bookmark inputs flow exclusively through the shell. Ensure the viewport only mutates state via `EditorUiOutput` so `egui` borrows remain short-lived, then validate by dragging gizmos, toggling guard-rails, and creating/editing bookmarks.
- **Script console + debugger**: Once the UI widgets are shell-driven, migrate script console commands, REPL history, debugger toggles, and error mirroring to the new helpers. After each chunk, run the targeted scripting suites (`cargo test --locked --test sprite_animation -- --test-threads=1`) to guarantee runtime/editor messaging still works.
- **Telemetry + plugin dashboards**: Finish by pointing analytics exports (plugin capability logs, mesh/atlas snapshots, animation validation queues) at the shell caches and deleting the legacy `App` storage. Re-run the headless renderer coverage (`present_mode_respects_vsync_flag`, `headless_render_collects_gpu_timings`, `headless_render_recovers_from_surface_loss`) so GPU timing and telemetry capture stay green.

## Step 4 Progress _(2025-11-19)_

- Stats/analytics panel now consumes a precomputed `light_cluster_metrics` snapshot via `EditorUiParams`, so the egui run no longer touches `self.renderer` for light-culling telemetry and the data flow matches the other shell-provided analytics snapshots.
- The Lighting & Environment inspector now edits a local copy of the renderer lighting state (`ScenePointLight` list + directional params) and reports changes through `EditorUiParams`/`UiActions`. `App` reapplies the sanitized values after each frame (`apply_editor_lighting_settings` + `point_light_update`), so egui no longer mutates `Renderer::lighting` mid-run and point-light edits stay borrow-safe.
- Scene dependency plumbing (prefab shelf + inspector helpers) now receives explicit `atlas_dependencies`/`mesh_dependencies`/`clip_dependencies` arrays and an optional `environment_dependency` record through `EditorUiParams`, so the dependency summary renders from the shell-owned `AtlasDependencyStatus`/`MeshDependencyStatus` snapshots without touching `self.assets`, `self.mesh_registry`, or the environment registry mid-egui. Retain actions still flow via `UiActions`, but every status label now comes from the cached structs, eliminating the last `App` borrows in that panel.
- Inspector translate/rotate/scale/velocity edits now emit structured `InspectorAction` requests via `UiActions`, and `App::render_editor_ui` applies them after `egui_ctx.run`. This moves the highest-traffic transform controls off of live ECS borrows during UI building and lays the groundwork for migrating the remaining inspector buttons to the same action queue.
- Transform clip editing (assign/clear clips, playback toggles, speed/group/time scrubbing, and both transform/property track masks) now flows through the same inspector action queue, so the corresponding egui widgets no longer call into `EcsWorld` or touch assets mid-frame. `App::render_editor_ui` executes the queued actions post-egui and surfaces success/failure via `set_inspector_status`, keeping the inspector borrow pattern consistent.
- Skeleton and sprite inspector panels now emit `InspectorAction` variants for every ECS mutation (skeleton attach/detach, clip playback controls, sprite atlas/region/timeline swaps, animation toggles, and frame scrubbing). `App::render_editor_ui` handles the queued actions (including sprite preview-event logging), so the egui pass only mutates local snapshots while the runtime applies real changes after the frame.
- Mesh inspector controls (material overrides, shadow flags, and lighting parameters) now follow the same deferred-action flow: the UI just queues the requested changes, and `App::render_editor_ui` performs the registry retain/release work plus ECS mutations afterward. This eliminates the final `material_registry`/`set_mesh_*` calls during `egui_ctx.run` and keeps inspector updates consistent with the rest of EditorShell.
- The remaining mesh controls (transform gizmo mirrors + tint override) now emit queued actions as well, so translation/rotation/scale/tint edits never touch `EcsWorld` during egui. `App::render_editor_ui` drains the new action variants (`SetMeshTranslation`, `SetMeshRotationEuler`, `SetMeshScale3D`, `SetMeshTint`) after the UI frame, keeping the inspector entirely shell-driven.
- Skin mesh tooling now rides the action queue too: joint count tweaks, skeleton assignment, joint sync, attach/detach buttons (and the “Add Skin Mesh” helper) all enqueue `InspectorAction`s that `App::render_editor_ui` applies post-egui. The inspector still updates its local snapshot immediately, but ECS mutations + status reporting happen only after the frame, matching the rest of the EditorShell migration.
- Plugin dashboards now consume shell-provided snapshots for status lists, asset readback metrics, ECS query history, and per-plugin watchdog logs. The egui layer renders purely from `EditorUiParams` and raises `UiActions` when watchdog logs should be cleared or asset readbacks retried, so we no longer call into `PluginManager` during `egui_ctx.run()` and `App` applies the requested mutations after the frame.
- Plugin manifest panel now receives the manifest entries/disabled-builtin list/path/error via `EditorUiParams`, so the UI no longer touches `PluginHost` directly while rendering. Builtin toggle requests still flow through `UiActions`, but the data powering the manifest view is purely shell-provided.
- Viewport panel now operates entirely on shell snapshots: `EditorUiParams` carries the active camera mode, a `Camera2D` clone, and cached 2D selection bounds so gizmo overlays/highlights render without touching `App` mid-frame. Gizmo mode changes piggyback on `EditorUiOutput`, so `App` applies camera-mode and gizmo-mode requests only after egui completes, matching the rest of the EditorShell pattern.
- Selection revert/highlight logic now consumes shell-provided snapshots too: `EditorUiParams` exposes both the current and previous `EntityInfo` payloads plus the associated 2D bounds, so `render_editor_ui` no longer queries `self.ecs` when the cursor leaves the viewport or when it draws the selection rectangle. Prefab drag/drop + gizmo overlay handling stay the same, but all viewport reads are now borrow-free.
- GPU timings panel now receives a shell-supplied `gpu_timing_supported` flag alongside the timing snapshots, so the UI displays capability warnings and export buttons without querying `Renderer` mid-egui. CSV export still happens after the UI frame through the existing `EditorUiOutput` flag.
- Audio debug panel now follows the same pattern: `EditorUiParams` exposes a plugin-present flag plus the trigger/history snapshots, egui queues audio enable/disable + clear-log intents via `UiActions`, and `App` performs the actual plugin mutations once `egui_ctx.run()` returns. No more mid-frame `plugin_runtime` borrows just to toggle audio.
- Mesh preview shelf no longer pings the runtime during egui: `EditorUiParams` now carries the preview mesh key, control mode, frustum-lock flag, orbit radius, free-fly speed, and plugin availability so the panel renders from immutable snapshots. `UiActions` still request control-mode/frustum changes, and `App` applies them after the UI run, matching the rest of the shell-driven panels.
- Animation controls are snapshot-driven now too: `App` clones the ECS `AnimationTime` resource before calling egui and hands it through `EditorUiParams`, so the playback/scale/fixed-step widgets inspect/edit local copies and `App` re-applies changes once the UI finishes. This removes the last direct ECS borrow from `render_editor_ui`.
- As part of that animation snapshot, `EditorUiParams` now carries the cloned `AnimationTime` directly instead of letting `render_editor_ui` query `self.ecs` up front, so the entire egui pass reads animation state purely from the shell-provided struct.
- UI & Camera/viewport widgets now consume `EditorUiParams` snapshots for `ViewportCameraMode`, a cloned `Camera2D`, and the window config knobs (size/fullscreen/vsync), so `render_editor_ui` no longer reads `self.camera` or `self.config` while egui is running. Viewport overlays, gizmo bounds, and display labels all draw from the shell-provided data, and viewport mode changes continue to flow through `UiActions`.

2025-11-17 validation note: ran `cargo test --locked` after relocating the animation helper unit tests into `app::animation_tooling`, giving the shell wiring an automated smoke check before scheduling the interactive editor sweep.
2025-11-18 validation note: re-ran the renderer headless coverage (`cargo test --locked present_mode_respects_vsync_flag`, `cargo test --locked headless_render_collects_gpu_timings`, `cargo test --locked headless_render_recovers_from_surface_loss`) plus the sprite animation integration suite (`cargo test --locked --test sprite_animation -- --test-threads=1`) to verify the editor-shell changes keep the runtime stable; all targeted tests now pass when the sprite harness is forced to run serially.
2025-11-19 validation note: repeated the sprite animation suite and the headless renderer tests after the viewport + animation snapshot work; `cargo test --locked --test sprite_animation -- --test-threads=1`, `cargo test --locked present_mode_respects_vsync_flag`, `cargo test --locked headless_render_collects_gpu_timings`, and `cargo test --locked headless_render_recovers_from_surface_loss` all pass.

## Step 5 Progress _(2025-11-20)_

- `docs/completed/remediation_plan.md` now lists the EditorShell/App decomposition milestone as complete so stakeholders can track that the shell owns all editor-only state and call sites.
- Validation sweep reran `cargo check`, `cargo test --locked present_mode_respects_vsync_flag`, `cargo test --locked headless_render_collects_gpu_timings`, `cargo test --locked headless_render_recovers_from_surface_loss`, and `cargo test --locked --test sprite_animation -- --test-threads=1`. All of them now pass after chaining the variable-rate schedule (`sys_apply_spin` -> ... -> `sys_apply_sprite_frame_states`) so the sprite frame queue drains after each drive pass; the previous failures were caused by the scheduler occasionally running `sys_apply_sprite_frame_states` before `sys_drive_sprite_animations`, which left `Sprite` components stuck on `redorb`.
- Added an opt-in frame-budget capture helper wired to `KESTREL_FRAME_BUDGET_CAPTURE=all_panels`: run `cargo run --release --features alloc_profiler` and the harness will (a) wait ~3 s with panels hidden, capture the idle baseline, (b) open the Keyframe Editor, Script Debugger, and Entity Lookup windows, wait ~4 s, capture the “all panels” snapshot, and (c) log both samples (plus the delta) to `perf/editor_all_panels.log` and the summary in `perf/editor_all_panels.txt`. Latest run recorded idle frame≈8.83 ms vs. panels-open frame≈14.34 ms (delta_update≈0 ms, delta_ui≈+1.18 ms, delta_alloc≈−26 KB).
