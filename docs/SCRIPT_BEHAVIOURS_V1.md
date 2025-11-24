# Script Behaviours v1 (engine-aligned)

## 1. Mission & Non-Goals

### Mission

Add per-entity Rhai behaviours that run `ready/world/process/physics_process` callbacks while keeping rendering, physics, and ECS ownership in Rust. This builds on the current single-script `ScriptHost`/`ScriptPlugin` (hot reload, command queue, `init/world/update`) instead of replacing them.

### Non-Goals (for v1)

- No Rhai import/module resolver.
- No in-editor code editor (external editor is fine).
- No persistence of per-instance state across scene reloads.
- No debugger/breakpoints.
- No signal/event bus for scripts beyond lifecycle callbacks.

---

## 2. Current Baseline (engine reality)

- `ScriptHost` loads one entry script (`project.scripts_entry_path()`), exposes `init(world)` and `update(world, dt)`, hot-reloads from disk, and captures `last_error`.
- `ScriptWorld` uses a command queue (`ScriptCommand`) plus handle-to-entity resolution in `App::apply_script_commands`; it already exposes spawn/move/tint/particles/log/rand helpers.
- `ScriptPlugin` runs inside the plugin system and already has `update`/`fixed_update` hooks and pause/step support.
- Studio has a script console/log panel but no per-entity script assignment or inspector UI.

The plan below extends this instead of throwing it away.

---

## 3. High-Level Plan

- Add a `ScriptBehaviour` ECS component (script path + runtime instance id) and serialize it in scenes/prefabs.
- Evolve `ScriptHost` into a multi-script registry (one AST per path), retaining hot-reload and `last_error`, and still keeping the legacy single-entry script running for compatibility.
- Track per-entity script instances (scope, flags, instance id) keyed off `ScriptBehaviour`.
- Drive lifecycle callbacks from `ScriptPlugin::update` (process) and `ScriptPlugin::fixed_update` (physics_process) while respecting pause/step.
- Extend `ScriptWorld` with entity-centric helpers (position/rotation/scale/velocity/despawn) that operate on ECS entities, but keep the existing command-based safety model.
- Studio: inspector UI to attach/edit script paths, simple asset picker, and error surfacing using the existing `last_error` plumbing.
- Ship sample scripts/scenes plus smoke tests.

---

## 4. Phase 1 — Data Model & Serialization

**Goal:** Introduce `ScriptBehaviour` as data without changing runtime execution yet.

- Define component (new `src/script_behaviours.rs` or inside `scripts.rs`):
  ```rust
  pub struct ScriptBehaviour {
      pub script_path: String, // e.g. "scripts/enemy.rhai"
      pub instance_id: u64,    // runtime-only; 0 = not bound
  }
  ```
- Register the component with ECS so it can be attached in code and scenes.
- Scene/prefab serialization:
  - Serialize `script_path`.
  - Do not serialize `instance_id` (or reset it to 0 on load).
- Helpers:
  ```rust
  impl ScriptBehaviour {
      pub fn new(path: impl Into<String>) -> Self {
          Self { script_path: path.into(), instance_id: 0 }
      }
  }
  ```
- Exit: scenes round-trip with `script_path` intact; runtime loads reset `instance_id` to 0.

---

## 5. Phase 2 — Multi-Script Host (compatibility-friendly)

**Goal:** Let the host compile/cache many scripts while keeping the legacy entry script working.

- Add a registry:
  ```rust
  struct CompiledScript {
      ast: rhai::AST,
      has_ready: bool,
      has_process: bool,
      has_physics_process: bool,
  }

  struct ScriptRegistry {
      scripts: HashMap<String, CompiledScript>,
  }
  ```
- Loading API:
  ```rust
  impl ScriptHost {
      pub fn load_script(&mut self, path: &str) -> Result<(), ScriptError>;
      pub fn ensure_script_loaded(&mut self, path: &str) -> Result<(), ScriptError>;
  }
  ```
  - Use the asset system to read script text (fall back to fs only where necessary until assets are wired).
  - Cache ASTs keyed by script path; reuse on subsequent calls.
- Function discovery: set `has_ready/process/physics_process` flags during compile.
- Error surfacing: keep `last_error` per host; include path + line/column in compile/runtime errors for Studio to display.
- Keep existing `init/world/update` entry script running so current samples/tests (`assets/scripts/main.rhai`) stay valid.

Exit: `ensure_script_loaded("scripts/example.rhai")` compiles once and reuses the cached AST; errors are readable and path-aware.

---

## 6. Phase 3 — Per-Entity Instances & Lifecycle

**Goal:** Bind scripts to entities and run lifecycle callbacks through the plugin hooks.

- Instance table:
  ```rust
  struct ScriptInstance {
      script_path: String,
      entity: Entity,
      scope: rhai::Scope,
      has_ready_run: bool,
      errored: bool,
  }

  struct ScriptInstanceTable {
      next_id: u64,
      instances: HashMap<u64, ScriptInstance>, // key = instance_id
  }
  ```
- Instance creation:
  - When visiting a `ScriptBehaviour` with `instance_id == 0`, allocate `next_id`, create `ScriptInstance`, write `instance_id` back to the component.
  - Initialize an empty `Scope` (later we can seed helpers/constants).
- Lifecycle wiring:
  - In `ScriptPlugin::update(dt)`: iterate behaviours, ensure script is loaded, instantiate if needed, call `ready` once, then `process` each frame if present.
  - In `ScriptPlugin::fixed_update(dt)`: call `physics_process` for scripts that define it.
  - Respect pause/step flags already present on `ScriptPlugin`.
- Error handling:
  - Catch Rhai errors, include path + fn name, set `errored = true`, and skip further calls until reload or reset.
- Cleanup:
  - When an entity loses `ScriptBehaviour` or is despawned, remove its instance; consider an `exit` callback later if needed.

Exit: each behaviour runs `ready` once, then `process` per frame and `physics_process` on fixed ticks; errors are isolated and do not panic the engine.

---

## 7. Phase 4 — ScriptWorld API for Behaviours

**Goal:** Let per-entity scripts manipulate their own entity safely while retaining the deferred-command model.

- Extend `ScriptWorld` with entity-centric helpers that enqueue commands or validate existence:
  - `world.entity_set_position(entity, x, y)`
  - `world.entity_set_rotation(entity, radians)`
  - `world.entity_set_scale(entity, sx, sy)`
  - `world.entity_set_velocity(entity, vx, vy)`
  - `world.entity_despawn(entity)`
  - Optional: `world.spawn_prefab(path)` or `world.spawn_from_template(name)`
- Map the `entity` Rhai value to `bevy_ecs::Entity` (likely `INT`).
- Keep existing handle-based helpers for the legacy global script; do not break `ScriptCommand` plumbing.

Exit: a behaviour can move/rotate/scale/kill its own entity and optionally spawn new ones without bypassing ECS safety.

---

## 8. Phase 5 — Studio Integration

**Goal:** Make behaviours visible/editable in the UI and surface errors.

- Inspector:
  - Show a "Script" section when `ScriptBehaviour` exists.
  - Text field for `script_path`; optional dropdown populated by scanning `assets/scripts/*.rhai`.
  - Hide or mark `instance_id` as runtime-only.
- Error surfacing:
  - Reuse `ScriptHost::last_error` and display path + message in the scripts panel.
  - Optional: tag entities whose behaviour instance is currently errored.

Exit: users can assign/change script paths from the inspector and see clear script errors in Studio.

---

## 9. Phase 6 — Samples & Tests

**Example scripts (entity-centric signatures):**

`assets/scripts/spinner.rhai`
```rhai
fn ready(world, entity) {
    world.log("spinner ready for entity " + entity.to_string());
}

fn process(world, entity, dt) {
    let speed = 1.5;
    world.entity_set_rotation(entity, speed * dt);
}
```

`assets/scripts/wanderer.rhai`
```rhai
let state = #{ timer: 0.0 };

fn ready(world, entity) {
    state.timer = 0.0;
}

fn process(world, entity, dt) {
    state.timer += dt;
    if state.timer >= 0.5 {
        state.timer = 0.0;
        let angle = world.rand(0.0, 6.28318);
        let speed = world.rand(0.2, 0.6);
        let vx = speed * cos(angle);
        let vy = speed * sin(angle);
        world.entity_set_velocity(entity, vx, vy);
    }
}
```

**Example scene:** `assets/scenes/script_behaviour_demo.kscene`
- Camera + a few sprites.
- Three entities share a sprite but have different scripts (`spinner`, `wanderer`, `blinker`).
- Shows distinct behaviours when run in Studio.

**Tests:**
- Serialization: scene with `ScriptBehaviour` round-trips, `script_path` survives, `instance_id` resets to 0.
- Compile: `ensure_script_loaded("scripts/spinner.rhai")` succeeds and caches.
- Runtime smoke: create ECS world + one entity with `ScriptBehaviour`, run `ScriptPlugin::update` once, `instance_id` assigned, no panic.
- Legacy: existing `assets/scripts/main.rhai` still runs through `init/update` to protect current sample content.

Exit: CI fails if scripts do not compile or if behaviours break serialization/runtime assumptions.

---

## 10. Stretch Goals

- Signals/events for scripts via the existing event bus.
- Rhai import resolver for shared helpers under `assets/scripts/`.
- Per-script hot reload that refreshes instances safely.
- Debug tools (variable inspection, call tracing).
- Generated API docs for `ScriptWorld` exposure.
