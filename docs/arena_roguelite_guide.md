# Kestrel Arena Roguelite Guide

Build a fast-paced top-down arena shooter roguelite with the safe scripting surface. This guide shows patterns for spawns, waves, AI, and reload hygiene now that handle safety is in place.

## Core Practices

- Use the safe API only: `spawn_*_safe` (prefab/template/player/enemy/sprite) and `despawn_safe`. Legacy names forward to the same path; treat `_safe` as canonical.
- Guard deferred spawns: check `h != ()` and `handle_is_alive(h)` before commands (position/velocity/tint/etc.).
- Validate reused handles across frames/reloads: `h = world.handle_validate(h); if h == () { rebuild/respawn; }`.
- Keep persisted state handle-free; store your own IDs and recompute handles on load/reload.
- Let built-in warnings/metrics surface invalid handles or spawn failures; avoid noisy logs.

## Game Loop Structure

- **Main script:** drive global timers, wave progression, loot drops, difficulty ramp.
- **Behaviour scripts:** per-entity AI (enemies, turrets, bosses), always guarding handles before actions.
- **Events:** use `listen`/`emit` for damage, pickups, wave transitions; scope to entities when needed.

## Spawning & Waves

Use prefabs/templates for player, enemies, weapons, bosses; spawn via safe helpers.

Wave tick (pseudocode):

```rhai
fn tick_waves(world, state, dt) {
    state.timer -= dt;
    if state.timer <= 0.0 && state.remaining > 0 {
        let h = world.spawn_template_safe(state.current_template, "enemy");
        if h != () {
            if world.handle_is_alive(h) { world.set_position(h, state.spawn[0], state.spawn[1]); }
            state.enemies.push(h);
            state.remaining -= 1;
        }
        state.timer = state.cadence;
    }
}
```

Guard liveness before commands:

```rhai
if world.handle_is_alive(h) { world.set_velocity(h, vx, vy); }
```

On wave completion: scale stats (hp/speed/damage) and queue next wave/boss.

## Weapons & Projectiles

- Fire: `let p = world.spawn_prefab_safe("projectile"); if p != () && world.handle_is_alive(p) { world.set_velocity(p, vx, vy); }`
- Multi-weapon loadouts: iterate equipped weapons and fire independently.
- Melee/AoE: `overlap_circle` and apply damage via events.

## Player & Characters

- Character select: load stats from data; apply via setters; store character ID (not handles).
- Respawn/invuln: on death set handle to `()`/`-1`, schedule respawn; revalidate on spawn.

## Builds & Items

- Store items/buffs in state maps/arrays (no handles). On pickup, emit an event to apply stat changes; reverse on drop/expiry.
- Use `stat_set/stat_get` for shared counters (score, currency, waves, kills) and per-run tuning.

## Reload & Persistence Hygiene

On ready/reload:

```rhai
if world.is_hot_reload() {
    let hs = world.state_get("handles");
    if type_of(hs) == "array" {
        world.state_set("handles", hs.map(|h| world.handle_validate(h)).filter(|h| h != ()));
    }
} else {
    world.state_set("handles", []);
}
```

Do not persist handles; recompute from saved identifiers.

## AI Patterns

- Movement: read `entity_position`, steer; guard handle before writes.
- Targeting: keep a target handle and revalidate each frame; retarget if invalid.
- Boss phases: phase machine in script; switch patterns on timers/hp thresholds.

## Difficulty & Replayability

- Short runs: cap wave count/time; spawn final boss; end run cleanly.
- Scaling: per-wave multipliers (HP/speed/damage, spawn cadence) plus item/character bonuses.
- Variety: data-driven lists for enemies/weapons/items; scripts consume config maps and apply buffs.

## Determinism & Testing

- For repros/tests, use `enable_deterministic_mode(seed)` (stable RNG/handles).
- Add small asserts/logs for critical paths (boss spawn, wave transitions, loot drops).

## Example Spawn + Command Pattern

```rhai
fn spawn_enemy(world, template, pos, vel) {
    let h = world.spawn_template_safe(template, "enemy");
    if h == () { return h; }
    if world.handle_is_alive(h) {
        world.set_position(h, pos[0], pos[1]);
        world.set_velocity(h, vel[0], vel[1]);
    }
    h
}
```

## Script Layout Suggestion

- `main.rhai`: run/wave loop, boss triggers, global stats, loot drops.
- `player_behaviour.rhai`: input, movement, damage handling, weapon firing.
- `enemy_behaviour_X.rhai`: per-enemy AI; guard all handle actions.
- `weapon.rhai`: firing patterns, projectile spawns.
- `boss.rhai`: phase machine, special attacks, summons via safe spawns.

## What to Avoid

- Using handles without `handle_is_alive` checks (especially after deferred spawns).
- Storing handles in persisted state; always revalidate or rebuild.
- Per-frame `handles_with_tag` in hot loops; cache once per frame if needed.
