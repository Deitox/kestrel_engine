fn init(world) {
    world.log("Script init");
}

fn update(world, dt) {
    if dt <= 0.0 {
        return;
    }

    let remaining = dt;
    while remaining >= 1.0 {
        spawn_entity(world);
        remaining -= 1.0;
    }

    if world.rand(0.0, 1.0) < remaining {
        spawn_entity(world);
    }
}

fn spawn_entity(world) {
    let x = world.rand(-1.1, 1.1);
    let y = world.rand(-0.8, 0.8);
    let entity = world.spawn_sprite("main", "green", x, y, 0.25, 0.0, 0.0);
    if entity >= 0 {
        world.log("Spawned entity: " + entity.to_string());
    }
}
