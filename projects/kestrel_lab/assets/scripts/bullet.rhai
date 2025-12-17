let state = #{ age: 0.0 };

fn ready(_world, _entity) {
    state.age = 0.0;
}

fn process(world, entity, dt) {
    if dt <= 0.0 {
        return;
    }
    state.age += dt;
    if state.age >= 2.0 {
        world.entity_despawn(entity);
    }
}

