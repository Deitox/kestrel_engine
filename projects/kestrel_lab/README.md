# Kestrel Lab

Clean, minimal sandbox project used to validate engine + scripting workflows without relying on legacy gamekit scripts.

## Run

From the repo root:

`cargo run -p kestrel_studio -- --project projects/kestrel_lab/project.kestrelproj`

## Controls (default bindings)

- Move: `WASD`
- Boost: `Shift`
- Shoot: Left mouse (aim with cursor; falls back to last move direction)

## Goal

Shoot the moving blue targets. When all targets are destroyed, a new wave spawns.
