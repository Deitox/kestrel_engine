# Kestrel Engine - Milestone 7

**Scripting layer with hot reload**

## New
- Embedded Rhai runtime automatically loads `assets/scripts/main.rhai` and hot-reloads on file save.
- Scripts can spawn, move, and despawn ECS entities through `world.spawn_sprite`, `set_velocity`, `set_position`, and `despawn`.
- Script API exposes logging and random helpers for quick prototyping.
- Debug UI surfaces script status with enable toggle, manual reload, and inline error reporting.

## Still here
- egui overlay shows camera status, cursor world position, and selection details.
- Right mouse drag pans the camera; mouse wheel zooms with clamped limits.
- Selection gizmo highlights the chosen entity and supports deletion from the UI.

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
