fn ready(world, entity) {
    world.log("spinner ready for entity " + entity.to_string());
}

fn process(world, entity, dt) {
    let speed = 1.5; // radians per second
    world.entity_set_rotation(entity, speed * dt);
}
