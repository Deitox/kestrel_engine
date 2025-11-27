import "gamekit" as kit;

let cfg = kit::config();
cfg.player.template = "gamekit_player";
cfg.player.spawn_point = [-0.5, -0.2];
cfg.player.health = 3.0;
cfg.player.lives = 2;
cfg.wave_gap = 0.3;
cfg.rewards.kill_score = 4.0;
cfg.rewards.kill_currency = 1.5;
cfg.rewards.wave_clear_bonus = 3.0;
cfg.enemy_health["gamekit_enemy_light"] = 2.0;
cfg.enemy_health["gamekit_enemy_heavy"] = 5.0;

cfg.waves.push(kit::wave("intro", "gamekit_enemy_light", 2, 0.1, 0.05));
cfg.waves.push(kit::wave("finale", "gamekit_enemy_heavy", 1, 0.0, 0.25));
cfg.waves[0].reward_score = 2.0;
cfg.waves[0].reward_currency = 0.5;

cfg.upgrades.push(kit::upgrade("auto_repair", 3.0, "patch kit"));
cfg.upgrades.push(kit::upgrade("income_boost", 2.0, "+0.5 scrap per kill"));
cfg.upgrades.push(kit::upgrade("tough_hull", 4.0, "hull+1"));

let state = kit::state(cfg);
state.flags.first_kill = false;
state.flags.second_kill = false;
state.flags.final_kill = false;

fn ready(world, entity) {
    state = kit::start(world, state);
    world.log("kit:ready");
}

fn process(world, entity, dt) {
    state = kit::tick(world, state, dt);

    if !state.flags.first_kill && state.timers.elapsed >= 0.2 {
        let h = kit::enemy_handle(state, "gamekit_enemy_light");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 2.0);
            state = res.state;
            state.flags.first_kill = res.killed;
        }
    }
    if !state.flags.second_kill && state.timers.elapsed >= 0.4 {
        let h = kit::enemy_handle(state, "gamekit_enemy_light");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 2.0);
            state = res.state;
            state.flags.second_kill = res.killed;
        }
    }

    if state.stats.currency >= 2.0 && !kit::has_upgrade(state, "income_boost") {
        let result = kit::try_purchase_upgrade(world, state, "income_boost");
        state = result.state;
        if result.purchased { world.log("kit:upgrade income_boost"); }
    }
    if state.stats.currency >= 3.0 && !kit::has_upgrade(state, "auto_repair") {
        let result = kit::try_purchase_upgrade(world, state, "auto_repair");
        state = result.state;
        if result.purchased { world.log("kit:upgrade auto_repair"); }
    }
    if state.stats.currency >= 4.0 && !kit::has_upgrade(state, "tough_hull") {
        let result = kit::try_purchase_upgrade(world, state, "tough_hull");
        state = result.state;
        if result.purchased { world.log("kit:upgrade tough_hull"); }
    }

    if !state.flags.final_kill && state.timers.elapsed >= 0.6 {
        let h = kit::enemy_handle(state, "gamekit_enemy_heavy");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 5.0);
            state = res.state;
            state.flags.final_kill = res.killed;
        }
    }

    state = kit::log_progress(world, state, 0.4);
    if state.timers.elapsed >= 0.6 {
        let score = world.stat_get("score");
        let scrap = world.stat_get("scrap");
        world.log("kit:stats score=" + score.to_string() + " scrap=" + scrap.to_string());
    }
}
