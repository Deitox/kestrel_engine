import "gamekit" as kit;

let cfg = kit::config();
cfg.player.template = "gamekit_player";
cfg.player.spawn_point = [0.2, -0.1];
cfg.player.health = 3.0;
cfg.rewards.kill_score = 4.0;
cfg.rewards.kill_currency = 1.0;
cfg.rewards.wave_clear_bonus = 2.0;
cfg.spawn_jitter = 0.15;
cfg.difficulty.health_growth = 0.2;
cfg.difficulty.reward_growth = 0.5;
cfg.enemy_health["gamekit_enemy_dash"] = 2.0;
cfg.enemy_speed["gamekit_enemy_dash"] = 1.4;

cfg.waves.push(kit::wave("dash_a", "gamekit_enemy_dash", 1, 0.0, 0.0));
cfg.waves.push(kit::wave("dash_b", "gamekit_enemy_dash", 1, 0.0, 0.05));

cfg.upgrades.push(kit::upgrade("shield_overload", 1.0, "brief invulnerability"));

let state = kit::state(cfg);
state.flags.first = false;
state.flags.second = false;

fn ready(world, entity) {
    state = kit::start(world, state);
    world.log("dash:ready");
}

fn process(world, entity, dt) {
    state = kit::tick(world, state, dt);

    if !state.flags.first && state.timers.elapsed >= 0.05 {
        let h = kit::enemy_handle(state, "gamekit_enemy_dash");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 4.0);
            state = res.state;
            state.flags.first = res.killed;
        }
    }

    if !state.flags.second && state.flags.first && state.timers.elapsed >= 0.35 {
        let h = kit::enemy_handle(state, "gamekit_enemy_dash");
        if h >= 0 {
            let res = kit::enemy_hit(world, state, h, 4.0);
            state = res.state;
            state.flags.second = res.killed;
        }
    }

    if state.stats.currency >= 1.0 && !kit::has_upgrade(state, "shield_overload") {
        let res = kit::try_purchase_upgrade(world, state, "shield_overload");
        state = res.state;
        if res.purchased { world.log("dash:upgrade shield_overload"); }
    }

    state = kit::log_progress(world, state, 0.25);

    if state.flags.second {
        world.log("dash:done score=" + state.stats.score.to_string() + " scrap=" + state.stats.currency.to_string());
    }
}
