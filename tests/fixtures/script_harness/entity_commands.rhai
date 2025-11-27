fn ready(world, entity) {
    world.entity_set_position(entity, 1.0, 2.0);
    world.entity_set_velocity(entity, 0.5, 0.0);
    world.log("ready");
}

fn process(world, entity, dt) {
    let pos = world.entity_position(entity);
    let x = if pos.len() > 0 { pos[0] } else { 0.0 };
    let y = if pos.len() > 1 { pos[1] } else { 0.0 };
    world.log("pos:" + x.to_string() + "," + y.to_string());
}
