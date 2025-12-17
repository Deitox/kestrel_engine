let state = #{
    fire_prev: false,
    fire_cooldown: 0.0,
    aim_x: 1.0,
    aim_y: 0.0
};

fn ready(world, _entity) {
    world.log("player_controller ready");
}

fn aim_from_cursor(world) {
    let cursor = world.input_cursor();
    if cursor.len() < 2 {
        return [0.0, 0.0];
    }

    // Derived from Kestrel Studio camera defaults (Ortho2D, base_half_height=1.2, zoom=1.0)
    // and this project's window size (1280x720).
    let window_w = 1280.0;
    let window_h = 720.0;
    let half_w_px = window_w * 0.5;
    let half_h_px = window_h * 0.5;
    let aspect = window_w / window_h;
    let half_h_world = 1.2;
    let half_w_world = half_h_world * aspect;

    let dx = ((cursor[0] - half_w_px) / half_w_px) * half_w_world;
    let dy = ((half_h_px - cursor[1]) / half_h_px) * half_h_world;
    let len = (dx * dx + dy * dy).sqrt();
    if len <= 0.0001 {
        return [0.0, 0.0];
    }
    [dx / len, dy / len]
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

    let aim = aim_from_cursor(world);
    if aim[0].abs() > 0.0001 || aim[1].abs() > 0.0001 {
        state.aim_x = aim[0];
        state.aim_y = aim[1];
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
