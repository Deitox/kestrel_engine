let state = #{ timer: 0.0, on: true };

fn ready(world, entity) {
    state.timer = 0.0;
    state.on = true;
    world.log("blinker ready for entity " + entity.to_string());
}

fn process(world, entity, dt) {
    if dt <= 0.0 {
        return;
    }
    state.timer += dt;
    if state.timer >= 0.4 {
        state.timer = 0.0;
        state.on = !state.on;
        if state.on {
            world.entity_set_tint(entity, 0.9, 0.3, 0.3, 1.0);
        } else {
            world.entity_clear_tint(entity);
        }
    }
}
