// Lightweight game kit helpers for player/wave/stat/upgrades flows.

fn config() {
    #{
        player: #{
            template: "gamekit_player",
            spawn_point: [0.0, 0.0],
            health: 5.0,
            lives: 3,
            respawn_delay: 1.0,
            invulnerable: 0.75
        },
        enemy_health: #{
            "default": 1.0,
            "gamekit_enemy_light": 2.0,
            "gamekit_enemy_heavy": 4.0
        },
        wave_gap: 0.5,
        waves: [],
        rewards: #{
            kill_score: 10.0,
            kill_currency: 1.0,
            wave_clear_bonus: 5.0
        },
        upgrades: []
    }
}

fn wave(name, template, count, cadence, delay) {
    #{
        name: if name.trim() == "" { template } else { name },
        template: template,
        count: count.max(0),
        cadence: cadence.max(0.0),
        delay: delay.max(0.0),
        reward_score: 0.0,
        reward_currency: 0.0
    }
}

fn upgrade(name, cost, description) {
    #{
        name: name,
        cost: cost.max(0.0),
        description: description,
        applied: false
    }
}

fn state(cfg) {
    let out = #{
        config: cfg,
        player: #{
            handle: -1,
            health: cfg.player.health,
            lives: cfg.player.lives,
            invuln: 0.0,
            respawn: 0.0,
            spawns: 0,
            deaths: 0
        },
        waves: #{
            index: 0,
            spawned: 0,
            kills: 0,
            timer: 0.0
        },
        stats: #{
            score: 0.0,
            currency: 0.0,
            kills: 0,
            waves_cleared: 0,
            upgrades: []
        },
        bonuses: #{
            score_per_kill: 0.0,
            scrap_per_kill: 0.0,
            damage_mult: 1.0,
            spawn_haste: 0.0
        },
        enemies: [],
        timers: #{
            elapsed: 0.0,
            log: 0.0
        },
        flags: #{
            started: false
        }
    };
    if cfg.waves.len() > 0 {
        out.waves.timer = cfg.waves[0].delay;
    }
    out
}

fn start(world, state) {
    let out = state;
    out.flags.started = true;
    out.timers.elapsed = 0.0;
    out.timers.log = 0.0;
    sync_stats(world, out);
    maybe_spawn_player(world, out)
}

fn tick(world, state, dt) {
    let out = state;
    if dt <= 0.0 {
        return out;
    }
    out.timers.elapsed += dt;
    out.timers.log += dt;
    if out.player.invuln > 0.0 {
        out.player.invuln = (out.player.invuln - dt).max(0.0);
    }
    if out.player.respawn > 0.0 {
        out.player.respawn = (out.player.respawn - dt).max(0.0);
    }
    let with_player = maybe_spawn_player(world, out);
    let after_waves = drive_waves(world, with_player, dt);
    sync_stats(world, after_waves);
    after_waves
}

fn on_enemy_defeated(world, state, label) {
    let out = state;
    out.stats.kills += 1;
    let rewards = out.config.rewards;
    out.stats.score += rewards.kill_score;
    out.stats.currency += rewards.kill_currency;
    out.stats.score += out.bonuses.score_per_kill;
    out.stats.currency += out.bonuses.scrap_per_kill;
    let wave_index = out.waves.index;
    let wave_list = out.config.waves;
    if wave_index < wave_list.len() {
        out.waves.kills += 1;
        let wave = wave_list[wave_index];
        if wave.reward_score > 0.0 {
            out.stats.score += wave.reward_score;
        }
        if wave.reward_currency > 0.0 {
            out.stats.currency += wave.reward_currency;
        }
        if out.waves.kills >= wave.count {
            out.stats.score += rewards.wave_clear_bonus;
            out.stats.waves_cleared += 1;
            out.waves.index = wave_index + 1;
            out.waves.spawned = 0;
            out.waves.kills = 0;
            out.waves.timer = next_wave_delay(out.config, out.waves.index);
        }
    }
    sync_stats(world, out);
    out
}

fn on_player_hit(world, state, damage) {
    let out = state;
    if out.player.handle < 0 {
        return #{ state: out, died: false };
    }
    if out.player.invuln > 0.0 {
        return #{ state: out, died: false };
    }
    let taken = damage.max(0.0);
    out.player.health -= taken;
    let died = out.player.health <= 0.0;
    if died {
        out.player.deaths += 1;
        out.player.handle = -1;
        if out.player.lives > 0 {
            out.player.lives -= 1;
        }
        out.player.respawn = out.config.player.respawn_delay;
        out.player.health = out.config.player.health;
    } else {
        out.player.invuln = out.config.player.invulnerable;
    }
    sync_stats(world, out);
    #{ state: out, died: died }
}

fn try_purchase_upgrade(world, state, name) {
    let out = state;
    let purchased = false;
    let target = -1;
    let idx = 0;
    while idx < out.config.upgrades.len() {
        if out.config.upgrades[idx].name == name {
            target = idx;
            break;
        }
        idx += 1;
    }
    if target >= 0 {
        let already = out.config.upgrades[target].applied;
        let cost = out.config.upgrades[target].cost;
        if !already && out.stats.currency + 0.0001 >= cost {
            out.stats.currency -= cost;
            out.config.upgrades[target].applied = true;
            out.stats.upgrades.push(name);
            purchased = true;
        }
    }
    if purchased {
        out = apply_upgrade_effect(world, out, name);
        sync_stats(world, out);
    }
    #{ state: out, purchased: purchased }
}

fn has_upgrade(state, name) {
    for u in state.stats.upgrades {
        if u == name {
            return true;
        }
    }
    false
}

fn log_progress(world, state, interval_seconds) {
    let out = state;
    if interval_seconds <= 0.0 {
        return out;
    }
    if out.timers.log >= interval_seconds {
        out.timers.log -= interval_seconds;
        let next_label = "done";
        let next_index = out.waves.index;
        let wave_count = out.config.waves.len();
        if next_index < wave_count {
            next_label = out.config.waves[next_index].name;
        }
        let msg = "kit:score=" + out.stats.score.to_string();
        let msg = msg + " scrap=" + out.stats.currency.to_string();
        let msg = msg + " wave=" + out.stats.waves_cleared.to_string();
        let msg = msg + " next=" + next_label;
        world.log(msg);
    }
    out
}

fn summary(state) {
    #{
        score: state.stats.score,
        currency: state.stats.currency,
        wave_index: state.waves.index,
        waves_cleared: state.stats.waves_cleared,
        lives: state.player.lives,
        upgrades: state.stats.upgrades.clone()
    }
}

fn maybe_spawn_player(world, state) {
    let out = state;
    if out.player.handle >= 0 || out.player.lives <= 0 || out.player.respawn > 0.0 {
        return out;
    }
    let handle = world.spawn_template(out.config.player.template);
    if handle >= 0 {
        let spawn_pos = out.config.player.spawn_point;
        world.set_position(handle, spawn_pos[0], spawn_pos[1]);
        out.player.handle = handle;
        out.player.health = out.config.player.health;
        out.player.invuln = out.config.player.invulnerable;
        out.player.spawns += 1;
    }
    out
}

fn drive_waves(world, state, dt) {
    let out = state;
    if out.waves.index >= out.config.waves.len() {
        return out;
    }
    out.waves.timer -= dt;
    let current = out.config.waves[out.waves.index];
    if out.waves.spawned < current.count && out.waves.timer <= 0.0 {
        let spawn_result = spawn_enemy(world, out, current.template);
        out = spawn_result.state;
        if spawn_result.handle >= 0 {
            out.waves.spawned += 1;
        }
        let cadence = current.cadence;
        if cadence <= 0.0 {
            out.waves.timer = 0.0;
        } else {
            let haste = 1.0 + out.bonuses.spawn_haste;
            out.waves.timer = cadence / haste;
        }
    }
    out
}

fn sync_stats(world, state) {
    world.stat_set("score", state.stats.score);
    world.stat_set("scrap", state.stats.currency);
    world.stat_set("wave", state.stats.waves_cleared + 1);
    world.stat_set("lives", state.player.lives);
}

fn next_wave_delay(cfg, index) {
    if index < cfg.waves.len() {
        let next = cfg.waves[index];
        let delay = cfg.wave_gap;
        if next.delay > delay {
            return next.delay;
        }
        delay
    } else {
        0.0
    }
}

fn spawn_enemy(world, state, template) {
    let out = state;
    let handle = world.spawn_template(template);
    if handle >= 0 {
        let spawn_pos = out.config.player.spawn_point;
        world.set_position(handle, spawn_pos[0], spawn_pos[1]);
        let hp = enemy_health(out.config, template);
        out.enemies.push(#{
            handle: handle,
            template: template,
            health: hp
        });
    }
    #{ state: out, handle: handle }
}

fn enemy_health(cfg, template) {
    let hp = cfg.enemy_health[template];
    if type_of(hp) == "()" {
        let fallback = cfg.enemy_health["default"];
        if type_of(fallback) == "()" { 1.0 } else { fallback }
    } else {
        hp
    }
}

fn enemy_handle(state, template) {
    let idx = 0;
    while idx < state.enemies.len() {
        if state.enemies[idx].template == template && state.enemies[idx].health > 0.0 {
            return state.enemies[idx].handle;
        }
        idx += 1;
    }
    -1
}

fn enemy_hit(world, state, handle, damage) {
    let out = state;
    let killed = false;
    let template = "";
    let idx = 0;
    while idx < out.enemies.len() {
        if out.enemies[idx].handle == handle {
            let enemy = out.enemies[idx];
            let dealt = damage.max(0.0) * out.bonuses.damage_mult.max(0.0);
            enemy.health = enemy.health - dealt;
            out.enemies[idx] = enemy;
            if enemy.health <= 0.0 {
                killed = true;
                template = enemy.template;
            }
            break;
        }
        idx += 1;
    }
    if killed {
        world.despawn(handle);
        out = on_enemy_defeated(world, out, template);
    }
    #{ state: out, killed: killed }
}

fn enemy_hit_template(world, state, template, damage) {
    let out = state;
    let target = enemy_handle(out, template);
    if target < 0 {
        if out.enemies.len() > 0 {
            target = out.enemies[0].handle;
        }
    }
    if target < 0 {
        return #{ state: out, killed: false };
    }
    enemy_hit(world, out, target, damage)
}

fn enemy_hit_handle(world, state, handle, damage) {
    if handle < 0 {
        return #{ state: state, killed: false };
    }
    enemy_hit(world, state, handle, damage)
}

fn apply_upgrade_effect(world, state, name) {
    let out = state;
    if name == "auto_repair" {
        out.player.health += 1.0;
    } else if name == "tough_hull" {
        out.config.player.health += 1.0;
        out.player.health = out.config.player.health;
    } else if name == "income_boost" {
        out.bonuses.scrap_per_kill += 0.5;
    } else if name == "damage_boost" {
        out.bonuses.damage_mult += 0.5;
    } else if name == "spawn_haste" {
        out.bonuses.spawn_haste += 0.25;
    }
    out
}
