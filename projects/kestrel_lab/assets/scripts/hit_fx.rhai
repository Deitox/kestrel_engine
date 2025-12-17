let state = #{ age: 0.0 };

fn ready(world, entity) {
    state.age = 0.0;
    world.entity_set_tint(entity, 1.0, 1.0, 0.2, 0.8);
    return;
}

fn process(world, entity, dt) {
    if dt <= 0.0 {
        return;
    }

    state.age += dt;

    let lifetime = 0.22;
    let t = (state.age / lifetime).min(1.0);
    let scale = 0.22 * (1.0 - t) + 0.02;
    world.entity_set_scale(entity, scale, scale);
    world.entity_set_tint(entity, 1.0, 1.0, 0.2, 0.8 * (1.0 - t));

    if state.age >= lifetime {
        world.entity_despawn(entity);
    }
    return;
}
