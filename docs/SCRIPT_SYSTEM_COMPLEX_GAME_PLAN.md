# Script System Expansion — Complex Game Readiness

Goal: expand Rhai behaviours so we can comfortably build a complex, content-rich game while keeping ECS ownership, safety, and hot-reload friendliness. Organized as phased, deliverable-oriented work.

## Phase 1 — Ergonomics & Core Helpers
- Shared libs/imports: add a cached import resolver rooted at `assets/scripts/` with basic path hygiene (no `..`). Ship a `common.rhai` with math/timers/tweens/cooldowns/helpers; update compiler to hash+cache imported ASTs per path.
- Hot reload safety: per-script reload that resets scopes, re-runs globals, re-invokes `ready` with optional preserved state; clear errors on successful reload.
- Convenience API: add helpers (`move_toward`, `look_at`, `clamp_length`, `cooldown`, `timer`), and sugar for vectors/angles to cut boilerplate in behaviours.
- Deterministic mode: seed RNG from engine; expose `world.rand_seed(seed)` for tests; add a toggle for deterministic frame ordering in script world command application.
- Deliverables: import resolver; shared helper file; reload path that refreshes instances; tests covering import caching, reload, deterministic RNG.

### Current determinism wiring
- Config: `AppConfig.scripts` supports `deterministic_ordering` and optional `deterministic_seed`. Studio also honors env vars `KESTREL_SCRIPT_DETERMINISTIC` (on/off) and `KESTREL_SCRIPT_SEED=<u64>`; both force ordering and seeding.
- Behaviour: when enabled, the script RNG is seeded and we sort the behaviour worklist plus per-frame command queue for stable execution order; scripts can still call `world.rand_seed(seed)` ad hoc.
- Tests: unit tests cover RNG determinism, worklist ordering, command queue ordering, and reload when imported modules change.

## Phase 2 — Game-Facing APIs
- Read APIs: expose read-only snapshots (transform, velocity, tint, scale) to scripts; return structs not ECS refs.
- Physics queries: `world.raycast(origin, dir, max_dist) -> hit?` and `world.overlap_circle(pos, radius) -> [entities]` with filters; backed by existing physics broadphase.
- Spawning: `world.spawn_prefab(path)` / `world.spawn_template(name)` using existing prefab loader; return entity handle; respect deferred command model.
- Input/time: expose current input state (pressed/held), time scale, dt, and a simple timer/alarm registration API surfaced via script world.
- Deliverables: new ScriptWorld fns + safety checks; tests exercising queries and prefab spawn from scripts.

## Phase 3 — State & Lifecycle
- Persistent state: optional instance data blob serialized per scene save/checkpoint; opt-in field on `ScriptBehaviour` to persist a map; clear on prefab load unless flagged.
- Lifecycle: finalize `exit(world, entity)` callback and ensure it fires on despawn and script swap; add on-reload `ready` re-run with preserved optional state.
- Hot reload policy: on script change, rerun globals, reset scopes unless persistence is enabled; expose a script-visible `world.is_hot_reload()` flag during `ready`.
- Deliverables: persisted state plumbing; exit callback coverage; tests for reload with/without persistence.

## Phase 4 — Events & Signals
- Event bus: lightweight, type-tagged string/channel events with optional payload (map/array); scripts can `emit(channel, payload)` and `on(channel, fn(payload))`.
- Entity scoping: support entity-tagged events to reduce global noise; auto-unsubscribe on instance drop.
- Safety: bound queue size per frame and per-listener execution time budget; errors isolated per listener.
- Deliverables: event API in ScriptWorld, unsubscribe on drop, tests for emit/listen, isolation, and quotas.

## Phase 5 — Tooling & Observability
- Tracing: opt-in trace logging with call stacks and per-callback timing; store recent log ring per entity instance; surface in Studio (inspector + console).
- Profiling: lightweight counters for time spent in `ready/process/physics_process/exit`; expose top offenders list; guard with feature flag.
- Error UX: better messages with path:line:col + call stack; per-instance “mute until reload” switch.
- Deliverables: timing counters, trace logs, Studio plumbing, tests for error formatting and mute.

## Phase 6 — Safety & Performance
- Budgets: per-callback instruction/time budget; configurable defaults; abort execution with a clear error when exceeded.
- Command quotas: cap number of queued commands per instance per frame; drop with warning when exceeded.
- Determinism: optional deterministic mode ties RNG + physics query ordering + command application order; add regression tests.
- Deliverables: budget enforcement, quotas, deterministic test harness.

## Phase 7 — Studio/Editor UX
- Inspector polish: dropdowns for script paths (asset scan), inline docs/tooltips for exposed API, per-instance reload/reset buttons, error badge, and recent logs view.
- Docs: generate API reference from registered functions; surface in Studio tooltips and a “Script API” panel.
- Event viewer: live view of emitted events per entity when tracing is on.
- Deliverables: UI wiring + docs generation; manual QA script for studio behaviours.

## Phase 8 — Packaging & Testing
- Headless harness: run scripts/behaviours in a headless test world; CLI to execute script suites; seedable deterministic runs.
- Golden tests: allow fixture-driven script outputs (logs/commands) to be compared against goldens.
- Build artifacts: optional AOT AST cache keyed by content hash for shipping builds; asset pipeline step to precompile scripts.
- Deliverables: headless runner, golden test helpers, AOT cache tooling.

## Risks & Mitigations
- Reload safety: ensure globals rerun and state resets/persists correctly; mitigate with thorough reload tests.
- Event bus unbounded growth: enforce per-frame queue limits and auto-unsubscribe; observability to find noisy emitters.
- Performance: budget enforcement and profiling to catch heavy scripts early.
- API drift: generate docs from the registered API to keep Studio tooltips aligned.

## Suggested Order of Execution
1) Phase 1 (imports, reload safety, helpers, deterministic RNG).  
2) Phase 2 (read APIs + queries + spawn) to unlock richer behaviours.  
3) Phase 3 (state/lifecycle) to stabilize hot reload + persistence.  
4) Phase 4 (events) once basics are solid.  
5) Phase 5–6 (tooling/safety) to harden.  
6) Phase 7–8 (Studio polish, packaging, test harness).  

Each phase should land with tests (unit + integration) and a short Studio manual check where applicable.

## Game Kit Layer (Player/Waves/Stats/Upgrades)
- Purpose: provide a small, opinionated helper on top of `ScriptWorld` so authors don’t reimplement common “run-based” patterns.
- Player/session: health/shield/stamina, invulnerability windows, damage application with source tags, respawn checkpoints, deterministic seed carry-through.
- Waves/spawning: timed schedules, weighted enemy tables, pacing curves, encounter states (intro/active/cleanup), hooks for music/FX cues.
- Economy/upgrades: currency drop/pickup helpers, stat-mod stacks (add/multiply), rarity tables, upgrade roll/choice API with reroll costs, mutators/modifiers.
- Progression/persistence: meta progression save/load hooks, difficulty scaling over time, per-run modifiers.
- UI hooks: events/callbacks for HUD (health, ammo, wave timer, currency) so scripts can update UI without tight coupling.
- Safety/determinism: reuse budgets/quotas, deterministic seeds through the kit, and guardrails on spawn/damage rates.
- Delivery: ship as `gamekit.rhai` (shared helpers) plus a few Rust-exposed functions for perf-sensitive bits (damage resolution, spawner ticking). Include docs and sample scripts using the kit.
