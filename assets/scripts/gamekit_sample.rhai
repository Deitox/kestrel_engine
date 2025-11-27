import "gamekit" as kit;

let cfg = kit::config();
cfg.player.template = "gamekit_player";
cfg.player.spawn_point = [-0.35, -0.2];
cfg.player.health = 4.0;
cfg.player.lives = 2;
cfg.wave_gap = 0.35;
cfg.rewards.kill_score = 6.0;
cfg.rewards.wave_clear_bonus = 8.0;
cfg.enemy_health["gamekit_enemy_light"] = 2.0;
cfg.enemy_health["gamekit_enemy_heavy"] = 5.0;
cfg.rewards.kill_currency = 2.0;

cfg.waves.push(kit::wave("intro", "gamekit_enemy_light", 2, 0.1, 0.0));
cfg.waves.push(kit::wave("finale", "gamekit_enemy_heavy", 1, 0.0, 0.25));
cfg.waves[0].reward_score = 2.0;
cfg.waves[0].reward_currency = 0.5;

cfg.upgrades.push(kit::upgrade("income_boost", 1.5, "+0.5 scrap per kill"));
cfg.upgrades.push(kit::upgrade("damage_boost", 1.0, "+0.5 damage"));
cfg.upgrades.push(kit::upgrade("spawn_haste", 0.5, "Faster waves"));
cfg.upgrades.push(kit::upgrade("auto_repair", 2.5, "Restore 1 health on purchase"));
cfg.upgrades.push(kit::upgrade("tough_hull", 4.0, "Max health +1"));

let state = kit::state(cfg);
state.flags.first_kill = false;
state.flags.second_kill = false;
state.flags.final_kill = false;

fn ready(world, entity) {
    state = kit::start(world, state);
    world.log("gamekit sample ready");
}

fn process(world, entity, dt) {
    state = kit::tick(world, state, dt);

    if !state.flags.first_kill && state.timers.elapsed >= 0.12 {
        let h = kit::enemy_handle(state, "gamekit_enemy_light");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 2.0);
            state = res.state;
            state.flags.first_kill = res.killed;
        }
    }
    if !state.flags.second_kill && state.timers.elapsed >= 0.28 {
        let h = kit::enemy_handle(state, "gamekit_enemy_light");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 2.0);
            state = res.state;
            state.flags.second_kill = res.killed;
        }
    }

    if state.stats.currency >= 1.5 && !kit::has_upgrade(state, "income_boost") {
        let result = kit::try_purchase_upgrade(world, state, "income_boost");
        state = result.state;
        if result.purchased { world.log("income_boost purchased"); }
    }
    if state.stats.currency >= 1.0 && !kit::has_upgrade(state, "damage_boost") {
        let result = kit::try_purchase_upgrade(world, state, "damage_boost");
        state = result.state;
        if result.purchased { world.log("damage_boost purchased"); }
    }
    if state.stats.currency >= 0.5 && !kit::has_upgrade(state, "spawn_haste") {
        let result = kit::try_purchase_upgrade(world, state, "spawn_haste");
        state = result.state;
        if result.purchased { world.log("spawn_haste purchased"); }
    }
    if state.stats.currency >= 2.5 && !kit::has_upgrade(state, "auto_repair") {
        let result = kit::try_purchase_upgrade(world, state, "auto_repair");
        state = result.state;
        if result.purchased { world.log("auto_repair purchased"); }
    }
    if state.stats.currency >= 4.0 && !kit::has_upgrade(state, "tough_hull") {
        let result = kit::try_purchase_upgrade(world, state, "tough_hull");
        state = result.state;
        if result.purchased { world.log("tough_hull purchased"); }
    }

    if !state.flags.final_kill && state.timers.elapsed >= 0.46 {
        let h = kit::enemy_handle(state, "gamekit_enemy_heavy");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 5.0);
            state = res.state;
            state.flags.final_kill = res.killed;
        }
    }

    state = kit::log_progress(world, state, 0.4);
}
