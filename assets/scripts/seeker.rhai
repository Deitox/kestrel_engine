// Sample behaviour using input helpers + raycast/overlap queries.
// Attach to an entity to steer with WASD and tint when facing a nearby collider.
import "common" as common;

const SPEED = 4.0;
const AVOID_DISTANCE = 2.0;
const NEAR_RADIUS = 3.0;

fn ready(world, entity) {
    world.log("seeker ready: use WASD/arrow keys");
}

fn process(world, entity, dt) {
    let dir = read_input(world);
    if dir[0] == 0.0 && dir[1] == 0.0 {
        world.entity_set_velocity(entity, 0.0, 0.0);
        world.entity_clear_tint(entity);
        return;
    }
    dir = normalize(dir);

    let pos = world.entity_position(entity);
    let hit = world.raycast(pos[0], pos[1], dir[0], dir[1], AVOID_DISTANCE, #{ exclude: [entity] });
    if hit.len() > 0 && ("point" in hit) {
        // Tint to warn we are staring at an obstacle.
        world.entity_set_tint(entity, 1.0, 0.4, 0.4, 1.0);
    } else {
        world.entity_clear_tint(entity);
    }

    // Simple proximity log.
    let nearby = world.overlap_circle(pos[0], pos[1], NEAR_RADIUS, #{ exclude: [entity] });
    if nearby.len() > 0 {
        world.log("seeker sees " + nearby.len().to_string() + " neighbours");
    }

    world.entity_set_velocity(entity, dir[0] * SPEED, dir[1] * SPEED);
}

fn read_input(world) {
    let dx = bool_to_float(world.input_right()) - bool_to_float(world.input_left());
    let dy = bool_to_float(world.input_forward()) - bool_to_float(world.input_backward());
    [dx, dy]
}

fn normalize(v) {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len <= 0.0001 {
        [0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len]
    }
}

fn bool_to_float(b) {
    if b { 1.0 } else { 0.0 }
}
