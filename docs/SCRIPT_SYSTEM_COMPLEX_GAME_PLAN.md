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

## Phase 4 - Events & Signals [x]
- [x] Event bus: script-facing emit/listen APIs with payload support.
- [x] Entity scoping: listeners can be tagged to entities and auto-unsubscribe when instances are removed.
- [x] Safety: per-frame queue caps with overflow logging plus listener error isolation.
- [x] Deliverables: event API, unsubscribe handles, and regression tests added.

## Phase 5 - Tooling & Observability [~]
- [x] Tracing: per-callback timings recorded (init/update/ready/process/physics/exit/event) with Studio surfacing.
- [~] Profiling: counters exist for callbacks; no offender list or studio charting beyond tabular view.
- [~] Error UX: path:line:col formatting exists, but no call stacks or per-instance mute switch.
- [~] Deliverables: timing counters surfaced; trace logs/call stacks remain.

## Phase 6 - Safety & Performance [~]
- [x] Budgets: configurable per-callback time budget (`scripts.callback_budget_ms`) halts callbacks, marks instances errored, and surfaces the budget error when exceeded.
- [x] Command quotas: per-owner (host/instance) command quotas (`scripts.command_quota`) enforced per frame with log messages when a quota is exceeded.
- [~] Determinism: RNG seeding and command/worklist sorting exist; no deterministic harness tying physics query ordering + command application beyond that.
- [~] Deliverables: budget/quota enforcement landed; expanded deterministic harness still not implemented.

## Phase 7 - Studio/Editor UX [~]
- [~] Inspector polish: script path dropdowns and error badges present; still no per-instance reload/reset buttons or inline API docs/tooltips.
- [ ] Docs: no generated ScriptWorld API reference or Studio tooltips/panel.
- [x] Event viewer: Studio now surfaces recent game events via the analytics feed (`kestrel_studio/src/app/editor_ui.rs`, `src/analytics.rs`).
- [~] Deliverables: scripts sidebar + debugger window ship enable/pause/step/reload controls, handle table, timings, and a console/REPL (`kestrel_studio/src/app/editor_ui.rs`, `kestrel_studio/src/app/script_console.rs`); per-entity reload/reset tooling and API docs still missing.

## Phase 8 - Packaging & Testing [ ]
- [ ] Headless harness: no script/behaviour headless runner or CLI for script suites.
- [ ] Golden tests: no fixture-driven script output comparisons.
- [ ] Build artifacts: no AOT AST cache tooling.
- [ ] Deliverables: headless runner, golden helpers, and AOT tooling not implemented.

## Game Kit Layer (Player/Waves/Stats/Upgrades) [ ]
- [ ] Not yet implemented; no `gamekit.rhai`, Rust helpers, docs, or sample scripts.
