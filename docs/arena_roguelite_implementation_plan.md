# Arena Roguelite Implementation Plan (Kestrel)

Target: fast-paced top-down arena shooter roguelite (10–20 minute runs, 6-weapon loadouts, waves → boss, build-heavy replayability). This plan focuses on scripting + data; art/audio can be stubbed and swapped later.

## Phase 0: Project Skeleton
- Create `assets/scripts/roguelite_main.rhai` entry point that drives the run loop, wave scheduler, and loadout state (initially stubbed with logs + spawn of a test player/enemy).
- Keep existing `assets/scripts/main.rhai` but forward to `roguelite_main` when the content is ready; once stable, set this as the default boot script (see “Default Boot” below).
- Define data folders: `assets/data/characters.json`, `assets/data/weapons.json`, `assets/data/items.json`, `assets/data/waves.json`, `assets/data/enemies.json`.
- Prefabs/templates: create baseline prefabs for player, enemy archetypes, projectiles, pickups, and a test boss.

## Phase 1: Core Loop & Waves
- Implement wave scheduler in `roguelite_main.rhai`:
  - Countdown timer → spawn wave (safe spawns + liveness guards).
  - Track kills/remaining; when wave clears, scale difficulty (HP/speed/damage) and move to next wave; after final wave spawn boss.
- Player lifecycle: spawn player prefab, handle deaths/respawns with invuln window; use safe spawns and `handle_is_alive` checks before commands.
- Stats: use `stat_set/stat_get` for score, currency, wave index, kill count; per-run state stored in script maps (no handles persisted).

## Phase 2: Combat Systems
- Weapons: script weapon fire patterns (single/burst/shotgun/beam/melee/AoE) using `spawn_prefab_safe` for projectiles and `handle_is_alive` guards for commands.
- Damage/events: standardize events for hit/damage/pickup; use `listen/emit` to decouple behaviours.
- Enemies: implement per-archetype behaviours (chaser, shooter, dasher, summoner); each behaviour script guards handle use and retargets when `handle_validate` fails.
- Boss: phase machine with telegraphed attacks and summons (safe spawns).

## Phase 3: Builds & Progression
- Characters: load from `characters.json` (base stats, quirks); apply modifiers on spawn.
- Items/upgrades: data-driven effects; on pickup, apply stat deltas/buffs (stored in state, not handles); support stacking and time-limited effects.
- Economy: currency drops, shop/selection between waves; reroll/choice UI fed by data.

## Phase 4: UI & Feedback
- HUD: health, ammo/weapon indicators, cooldowns, currency, wave timer, mini-log.
- Upgrade/selection screens between waves (controller/keyboard-friendly).
- FX: screen shake, hit flashes, enemy death cues, basic audio hooks (stub OK).

## Phase 5: Content Fill & Tuning
- Expand enemy roster, weapons, items, characters per design goals.
- Balance passes on wave pacing, difficulty curves, drop rates, and build synergies.
- Add daily/seeded mode via deterministic mode (`enable_deterministic_mode(seed)`).

## Default Boot
- Once `roguelite_main.rhai` is stable, make it the default by either:
  - Replacing the body of `assets/scripts/main.rhai` to forward into `roguelite_main`, **or**
  - Pointing the engine’s startup script path (where configured) to `assets/scripts/roguelite_main.rhai`.
- Keep a flag to launch the editor/legacy demo if needed (e.g., env var or CLI arg).

## Scripting Patterns (refresh)
- Spawns: `let h = world.spawn_template_safe("enemy_basic", "enemy"); if h != () && world.handle_is_alive(h) { world.set_position(h, x, y); }`
- Reuse: `h = world.handle_validate(h); if h == () { /* reacquire/respawn */ }`
- Reload hygiene: on hot reload, revalidate cached handles and drop invalid entries.
- Performance: avoid per-frame `handles_with_tag` in hot loops; cache once per frame.

## Milestones
1) Skeleton + wave loop + test player/enemy; default boot toggle ready.
2) Weapons/projectiles + 3 enemy archetypes + pickups; basic HUD.
3) Boss + items/upgrades + character select; between-wave UI.
4) Content fill/balancing + polish; set roguelite as default boot.
