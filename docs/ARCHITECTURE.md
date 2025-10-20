# Architecture Overview

```
+---------------+    input events    +-------------+    fixed update    +-----------+
|     Winit     | -----------------> |    Input    | -----------------> |    ECS    |
+-------+-------+                    +------+------+                    +-----+-----+
        |                                   |                                |
        | window/device events              | entity data                    | render data
        v                                   v                                v
+-------+-------+    timing info    +------+------+    egui I/O         +-----------+
|      Time     | -----------------> |     App      | -----------------> | Renderer  |
+---------------+                    +-------------+                    +-----------+
                                             |
                                             v
                                       +-----------+
                                       |    egui   |
                                       +-----------+
```

- `src/lib.rs` drives Winit's `EventLoop`, advances the simulation, runs scripts, renders, and feeds egui.
- `src/input.rs` accumulates keyboard and mouse state for the current frame.
- `src/time.rs` tracks elapsed time and maintains the fixed 60 Hz timestep.
- `src/ecs.rs` hosts the Bevy ECS world: sprites, transforms, Rapier physics, particle emitters, and utility resources.
- `src/renderer.rs` owns WGPU device and swapchain setup, sprite batching, and egui rendering.
- `src/assets.rs` lazily loads texture atlases and exposes UV lookups to the ECS.
- `src/camera.rs` implements the 2D camera with pan and zoom helpers.
- `src/config.rs` loads `config/app.json` and hands window defaults to the renderer.
- `src/scripts.rs` embeds Rhai, hot-reloads scripts, queues gameplay commands for the app to apply, and captures script log messages.
- `src/events.rs` defines `GameEvent` plus the `EventBus` resource that records gameplay signals for tooling and audio.
- `src/scene.rs` describes the JSON scene format, tracks asset dependencies, and handles serialization/deserialization of entity hierarchies for save/load operations.
- `src/audio.rs` contains `AudioManager`, which uses `rodio` to emit lightweight synthesized cues in response to `GameEvent`s.

### Frame Flow
1. **Input ingest** - `ApplicationHandler::window_event` converts Winit events into `InputEvent` values, storing them on `Input`.
2. **Camera controls** - `App::about_to_wait` applies zoom and pan so the view matrix matches player input before simulation.
3. **Scripting** - `ScriptHost::update` reloads Rhai scripts, queues commands, and the app drains those commands before the fixed step.
4. **Physics and simulation** - Rapier advances rigid bodies at the fixed timestep, ECS mirrors poses back into transforms, and the particle integrator runs while systems emit `GameEvent` entries (including collision hits and script messages).
5. **Rendering prep** - ECS collects sprite instances, builds GPU buffers, and the camera produces the view-projection matrix.
6. **Rendering** - `Renderer::render_batch` submits draw calls, egui consumes input and produces overlay meshes, and the frame is presented.
7. **UI feedback** - egui surfaces frame time, spawn controls, emitter tuning, camera details, selection status, and script toggles.

### Module Relationships
- `App` owns `Renderer`, `EcsWorld`, `Input`, `Camera2D`, `AssetManager`, `Time`, and `ScriptHost`.
- `Renderer` consults `WindowConfig` (from `config.rs`) for swapchain setup.
- `EcsWorld` uses `AssetManager` to resolve atlas regions when building instance buffers.
- `Camera2D` depends on window size data supplied by `Renderer`.
- `ScriptHost` issues commands back into `App`, which resolves script handles to ECS entities.
- `RapierState` lives inside `EcsWorld` and synchronizes rigid-body data each fixed tick.
- `EventBus` is stored as an ECS resource so systems can push `GameEvent` values that the app drains after each frame.
- `AudioManager` listens to drained `GameEvent`s so tooling can preview which sounds would fire for spawns, despawns, collisions, or script-driven cues while also playing the corresponding rodio tone when audio is available.
- `Scene` helpers let the app export/import entity graphs; the debug UI exposes quick-save/quick-load controls that hand JSON files to these helpers.

The data always flows in the same order - Input -> ECS -> Renderer -> UI - keeping subsystems decoupled and deterministic.

### Scripting Guidelines
- Declare `global name;` inside functions before mutating module-level state so Rhai updates the shared variable rather than shadowing it.
- `world.spawn_sprite` returns a negative handle until the engine materializes the entity; use that handle with other `world.*` calls and the app will resolve it when commands are applied.
- Scripts can override debug UI settings such as spawn counts or auto spawn rate via `set_spawn_per_press` and `set_auto_spawn_rate`.
- Use the emitter helpers (`set_emitter_rate`, `*_spread`, `*_speed`, `*_lifetime`, `*_start_color`, `*_end_color`, `*_start_size`, `*_end_size`) to tweak the particle system at runtime.
