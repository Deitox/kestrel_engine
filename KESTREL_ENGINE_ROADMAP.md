# 🦅 Kestrel Engine — Full Development Roadmap

**Version:** 0 → 1.0  
**Language/Stack:** Rust + WGPU + Winit + Bevy ECS + Egui  
**Philosophy:** Clean, self-contained, data-driven engine — code-first, no drag-drop UI required.  

---

## 🩵 Milestone 0 — Concept & Architecture Blueprint  
**Goal:** Define scope, subsystems, and philosophy before a single line of code.  

**Deliverables:**
- Overall architecture diagram (Renderer, ECS, Asset, Input, UI).
- Naming and crate structure (`kestrel_engine/src/` layout).
- Decision log on external crates (Bevy ECS, WGPU, Egui, Image, Serde).
- Foundational code style + build setup (Cargo, Clippy, rustfmt).

**Key Design Rules:**
1. Every frame is a clean data flow: Input → ECS → Renderer → UI.
2. No “magic singletons”: all resources explicit.
3. Deterministic simulation loop (fixed + variable time steps).

---

## ⚙️ Milestone 1 — Core Runtime & Renderer Initialization  
**Goal:** Get pixels on screen.  

**Deliverables:**
- Winit window loop using `ApplicationHandler`.
- WGPU initialization with SRGB format.
- Simple quad rendering with WGSL shader.
- `Time` utility with delta and elapsed.
- Resize & surface reconfiguration.

**Stretch:**
- Configurable vsync.
- Hot-reload clear color from JSON.

---

## 🖼️ Milestone 2 — Sprites, Atlas, and Transform Hierarchy  
**Goal:** Batch-render many sprites with hierarchical transforms.  

**Deliverables:**
- AssetManager with texture + atlas loader.
- ECS Components: `Transform`, `Parent`, `Children`, `WorldTransform`, `Sprite`.
- Instanced vertex data (matrix + UV rect).
- Atlas batching → one draw call for 1000s of sprites.

**Stretch:**
- Animated sprite frames (time-based UV cycling).

---

## ⌨️ Milestone 3 — Input, Spawning, Fixed / Variable Time  
**Goal:** Allow player input & large-scale entity management.  

**Deliverables:**
- Input event manager (keyboard, mouse, wheel).
- Fixed 60 Hz update loop; variable-step visuals.
- On-demand spawning (space/B) and dynamic instance buffer growth.
- Early performance stress tests.

**Stretch:**
- Simple input remapping config file.

---

## 🧮 Milestone 3.5 — Spatial Hashing & Collisions  
**Goal:** Introduce spatial awareness & simple physics.  

**Deliverables:**
- Broadphase spatial grid.
- Simple impulse-based separation (AABB overlap).
- Debug visualization (bounding boxes or cell overlays).

**Stretch:**
- Basic quadtree fallback when density spikes.

---

## 🧰 Milestone 4 — Stability, Error Handling, and Config  
**Goal:** Make the foundation robust and user-friendly.  

**Deliverables:**
- Error propagation via `anyhow`.
- Config file for display mode, vsync, resolution.
- Graceful asset failure recovery.
- Clean module boundaries and documentation comments.

**Stretch:**
- Optional CLI flags to override config.

---

## 🧭 Milestone 5 — Egui Debug UI  
**Goal:** In-window control panel for live tuning.  

**Deliverables:**
- Entity counter, sliders for spawn count & spatial cell.
- Real-time frame-time histogram.
- Runtime toggles for debug visuals.

**Stretch:**
- Collapsible panels and profiler integration.

---

## 🔍 Milestone 6 — Camera, Picking, and Gizmos  
**Goal:** Fully navigable 2D world & interactive selection.  

**Deliverables:**
- Camera pan/zoom (RMB + wheel).
- Screen→world & world→screen conversion.
- Click-to-select entities, highlight gizmo.
- Delete / inspect via UI.

**Stretch:**
- Multiple cameras & “follow target” logic.

---

## 🧠 Milestone 7 — Scripting Layer  
**Goal:** Extend the engine with modifiable gameplay logic.  

**Deliverables:**
- Embed a scripting language (Rhai or Lua).
- Bind ECS entity manipulation (spawn, move, despawn).
- Script reload on file save (hot-reload).

**Stretch:**
- Scripting debugger or REPL console.

---

## 🧱 Milestone 8 — Physics & Particles  
**Goal:** Add realism and visual flourish.  

**Deliverables:**
- 2D rigidbody and collider ECS components.
- Integrate Rapier2D for collisions.
- Simple particle emitter system (instanced billboards).

**Stretch:**
- Force fields, attractors, particle trails.

---

## 🌗 Milestone 9 — Audio & Event Bus  
**Goal:** Add sound and reactive systems.  

**Deliverables:**
- Simple `AudioManager` (rodio crate).
- Global `EventBus` resource for messaging between systems.
- Sound playback from ECS events.

**Stretch:**
- 3D positional audio with falloff.

---

## 🪶 Milestone 10 — Scene Graph & Serialization  
**Goal:** Load and save structured scenes.  

**Deliverables:**
- `Scene` serializer (Serde JSON or Ron).
- Load scenes from disk and restore ECS state.
- Asset dependency graph and reference counting.

**Stretch:**
- Binary `.kscene` format with compression.

---

## 🪄 Milestone 11 — Editor Layer  
**Goal:** Visual inspector and scene manipulation (within window).  

**Deliverables:**
- Egui-based entity inspector.
- Transform gizmos (translate/rotate/scale).
- Save / load scene button.

**Stretch:**
- Drag-drop prefab creation.

---

## 🌍 Milestone 12 — 3D Extension  
**Goal:** Add depth support.  

**Deliverables:**
- Switch to perspective projection path.
- Mesh loading (glTF).
- Basic PBR shader pipeline.

**Stretch:**
- Shadow maps and light culling.

---

## 🧩 Milestone 13 — Plugin / Module System  
**Goal:** Make the engine extensible.  

**Deliverables:**
- Plugin registration API (init/update hooks).
- Dynamic load via `.dll` / `.so` (optional).
- Versioned feature registry.

**Stretch:**
- Sandbox for untrusted plugins.

---

## 🚀 Milestone 14 — Build & Distribution  
**Goal:** Turn engine into a portable product.  

**Deliverables:**
- CLI tool (`kestrel-build`) for bundling games.
- Asset packer and release mode pipeline.
- Windows/Linux/macOS builds.

**Stretch:**
- WebAssembly target via `wgpu`+`winit` web backend.

---

## 🎯 Milestone 15 — Finalization & Docs  
**Goal:** Public-facing release.  

**Deliverables:**
- Comprehensive documentation (user guide + API docs).
- Example games (`pong`, `asteroids`, `arena`).
- Version 1.0 release tagging.

**Stretch:**
- Automated CI/CD, crates.io publication, versioned templates.

---

## 🧩 Long-Term Vision (Post-1.0)
- Headless server mode for networked simulations.  
- ECS hot-migration for multiplayer state sync.  
- Procedural asset pipelines (noise, generation, shaders).  
- Visual node editor for logic scripting.  
