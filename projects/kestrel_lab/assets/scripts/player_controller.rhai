let state = #{
    fire_prev: false,
    fire_cooldown: 0.0,
    aim_x: 1.0,
    aim_y: 0.0
};

fn ready(world, _entity) {
    world.log("player_controller ready");
}

fn process(world, entity, dt) {
    if dt <= 0.0 {
        return;
    }

    let pos = world.entity_position(entity);
    if pos.len() < 2 {
        return;
    }
    let x = pos[0];
    let y = pos[1];

    let speed = if world.input_boost() { 1.9 } else { 1.2 };
    let dx = 0.0;
    let dy = 0.0;
    if world.input_left() {
        dx -= 1.0;
    }
    if world.input_right() {
        dx += 1.0;
    }
    if world.input_forward() {
        dy += 1.0;
    }
    if world.input_backward() {
        dy -= 1.0;
    }

    let len = (dx * dx + dy * dy).sqrt();
    let mvx = if len > 0.0001 { dx / len } else { 0.0 };
    let mvy = if len > 0.0001 { dy / len } else { 0.0 };
    if len > 0.0001 {
        state.aim_x = mvx;
        state.aim_y = mvy;
    }

    let cursor_world = world.input_cursor_world();
    if cursor_world.len() >= 2 {
        let ax = cursor_world[0] - x;
        let ay = cursor_world[1] - y;
        let alen = (ax * ax + ay * ay).sqrt();
        if alen > 0.0001 {
            state.aim_x = ax / alen;
            state.aim_y = ay / alen;
        }
    }

    world.entity_set_velocity(entity, mvx * speed, mvy * speed);

    state.fire_cooldown = (state.fire_cooldown - dt).max(0.0);
    let fire = world.input_left_mouse();
    if fire && !state.fire_prev && state.fire_cooldown <= 0.0 {
        state.fire_cooldown = 0.12;
        spawn_bullet(world, x, y, state.aim_x, state.aim_y);
    }
    state.fire_prev = fire;
}

fn spawn_bullet(world, x, y, dx, dy) {
    let bullet = world.spawn_template_safe("bullet");
    if bullet == () {
        world.log("spawn_bullet failed");
        return;
    }
    world.set_position(bullet, x + dx * 0.32, y + dy * 0.32);
    world.set_velocity(bullet, dx * 3.0, dy * 3.0);
}
