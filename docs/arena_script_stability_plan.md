# Arena Script Stability & UX Plan (Handle-Safe Gameplay Loop)

## Objectives

- Prevent **invalid-entity panics** from scripts by giving them safe, validated handles and predictable lifecycle semantics.
- Provide **first-class spawn/despawn helpers** so gameplay scripts do not need to hand-roll handle tracking or prefab plumbing.
- Reduce **Rhai complexity footguns** (expression limits, reload scope/resets) via small utilities and recommended patterns.
- Maintain **backwards compatibility** for existing scripts while offering an opt-in safe path that can become the default for new content.

---

## Problems to Solve (from recent failures)

- Scripts receive raw entity IDs/bits; stale or recycled IDs can panic the engine when reused.
- There is no direct `is_alive`/handle validity API; scripts infer liveness by "can I read the position?".
- No engine-tracked spawn helpers; scripts manually queue spawn commands and manage their own handle mapping.
- Prefab/template spawns are visible only as deferred commands; scripts have no standard, handle-safe way to work with spawned entities.
- Script globals reset on hot reload, but persisted state may keep stale handles; there is no explicit, supported way to sanitize them.
- Rhai's expression/AST limits are easy to hit when scripts use big inline maps/conditionals instead of small helper functions.

---

## Proposed Features

### 1. Safe Handle API

Treat entity references as an explicit concept: **`EntityHandle`** (implemented as `int` in Rhai, but documented as a distinct kind of value). Handles are per-session and never intended to survive save/load or level transitions.

Script-facing API:

- `world.handle_is_alive(handle: int) -> bool`  
  - Returns `true` if the handle refers to a currently live entity, `false` otherwise.  
  - Never panics; always safe to call.

- `world.handle_validate(handle: int) -> int | ()`  
  - Returns the same handle if it is currently live.  
  - Returns `()` if the handle is invalid or stale.  
  - Intended for "cleanup and early-return" patterns:  
    ```rhai
    let h2 = world.handle_validate(h);
    if h2 == () { return; }
    // safe to use h2 from here on
    ```

- `world.handles_with_tag(tag: &str) -> array` *(optional convenience)*  
  - Returns an array of handles that are currently live and associated with the given tag.  
  - Engine lazily filters out dead handles when queried (no panics).  
  - Ordering is unspecified; callers should not depend on stability. Documented as a coarse tool (e.g., "all enemies" / "all pickups"), not a per-entity inner-loop primitive.

**Engine requirements:**

- Handles embed generation/nonce data; liveness is validated on every ScriptWorld entry that accepts a handle (no raw ID reuse).
- ScriptWorld calls that take handles never panic on invalid handles; they validate at entry and either early-return or return `false` / `()` / no-op.
- Handle values are session-ephemeral and are not serialized; any persisted handle-like data must be revalidated or discarded on load/reload.

---

### 2. Safe Spawns & Safe Despawn (Script-Facing)

Provide spawn/despawn helpers that return `EntityHandle`s and never panic if misused.

Script-facing API:

- `world.spawn_prefab_safe(path: &str, tag: &str = "") -> int | ()`
- `world.spawn_template_safe(name: &str, tag: &str = "") -> int | ()`
- `world.spawn_player(tag: &str = "player") -> int | ()`
- `world.spawn_enemy(template: &str, tag: &str = "enemy") -> int | ()`
- `world.despawn_safe(handle: int)`

Behavior:

- These functions still use the deferred spawn model: the engine enqueues a spawn command, and the entity is materialized at the appropriate point in the frame.
- The returned value is a handle that becomes usable once the entity exists; scripts should always guard with `world.handle_is_alive(handle)` before using it.
- On failure (missing prefab, rejected spawn), they return `()` instead of a sentinel like `-1` and emit a dev-facing reason to logs/metrics.
- `world.despawn_safe` is idempotent: invalid or already-despawned handles are a no-op (no panic), with an optional throttled dev warning.

Typical pattern:

```rhai
let player = world.spawn_player();
if player == () { return; }

// Later:
if world.handle_is_alive(player) {
    world.move_toward(player, tx, ty, speed);
}
```

---

### 3. Reload and Persistence Hygiene

- Handles are per-session only; do not serialize them across save/load or level transitions. Drop any stored handles on load.
- On hot reload, scripts should revalidate or clear any stored handles using `world.handle_validate` (or by rebuilding handle collections) before reuse.
- Provide a small pattern snippet for scripts that cache handles: run a cleanup function on reload/start-of-frame to purge invalid handles.

---

### 4. Observability and Dev Ergonomics

- Throttled dev warnings for invalid handle use and despawn of already-dead entities; avoid noisy logs in release builds.
- Spawn helpers emit lightweight reasons for failure (e.g., missing prefab/template name, denied by rules) to dev logs and increment counters.
- Metrics counters for: invalid-handle calls, spawn failures by reason, and despawn of dead handles; visible in dev HUD or debug overlay for quick triage.

---

### 5. Migration Strategy

- Keep legacy handle-taking APIs but route them through the safe validation path to eliminate panics; emit a one-time per-callsite warning when an invalid handle is passed.
- Mark legacy spawn helpers as deprecated in docs and steer new content to `_safe` variants; feature-flag a mode that hard-errors on legacy unsafe calls in dev builds.
- Update existing sample scripts to use `_safe` helpers and `handle_is_alive` guards to provide working references.

---

### 6. Guidance for Rhai Usage

- Prefer small helper functions over large inline maps/conditionals to stay under AST/expression limits.
- Encourage scripts to store only handles (not raw IDs) and to validate them before every use that crosses a frame boundary.
- Provide a short lint-like checklist in docs: guard deferred spawns, validate handles on reuse, avoid per-frame `handles_with_tag` in hot loops.

---

### 7. Validation and Acceptance

- Add integration tests that exercise: invalid handle into move/read calls (no panic, returns default/no-op), spawn failure returns `()`, and `despawn_safe` on dead handles is a no-op.
- Add a dev script sample showing reload hygiene: caching handles, revalidating on reload, and handling spawn failures gracefully.
