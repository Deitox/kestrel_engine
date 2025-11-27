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
- [x] Deliverables: runtime persistence/reload tests exist; persisted state now serializes into scenes/checkpoints and reloads with entities.

## Phase 4 - Events & Signals [x]
- [x] Event bus: script-facing emit/listen APIs with payload support.
- [x] Entity scoping: listeners can be tagged to entities and auto-unsubscribe when instances are removed.
- [x] Safety: per-frame queue caps with overflow logging plus listener error isolation.
- [x] Deliverables: event API, unsubscribe handles, and regression tests added.

## Phase 5 - Tooling & Observability [~]
- [x] Tracing: per-callback timings recorded (init/update/ready/process/physics/exit/event) with Studio surfacing.
- [~] Profiling: counters exist for callbacks; Studio now shows a slow-callback offender list, but no offender charting beyond tables.
- [x] Error UX: path:line:col formatting now includes call stacks and supports per-instance error mute in the inspector.
- [~] Deliverables: timing counters surfaced; call stacks/mute delivered; offender list present; richer profiling still remains.

## Phase 6 - Safety & Performance [x]
- [x] Budgets: configurable per-callback time budget (`scripts.callback_budget_ms`) halts callbacks, marks instances errored, and surfaces the budget error when exceeded.
- [x] Command quotas: per-owner (host/instance) command quotas (`scripts.command_quota`) enforced per frame with log messages when a quota is exceeded.
- [x] Determinism: RNG seeding and command/worklist sorting exist; physics queries now iterate deterministically and a harness fixture locks query/command ordering across runs.
- [x] Deliverables: budget/quota enforcement landed; deterministic harness coverage and query ordering guarantees implemented.

## Phase 7 - Studio/Editor UX [x]
- [x] Inspector polish: script path dropdowns and error badges present; per-entity reload/reset buttons now available with tooltips in the inspector.
- [x] Docs: in-Studio ScriptWorld API reference panel/tooltips added to the Scripts sidebar and debugger window.
- [x] Event viewer: Studio now surfaces recent game events via the analytics feed (`kestrel_studio/src/app/editor_ui.rs`, `src/analytics.rs`).
- [x] Deliverables: scripts sidebar + debugger window ship enable/pause/step/reload controls, handle table, timings, console/REPL, API reference, and per-entity reload/reset actions (`kestrel_studio/src/app/editor_ui.rs`, `kestrel_studio/src/app/script_console.rs`).

## Phase 8 - Packaging & Testing [~]
- [x] Headless harness: `src/bin/script_harness.rs` runs fixtures against `ScriptPlugin` headlessly with deterministic seed support.
- [x] Golden tests: fixture-driven script output comparisons live under `tests/script_harness.rs` and `tests/fixtures/script_harness/*.golden.json`.
- [ ] Build artifacts: no AOT AST cache tooling.
- [~] Deliverables: headless runner and golden helpers landed; AOT tooling still not implemented.

## Game Kit Layer (Player/Waves/Stats/Upgrades) [ ]
- [ ] Not yet implemented; no `gamekit.rhai`, Rust helpers, docs, or sample scripts.
