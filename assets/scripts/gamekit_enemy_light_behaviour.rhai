// Light enemy: damages player on overlap.

let damage = 1.0;
let radius = 0.3;
const TEMPLATE = "gamekit_enemy_light";

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
            world.emit("kit_enemy_contact", #{ damage: damage, source: TEMPLATE });
        }
        idx += 1;
    }
}
