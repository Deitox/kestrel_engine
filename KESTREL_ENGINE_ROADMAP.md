# Kestrel Engine - Full Development Roadmap

**Version:** 0 to 1.0  
**Technology Stack:** Rust, WGPU, Winit, Bevy ECS, egui  
**Philosophy:** Clean, self-contained, data-driven engine; code-first with no mandatory editor.

---

## Milestone 0 - Concept & Architecture Blueprint
**Goal:** Define scope, subsystems, and guiding principles before coding.

**Deliverables**
- Architecture outline covering renderer, ECS, input, assets, and UI.
- Crate and module layout (`kestrel_engine/src/...`).
- Decision log for third-party crates (Bevy ECS, WGPU, egui, image, serde).
- Code-style guide and build setup (Cargo, Clippy, rustfmt).

**Key Design Rules**
1. Every frame follows a predictable flow: Input -> ECS -> Renderer -> UI.
2. Avoid hidden singletons; keep resources explicit.
3. Maintain a deterministic simulation loop with fixed and variable timesteps.

---

## Milestone 1 - Core Runtime and Renderer Initialization
**Goal:** Show pixels on screen.

**Deliverables**
- Winit application loop using `ApplicationHandler`.
- WGPU initialization with sRGB surface.
- Simple quad rendered with WGSL shader.
- `Time` helper tracking delta and elapsed time.
- Resize handling and surface recreation.

**Stretch Ideas**
- Configurable VSync toggle.
- Reload clear color from JSON.

---

## Milestone 2 - Sprites, Atlases, and Transform Hierarchy
**Goal:** Efficiently batch-render sprites with hierarchical transforms.

**Deliverables**
- AssetManager capable of loading textures plus atlas metadata.
- ECS components: `Transform`, `Parent`, `Children`, `WorldTransform`, `Sprite`.
- Instanced vertex data path (matrix + UV rectangle).
- Single-draw-call batching for thousands of sprites.

**Stretch Ideas**
- Time-based animated sprite frames.

---

## Milestone 3 - Input, Spawning, and Fixed vs Variable Time
**Goal:** Allow user input and large-scale entity creation.

**Deliverables**
- Keyboard, mouse, and wheel input manager.
- Fixed 60 Hz simulation step plus variable rendering step.
- Burst spawning via input (Space/B) and dynamic instance buffer growth.
- Early performance stress-test scenarios.

**Stretch Ideas**
- Config-driven input remapping.

---

## Milestone 3.5 - Spatial Hashing and Collisions
**Goal:** Introduce spatial awareness and basic physics responses.

**Deliverables**
- Spatial hash grid for broad-phase queries.
- Simple impulse-based separation for overlapping AABBs.
- Debug visualization of bounding regions.

**Stretch Ideas**
- Quadtree fallback for high-density zones.

---

## Milestone 4 - Stability, Error Handling, and Configuration
**Goal:** Harden the foundation and improve usability.

**Deliverables**
- Error propagation via `anyhow`.
- Config file controlling display mode, VSync, resolution.
- Graceful asset failure handling.
- Clean module boundaries with documentation comments.

**Stretch Ideas**
- Optional CLI overrides for config values.

---

## Milestone 5 - egui Debug UI
**Goal:** Deliver an in-window control panel for rapid iteration.

**Deliverables**
- Entity counter plus sliders for spawn counts and spatial cell size.
- Real-time frame-time histogram.
- Runtime toggles for debug visuals.

**Stretch Ideas**
- Collapsible panels and profiler integration.

---

## Milestone 6 - Camera, Picking, and Gizmos
**Goal:** Provide a fully navigable 2D view with basic selection tools.

**Deliverables**
- Camera pan/zoom (RMB + wheel).
- Screen-to-world and world-to-screen conversions.
- Click-to-select entities with highlight gizmo.
- Delete or inspect entities via UI.

**Stretch Ideas**
- Multi-camera support or follow-target logic.

---

## Milestone 7 - Scripting Layer
**Goal:** Extend the engine with hot-reloadable gameplay logic.

**Deliverables**
- Embed a scripting language (Rhai or Lua).
- Bind ECS entity operations (spawn, move, despawn).
- Hot-reload scripts on file changes.

**Stretch Ideas**
- Scripting debugger or REPL console.

---

## Milestone 8 - Physics and Particles
**Goal:** Add rigid-body dynamics and visual effects.

**Deliverables**
- Rigid-body and collider ECS components.
- Rapier2D integration for collisions.
- Particle emitter system with instanced billboards.

**Stretch Ideas**
- Force fields, attractors, or particle trails.

---

## Milestone 9 - Audio and Event Bus
**Goal:** Introduce sound and reactive messaging.

**Deliverables**
- Simple `AudioManager` (e.g., rodio).
- Global `EventBus` resource for cross-system communication.
- Play sounds in response to ECS events.

**Stretch Ideas**
- 3D positional audio with falloff.

---

## Milestone 10 - Scene Graph and Serialization
**Goal:** Load and save structured scenes.

**Deliverables**
- `Scene` serializer using JSON, RON, or similar.
- Restore ECS state from disk.
- Asset dependency tracking and reference counting.

**Status:** JSON scene quick-save/load, restore, and asset dependency tracking with reference counting are live.

**Stretch Ideas**
- Binary `.kscene` format with compression.

---

## Milestone 11 - Editor Layer
**Goal:** Provide in-window inspection and manipulation tools.

**Deliverables**
- egui-based entity inspector.
- Transform gizmos for translate/rotate/scale.
- Save and load scene buttons.

**Status:** Inspector editing spans sprites and meshes with perspective gizmos, a frame-selection helper, and a scene toolbar that tracks paths + dependency health.

**Stretch Ideas**
- Drag-and-drop prefab creation.

---

## Milestone 12 - 3D Extension
**Goal:** Add depth support and a 3D rendering path.

**Deliverables**
- Perspective projection pipeline.
- Mesh loading (glTF or similar).
- Basic PBR shader.
- HDR environment lighting with diffuse irradiance, specular prefiltering, and a BRDF LUT.

**Stretch Ideas**
- Shadow mapping and light culling.

---

## Milestone 13 - Plugin and Module System
**Goal:** Make the engine extensible.

**Deliverables**
- Plugin registration API with init/update hooks.
- Optional dynamic library loading (`.dll` / `.so`).
- Versioned feature registry.

**Stretch Ideas**
- Sandbox for untrusted plugins.

---

## Milestone 14 - Build and Distribution
**Goal:** Package the engine for release.

**Deliverables**
- CLI tool (`kestrel-build`) for bundling games.
- Asset packer and release-mode pipeline.
- Windows, Linux, and macOS builds.

**Stretch Ideas**
- WebAssembly target leveraging wgpu + winit web backends.

---

## Milestone 15 - Finalization and Documentation
**Goal:** Prepare a public-facing release.

**Deliverables**
- Comprehensive documentation (user guide + API docs).
- Example games (pong, asteroids, arena).
- Tag and publish version 1.0.

**Stretch Ideas**
- Automated CI/CD, crates.io publication, versioned templates.

---

## Long-Term Vision (Post-1.0)
- Headless server mode for networked simulations.
- ECS hot-migration for multiplayer state sync.
- Procedural asset pipelines (noise, generation, shaders).
- Visual node editor for logic scripting.
