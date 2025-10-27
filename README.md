# Kestrel Engine - Milestone 13

**Plugin system, scene/editor maturity, and a 2D/3D toolchain ready for extension**

## Highlights
- Hybrid transform graph - 2D sprites and 3D meshes share the same parent/child hierarchy so scene parenting stays consistent across spaces. A unified transform propagator keeps world matrices in sync for both billboards and meshes.
- Scene dependency tracker - Scene exports now record atlas and mesh requirements, and runtime reference counting retains and releases those assets automatically when scenes load or unload.
- Mesh metadata - Mesh entities carry material identifiers plus lighting flags (cast/receive shadows, emissive tint). The scene format and round-trip loader persist that data, paving the way for the Milestone 12 PBR work.
- HDR environment lighting - Load equirectangular HDR maps to drive diffuse irradiance, specular reflections, and a BRDF LUT so materials react to image-based lighting alongside the directional key light.
- Camera tooling - The mesh preview offers three modes (Disabled, Orbit, Free-fly). Free-fly introduces WASD/QE + Shift navigation with mouse look and roll, while orbit mode remains handy for turntable inspection.
- Perspective viewport editing - Ray-based picking, translate/rotate/scale gizmos, and a frame-selection helper keep mesh workflows aligned with the inspector.
- Plugin system - The new `EnginePlugin` trait, feature registry, and manifest-driven loader let subsystems (audio, scripting, analytics, future tooling) hook into init/update/fixed/event stages without modifying the core loop, paving the way for third-party extensions.
- Scene toolbar upgrades - Quick path history, dependency health readouts, and one-click retain buttons make Save/Load workflows safer.
- Scene I/O guardrails - Mesh-aware helpers (save_scene_to_path_with_mesh_source, load_scene_with_mesh) ensure custom assets keep their source paths and metadata during save/load workflows.
- Particle telemetry - The Stats panel now surfaces particle budget metrics (active count, spawn budget, emitter backlog) so runaway emitters are obvious without diving into the ECS.

## Core Systems
- Physics - Rapier2D simulates rigid bodies. ECS components (Transform, Velocity, RapierBody, RapierCollider) mirror state back into the world every fixed step.
- Rendering - A WGPU renderer performs depth-tested mesh draws, batched sprite passes, and egui compositing inside a single swapchain frame.
- Scripting - Rhai scripts hot-reload, queue gameplay commands, and surface log output through the debug UI.
- Assets - The asset manager loads texture atlases on demand, while the mesh registry keeps CPU/GPU copies of glTF data and now reference-counts scene dependencies so unused assets are released automatically.
- Audio - Lightweight rodio-backed cues highlight spawn/despawn/collision events.
- Scene management - JSON scenes capture the full entity graph (including materials/lighting) and can be saved/loaded from the UI or tests.

## Controls
- Space - spawn the configured burst count (remappable via `config/input.json`)
- B - spawn 5x as many sprites (minimum 1000)
- Right Mouse - pan the 2D camera (Disabled) / orbit preview (Orbit) / look around (Free-fly)
- Mouse Wheel - zoom the 2D camera (Disabled) / adjust orbit radius (Orbit) / tune fly speed or focus distance (Free-fly)
- M - cycle mesh preview camera mode (Disabled -> Orbit -> Free-fly)
- W, A, S, D, Q, E - move the preview camera in Free-fly
- Z, C - roll the preview camera in Free-fly
- L - toggle frustum lock for the preview camera
- Shift - boost movement speed in Free-fly
- Esc - quit

## Script Debugger & REPL
- Open the **Stats → Scripts** section inside the left panel to toggle scripting, pause updates, step once while paused, or hot-reload the active Rhai file. Click **Open debugger** from that section (or press the same button inside the Scripts window) to pop out the dedicated console.
- The debugger window shows a scrollback console that mixes script logs, REPL input/output, and runtime errors. Use **Clear Console** to reset the log without touching the underlying script state.
- Type Rhai commands into the REPL field and press **Enter** or **Run**; commands execute against the live `World` just like the main script, so you can tweak emitters, spawn sprites, or inspect state at runtime.
- Arrow keys cycle through command history, and the History list lets you click to rehydrate older commands for editing. The input box auto-focuses whenever a script error occurs so you can fix issues quickly.
- Errors that occur during REPL execution or regular script updates automatically reopen the debugger and highlight the failure, keeping the workflow tight during iteration.

## Build
`
cargo run
`

## Plugins
- `pwsh scripts/build_plugins.ps1 [-Release]` builds every enabled entry from `config/plugins.json` by inferring the crate root from each artifact path.
- After rebuilding a plugin, open the Plugins panel in-app and click “Reload plugins” to rescan the manifest without restarting.

## Configuration
- Edit config/app.json to tweak window title, resolution, vsync, or fullscreen defaults.
- Override width/height/vsync from the CLI with `kestrel_engine --width 1920 --height 1080 --vsync off` (CLI overrides take precedence over config/app.json, which takes precedence over built-in defaults).
- Remap keyboard input by editing config/input.json (missing or invalid entries fall back to the built-in bindings with warnings).
- Toggle dynamic plugins via config/plugins.json (paths are resolved relative to that file; set `enabled` per entry).
- Disable built-in plugins by listing their names in `config/plugins.json` → `disable_builtins`.
- The engine falls back to built-in defaults and logs a warning if the file is missing or malformed.
- If a dynamic plugin’s path is missing or invalid, the loader logs it and automatically marks it disabled (the app will proceed without crashing).

## Documentation
- docs/ARCHITECTURE.md - subsystem responsibilities, frame flow, and notes on the hybrid transform pipeline.
- docs/DECISIONS.md - crate and technology choices (e.g., winit, wgpu, gltf, Rapier).
- docs/CODE_STYLE.md - formatting, linting, and error-handling guidelines.
- docs/PLUGINS.md - dynamic plugin manifest format, feature registry rules, and an example cdylib plugin.

