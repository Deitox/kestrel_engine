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

cfg.waves.push(kit::wave("intro", "gamekit_enemy_light", 2, 0.1, 0.05));
cfg.waves.push(kit::wave("finale", "gamekit_enemy_heavy", 1, 0.0, 0.25));
cfg.waves[0].reward_score = 2.0;
cfg.waves[0].reward_currency = 0.5;

cfg.upgrades.push(kit::upgrade("auto_repair", 3.0, "patch kit"));

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
        state = kit::on_enemy_defeated(world, state, "intro");
        state.flags.first_kill = true;
    }
    if !state.flags.second_kill && state.timers.elapsed >= 0.4 {
        state = kit::on_enemy_defeated(world, state, "intro");
        state.flags.second_kill = true;
    }

    if state.stats.currency >= 3.0 && !kit::has_upgrade(state, "auto_repair") {
        let result = kit::try_purchase_upgrade(world, state, "auto_repair");
        state = result.state;
        if result.purchased {
            state.player.health += 1.0;
            world.log("kit:upgrade auto_repair");
        }
    }

    if !state.flags.final_kill && state.timers.elapsed >= 0.6 {
        state = kit::on_enemy_defeated(world, state, "finale");
        state.flags.final_kill = true;
    }

    state = kit::log_progress(world, state, 0.4);
    if state.timers.elapsed >= 0.6 {
        let score = world.stat_get("score");
        let scrap = world.stat_get("scrap");
        world.log("kit:stats score=" + score.to_string() + " scrap=" + scrap.to_string());
    }
}
