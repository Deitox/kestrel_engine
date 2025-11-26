# Script System Expansion - Complex Game Readiness

Status legend: `[x]` done, `[~]` partial/incomplete, `[ ]` not started/unknown.

## Phase 1 - Ergonomics & Core Helpers [x]
- [x] Shared libs/imports: cached module resolver rooted at `assets/scripts/` with path hygiene and import digests; `assets/scripts/common.rhai` ships math/timer/tween/cooldown helpers; imports hash+cache per path with tests (`module_import_*` in `src/scripts.rs`).
- [x] Hot reload safety: per-script reload resets scopes, reruns globals, reinvokes `ready`, preserves optional state, and clears errors on success (see `behaviour_reload_*` tests).
- [x] Convenience API: `world.move_toward`, vector/angle helpers, cooldown/timer helpers in `common.rhai`, plus inline vector sugar on `World`.
- [x] Deterministic mode: RNG seeding via config/env (`deterministic_ordering`/`deterministic_seed` and `KESTREL_SCRIPT_*` env vars), `world.rand_seed`, and deterministic sorting of behaviour worklists + command queues.
- [x] Deliverables: import resolver + shared helper file + reload path + tests for import caching, reload, and deterministic RNG/ordering.

### Current determinism wiring [x]
- [x] Config/env toggles for deterministic ordering and seeding.
- [x] Behaviour ordering and per-frame command queues sorted when deterministic mode is on; `world.rand_seed(seed)` available.
- [x] Tests cover RNG determinism, worklist ordering, command queue ordering, and reload when imports change.

## Phase 2 - Game-Facing APIs [~]
- [x] Read APIs: `entity_snapshot` plus position/rotation/scale/velocity/tint accessors backed by per-frame snapshots.
- [~] Physics queries: `raycast` and `overlap_circle` use snapshot AABBs and now accept include/exclude filters; still no physics broadphase integration.
- [x] Spawning: `world.spawn_prefab(path)` enqueues deferred prefab spawns and `spawn_template(name)` now looks up prefab library entries (JSON preferred) with tests, plus optional `assets/prefabs/aliases.json` alias mapping.
- [x] Input/time: input state helpers exist; `World` now exposes time scale, scaled/unscaled time/delta, and timer registration helpers in addition to `dt` in callbacks, and engine physics/animation respect the script time scale.
- [~] Deliverables: prefab/query helpers and basic tests are in place; template/time/filter coverage landed; physics coupling/broadphase remains.

## Phase 3 - State & Lifecycle [~]
- [~] Persistent state: `ScriptBehaviour.persist_state` + `world.state_get/set/clear/keys` preserve instance maps across reload when opted in; not serialized into scene saves/checkpoints.
- [x] Lifecycle: `exit(world, entity)` fires on despawn and reload/script swap; `ready` reruns after reload with optional state preservation.
- [x] Hot reload policy: reload reruns globals, resets scopes unless persistence is enabled, and exposes `world.is_hot_reload()` during the first `ready`.
- [~] Deliverables: runtime persistence/reload tests exist; scene/checkpoint serialization for persisted state remains.

## Phase 4 - Events & Signals [~]
- [x] Event bus: script-facing emit/listen APIs with payload support.
- [x] Entity scoping: listeners can be tagged to entities and auto-unsubscribe when instances are removed.
- [x] Safety: per-frame queue caps with overflow logging plus listener error isolation.
- [x] Deliverables: event API, unsubscribe handles, and regression tests added.

## Phase 5 - Tooling & Observability [ ]
- [ ] Tracing: no script trace logging with per-callback timing or Studio surfacing.
- [ ] Profiling: no per-callback counters or offender lists.
- [~] Error UX: path:line:col formatting exists, but no call stacks or per-instance mute switch.
- [ ] Deliverables: timing counters, trace logs, Studio wiring, and tests not implemented.

## Phase 6 - Safety & Performance [ ]
- [ ] Budgets: no per-callback instruction/time budgets or enforcement.
- [ ] Command quotas: no per-instance command caps.
- [~] Determinism: RNG seeding and command/worklist sorting exist; no deterministic harness tying physics query ordering + command application beyond that.
- [ ] Deliverables: budget enforcement, quotas, expanded deterministic harness not implemented.

## Phase 7 - Studio/Editor UX [~]
- [~] Inspector polish: script path dropdowns and error badges present; no per-instance reload/reset buttons, inline API docs/tooltips, or recent logs view.
- [ ] Docs: no generated ScriptWorld API reference or Studio tooltips/panel.
- [ ] Event viewer: no tracing-driven event view.
- [~] Deliverables: partial inspector wiring only.

## Phase 8 - Packaging & Testing [ ]
- [ ] Headless harness: no script/behaviour headless runner or CLI for script suites.
- [ ] Golden tests: no fixture-driven script output comparisons.
- [ ] Build artifacts: no AOT AST cache tooling.
- [ ] Deliverables: headless runner, golden helpers, and AOT tooling not implemented.

## Game Kit Layer (Player/Waves/Stats/Upgrades) [ ]
- [ ] Not yet implemented; no `gamekit.rhai`, Rust helpers, docs, or sample scripts.
