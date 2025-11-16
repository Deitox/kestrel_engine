# Editor Shell Extraction Plan

1. **Catalog Dependencies**
   - Enumerate the existing `App` fields that only serve the editor UI (prefab inputs, script console, animation keyframe panel, telemetry caches, egui handles).
   - Note the types they depend on (`ClipEditRecord`, `AnimationValidationEvent`, `ScriptConsoleEntry`, etc.) so the new module exposes them via re-exports or wrapper APIs.

2. **Introduce `EditorUiState`**
   - Define a struct in `app::editor_shell` that holds the `// UI State` fields currently on `App`, along with helper methods for default construction.
   - Keep the existing structs (`ClipEditRecord`, etc.) near their current definitions for now and reference them from the new state struct.

3. **Refactor App Construction**
   - Update `App::new` to initialize `EditorUiState` through the shell and remove the duplicated fields from `App`.
   - Provide accessors on `EditorShell` (e.g., `ui_state_mut()`) so `App` can read/write state without exposing internals.

4. **Port Call Sites Incrementally**
   - Replace direct `self.ui_*` and related references with `self.editor_shell.ui.*` in small sections (e.g., Stats panel data, prefab workflows, script console) and run `cargo check` after each batch.

5. **Document & Test**
   - Once the migration is stable, update `docs/remediation_plan.md` to note progress on the EditorShell milestone.
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

Next up for Step 3: update `App::new` to populate `EditorUiState` via these helpers so that subsequent call-site ports only need to switch to `self.editor_shell.ui_state_mut()` instead of recreating the defaults in multiple places. Once that’s in place we can start deleting the duplicated fields from `App` in batches.

## App Wiring Progress _(Step 3 – 2025-11-16)_

- `App::new` now constructs `EditorUiState` using `EditorUiStateParams`/`EmitterUiDefaults` and installs it on `EditorShell` right after the struct is created. This keeps the egui handles + UI state packaged together.
- Introduced `App::editor_ui_state()` / `editor_ui_state_mut()` helpers so call sites can reach into the new state without touching the shell internals.
- First migration batch: all script debugger / REPL data (`script_debugger_open`, focus flag, REPL input/history buffers, console logs, last reported error) now live exclusively inside `EditorUiState`. The associated helpers (`push_script_console`, `append_script_history`, `script_repl_history_arc`, `sync_script_error_state`, etc.) were updated to read/write the shell state, and the egui `EditorUiParams`/`EditorUiOutput` plumbing was rewired accordingly.

Next slices for Step 3: move other tightly scoped blocks (e.g., prefab inputs or particle sliders) into `EditorUiState` via the same helper methods so future call-site ports in Step 4 become mostly mechanical.
