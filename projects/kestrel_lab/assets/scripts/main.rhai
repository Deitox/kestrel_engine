let state = #{
    wave_size: 8,
    wave_pending: false
};

fn init(world) {
    world.log("Kestrel Lab: init");
    world.rand_seed(1);
    world.stat_set("score", 0);
    state.wave_pending = true;
    spawn_wave(world, state.wave_size);
}

fn update(world, _dt) {
    let targets = world.entities_with_tag("target");
    if targets.len() == 0 {
        if !state.wave_pending {
            state.wave_pending = true;
            spawn_wave(world, state.wave_size);
        }
    } else {
        state.wave_pending = false;
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
