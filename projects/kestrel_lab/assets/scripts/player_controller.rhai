let state = #{
    facing: 1.0,
    fire_prev: false,
    fire_cooldown: 0.0,
    jump_prev: false
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
    let vx = 0.0;
    if world.input_left() {
        vx -= speed;
    }
    if world.input_right() {
        vx += speed;
    }
    if vx < -0.01 {
        state.facing = -1.0;
    } else if vx > 0.01 {
        state.facing = 1.0;
    }

    let vel = world.entity_velocity(entity);
    let vy = if vel.len() >= 2 { vel[1] } else { 0.0 };

    let jump = world.input_ascend();
    let on_ground = y <= -0.79 && vy.abs() < 0.05;
    if jump && !state.jump_prev && on_ground {
        vy = 1.35;
    }
    state.jump_prev = jump;

    world.entity_set_velocity(entity, vx, vy);

    state.fire_cooldown = (state.fire_cooldown - dt).max(0.0);
    let fire = world.input_left_mouse();
    if fire && !state.fire_prev && state.fire_cooldown <= 0.0 {
        state.fire_cooldown = 0.12;
        spawn_bullet(world, x, y, state.facing);
    }
    state.fire_prev = fire;
}

fn spawn_bullet(world, x, y, facing) {
    let bullet = world.spawn_template_safe("bullet");
    if bullet == () {
        world.log("spawn_bullet failed");
        return;
    }
    world.set_position(bullet, x + facing * 0.32, y);
    world.set_velocity(bullet, facing * 3.0, 0.0);
}
