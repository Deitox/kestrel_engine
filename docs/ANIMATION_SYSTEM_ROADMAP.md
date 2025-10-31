# Animation System Roadmap

## Mission
- Deliver a production-grade animation stack that begins with sprite flipbooks and scales to transform, skeletal, and blended character motion.
- Preserve author-friendly workflows, ECS integration, determinism, and predictable performance at every milestone.
- Ship in incremental, verifiable milestones with clear exit criteria, perf budgets, and tooling support.

## Baseline (Shipping Today)
- Atlas timelines (`assets/images/atlas.json`) expose sprite animations with per-frame durations and loop flags.
- `SpriteAnimation` components plus `sys_drive_sprite_animations` advance timelines and update `Sprite` data.
- Inspector UI supports selecting timelines, play/pause, loop toggles, resets, and speed adjustments.
- Scene import/export and tests (`tests/sprite_animation.rs`) cover parsing, progression, pause/resume, and reset flows.

This foundation remains unchanged; new work layers on top of it.

## Operating Principles
- **Performance Budgets:** Each milestone owns a measurable CPU/GPU budget (see table below). Budgets are enforced via automated benchmarks (`tests/animation_bench.rs`) that report min/avg/max times and entity counts.
- **No Per-Frame Allocation:** Playback systems must avoid heap allocations during `update()`; preallocate storage, intern region names, and reuse buffers.
- **Determinism:** Provide variable-step default with an optional fixed-step path for capture/replays. Extend golden tests to verify repeatability.
- **Versioned Data:** Every animation-related asset (atlas, clips, graphs) carries a schema version; migrators live under `scripts/`.
- **Plugin-safe APIs:** Public ECS helpers stay stable and are gated by feature flags where necessary.
- **Tool-First:** Hot-reload and inspector features land alongside runtime capabilities so authors can validate changes immediately.
- **Scripts Command, Engine Drives:** Scripting surfaces control operations (play, stop, seek) but never per-frame evaluation logic.

## Core Performance Targets

| Feature Area             | Budget (Release Build)                     | Test Coverage                          |
|--------------------------|--------------------------------------------|----------------------------------------|
| Sprite timelines         | <= 0.20 ms CPU for 10 000 animators        | `animation_bench` sweep + golden tests |
| Transform/property clips | <= 0.40 ms CPU for 2 000 clips (linear/step) | Bench sweep + end-pose golden tests   |
| Skeletal evaluation      | <= 1.20 ms CPU for 1 000 bones             | Bench sweep + pose verification        |
| Skeletal upload          | <= 0.50 ms GPU upload per frame            | Renderer metrics + analytics hook      |
| Graph evaluation         | TBD (track with 5 actors x 3 layers)       | Deterministic state/transition tests   |

Benchmarks emit CSV summaries for CI. Failing budgets block the milestone exit.

---

## Milestone 1 - Productionize Sprite Timelines
**Objective:** Turn the existing sprite animation into a hot-reloadable, ergonomic, performant system ready for shipped games.

### Scope
- [x] **Importer MVP:** CLI tool to convert **Aseprite JSON** exports into atlas timelines; document end-to-end workflow in `docs/animation_workflows.md` (5-minute author path).
- [x] **Loop Modes:** Support `OnceStop` (existing), `OnceHold`, `Loop`, and `PingPong` (ensure edge-frame correctness).
- [x] **Events:** Optional per-frame events referencing frame indices (no payloads yet) that dispatch via `EventBus`.
- [x] **Phase Controls:** `start_offset` per instance plus `random_start` toggle to de-sync crowds.
- [x] **Hot-Reload:** Watch `atlas.json`; when timelines change, rebind by frame **name** (not index) and preserve `frame_index`/`elapsed_in_frame` when possible.
- [x] **Time Controls:** Global `AnimationTime` resource (scale & pause) and optional per-group scalars; fixed-step evaluation toggle with remainder accumulation.
- [x] **Inspector UX:** Add a timeline scrubber, left/right frame nudge buttons, frame duration display, and event preview toggle that logs fired events.
- [x] **Performance Polish:** Intern region names to IDs, store `u16` region indices, precompute UV rectangles, and only write to `Sprite` when frames change. Enforce zero allocations per frame. *(region IDs + cached UVs landed; latest bench at 0.192 ms for 10k animators — now within the 0.20 ms target)*

### Exit Criteria
- [x] `animation_bench` demonstrates <= 0.20 ms CPU for 10 000 animators (release build).
- [x] Golden playback tests cover all loop modes, phase offsets, ping-pong edge frames, and event dispatch.
- [x] Hot-reload regression test confirms frame continuity when names persist.
- [x] Authoring doc published; importer validated via automated test using a fixture Aseprite export.

---

## Milestone 2 - Transform & Property Tracks
**Objective:** Introduce clip-driven animation for transforms and simple properties with deterministic playback.

### Scope
- `AnimationClip` assets (JSON/`.kscene`) with versioned schema containing named tracks for translation, rotation, scale, and tint.
- Interpolation limited to **Step** and **Linear**; track resolution operates in O(N) with "last writer wins" semantics per component.
- ECS components/resources: `ClipInstance`, `TransformTrackPlayer`, `PropertyTrackPlayer`, reuse global/layer time scale controls.
- Systems: `sys_drive_transform_tracks` and property update systems respecting the no-allocation rule.
- Inspector: clip assignment widget, play/pause, speed, scrubber (read-only keyframe markers), and per-entity track status.
- Serialization updates for scenes/prefabs with versioned migrations.

### Progress
- [x] Transform/property clip authoring workflow documented in `docs/animation_workflows.md` (schema overview, template fixture, validation steps).
- [x] `ClipInstance` runtime introduced with transform/tint application and unit coverage (`tests/transform_clips.rs`).
- [x] Inspector exposes transform/property clip assignment, playback controls, scrubbing, and track masks.
- [x] `animation_bench` extended with transform clip sweep (2 000 clips) and CSV reporting, with CI script enforcement.

### Exit Criteria
- Benchmarks show <= 0.40 ms CPU for 2 000 active clips (release).
- Golden tests validate interpolation correctness and final poses after deterministic playback.
- Scene/prefab round-trip tests verify clip bindings remain intact.

---

## Milestone 3 - Skeletal Animation MVP (GLTF Focus)
**Objective:** Support bone-driven animation for character rigs using GLTF assets and GPU skinning.

### Scope
- GLTF import pipeline (via `gltf` crate) extracting skeleton hierarchy, bind poses, skin weights, and animation clips into `AssetManager`.
- ECS additions: `SkeletonInstance`, `SkinMesh`, `BoneTransforms`, `SkeletalClipPlayer`, cached joint matrices.
- Runtime: CPU pose evaluation, GPU skinning via uniform/storage buffers (no CPU skinning fallback initially).
- Renderer integration: extend mesh pipeline to bind joint palettes and split batches when exceeding hardware limits.
- Editor tooling: skeleton hierarchy inspector, bone overlays in viewport, clip playback controls with loop mode support.
- Analytics: expose active bone counts and CPU/GPU timings via existing profiler panel.

### Exit Criteria
- Benchmarks confirm <= 1.20 ms CPU for evaluating 1 000 bones and <= 0.50 ms GPU upload per frame.
- Golden pose tests compare evaluator output against fixture data.
- Import regression test validates skeleton/clip data across sample GLTF files.

---

## Milestone 4 - Animation Graphs v0
**Objective:** Enable deterministic state machines with light blending for sprite, transform, and skeletal clips.

### Scope
- `AnimationGraph` assets: versioned JSON describing states, transitions, and parameters.
- Start with a stateless core: state machine transitions triggered by time-based events and boolean flags.
- Extend to float parameters and 1D blends once stateless flow is stable; defer 2D blends/additive layers.
- `AnimationGraphInstance` component storing parameter set, active state, and blend info.
- Graph evaluator system sampling referenced clips and writing into shared pose buffers (respect budgets).
- Scripting API: commands to set flags, floats, trigger transitions, and await clip completion (`await_anim_end` semantics handled engine-side).
- Editor debugging: panel showing active state, transition timers, blend weights, and recent events.

### Exit Criteria
- Deterministic graph tests (unit + integration) pass with seeded parameter drives.
- Performance tracked with scenario: 5 characters x 3 layers stays within allocated CPU budget (TBD, record and set threshold).
- Scripting API documented with samples.

---

## Milestone 5 - Tooling, Automation, and Analytics
**Objective:** Round out authoring experience, automation, and visibility.

### Scope
- Lightweight keyframe editor in `editor_ui`: layer list, per-track key display, add/move/delete for Step/Linear keys, live scrubbing.
- Asset watchers that auto-reload clips/graphs and run validators (reuse importer infrastructure).
- CLI utilities: `animation_check` (validate schemas, budgets), `migrate_atlas` (bump versions), `animation_bench` (generate CSV).
- Analytics overlay: display animation evaluation cost, animator count, bone count, and budget thresholds in the HUD/status bar.
- Sample content: curated scenes showcasing sprite timelines, transform tracks, skeletal rigs, and graph-driven characters.
- Documentation: updated tutorials, troubleshooting guides, and scripting best practices.

### Exit Criteria
- Authoring workflow documented end-to-end; tutorial verified by sample project.
- CI executes validation + bench suites and surfaces metrics in logs.
- Playback visualization test generates hashed frame dumps to guard against regressions.

---

## Cross-Cutting Concerns
- **Testing & CI:** Expand regression coverage per milestone; integrate offscreen rendering hash tests to detect visual changes; ensure benchmarks run on CI hardware analogues.
- **Error Surfacing:** Log asset/schema issues via `EventBus`, show warnings in inspector, and track analytics counters for failures.
- **Serialization & Migration:** Maintain changelog of schema updates; provide scripts for migrating legacy files; enforce version checks in loaders.
- **Platform Constraints:** Track GPU palette limits (WebGPU/DX12/Vulkan) and split draw calls accordingly; gate features behind flags when not supported.
- **Risk Watch:** Ping-pong edge duplication, event flood at high FPS, skinning buffer exhaustion, and graph oscillation loops - each gets targeted tests before milestone close.

## Immediate Next Actions
- [x] Land GLTF skeleton importer and fixture assets (sample rig + clip extraction into `AssetManager`). *(Importer module + AssetManager retention APIs merged; minimal slime rig fixture + regression test now in place.)*
- [x] Introduce ECS skeleton components (`SkeletonInstance`, `SkinMesh`, `BoneTransforms`) and hook them into transform propagation. *(Component scaffolding + pose system now live; BoneTransforms updated each frame.)*
- [ ] Implement CPU pose evaluator with golden pose tests using the fixture clip.
- [ ] Extend renderer skinning to upload joint palettes, split batches when limits hit, and record GPU timing.
- [x] Add skeletal evaluation coverage to `animation_bench` capturing the 1 000-bone CPU budget. *(New bench suite seeds 10-bone rigs and tracks the 1 000-bone target via CSV output.)*
- [ ] Expand `docs/animation_workflows.md` skeletal section with authoring steps and inspector expectations.

This roadmap supersedes earlier drafts and reflects the final agreed-upon plan for animation system development. Further adjustments will follow formal change control once milestones begin execution.


