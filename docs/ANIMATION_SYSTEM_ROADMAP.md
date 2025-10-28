# Animation System Roadmap

## Mission
- Provide a production-ready, data-driven animation stack that scales from sprite flipbooks to complex character rigs.
- Keep workflows author-friendly, ECS-first, and deterministic so gameplay systems, tools, and tests stay in sync.
- Layer new capabilities incrementally without disrupting the already shippable sprite animation baseline.

## Baseline (Complete)
- Atlas timelines (`assets/images/atlas.json`) describe sprite clips with per-frame durations and loop flags.
- `SpriteAnimation` component and `sys_drive_sprite_animations` advance timelines and write to `Sprite`.
- Inspector UI exposes timeline selection, play/pause, loop, reset, and speed controls.
- Scene import/export and tests (`tests/sprite_animation.rs`) cover timeline parsing and runtime behavior.

Everything below builds on this foundation.

---

## Milestone 1 — Productionize Sprite Timelines
**Goal:** Turn the existing sprite system into a fast, ergonomic, hot-reloadable solution suitable for shipping games.

### Authoring & Import
- CLI/plugin to convert Aseprite or LDtk exports into atlas timelines (asset automation hook under `scripts/`).
- Hot-reload `atlas.json`: on change reparse timelines, update bound entities in-place, and preserve `frame_index`/`elapsed_in_frame` when possible.
- Document the atlas schema and importer usage in `docs/animation_workflows.md`.

### Runtime Semantics & Control
- New loop modes: `PingPong`, `OnceHold` (pauses on the last frame), `OnceStop` (existing behavior).
- Phase control: support explicit `start_offset` and optional `random_start` per timeline/instance.
- Lightweight per-frame events: `{ frame, name }` definitions that dispatch via `EventBus`; systems may ignore unhandled names.
- Global & layered time scaling: `AnimationTime` resource (global) plus optional tags/groups for cutscene control.
- Optional fixed-step evaluation path (e.g., 60 Hz) for deterministic capture and replay, with remainder accumulation.
- Public API additions (`seek_sprite_animation_frame`, `seek_sprite_animation_time`, `set_sprite_animation_phase_offset`) for authoring tools and scripting.

### Performance Polish
- Intern region names to IDs when loading timelines; store `u16 region_index` in `SpriteAnimationFrame`.
- Precompute UV rectangles alongside frames to avoid atlas lookups during playback.
- Write sprite state only when the frame changes and expose counters to analytics.
- (Deferred) Evaluate benefits of SoA storage if large entity counts demand it.

### Editor & UX
- Inspector scrubber: timeline slider, frame nudge buttons, current duration display.
- Optional frame thumbnails (lazy generated via atlas regions) to speed visual selection.
- Quick actions: reset to first/last frame, toggle event previews (log fired events to inspector status).

### Data Niceties & Gameplay Hooks
- Optional per-frame multipliers (tint/scale) applied when frames advance.
- Directional timeline helper: small utility that maps velocity/quadrant to timeline names.
- Audio hooks: map animation events to audio cues through existing audio diagnostics panel.
- Prefab support: allow prefabs to specify timelines and `start_paused`.

### Testing & CI
- Extend `tests/sprite_animation.rs` with golden playback coverage for each loop mode, phase offsets, and event dispatch.
- Add hot-reload regression test: mutate atlas data mid-playback and assert consistent state.
- Integrate animation checks into CI (parse → play → serialize → reload).

**Exit Criteria**
- All new loop modes, controls, hot-reload behavior, and tests are merged.
- CLI importer and documentation available for content creators.
- Inspector exposes scrubbing and event diagnostics.

---

## Milestone 2 — Transform & Property Tracks
**Goal:** Animate entity transforms and other properties through reusable clips.

- Define `AnimationClip` assets (JSON/`.kscene`) containing named tracks (`translation`, `rotation`, `scale`, `tint`, custom scalars) with interpolation metadata.
- Introduce ECS components (`ClipInstance`, `TransformTrackPlayer`, `PropertyTrackPlayer`) and reuse the animation timing controls from Milestone 1.
- Implement systems (`sys_drive_transform_tracks`, etc.) to evaluate clips and update `Transform`, `WorldTransform`, `Tint`, or custom components.
- Extend inspector with clip assignment, playback controls, and a read-only timeline view for keyframes.
- Update scene/prefab serialization to persist clip bindings.
- Add unit tests for interpolation (linear, step, cubic) and integration tests that verify final transforms after deterministic playback.

---

## Milestone 3 — Skeletal Animation Pipeline
**Goal:** Support bone-driven animation for 2D rigs and 3D meshes.

- Import GLTF (preferred) and optional Spine-style data into `AssetManager` as `Skeleton`, `SkinWeights`, and `SkeletalClip` assets.
- ECS additions: `SkeletonInstance`, `SkinMesh`, `BoneTransforms`, `SkeletalClipPlayer`, joint caches.
- Runtime: CPU evaluation of poses, GPU upload of joint matrices, renderer updates for skinned meshes (extend `renderer.rs`).
- Editor: hierarchy view, bone overlays in the viewport, clip selection/playback UI.
- Tests: golden import checks (joint hierarchy/rest pose), runtime pose validation, basic skinning correctness assertions.

---

## Milestone 4 — Animation Graphs & Blending
**Goal:** Combine clips through state machines, blend trees, and additive layers.

- Define `AnimationGraph` assets with states, transitions, parameters, and blend nodes (1D/2D, additive).
- `AnimationGraphInstance` component stores runtime parameters and active nodes.
- Graph evaluator system samples referenced clips (sprite, transform, skeletal) and composes results into shared pose buffers.
- Expose scripting API to adjust parameters and trigger transitions; integrate with timeline events for hooks (e.g., footstep → transition).
- Editor debugging tools: state list, transition timers, active blend weights, and (future) node-graph UI.
- Tests: deterministic graph evaluation, blend correctness, parameter edge cases.

---

## Milestone 5 — Tooling & Automation
**Goal:** Improve authoring productivity and operational visibility.

- Full timeline/keyframe editor inside `editor_ui`: layer stacks, curve editing, drag-and-drop clip sequencing, live scrubbing.
- Asset pipeline automation: watch tasks that hot-reload clips, exporters for third-party tools, validation CLI (`scripts/animation_check`).
- Analytics integration: extend `SystemProfiler` and stats panels with animation evaluation cost, active clips, bone counts.
- Sample content: curated scenes demonstrating sprite, transform, skeletal, and graph-driven characters.
- Documentation refresh: `docs/animation_workflows.md`, tutorials, and troubleshooting guide.

---

## Cross-Cutting Concerns
- **Determinism:** fixed-step evaluation and golden tests ensure repeatable results.
- **Performance:** monitor CPU/GPU budgets per milestone; gate heavy features behind opt-in feature flags (`skeletal_animation`, `animation_graph`).
- **Serialization & Versioning:** version all new asset formats and provide migration helpers.
- **Plugin Surface:** expose stable APIs for plugins to query/control animations and register custom importers.
- **Error Handling:** surface asset issues through `EventBus`, editor warnings, and analytics logs.

---

## Open Questions
- Preferred authoring tools and formats (Aseprite, LDtk, DragonBones, Spine) for prioritizing importer work.
- Requirements for runtime retargeting between different skeletons.
- Target platforms for GPU skinning (WebGPU, Vulkan, DX12) and any constraints they impose.
- Desired integration depth with scripting debugger (step, rewind, visualize events).
- Scope for procedural/physics-driven animation (IK, ragdolls) after graph support lands.

This roadmap supersedes prior drafts and will be iterated as milestones land or new requirements surface.
