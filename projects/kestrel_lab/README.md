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

Shoot the moving blue targets. When all targets are destroyed, the next wave spawns after a short delay and ramps up in size.

## Scripted Engine Tests

- Events: bullets emit `target_destroyed`, main script listens and drives scoring + hit FX.
- Timers: wave respawn uses `timer_start/timer_fired`, with a `next_wave` countdown stat.
