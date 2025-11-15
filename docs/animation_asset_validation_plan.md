# Animation Asset Watcher & Validation Strategy

*Status (2025-11-15): Implemented. Asset watchers now cover clips, graphs, and skeletal GLTF sources (see `src/app/mod.rs`, `src/app/animation_watch.rs`, `src/assets.rs`). Validators share the same parsing helpers used by the runtime, and CLI/editor paths emit identical `AnimationValidationEvent` records.*

Milestone 5 requires the editor to automatically reload animation clips/graphs, run validators, and surface actionable feedback. This document outlines the required behaviors and dependencies before implementation begins.

## Goals
1. Detect when animation assets (sprite timelines, transform clips, skeletal graphs) change on disk.
2. Reload the asset, run schema & semantic validators, and push results to the inspector + analytics.
3. Ensure failures are deterministic, logged with clear context, and block usage when critical.

## Assets in Scope
| Asset Type | Path Patterns | Notes |
|------------|---------------|-------|
| Sprite atlas timelines | `assets/images/*.json` (already watched) | Extend watcher so that timeline changes trigger animation validators, not just atlas reload. |
| Transform clips | `assets/animations/clips/**/*.json` | New watchers required; clip format parsed via `AnimationLibrary`. |
| Animation graphs/blueprints | `assets/animations/graphs/**/*.json` | Graph schema to be defined; watchers must debounce rapid saves. |
| Skeletal rigs/poses | `assets/animations/skeletal/**/*.json` | Reuse joint setup validators. |

## Watcher Behavior
1. **File Watch Source**: Extend existing `AtlasHotReload` or add a new `AnimationHotReload` that uses `notify` crate with debounce.
2. **Event Pipeline**:
   - File change event -> canonicalize path -> match against asset registry.
   - If asset recognized, schedule reload on the main thread (avoid cross-thread ECS mutations).
   - After reload, call the validator pipeline (see below).
3. **Debounce**: 100–200 ms debounce window to coalesce multiple save events.
4. **Error Handling**: If reload or validation fails, log via `EventBus`, show inspector warning, and display banner in Stats panel.

## Validator Pipeline
1. **Schema Validation**: Ensure JSON structure matches expected version; emit specific errors for missing fields, type mismatches, or version drift. (Reuse existing serde structs with version fields.)
2. **Semantic Checks**:
   - Timeline monotonic key times, non-empty key arrays, clamp values (e.g., probability ranges 0–1).
   - Graph validation for cycles, missing states, duplicate transitions.
   - Skeletal validators for bone counts, per-track interpolation compatibility.
3. **Performance Guardrails**: Optional heuristics (e.g., clip length limits, event density warnings) that warn but do not fail.
4. **Result Surface**:
   - `AnimationValidationEvent` struct with severity (Info/Warning/Error), asset path, summary, and optional payload.
   - Push to Analytics plugin queue for Stats panel.
   - Display inline in inspector when editing the affected asset.

## Editor Surfacing
1. Inspector banner with latest validation status.
2. Stats panel list of recent validation warnings/errors (similar to capability/asset readback logs).
3. Optional toast/notification when critical failure occurs.

## CLI Integration
Validators must be callable headlessly via `animation_check`:
1. CLI accepts file/dir/glob, runs same validator pipeline, prints structured output, and returns non-zero on errors.
2. CI stage runs `animation_check assets/animations` after asset changes.

## Telemetry
- Record validation events (asset, severity, duration) into Analytics plugin.
- Add metrics to Stats HUD for “Last validation status” and “Pending validation count”.

## Implementation Notes
1. `AnimationValidationEvent` plus analytics surfacing landed in `src/animation_validation.rs` / `src/analytics.rs`; warnings/errors appear in the Stats panel and inspector banner.
2. `AnimationAssetWatcher` (see `src/app/animation_watch.rs`) subscribes to clips, graphs, and skeletal directories, canonicalizes paths, and triggers reload/validation on the main thread.
3. Reload hooks live in `src/app/mod.rs` (`reload_clip_from_disk`, `reload_graph_from_disk`, `reload_skeleton_from_disk`) and reuse the shared parsing helpers in `src/assets.rs`. Skeleton reloads preserve active clip/time/playing state.
4. Validators now perform full schema + semantic checks (clip timelines, graph states/transitions, skeletal joint counts) and are exercised by fixture tests.
5. CLI coverage: `cargo run --bin animation_check -- assets/animations` runs the same pipeline; a regression test suite (`cargo test animation_validation`) keeps the validators in sync with asset formats.
