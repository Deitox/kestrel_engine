# Milestone 0–13 Quality Tracker

This tracker captures the current implementation evidence for milestones 0 through 13 and lists the polish tasks that will bring each milestone from “working” to “perfected.” Link back to the relevant source lines whenever you validate a deliverable or update a task.

## Status At A Glance

| Milestone | Status | Evidence Snapshot | Key Polish Targets |
| --- | --- | --- | --- |
| 0 – Concept & Architecture | Complete | Architecture, module map, decision + style guides committed (`docs/ARCHITECTURE.md:1`, `src/lib.rs:1`) | Keep docs/code in sync as new plugins land; capture high-level diagrams when systems evolve |
| 1 – Core Runtime & Renderer | Complete | Winit `ApplicationHandler` loop, WGPU init, WGSL quad, `Time` helper, resize path (`src/app/mod.rs:109`, `assets/shaders/sprite_batch.wgsl:1`) | Grow the new surface-error regression tests into a headless swapchain harness for full reconfigure coverage |
| 2 – Sprites, Atlases, Transforms | Complete | AssetManager + ECS components + instancing + batching (`src/assets.rs:6`, `src/ecs/world.rs:580`) | Add animated sprite timelines per stretch goal (`KESTREL_ENGINE_ROADMAP.md:51`) |
| 3 – Input & Fixed/Variable Time | Complete | Keyboard/mouse manager, 60 Hz fixed step, burst spawning/input stressors (`src/input.rs:1`, `src/app/mod.rs:1262`) | Implement config-driven input remapping (`KESTREL_ENGINE_ROADMAP.md:65`) and document stress test baselines |
| 3.5 – Spatial Hashing & Collisions | Complete | Spatial hash resource, separation impulse, debug overlays (`src/ecs/physics.rs:317`, `src/app/editor_ui.rs:269`) | Prototype quadtree fallback for dense zones (`KESTREL_ENGINE_ROADMAP.md:78`) and add perf telemetry |
| 4 – Stability & Configuration | Complete | `anyhow` plumbing, JSON config loader, asset failure guards (`src/app/mod.rs:109`, `src/config.rs:1`) | Wire up CLI overrides for config values (`KESTREL_ENGINE_ROADMAP.md:92`) and extend config validation |
| 5 – egui Debug UI | Complete | Stats panel, frame-time histogram, spawn/emitter sliders, overlay toggles (`src/app/editor_ui.rs:248`) | Add collapsible profiler/metrics panes (`KESTREL_ENGINE_ROADMAP.md:105`) and capture UI snapshot tests |
| 6 – Camera, Picking, Gizmos | Complete | 2D camera pan/zoom, 3D picking, gizmo workflows, entity delete/inspect controls (`src/camera.rs:12`, `src/app/gizmo_interaction.rs:277`, `src/app/editor_ui.rs:666`) | Explore multi-camera/follow-target tooling (`KESTREL_ENGINE_ROADMAP.md:119`) and add 3D gizmo snapping QA list |
| 7 – Scripting Layer | Complete | Rhai host with hot reload, command queue, plugin wrapper (`src/scripts.rs:20`, `src/scripts.rs:433`, `src/app/mod.rs:1918`) | Prototype scripting debugger/REPL (`KESTREL_ENGINE_ROADMAP.md:132`) and add script fixture tests |
| 8 – Physics & Particles | Complete | Rapier resources/components, physics systems, particle emitters + caps (`src/ecs/physics.rs:90`, `src/ecs/systems/particles.rs:9`) | Investigate force fields/particle trails (`KESTREL_ENGINE_ROADMAP.md:145`); deterministic physics tests now live in `tests/physics_determinism.rs:1` |
| 9 – Audio & Event Bus | Complete | `EventBus`, audio plugin reacting to gameplay events, UI feedback (`src/events.rs:1`, `src/audio.rs:54`, `src/app/editor_ui.rs:1442`) | Add positional audio/falloff experiments (`KESTREL_ENGINE_ROADMAP.md:158`); audio health telemetry captured via `AudioHealthSnapshot` (`src/audio.rs:17`, `src/app/editor_ui.rs:1415`) |
| 10 – Scene Graph & Serialization | Complete | Scene structs, save/load with dependency tracking + ref-counted assets (`src/scene.rs:1`, `src/ecs/world.rs:887`, `src/app/mod.rs:742`) | Evaluate binary `.kscene` format (`KESTREL_ENGINE_ROADMAP.md:173`) and automated scene round-trip tests |
| 11 – Editor Layer | Complete | Entity inspector, gizmo mode toggles, scene toolbar, save/load actions (`src/app/editor_ui.rs:666`, `src/app/editor_ui.rs:1210`, `src/app/mod.rs:1725`) | Add drag-and-drop prefab pipeline (`KESTREL_ENGINE_ROADMAP.md:188`) and regression tests for editor ops |
| 12 – 3D Extension | Complete | glTF mesh path, material registry, HDR environment lighting, camera controls, mesh preview plugin (`src/mesh.rs:1`, `src/material_registry.rs:1`, `src/environment.rs:1`, `src/mesh_preview.rs:1`) | Pursue advanced shadow/culling techniques (`KESTREL_ENGINE_ROADMAP.md:202`) and GPU perf baselines |
| 13 – Plugin & Module System | Complete | `EnginePlugin` API, feature registry, manifest-driven loader, runtime reload hooks (`src/plugins.rs:19`, `src/plugins.rs:294`, `config/plugins.json:1`, `README.md:41`) | Design plugin sandboxing story (`KESTREL_ENGINE_ROADMAP.md:217`) and add compatibility contract tests |

## Detailed Notes

### Milestone 0 – Concept & Architecture Blueprint
- **Status:** Complete.
- **Evidence:** `docs/ARCHITECTURE.md:1` documents subsystem responsibilities and frame flow; `src/lib.rs:1` establishes the crate/module layout surfaced in the published API; `docs/DECISIONS.md:1` records third-party crate selections; `docs/CODE_STYLE.md:1` plus `Cargo.toml:1` lock in style and build rules.
- **Polish targets:** Keep the architecture doc updated whenever the plugin surface or frame ordering changes; add lightweight component diagrams when new subsystems (e.g., analytics, mesh preview) are added to make onboarding faster.

### Milestone 1 – Core Runtime and Renderer Initialization
- **Status:** Complete.
- **Evidence:** `src/app/mod.rs:109` launches a `winit` event loop and `run_app` handler, while `src/app/mod.rs:1000` implements `ApplicationHandler`; `src/renderer.rs:237` constructs the WGPU device/surface and prepares pipelines feeding the sprite quad shader at `assets/shaders/sprite_batch.wgsl:1`; `src/time.rs:1` tracks delta/elapsed time and feeds the fixed update logic; `src/app/mod.rs:1115` resizes the surface and egui screen descriptor on window events; `src/renderer.rs:202` now records supported present modes, exposes `set_vsync`, and ships surface-error classification tests, while `src/app/editor_ui.rs:248` wires an interactive VSync toggle into the Stats panel.
- **Polish targets:** Expand the new surface-error regression tests into a headless swapchain harness that exercises renderer reconfiguration end-to-end.

### Milestone 2 – Sprites, Atlases, and Transform Hierarchy
- **Status:** Complete.
- **Evidence:** `src/assets.rs:6` implements an `AssetManager` that loads atlas metadata + textures and keeps reference counts; `src/ecs/types.rs:8`, `:23`, and `:42` define `Transform`, `Parent`/`Children`, and `Sprite` components, while `src/ecs/types.rs:168` captures instanced vertex payloads; `src/ecs/world.rs:580` gathers sprite instances with UVs/tint, `src/app/mod.rs:1271` batches them per-atlas, and `src/renderer.rs:1497` streams them through a single instanced draw per batch.
- **Polish targets:** Implement the roadmap’s animated sprite frames (`KESTREL_ENGINE_ROADMAP.md:51`) and build automated scene snapshots to confirm hierarchy propagation stays deterministic.

### Milestone 3 – Input, Spawning, and Fixed vs Variable Time
- **Status:** Complete.
- **Evidence:** `src/input.rs:1` covers keyboard, mouse, wheel, and cursor deltas with helpers like `take_space_pressed`; `src/app/mod.rs:1262` maintains a 60 Hz fixed-step accumulator while rendering continues at display rate; `src/app/mod.rs:1152` and `:1159` drive auto-spawn + burst spawning from Space/B; `src/ecs/world.rs:229` performs the randomized burst instantiation with Rapier bodies for stress testing.
- **Polish targets:** Layer in config-driven input remapping per the roadmap (`KESTREL_ENGINE_ROADMAP.md:65`) and document reproducible stress-test entity counts to benchmark regression runs.

### Milestone 3.5 – Spatial Hashing and Collisions
- **Status:** Complete.
- **Evidence:** `src/ecs/physics.rs:317` defines the `SpatialHash` resource, while `src/ecs/systems/physics.rs:130` builds and queries the grid, applying impulses when overlaps are detected; `src/ecs/world.rs:795` exposes rectangles for tooling, and `src/app/editor_ui.rs:269` plus `:1547` allow toggling the debug overlay so bounding cells and colliders render in the viewport.
- **Polish targets:** Prototype the quadtree fallback noted in the roadmap (`KESTREL_ENGINE_ROADMAP.md:78`) and add metrics (cell occupancy, overlap counts) to quickly spot regressions in collision fidelity.

### Milestone 4 – Stability, Error Handling, and Configuration
- **Status:** Complete.
- **Evidence:** `src/app/mod.rs:109` wraps event loop creation in `anyhow::Context`; `src/config.rs:1` unmarshals `config/app.json:2` so display/VSync defaults are data-driven; `src/ecs/world.rs:580` and the surrounding asset helpers propagate contextual errors when atlas lookups fail; `src/app/mod.rs:742` retains/release atlases + meshes cleanly when scene dependencies change.
- **Polish targets:** Add the optional CLI overrides described in the roadmap (`KESTREL_ENGINE_ROADMAP.md:92`) and extend config validation/telemetry so missing configs surface in-editor warnings instead of console-only logs.

### Milestone 5 – egui Debug UI
- **Status:** Complete.
- **Evidence:** `src/analytics.rs:5` tracks frame history and recent events; `src/app/editor_ui.rs:248` lists entity/instance counts while `:254` renders a frame-time histogram; `src/app/editor_ui.rs:463` exposes sliders for spawn count, spatial cell size, and emitter tuning; `src/app/editor_ui.rs:269` toggles runtime overlays for hash cells/colliders.
- **Polish targets:** Implement the roadmap’s profiler panel (`KESTREL_ENGINE_ROADMAP.md:105`) and add egui screenshot tests (or golden metrics) so UI regressions are caught in CI.

### Milestone 6 – Camera, Picking, and Gizmos
- **Status:** Complete.
- **Evidence:** `src/camera.rs:12` provides pan/zoom and screen/world conversions; `src/app/gizmo_interaction.rs:38`/`:45` apply scroll zoom + RMB panning, while `:277` kicks off 3D picking via `EcsWorld::pick_entity_3d` and `:296` handles 2D selection; `src/app/editor_ui.rs:666` exposes inspector + gizmo mode toggles, and `src/app/mod.rs:1814` lets the UI delete selected entities safely.
- **Polish targets:** Explore multi-camera/follow-target flows noted in the roadmap (`KESTREL_ENGINE_ROADMAP.md:119`) and add gizmo usability checklists (snap granularity, cursor feedback) for QA passes.

### Milestone 7 – Scripting Layer
- **Status:** Complete.
- **Evidence:** `src/scripts.rs:20` defines the command enum the runtime understands; `src/scripts.rs:202` + `:274` manage the Rhai host, hot-reloading scripts when files change; `src/scripts.rs:433` wraps the host in an `EnginePlugin`; `src/app/mod.rs:1918` drains script commands and mutates the ECS accordingly.
- **Polish targets:** Prototype the scripting debugger/REPL from the roadmap (`KESTREL_ENGINE_ROADMAP.md:132`), add a handful of script integration tests, and surface script log levels in the analytics panel for quicker diagnosis.

### Milestone 8 – Physics and Particles
- **Status:** Complete.
- **Evidence:** `src/ecs/types.rs:151`/`:158` define `RapierBody`/`RapierCollider` components; `src/ecs/physics.rs:90` builds the Rapier pipeline/resources; `src/ecs/systems/physics.rs:34` etc. step Rapier, sync transforms, and handle world bounds; `src/ecs/systems/particles.rs:9` updates emitters/particles with caps from `ParticleCaps`; `src/ecs/world.rs:223` seeds a demo emitter for tooling; `tests/physics_determinism.rs:1` verifies the fixed-step pipeline stays deterministic.
- **Polish targets:** Experiment with force fields/trails per the roadmap (`KESTREL_ENGINE_ROADMAP.md:145`) and surface particle budgets in the analytics plugin.

### Milestone 9 – Audio and Event Bus
- **Status:** Complete.
- **Evidence:** `src/events.rs:1` defines `GameEvent` plus `EventBus`; `src/app/mod.rs:453` drains ECS events each frame and forwards them to plugins; `src/audio.rs:54` maps events to rodio playback and keeps a rolling trigger log; `src/audio.rs:17` introduces `AudioHealthSnapshot` so device/driver failures are captured; `src/app/editor_ui.rs:1415` surfaces that telemetry to developers.
- **Polish targets:** Prototype 3D positional audio/falloff curves (`KESTREL_ENGINE_ROADMAP.md:158`) and expand analytics/telemetry to include device capability diagnostics so unsupported backends fall back gracefully.

### Milestone 10 – Scene Graph and Serialization
- **Status:** Complete.
- **Evidence:** `src/scene.rs:1` captures scene metadata, dependencies, and entity payloads; `src/ecs/world.rs:887`–`:913` export scenes with mesh/material sources, and matching load helpers validate dependencies; `src/app/mod.rs:742` retains/releases atlases + meshes per-scene; `src/assets.rs:84` and `src/mesh_registry.rs:80` implement reference-counted retains; `src/app/mod.rs:1725` wires save/load actions into the UI with metadata persistence.
- **Polish targets:** Investigate the binary `.kscene` format with compression (`KESTREL_ENGINE_ROADMAP.md:173`), add automated scene round-trip/regression tests, and expand dependency health reporting (e.g., stale mesh sources).

### Milestone 11 – Editor Layer
- **Status:** Complete.
- **Evidence:** `src/app/editor_ui.rs:666` renders the entity inspector with gizmo mode toggles and tips; `src/app/gizmo_interaction.rs:73` handles translate/rotate/scale workflows (including 3D variants); `src/app/editor_ui.rs:1210` provides Save/Load buttons + history; `src/app/mod.rs:1725` executes those actions, refreshes scene metadata, and resets selection; `src/app/mod.rs:1606` supports ID lookup-driven selection changes.
- **Polish targets:** Build drag-and-drop prefab creation per roadmap (`KESTREL_ENGINE_ROADMAP.md:188`), and add regression coverage for editor UX flows (selection persistence, gizmo switching, history management).

### Milestone 12 – 3D Extension
- **Status:** Complete.
- **Evidence:** `src/mesh.rs:1` provides glTF import + GPU-friendly vertex packing; `src/mesh_registry.rs:17`/`:80` manage CPU/GPU meshes + reference counts; `src/material_registry.rs:1` uploads PBR material data + textures; `assets/shaders/mesh_basic.wgsl:1` implements a PBR fragment shader with normal/metallic/emissive inputs; `src/environment.rs:1` processes HDR maps into diffuse/specular/BRDF resources; `src/renderer.rs:295` and `:894` bind those resources and draw mesh passes with a shadow pipeline; `src/camera3d.rs:1` handles projection math, while `src/mesh_preview.rs:1` provides the editor-facing mesh viewport with orbit/free-fly controls.
- **Polish targets:** Continue the roadmap’s advanced lighting work (shadow improvements, light culling at `KESTREL_ENGINE_ROADMAP.md:202`), add GPU performance counters, and validate HDR asset ingestion across platforms.

### Milestone 13 – Plugin and Module System
- **Status:** Complete.
- **Evidence:** `src/plugins.rs:19` seeds the default feature registry, `src/plugins.rs:146` exposes `PluginContext`/facades, `src/plugins.rs:294` loads manifest entries via `libloading` and enforces API versions, and `config/plugins.json:1` keeps dynamic entries; `README.md:41` documents the reload workflow so users can rescan plugins without restarting.
- **Polish targets:** Design and implement the roadmap’s sandbox for untrusted plugins (`KESTREL_ENGINE_ROADMAP.md:217`), add compatibility tests that ensure feature declarations fail loudly, and capture plugin lifecycle metrics in analytics for observability.
