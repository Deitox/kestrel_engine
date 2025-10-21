# Kestrel Engine - Milestone 8+

**Rapier-driven physics with resilient particles and an emerging 3D toolchain**

## New
- Rapier2D now drives rigid body motion: sprites carry collider/rigid-body handles that live in the ECS alongside `Transform`/`Velocity`.
- Static boundary colliders keep bursts and scripted spawns inside the arena with restitution and friction authored in one place.
- Script and UI helpers (`set_velocity`, `set_position`, spawn burst) all push updates through Rapier, so the physics state stays authoritative even after hot-reloads.
- Demo scene and random bursts automatically attach dynamic colliders while the existing particle emitter keeps using the lightweight force integrator for thousands of billboards.
- Debug UI now offers scene quick-save/load, serializing the active entity hierarchy to JSON to bootstrap Milestone 10.
- Inspect and tweak the selected entity directly in the debug UI (position, rotation, scale, sprite region, tint).
- Event-driven audio cues now play synthesized beeps via rodio whenever spawn/despawn/collision events fire.
- Experimental mesh pipeline: the renderer now manages a depth buffer, a dedicated mesh shader, and a per-frame mesh pass that draws both the preview object and ECS-authored mesh entities before sprites.
- Mesh registry with glTF ingestion keeps CPU/GPU copies of reusable meshes, tracks dependencies for scene serialization, and exposes them through the editor UI.
- 3D preview controls live in the right-hand panel: pick a mesh asset, toggle orbit navigation, reset the camera, or spawn mesh-backed entities directly into the scene.

## Still here
- Hot-reloadable Rhai scripting with emitter controls, spawn automation, and script-driven entity management.
- egui overlay shows camera status, cursor world position, selection info, and exposes particle + spawn tuning.
- Camera pan/zoom (RMB + wheel), selection gizmo with deletion, and deterministic fixed-step integration for particles.
- Lightweight particle emitter with color/size gradients and lifetime control for quick visual iteration.
- Sprite batching continues to render thousands of billboards efficiently while the mesh pass executes in the same frame, keeping 2D and 3D content synchronized.

## Controls
- Space - spawn N sprites (configurable)
- B - spawn 5xN (>=1000)
- Right mouse - pan 2D camera (when mesh orbit control is disabled) / orbit the preview camera (when enabled)
- Mouse wheel - zoom (2D camera or mesh orbit depending on mode)
- M - toggle mesh preview orbit control
- Esc - quit

## Build
```bash
cargo run
```

## Configuration
- Edit `config/app.json` to tweak window title, resolution, vsync, or fullscreen defaults.
- The engine falls back to built-in defaults and logs a warning if the file is missing or malformed.

## Docs
- `docs/ARCHITECTURE.md` outlines subsystem responsibilities, now including the mesh registry and dual-pass renderer.
- `docs/DECISIONS.md` records crate and technology choices, including the `gltf` importer powering mesh assets.
- `docs/CODE_STYLE.md` captures formatting, linting, and error-handling guidelines.

