let state = #{ timer: 0.0 };

fn init(world) {
    world.log("Script init");
    world.set_auto_spawn_rate(0.0);
    world.set_spawn_per_press(100);
    world.set_emitter_spread(1.0);
    world.set_emitter_speed(1.0);
    world.set_emitter_lifetime(1.2);
    world.set_emitter_start_size(0.18);
    world.set_emitter_end_size(0.05);
    world.set_emitter_start_color(1.0, 0.8, 0.2, 0.8);
    world.set_emitter_end_color(1.0, 0.2, 0.2, 0.0);
    state.timer = 0.0;
}

fn update(world, dt) {
    if dt <= 0.0 {
        return;
    }

    state.timer += dt;
    while state.timer >= 1.0 {
        state.timer -= 1.0;
        spawn_entity(world);
    }
}

fn spawn_entity(world) {
    let x = world.rand(-1.1, 1.1);
    let y = world.rand(-0.8, 0.8);
    let handle = world.spawn_sprite("main", "green", x, y, 0.25, 0.0, 0.0);
    if handle >= 0 {
        world.log("Spawned entity handle: " + handle.to_string());
    } else {
        world.log("Failed to spawn entity: " + handle.to_string());
    }
    let angle = world.rand(0.0, 6.28318);
    let speed = world.rand(0.35, 0.95);
    let vx = speed * cos(angle);
    let vy = speed * sin(angle);
    world.set_velocity(handle, vx, vy);
}
