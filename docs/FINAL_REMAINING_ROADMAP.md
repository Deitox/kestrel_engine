# Remaining Delivery Roadmap

This document consolidates the outstanding objectives that remain after closing milestones 0–13 and the shipped animation/tooling plans. Items are grouped by area so teams can plan the final push to 1.0.

## Engine Runtime (Core Roadmap)
- **Milestone 14 – Build & Distribution:** Ship `kestrel-build` CLI, asset packer, release-mode pipeline, and Windows/Linux/macOS bundles.
- **Milestone 15 – Finalization & Docs:** Publish user/API docs, example games (pong/asteroids/arena), and a 1.0 release/tag with templates.
- **Outstanding polish targets (from milestone checklists):** animated sprite timelines; config-driven input remap; quadtree fallback for dense collision zones; CLI overrides for config values + validation; profiler/debug UI panel expansion; multi-camera/follow-target flows; scripting debugger/REPL; force-field/particle trails experiments; positional audio/falloff; binary `.kscene` format; plugin sandboxing contract/compatibility tests.

## Animation System
- **Milestone 3 – Skeletal Animation MVP:** GLTF import validation (skeleton/clip data), ECS components (`SkeletonInstance`, `SkinMesh`, `BoneTransforms`), CPU pose evaluation with golden tests, GPU skinning + palette uploads with perf budgets, renderer batching/splitting, editor hierarchy overlays + clip controls, analytics surfacing of bone/palette costs.
- **Milestone 4 – Animation Graph v0:** Versioned graph assets (states/transitions/params), deterministic evaluator driving sprite/transform/skeletal clips, scripting API for flags/floats/triggers, editor debugger panel (active state, timers, blend weights), performance gate for 5 actors × 3 layers scenario, regression tests for deterministic transitions.
- **Perf guardrails still open:** Importer drift lint on synthetic noisy data; SIMD tail + event-flood tests for sprite animation fast path.

## Studio (S0–S7 Roadmap)
- **S1 – Project Model & Workspace UX:** New project templates (2D, 3D, minimal); per-project config (app/plugins) with project-local defaults; start screen persists recent projects; workspace layout persistence (dock layout, open panels, last-opened scene).
- **S2 – Scene Editor 2.0:** Hierarchy tree with drag/drop reparenting, rename, multi-select; undo/redo stack covering transforms, component add/remove, entity create/delete; multi-scene panel with startup scene marker; gizmo snapping (grid/angle/scale) and local/world modes.
- **S3 – Animation Suite:** Sprite event tracks with payloads; inline and in-viewport previews; optional preview mode decoupled from main simulation; skeletal inspector (joint tree, visibility, selection) with clip preview/playback and GLTF clip import settings; animation graph/state-machine editor with parameters and runtime hookup.
- **S4 – Profiling, Analytics & ECS Debugging:** ECS inspector (search by name/ID/component, component-level listings, optional viewport highlight); basic system graph view (order/phase); plugin health UI additions (CPU/event counts, enable/disable controls).
- **S5 – Plugin Ecosystem:** Editor extension API (custom inspectors/panels/overlays); controls to restart isolated plugins without crashing Studio.
- **S6 – Build & Distribution:** Build configuration panel (targets, build type, output dir, compression); build-and-run actions for Debug/Release; packaging flow producing distributable folder/zip (optional itch/installer helper).
- **S7 – UX & Onboarding:** First-run tour; bundled sample projects (tiny 2D and 3D); visual consistency pass; contextual help (tooltips, “?” links); versioned onboarding/docs (Getting Started, 3D basics, writing plugins).

## Performance & Bench Harness
- **Sprite Animation Perf Plan gaps:** Add importer drift lint test; add SIMD tail + event-flood regression coverage.
- **Sprite Benchmark Plan gaps:** CPU governor/turbo pin guidance; capture before/after CSVs with `animation_profile_snapshot`; lightweight CI check/scheduled run with `anim_stats`; optional dedicated `profile.animation`; const-dt SIMD path with scalar fallback and wrap/ping-pong tests; SoA↔AoS transcode correctness and prefetch/next-dt cache; finalize bench sweep and sign-off. 
