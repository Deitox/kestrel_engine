# Animation System Implementation Plan

## Vision
- Deliver a flexible, data-driven animation stack that supports 2D flipbooks, transform/property tracks, skeletal rigs, and runtime blending so games can mix authored and procedural motion.
- Integrate tightly with the existing ECS (`Sprite`, `Transform`, `MeshRef`, etc.), asset pipeline, editor tooling, analytics, and testing infrastructure.

## Current Capabilities (Baseline)
- Sprite timelines already parse from atlas JSON, populate `SpriteAnimation` components, and advance via `sys_drive_sprite_animations`.
- Inspector UI can select timelines, toggle play/loop, tweak speed, and issue reset commands.
- `EcsWorld` supports attaching timelines programmatically (`set_sprite_timeline`) and has regression coverage in `tests/sprite_animation.rs`.

These form Phase 0 – everything below builds on top of this foundation.

## Guiding Principles
- **Data first:** externalize authoring formats (JSON, `.kscene`, GLTF) with schema versioning and validation.
- **ECS-friendly:** expose state via components/resources; animation systems run inside existing schedules.
- **Deterministic & testable:** use deterministic stepping; provide unit/integration tests for each feature.
- **Editor-centric workflows:** every runtime feature should surface in `editor_ui` for inspection/debugging.
- **Incremental delivery:** grow capabilities in well-scoped phases to keep refactors manageable.

## Phase 1 — Sprite Timeline Enhancements
**Goals:** polish the flipbook system to close near-term gaps and lay groundwork for advanced features.

- **Data schema**
  - Extend atlas timelines with optional `ping_pong` / `reverse` flags and per-frame event markers (e.g., `events: ["spawn_spark"]`).
  - Add schema docs + JSON schema (under `docs/`), update loader in `AssetManager::parse_timelines`.
- **Runtime**
  - Update `SpriteAnimation` to support ping-pong playback, per-frame event queues, and blend weights.
  - Emit animation events through `EventBus` so gameplay systems can react.
  - Track elapsed clip time & normalized progress for blending/analytics.
- **Editor/UX**
  - Enhance inspector to toggle ping-pong, display pending frame events, and scrub preview (drag slider -> call new `ecs.preview_sprite_frame`).
  - Add analytics counters (playback speed, dropped frames) via existing profiler/analytics panels.
- **Testing**
  - Extend `tests/sprite_animation.rs` with cases for ping-pong, events, and normalization.

**Dependencies:** None beyond current baseline.  
**Deliverables:** Updated atlas schema, ECS runtime changes, editor UI, tests, documentation.

## Phase 2 — Transform & Property Tracks
**Goals:** animate entity transforms, colors, and other scalar/vector properties via keyframe tracks.

- **Data model**
  - Introduce `AnimationClip` assets (new JSON / `.kscene` block) containing named tracks (`translation`, `rotation`, `scale`, `tint`, custom scalar).
  - Support bezier/easing curves with interpolation metadata (`step`, `linear`, `cubic`).
  - Reference clips from scenes via new `AnimationBinding` component (entity-to-track mapping).
- **ECS components/resources**
  - `TransformTrackPlayer`, `PropertyTrackPlayer`, `ClipInstance` storing playback state, targets, and blend weights.
  - Reuse `TimeDelta`; add `AnimationClock` resource for global time scaling & pause.
- **Systems**
  - Author `sys_drive_transform_tracks` to evaluate clips each frame and write into `Transform`.
  - Inject tint/other property updates via dedicated systems to avoid resource contention.
- **Editor tooling**
  - Extend inspector with clip assignment UI, playback controls, and keyframe previews.
  - Add timeline panel (initially read-only) showing clip length, current time, and keyframes.
- **Import/export**
  - Update scene serializer to round-trip clips/bindings.
- **Testing**
  - Unit tests for interpolation correctness.
  - Integration test that plays a clip, verifies world-space transforms match expected values.

**Dependencies:** Phase 1 analytics events (for timeline debugging).  
**Deliverables:** Clip asset format, ECS components/systems, editor bindings, tests.

## Phase 3 — Skeletal Animation Pipeline
**Goals:** enable bone-driven animation for 2D/3D characters (mesh skinning + rigged sprites).

- **Asset ingestion**
  - Support GLTF skin + animation import (prefer `gltf` crate) or Spine-style JSON for 2D rigs.
  - Convert imported data into `Skeleton`, `SkinWeights`, and `SkeletalClip` assets stored in `AssetManager`.
- **ECS extensions**
  - Components: `SkeletonInstance`, `SkinMesh`, `BoneTransforms`, `SkeletalClipPlayer`.
  - Resources for joint matrices caches and GPU buffers.
- **Runtime systems**
  - CPU path: evaluate pose per frame, cache joint matrices.
  - GPU path: upload matrices, update renderer instance data (extend `mesh.rs` / `renderer.rs` to handle skinned meshes).
  - Optionally support 2D bone-driven sprites (regions bound to bones).
- **Editor support**
  - Skeleton hierarchy view, per-bone visualization (overlay in viewport using existing gizmo rendering).
  - Inspector controls for selecting clips, previewing poses, adjusting playback speed.
- **Testing**
  - Golden tests for GLTF import (compare joint hierarchy, rest pose).
  - Runtime test verifying skinning output (e.g., CPU evaluation matches expected vertex positions).

**Dependencies:** Phase 2 clip infrastructure (re-use player logic), renderer support for instancing updates.  
**Deliverables:** Import pipeline, ECS components/systems, renderer integration, editor tools, tests.

## Phase 4 — Animation Graphs & Blending
**Goals:** combine multiple clips with state machines, blend trees, and runtime parameters.

- **Graph assets**
  - Define `AnimationGraph` format: states referencing clips, transitions with conditions (parameters, events), blend nodes (1D/2D blending, additive layers).
  - Provide DSL/JSON + validation tooling.
- **Runtime**
  - Create `AnimationGraphInstance` component storing parameter set and active nodes.
  - Implement graph evaluator system (`sys_update_animation_graphs`) that samples clips (sprite, transform, skeletal) and writes results into shared pose buffers.
  - Support additive and override layers, aim offsets, IK hooks.
- **Scripting & gameplay**
  - Expose graph parameter API to scripts (`scripts.rs`) and events so gameplay can drive transitions.
- **Editor**
  - Graph inspector/editor (initially parameter table + state list; future visual node editor).
  - Runtime debug overlay showing active states, transition timers, and blend weights.
- **Testing**
  - Unit tests for graph evaluation (deterministic transitions).
  - Integration test verifying blend result between walk/run clips given speed parameter.

**Dependencies:** Phases 1–3 (clips, bones, events).  
**Deliverables:** Graph schema, ECS runtime, editor debugging UI, tests.

## Phase 5 — Tooling & Authoring Enhancements
**Goals:** improve productivity for content teams and ensure pipeline resilience.

- **Editor timeline panel**
  - Full timeline editor with keyframe creation, curve editing, event markers.
  - Drag-and-drop clip sequencing, layer stacking, scrubbing with live viewport preview.
- **Automation**
  - CLI tools for baking clips, validating graphs, and converting Spine/GLTF data.
  - Watcher tasks that hot-reload animation assets during development.
- **Analytics & profiling**
  - Extend `SystemProfiler` to track animation evaluation cost.
  - Add frame-time overlays summarizing animation complexity (active clips, bones).
- **Documentation & tutorials**
  - Author `docs/animation_workflows.md` with end-to-end guides.
  - Sample scenes demonstrating sprite, transform, skeletal, and blended animations.

**Dependencies:** Prior phases in place.  
**Deliverables:** Timeline editor, automation scripts, documentation, sample content.

## Cross-Cutting Concerns
- **Performance:** maintain CPU budgets; consider SIMD for pose evaluation, GPU skinning paths, caching strategies.
- **Memory:** share clip data across entities; reference counts via `AssetManager`.
- **Serialization:** version all new formats; provide migration scripts.
- **Determinism:** guarantee fixed-step evaluation for tests and replay systems.
- **Error handling:** emit actionable errors via `EventBus`, log asset issues, highlight problems in the editor.

## Risks & Mitigations
- **Complexity creep:** mitigate with phased delivery and feature flags (`animation_graph`, `skeletal_animation`).
- **Asset import variability:** rely on widely-used formats (GLTF) and create conformance tests.
- **Editor UX debt:** schedule dedicated polish cycles; gather feedback from content creators early.
- **Performance regression:** integrate profiling hooks per phase; add benchmarks under `tests/benchmarks/`.

## Open Questions
- Preferred third-party tooling? (Spine, DragonBones, Maya exports?)
- Do we need runtime retargeting between skeletons?
- What platforms must the GPU skinning path support (WebGPU, Vulkan, DX12)?
- How should animation data integrate with the scripting debugger (step/rewind controls)?
- Should physics-driven animations (ragdolls) feed back into animation graphs?

Answering these questions will refine the plan before major implementation begins.
