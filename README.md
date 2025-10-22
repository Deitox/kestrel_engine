# Kestrel Engine - Milestone 10

**Scene serialization with dependency tracking, asset lifecycle management, and a maturing 3D toolchain**

## Highlights
- Hybrid transform graph - 2D sprites and 3D meshes share the same parent/child hierarchy so scene parenting stays consistent across spaces. A unified transform propagator keeps world matrices in sync for both billboards and meshes.
- Scene dependency tracker - Scene exports now record atlas and mesh requirements, and runtime reference counting retains and releases those assets automatically when scenes load or unload.
- Mesh metadata - Mesh entities carry material identifiers plus lighting flags (cast/receive shadows, emissive tint). The scene format and round-trip loader persist that data, paving the way for the Milestone 12 PBR work.
- Camera tooling - The mesh preview offers three modes (Disabled, Orbit, Free-fly). Free-fly introduces WASD/QE + Shift navigation with mouse look and roll, while orbit mode remains handy for turntable inspection.
- Scene I/O guardrails - Mesh-aware helpers (save_scene_to_path_with_mesh_source, load_scene_with_mesh) ensure custom assets keep their source paths and metadata during save/load workflows.

## Core Systems
- Physics - Rapier2D simulates rigid bodies. ECS components (Transform, Velocity, RapierBody, RapierCollider) mirror state back into the world every fixed step.
- Rendering - A WGPU renderer performs depth-tested mesh draws, batched sprite passes, and egui compositing inside a single swapchain frame.
- Scripting - Rhai scripts hot-reload, queue gameplay commands, and surface log output through the debug UI.
- Assets - The asset manager loads texture atlases on demand, while the mesh registry keeps CPU/GPU copies of glTF data and now reference-counts scene dependencies so unused assets are released automatically.
- Audio - Lightweight rodio-backed cues highlight spawn/despawn/collision events.
- Scene management - JSON scenes capture the full entity graph (including materials/lighting) and can be saved/loaded from the UI or tests.

## Controls
- Space - spawn the configured burst count
- B - spawn 5x as many sprites (minimum 1000)
- Right Mouse - pan the 2D camera (Disabled) / orbit preview (Orbit) / look around (Free-fly)
- Mouse Wheel - zoom the 2D camera (Disabled) / adjust orbit radius (Orbit) / tune fly speed or focus distance (Free-fly)
- M - cycle mesh preview camera mode (Disabled -> Orbit -> Free-fly)
- W, A, S, D, Q, E - move the preview camera in Free-fly
- Z, C - roll the preview camera in Free-fly
- L - toggle frustum lock for the preview camera
- Shift - boost movement speed in Free-fly
- Esc - quit

## Build
`
cargo run
`

## Configuration
- Edit config/app.json to tweak window title, resolution, vsync, or fullscreen defaults.
- The engine falls back to built-in defaults and logs a warning if the file is missing or malformed.

## Documentation
- docs/ARCHITECTURE.md - subsystem responsibilities, frame flow, and notes on the hybrid transform pipeline.
- docs/DECISIONS.md - crate and technology choices (e.g., winit, wgpu, gltf, Rapier).
- docs/CODE_STYLE.md - formatting, linting, and error-handling guidelines.

