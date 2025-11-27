// Dash enemy: moves toward the player with contact damage.

import "common" as common;

let damage = 1.5;
let radius = 0.32;
let speed = 1.6;
const TEMPLATE = "gamekit_enemy_dash";

fn ready(world, entity) { }

fn process(world, entity, dt) {
    let pos = world.entity_position(entity);
    if pos.len() < 2 {
        return;
    }
    let hits = world.overlap_circle_hits(pos[0], pos[1], radius);
    let idx = 0;
    while idx < hits.len() {
        let other = hits[idx]["entity"];
        if other != entity {
            world.emit("kit_enemy_contact", #{ damage: damage, source: TEMPLATE, target: other });
        }
        idx += 1;
    }
    // Movement is set on spawn via kit helpers; this behaviour focuses on contact damage.
}
