# Kestrel Engine - Milestone 8

**Rapier-driven physics with resilient particles**

## New
- Rapier2D now drives rigid body motion: sprites carry collider/rigid-body handles that live in the ECS alongside `Transform`/`Velocity`.
- Static boundary colliders keep bursts and scripted spawns inside the arena with restitution and friction authored in one place.
- Script and UI helpers (`set_velocity`, `set_position`, spawn burst) all push updates through Rapier, so the physics state stays authoritative even after hot-reloads.
- Demo scene and random bursts automatically attach dynamic colliders while the existing particle emitter keeps using the lightweight force integrator for thousands of billboards.

## Still here
- Hot-reloadable Rhai scripting with emitter controls, spawn automation, and script-driven entity management.
- egui overlay shows camera status, cursor world position, selection info, and exposes particle + spawn tuning.
- Camera pan/zoom (RMB + wheel), selection gizmo with deletion, and deterministic fixed-step integration for particles.
- Lightweight particle emitter with color/size gradients and lifetime control for quick visual iteration.

## Controls
- Space - spawn N sprites (configurable)
- B - spawn 5xN (>=1000)
- Right mouse - pan camera
- Mouse wheel - zoom camera
- Esc - quit

## Build
```bash
cargo run
```

## Configuration
- Edit `config/app.json` to tweak window title, resolution, vsync, or fullscreen defaults.
- The engine falls back to built-in defaults and logs a warning if the file is missing or malformed.

## Docs
- `docs/ARCHITECTURE.md` outlines subsystem responsibilities and the frame flow.
- `docs/DECISIONS.md` records crate and technology choices.
- `docs/CODE_STYLE.md` captures formatting, linting, and error-handling guidelines.
