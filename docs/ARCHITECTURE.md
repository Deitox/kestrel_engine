# Architecture Overview

```
┌────────┐   input events   ┌────────────┐   fixed update   ┌─────────┐
│ Winit  │ ───────────────► │   Input    │ ───────────────► │  ECS    │
└────────┘                  └────────────┘                  └─────────┘
      ▲                           │   ▲                          │
      │ window resize             │   │ entity data              │
      │ frame timing              ▼   │                          ▼
┌────────────┐        timing   ┌────────┐    render data   ┌──────────┐
│    Time    │ ───────────────►│  Time  │ ───────────────► │ Renderer │
└────────────┘                 └────────┘                 └──────────┘
                                           ▲
                                           │ egui UI
                                      ┌─────────┐
                                      │  egui   │
                                      └─────────┘
```

- **Winit** drives the event loop (`src/lib.rs`) and feeds window/device events into the input system.
- **Input** (`src/input.rs`) accumulates per-frame keyboard and mouse state used by the simulation and camera.
- **Time** (`src/time.rs`) tracks elapsed and delta durations used for the fixed (60 Hz) and variable update paths.
- **ECS** (`src/ecs.rs`) stores game state using Bevy ECS: transform hierarchy, sprites, velocities, spatial hash, and collision systems.
- **Renderer** (`src/renderer.rs`) owns WGPU resources, sprite batching, and egui rendering.
- **AssetManager** (`src/assets.rs`) lazily loads texture atlases and provides UV lookup for ECS sprites.
- **Camera2D** (`src/camera.rs`) converts between screen and world coordinates and exposes pan/zoom controls.
- **Config** (`src/config.rs`) loads user configuration and feeds initial window settings into the renderer.
- **Physics** (`src/ecs.rs`) defines global parameters (gravity, damping) and systems that integrate forces/mass each fixed step.
- **ScriptHost** (`src/scripts.rs`) embeds the Rhai runtime, hot-reloads scripts, and queues gameplay commands (spawn/mutate/despawn/tweaks) that the app applies after each script tick.
- **App** (`src/lib.rs`) coordinates the subsystems: processes input, runs fixed/variable ECS schedules, executes scripts, renders sprites, and builds the egui debug UI.

### Frame Flow
1. **Input ingest** - `ApplicationHandler::window_event` converts Winit events into `InputEvent`s. `device_event` tracks raw mouse motion, and `about_to_wait` reads consumed events.
2. **Camera controls** - `App::about_to_wait` applies zoom/pan before simulation, ensuring the view-projection matrix reflects user intent.
3. **Scripting** - `ScriptHost::update` hot-reloads Rhai scripts, queues commands, and the app drains those commands before simulation so mutations stay deterministic.
4. **Physics & Simulation** - The fixed timestep applies gravity/forces, integrates positions, handles world bounds, and resolves collisions and particle lifetime.
5. **Rendering prep** - ECS collects instanced sprite data. Camera produces the view-projection matrix for sprite batching.
6. **Rendering** - `Renderer::render_batch` draws sprites; egui input is processed, overlays drawn, and the frame submitted.
7. **UI Feedback** - egui window exposes performance stats, spawn controls, emitter tuning (rate/spread/speed/lifetime/colors/sizes), camera details, selection gizmos, scripting status, and exposes script toggles (enable/reload).

### Module Relationships
- `App` owns instances of `Renderer`, `EcsWorld`, `Input`, `Camera2D`, `AssetManager`, and `Time`.
- `Renderer` references `WindowConfig` to honor user display preferences.
- `EcsWorld` queries `AssetManager` for atlas UVs during instance collection.
- `Camera2D` is stateless aside from position/zoom; it depends on window size from `Renderer`.
- `ScriptHost` bridges Rhai scripts to ECS/AssetManager operations via a command queue; the app resolves script handles to entities when executing those commands.

This architecture ensures each frame flows data in a clear order (Input → ECS → Renderer → UI) without hidden global state, supporting the project's deterministic and data-driven goals.


### Scripting Guidelines
- Use `global name;` inside functions when mutating module-level state so Rhai updates the shared variable instead of shadowing it.
- `world.spawn_sprite` returns a negative handle until the engine materializes the entity; pass that handle to other `world.*` calls and the app resolves it when it processes the queued commands.
- Scripts can broadcast designer tweaks (auto spawn rate, spawn counts) via `set_auto_spawn_rate` and `set_spawn_per_press`; these override the corresponding debug UI controls at runtime.
- Scripts can adjust emitter rate/spread/speed/lifetime via the `set_emitter_*` helpers.
