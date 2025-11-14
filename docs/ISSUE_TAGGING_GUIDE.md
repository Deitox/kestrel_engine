# Issue Tagging Guide

This guide defines a **lightweight, consistent labeling scheme** for issues and PRs across:

- `kestrel_engine` (runtime)
- `kestrel_studio` (editor / tools)
- Shared docs, build, and infra work

The goal: make it easy to see **what** an issue touches, **what kind** of work it is, and **how urgent** it is.

---

## 1. Label Categories

Each issue/PR should generally have:

- **1–3 `area:*` labels** (what subsystem / domain is affected)
- **1 `type:*` label** (what kind of work it is)
- **0–1 `priority:*` labels** (how time-sensitive it is)
- Optional **`milestone:*` label(s)** if it’s tied to a specific milestone

Try to avoid over-labeling; use the smallest set that still makes sense.

---

## 2. `area:*` Labels (What it touches)

These describe *where* in the engine/studio the work lives.  
Pick 1–3 that best fit.

### Core Runtime / Engine

- `area:engine`  
  General engine runtime work that doesn’t fit a more specific area below.

- `area:ecs`  
  Entity-component-system core: archetypes, storage, queries, scheduling, cursors.

- `area:rendering`  
  Renderer, GPU pipelines, materials, mesh rendering, sprite batching, HDR, etc.

- `area:animation`  
  Sprite timelines, transform clips, skeletal clips, animation graphs, timelines.

- `area:physics`  
  Physics integration, collision, spatial partitioning, queries, determinism.

- `area:audio`  
  Audio playback, mixer, buffers, audio health / diagnostics.

- `area:assets`  
  Asset loading, caching, lifetime management, GLTF import, atlases, resource formats.

- `area:input`  
  Keyboard/mouse/gamepad input, input mapping, event plumbing.

- `area:scripts`  
  Game scripting layers (Lua, Rust scripting, or other script integration).

- `area:time`  
  Clocks, fixed-step logic, timers, delta-time handling.

- `area:events`  
  Game/event bus, messaging between systems, analytics events.

### Studio / Editor

- `area:studio`  
  Cross-cutting Studio/editor work (application shell, main window, docking layout).

- `area:editor`  
  Scene editor UX: viewport, gizmos, inspectors, hierarchy interactions.

- `area:projects`  
  Project model: `.kestrelproj`, project templates, recent projects, workspace management.

- `area:scene`  
  Scene data model and tools: scene graphs, multi-scene support, save/load.

- `area:prefabs`  
  Prefab/blueprint systems, prefab authoring, prefab-instance relationships.

- `area:hierarchy`  
  Hierarchy tree, parenting/unparenting, entity naming and organization.

- `area:animation-editor`  
  Animation authoring tools: sprite timeline editor, transform tracks, skeletal preview, animation state machines.

- `area:analytics`  
  Profiling, metrics, frame-time graphs, analytics panels, debug overlays.

- `area:ecs-inspector`  
  ECS inspection in the editor: entity/component browsers, system graphs.

- `area:plugins`  
  Plugin system, capability model, plugin manifest parsing, plugin host / sandbox.

- `area:build`  
  Build pipeline from Studio: build configs, Build & Run, packaging.

- `area:distribution`  
  Output packaging, installers, upload helpers (itch.io, etc.).

- `area:ux`  
  General UX polish: layout, shortcuts, icons, menu structure, onboarding UX.

- `area:docs`  
  Documentation, tutorials, samples, API docs.

### Cross-Cutting / Infrastructure

- `area:architecture`  
  High-level design changes, refactors across multiple subsystems, slicing engine vs Studio.

- `area:testing`  
  Unit tests, integration tests, snapshot tests, test infrastructure.

- `area:ci`  
  Continuous integration, pipelines, linting, formatting, automation.

- `area:tooling`  
  Dev tools, scripts, benchmarks, helper binaries not part of Studio itself.

---

## 3. `type:*` Labels (What kind of work it is)

Pick **one** primary type per issue if possible.

- `type:feature`  
  New functionality or a major enhancement (e.g., “Sprite animation editor panel”).

- `type:improvement`  
  Upgrades to existing behavior without changing the fundamental capabilities  
  (e.g., “Better gizmo snapping,” “Cleaner inspector layout”).

- `type:bug`  
  Something is broken or behaves incorrectly relative to design / expectations.

- `type:regression`  
  Something used to work and now doesn’t (should often get higher priority).

- `type:refactor`  
  Change to internal structure/organization without intended behavior change.

- `type:performance`  
  Perf work: optimizing hot loops, reducing allocations, frame-time improvements.

- `type:polish`  
  Visual/UX polish, copy tweaks, small quality-of-life improvements.

- `type:docs`  
  Documentation tasks: new docs, updates, samples, diagrams.

- `type:infra`  
  CI, build scripts, infra glue, not directly user-visible features.

---

## 4. `priority:*` Labels (How urgent)

Use sparingly; most issues can omit a priority until they’re being actively triaged.

- `priority:urgent`  
  Needs attention immediately:
  - Build breaks,
  - Data loss,
  - Showstopper regressions,
  - Blocks a release.

- `priority:high`  
  Should be done soon:
  - Clears major obstacles for Studio 1.0,
  - Fixes severe but non-catastrophic bugs.

- `priority:medium`  
  Default for important work that isn’t blocking:
  - Core roadmap items for upcoming minor releases.

- `priority:low`  
  Nice-to-haves, polish, experiments, backlog items.

---

## 5. `milestone:*` Labels (Which roadmap milestone)

Use one of these when an issue clearly ties into Studio roadmap milestones (`docs/STUDIO_ROADMAP.md`) or engine milestones.

### Studio milestones

- `milestone:studio-s0` — Engine/Studio separation  
- `milestone:studio-s1` — Project model & workspace UX  
- `milestone:studio-s2` — Scene editor 2.0 (hierarchy, prefabs, undo)  
- `milestone:studio-s3` — Animation suite (sprite, transform, skeletal)  
- `milestone:studio-s4` — Profiling, analytics & ECS debugging  
- `milestone:studio-s5` — Plugin ecosystem & editor extensions  
- `milestone:studio-s6` — Build & distribution pipeline  
- `milestone:studio-s7` — UX polish, onboarding & docs  

### Engine milestones (if desired)

If you want to mirror engine milestones, you can add:

- `milestone:engine-m0` through `milestone:engine-m13`  

…but those can be optional if you’re already tracking engine progress elsewhere.

---

## 6. Tagging Examples

### Example 1 — Sprite animation editor panel

> “Add sprite animation timeline editor with frame-based editing and loop modes.”

Suggested labels:

- `area:animation`
- `area:animation-editor`
- `area:editor`
- `type:feature`
- `milestone:studio-s3`
- (priority as appropriate, e.g. `priority:medium`)

---

### Example 2 — ECS picker crashes in 3D viewport

> “Selecting entities in 3D sometimes panics when no mesh is under cursor.”

Suggested labels:

- `area:rendering`
- `area:ecs`
- `area:editor`
- `type:bug`
- Possibly `milestone:studio-s2` if tied to scene editor stability.
- `priority:high` if reproducible and annoying enough.

---

### Example 3 — Project start screen

> “Implement New/Open/Recent project start screen, wired to `.kestrelproj` files.”

Suggested labels:

- `area:studio`
- `area:projects`
- `type:feature`
- `milestone:studio-s1`

---

### Example 4 — Animation runtime micro-optimization

> “Reduce branch mispredicts in sprite timeline advance loop.”

Suggested labels:

- `area:animation`
- `area:engine`
- `type:performance`

If it directly supports a benchmark target:

- `priority:medium` or `priority:high` depending on how tight the perf budget is.

---

## 7. Tagging Guidelines

1. **Prefer clarity over perfection.**  
   It’s better to have “mostly right” labels now than perfect labels never.

2. **Keep it small.**  
   - 1–3 `area:*` labels  
   - 1 `type:*` label  
   - 0–1 `priority:*` labels  
   - 0–1 `milestone:*` labels

3. **Use `area:architecture` for broad or cross-cutting refactors.**  
   If something touches multiple subsystems, pair `area:architecture` with the most affected concrete area (e.g., `area:ecs`).

4. **Upgrade `type:bug` → `type:regression` if it used to work.**  
   That often deserves a higher priority.

5. **Add or adjust `milestone:*` labels as the roadmap evolves.**  
   When goals for a milestone solidify (e.g., “this must land before Studio 1.0”), add the matching `milestone:*` tag.

---

## 8. Quick Checklist for New Issues

When creating an issue:

1. **What does it touch?**  
   → Add 1–3 `area:*` labels.

2. **What kind of work is it?**  
   → Add 1 `type:*` label.

3. **Does it obviously belong to a roadmap milestone?**  
   → Add a `milestone:*` label if yes.

4. **Is there a clear time pressure?**  
   → Add a `priority:*` label if it’s urgent/high; otherwise leave it untagged or `priority:medium` if you’re actively planning it.

This should keep the issue list readable and make it much easier for coding agents (and Future You) to filter work by subsystem, type, or milestone.
