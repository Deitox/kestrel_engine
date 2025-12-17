let state = #{
    wave_size: 8,
    wave_index: 1,
    wave_pending: false
};

fn init(world) {
    world.log("Kestrel Lab: init");
    world.rand_seed(1);
    world.listen("target_destroyed", "on_target_destroyed");
    world.stat_set("score", 0);
    world.stat_set("wave", state.wave_index);
    world.stat_set("next_wave", 0.0);
    spawn_wave(world, state.wave_size);
    return;
}

fn update(world, _dt) {
    let targets = world.entities_with_tag("target");
    world.stat_set("targets", targets.len());
    if targets.len() == 0 {
        if !state.wave_pending {
            state.wave_pending = true;
            world.timer_start("wave_delay", 0.75);
        }
    } else {
        state.wave_pending = false;
    }

    if state.wave_pending {
        let remaining = world.timer_remaining("wave_delay");
        world.stat_set("next_wave", remaining);
        if world.timer_fired("wave_delay") {
            world.timer_clear("wave_delay");
            state.wave_pending = false;
            world.stat_set("next_wave", 0.0);
            state.wave_index += 1;
            state.wave_size = (state.wave_size + 2).min(18);
            world.stat_set("wave", state.wave_index);
            spawn_wave(world, state.wave_size);
        }
    }
    return;
}

fn on_target_destroyed(world, evt) {
    world.stat_add("score", 1);

    let x = 0.0;
    let y = 0.0;
    let target = evt["target"];
    if target != () {
        let pos = world.entity_position(target);
        if pos.len() >= 2 {
            x = pos[0];
            y = pos[1];
        }
    } else {
        let payload = evt["payload"];
        if payload != () {
            let px = payload["x"];
            let py = payload["y"];
            if px != () {
                x = px;
            }
            if py != () {
                y = py;
            }
        }
    }

    let fx = world.spawn_template_safe("hit_fx");
    if fx != () {
        world.set_position(fx, x, y);
    }
}

fn spawn_wave(world, count) {
    let center_x = 0.0;
    let center_y = 0.0;
    let player = world.find_scene_entity("player");
    if player != () {
        let pos = world.entity_position(player);
        if pos.len() >= 2 {
            center_x = pos[0];
            center_y = pos[1];
        }
    }

    for i in 0..count {
        let target = world.spawn_template_safe("target", "target");
        if target == () {
            world.log("spawn_wave: target spawn failed");
            return;
        }
        let x = center_x + world.rand(-1.1, 1.1);
        let y = center_y + world.rand(-0.75, 0.75);
        world.set_position(target, x, y);

        let vx = world.rand(-0.45, 0.45);
        let vy = world.rand(-0.45, 0.45);
        world.set_velocity(target, vx, vy);
    }
}
