# Kestrel Engine — Milestone 5

**In-window debug UI (egui) + perf controls**

## New
- **egui overlay** with:
  - Entity count & instances drawn
  - Sliders for **spatial cell size**, **spawn per press**, **auto-spawn rate**
  - Button to spawn immediately
  - **Frame-time histogram** (ms) with 240-sample history
- Keeps **spatial-hash collisions**, **persistent instance buffer**, and spawners from 3.5.

## Controls
- **Space** — spawn N sprites (configurable)
- **B** — spawn 5×N (≥1000)
- **Mouse wheel** — adjust root spin
- **Esc** — quit

## Build
```bash
cargo run
```
