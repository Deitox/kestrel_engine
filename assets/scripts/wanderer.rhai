let state = #{ timer: 0.0 };

fn ready(world, entity) {
    world.log("wanderer ready for entity " + entity.to_string());
    state.timer = 0.0;
}

fn process(world, entity, dt) {
    if dt <= 0.0 {
        return;
    }
    state.timer += dt;
    if state.timer >= 0.45 {
        state.timer = 0.0;
        let angle = world.rand(0.0, 6.28318);
        let speed = world.rand(0.25, 0.65);
        let vx = speed * cos(angle);
        let vy = speed * sin(angle);
        world.entity_set_velocity(entity, vx, vy);
    }
}
