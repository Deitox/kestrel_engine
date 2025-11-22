# Kestrel Studio Roadmap

This roadmap tracks **Studio-specific** work on top of `kestrel_engine`.  
Engine Milestones 0–13 are considered the runtime foundation; this document focuses on:

- Project-centric workflows (projects, scenes, assets)
- Editor UX (viewport, inspector, hierarchy, animation tools)
- Profiling, debugging, and plugin health
- Build & distribution

Milestones are labeled **S0–S7** to distinguish them from core engine milestones.

---

## S0 — Engine / Studio Separation

**Goal:** Cleanly separate the **runtime engine** from the **Studio shell**.  
`kestrel_engine` remains editor-agnostic; `kestrel_studio` becomes the primary editor binary.

**Suggested Issue Labels:**
- `area:architecture`
- `area:studio`
- `type:refactor`

### Tasks

- [x] **Create `kestrel_studio` crate**
  - [x] Add a new `kestrel_studio` binary crate to the workspace.
  - [x] Wire up window creation, main loop, and egui/docking from here instead of the engine crate.

- [x] **Move editor-only UI into Studio**
  - [x] Relocate viewport UI, docking layout, inspector panels, analytics overlays, mesh preview UI, etc. from `kestrel_engine` into `kestrel_studio`.
  - [ ] Keep only minimal debug hooks / stats APIs inside `kestrel_engine` (e.g., `SystemProfiler`, frame stats exporters).

- [x] **Define a runtime host interface**
  - [x] Introduce an `EngineHost` or `RuntimeHost` abstraction used by Studio:
    - [x] Load/unload scenes.
    - [x] Start/stop play mode.
    - [x] Step frames in paused mode.
    - [x] Access ECS snapshots for inspection where appropriate.
  - [x] Ensure Studio talks to the engine through this host abstraction.

- [ ] **Maintain engine-only builds**
  - [x] Confirm `kestrel_engine` can still:
    - [x] Build as a pure library with no `egui` / editor dependencies.
    - [x] Ship a "game-only" binary that links `kestrel_engine` but not `kestrel_studio`.
  - [ ] Add CI checks (if practical) to ensure `kestrel_engine` remains editor-agnostic. (Script: `scripts/ci/check_engine_no_editor.ps1`)

### Exit Criteria

- [ ] Running `cargo run -p kestrel_studio` launches a Studio app with:
  - [ ] A main window.
  - [ ] A viewport rendering a test scene using `kestrel_engine`.
- [ ] `kestrel_engine` can be built and used without pulling in Studio/editor dependencies.
- [ ] Existing game/demo code still compiles, with upgraded paths to use the new host/Studio structure.

---

## S1 — Project Model & Workspace UX

**Goal:** Move from “engine repo demo” to **project-centric** workflow.

**Suggested Issue Labels:**
- `area:studio`
- `area:projects`
- `type:feature`

### Tasks

- [ ] **Define `.kestrelproj` project format**
  - [x] Project name / ID.
  - [x] Asset root path(s).
  - [x] Default startup scene.
  - [ ] Enabled plugins, trust levels, capabilities.
  - [ ] Default build targets and configurations.

- [ ] **Start screen / project browser**
  - [ ] Implement Studio start screen with:
    - [ ] “New Project”
    - [ ] “Open Project”
    - [ ] “Recent Projects”
  - [ ] New project templates:
    - [ ] Empty 2D project.
    - [ ] Empty 3D project.
    - [ ] Minimal example project (optional).

- [ ] **Per-project configuration**
  - [x] Move `config/app.json` / `config/plugins.json` semantics into project-local equivalents.
  - [x] Ensure each project carries its own config instead of relying on engine repo defaults.

- [ ] **Workspace layout persistence**
  - [ ] Save/restore editor layout per project:
    - [ ] Docking arrangement.
    - [ ] Open panels.
    - [ ] Last-opened scene.

### Exit Criteria

- [ ] User can create a new Kestrel project from Studio.
- [ ] User can open an existing project from disk.
- [ ] Studio remembers:
  - [ ] Previously opened projects.
  - [ ] Per-project layout (open scenes, panels).
- [ ] All game-specific configuration is **project-local** (no hidden coupling to the engine repo).

---

## S2 — Scene Editor 2.0 (Hierarchy, Prefabs, Undo)

**Goal:** Upgrade the scene editor to a modern, “Unity-lite” experience with hierarchy, prefabs, and undo/redo.

**Suggested Issue Labels:**
- `area:editor`
- `area:scene`
- `type:feature`

### Tasks

- [ ] **Hierarchy panel improvements**
  - [ ] Tree view with parent/child relationships.
  - [ ] Drag-and-drop reparenting.
  - [ ] Entity renaming in-panel.
  - [ ] Multi-selection support.

- [x] **Prefab / blueprint system**
  - [x] Define `Prefab` asset format (e.g., `.kprefab`) representing ECS snapshots.
  - [x] Ability to instantiate prefabs into scenes.
  - [x] Basic workflow for:
    - [x] “Create prefab from selection.”
    - [x] “Apply changes to prefab” (for simple, non-destructive edits).

- [ ] **Undo/Redo infrastructure**
  - [ ] Introduce a generic command stack for editor actions:
    - [ ] Transform edits (position/rotation/scale).
    - [ ] Component add/remove.
    - [ ] Entity create/delete.
  - [ ] Wire all common editing actions through the command stack.

- [ ] **Multi-scene support**
  - [ ] Scene list panel.
  - [ ] Mark one scene as “startup scene” in project config.
  - [ ] Open multiple scenes across tabs (optional, nice-to-have).

- [ ] **Gizmos polish**
  - [ ] Snapping options:
    - [ ] Position (grid).
    - [ ] Rotation (angle increments).
    - [ ] Scale steps.
  - [ ] Local vs world-space gizmo mode.

### Exit Criteria

- [ ] User can:
  - [ ] Create, duplicate, and delete entities via hierarchy.
  - [ ] Parent/unparent entities via drag-and-drop.
  - [ ] Create and instantiate prefabs.
  - [ ] Undo/redo a series of common operations reliably.
- [ ] Multiple scenes can be stored and edited within a single project, with one configured as startup.

---

## S3 — Animation Suite (Sprite, Transform, Skeletal)

**Goal:** First-class **animation authoring** tools that match the runtime model: sprite, transform, and skeletal.

**Suggested Issue Labels:**
- `area:animation`
- `area:editor`
- `type:feature`

### Tasks

#### S3.1 Sprite Timeline Editor

- [x] Add a dedicated **Sprite Animation** panel:
  - [x] Timeline view of clips and frames.
  - [x] Per-frame duration editing.
  - [x] Loop modes: Loop, PingPong, OnceHold, OnceStop.
- [ ] Add event tracks:
  - [ ] Insert events at specific frames/times.
  - [ ] Event name + simple payload (string or JSON).
- [ ] Hook up preview:
  - [ ] Inline preview widget.
  - [ ] In-viewport preview on selected entity.

#### S3.2 Transform & Property Animation

- [x] Support authoring of `AnimationClip` for transforms:
  - [x] Keyframes for position, rotation, scale.
  - [x] Simple interpolation modes (step/linear; curves later).
- [ ] Timeline scrubbing:
  - [x] Scrub to a time and see scene update.
  - [ ] Optional “preview mode” where time is decoupled from the main simulation.

#### S3.3 Skeletal Animation Preview

- [ ] Skeleton inspector:
  - [ ] Visualize joint hierarchy.
  - [ ] Toggle joint visibility / selection.
- [ ] Clip preview:
  - [ ] Assign skeletal clip to a test mesh.
  - [ ] Play / pause / loop within a panel.
- [ ] Clip import settings:
  - [ ] UI for GLTF clip naming, ranges, scale.

#### S3.4 Animation Graph / State Machine (v1)

- [ ] Basic graph editor:
  - [ ] States (nodes) with assigned clips.
  - [ ] Transitions (edges) with simple conditions.
- [ ] Parameter UI:
  - [ ] Boolean / float parameters editable in inspector.
- [ ] Runtime hookup:
  - [ ] Use the animation graph runtime structures to drive characters in play mode.

### Exit Criteria

- [ ] User can:
  - [ ] Create a new sprite animation clip entirely in Studio and assign to an entity.
  - [ ] Author a transform animation (e.g., door opening) and preview it.
  - [ ] Import and preview a skeletal clip on a mesh.
- [ ] Animation assets saved by Studio are fully consumable by the engine runtime with no manual editing.

---

## S4 — Profiling, Analytics & ECS Debugging

**Goal:** Provide an integrated view of performance, ECS state, and plugin health directly inside Studio.

**Suggested Issue Labels:**
- `area:analytics`
- `area:ecs`
- `type:feature`

### Tasks

- [x] **Profiler panel**
  - [x] Frame-time history graph.
  - [x] Breakdown chart (per system or per system-group).
  - [x] Ability to pin a system and watch its cost over time.

- [ ] **ECS inspector**
  - [ ] Entity search:
    - [ ] By entity name/ID.
    - [ ] By component type.
  - [ ] Component view:
    - [ ] For a selected component type, show list of entities and key fields.
  - [ ] Optional: highlight selected entities in viewport.

- [ ] **System graph view (basic)**
  - [ ] Visualize system execution order.
  - [ ] Show which systems run in which phase (update, late_update, render, etc.).

- [x] **Plugin health UI**
  - [x] Per-plugin:
    - [x] Error counters.
    - [x] Capability violations.
    - [ ] Optional: rough CPU time or event counts.
  - [ ] Controls to disable/enable plugins at edit-time for debugging.

### Exit Criteria

- [ ] Performance hotspots can be identified from within Studio without external tooling.
- [ ] ECS state can be inspected in a targeted way (by component, by entity).
- [ ] Misbehaving plugins are visible (via counters/health indicators) without reading raw logs.

---

## S5 — Plugin Ecosystem & Editor Extensions

**Goal:** Make Kestrel Studio itself **extensible** via plugins, not just the runtime.

**Suggested Issue Labels:**
- `area:plugins`
- `area:studio`
- `type:feature`

### Tasks

- [ ] **Editor extension API**
  - [ ] Allow plugins to:
    - [ ] Register custom inspector widgets for specific component types.
    - [ ] Register custom panels/windows.
    - [ ] Register custom overlays in the main viewport.

- [x] **Plugin Manager panel**
  - [x] List all discovered plugins from project config.
  - [x] Show:
    - [x] Plugin name, description, version.
    - [x] Declared capabilities.
    - [x] Trust level (e.g., `Full`, `Isolated`).
  - [x] Allow per-project enable/disable toggles.

- [x] **Isolated plugin host (optional, advanced)**
  - [x] Implement `kestrel_plugin_host` helper process for `trust = "isolated"`.
  - [x] IPC to:
    - [x] Send events / ECS snapshots.
    - [x] Receive plugin outputs / commands.
  - [ ] Add controls to terminate/restart isolated plugins without crashing Studio.

### Exit Criteria

- [ ] A third-party plugin can:
  - [ ] Add a new component type + custom inspector.
  - [ ] Add a custom editor panel (e.g., Dialogue Graph Editor).
- [ ] Plugin capabilities and trust levels are visible and adjustable within Studio.
- [ ] (Optional) Isolated plugins can crash or misbehave without taking down the whole Studio process.

---

## S6 — Build & Distribution Pipeline

**Goal:** Ship games from Studio with minimal external tooling.

**Suggested Issue Labels:**
- `area:build`
- `area:distribution`
- `type:feature`

### Tasks

- [ ] **Build configuration panel**
  - [ ] Per-project build settings:
    - [ ] Targets (initially: Windows x64, Linux x64).
    - [ ] Build type: Debug / Release.
    - [ ] Output directory.
    - [ ] Asset compression options.

- [ ] **Build & Run integration**
  - [ ] “Build Debug & Run” action:
    - [ ] Runs the appropriate build command.
    - [ ] Launches the resulting game binary.
  - [ ] “Build Release” action:
    - [ ] Builds release assets and binary without running it.
  - [ ] Console view of build logs and errors.

- [ ] **Packaging**
  - [ ] Produce a distributable folder/zip with:
    - [ ] Game executable.
    - [ ] Required assets.
    - [ ] Config and any runtime dependencies.
  - [ ] Optional: helper for itch.io upload or simple installer script.

### Exit Criteria

- [ ] From within Studio, a user can:
  - [ ] Configure build settings.
  - [ ] Build and run the game in Debug mode.
  - [ ] Build a Release package that can be zipped and shared with players.

---

## S7 — UX Polish, Onboarding & Documentation

**Goal:** Make Kestrel Studio feel like a polished product rather than an internal tool.

**Suggested Issue Labels:**
- `area:ux`
- `area:docs`
- `type:polish`

### Tasks

- [ ] **Onboarding**
  - [ ] First-run “tour” overlay:
    - [ ] Highlight viewport, hierarchy, inspector, play controls.
  - [ ] Sample project(s) bundled:
    - [ ] Tiny 2D game (simple character, animations, input).
    - [ ] Tiny 3D scene (mesh, camera, lighting).

- [ ] **Visual consistency**
  - [ ] Standardize spacing, typography, and iconography across panels.
  - [ ] Ensure labels and toolbars feel coherent.

- [ ] **Contextual help**
  - [ ] “?” button on major panels linking to relevant docs.
  - [ ] Tooltips for non-obvious fields (especially animation & plugin settings).

- [ ] **Documentation**
  - [ ] “Getting Started with Kestrel Studio”:
    - [ ] Install → New Project → Import sprite → Create animation → Build.
  - [ ] “3D Basics in Kestrel Studio.”
  - [ ] “Writing a Plugin for Kestrel Studio.”
  - [ ] Keep docs versioned and tied to Studio releases.

### Exit Criteria

- [ ] A new, non-engine user can:
  - [ ] Install Kestrel Studio.
  - [ ] Follow docs/onboarding to create a small 2D or 3D prototype.
  - [ ] Build and share that prototype without touching engine internals.
- [ ] Studio UI feels visually unified and discoverable.

---

## Notes on Scope & Versions

- **Minimum for “Kestrel Studio 1.0”** (public-ish):
  - S0 — Engine/Studio separation
  - S1 — Project model
  - S2 — Scene Editor 2.0
  - S3 — Sprite + basic transform animation tools
  - S4 — Basic profiler + ECS inspector
  - S6 — Build & distribution (basic packaging)

- **Ideal for “Kestrel Studio 1.1+” (post-1.0 polish):**
  - Advanced animation graph tooling (S3.4).
  - Full plugin host isolation (S5 advanced).
  - Full onboarding/tutorial experience (S7).

Track these milestones alongside existing engine milestones to keep runtime and Studio evolving together.
