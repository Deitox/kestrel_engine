let logged = false;

fn ready(world, entity) {
    if logged { return; };
    logged = true;

    let hits = world.overlap_circle(0.0, 0.0, 10.0);
    let overlap = "overlap:".to_string();
    let first = true;
    for h in hits {
        if first {
            first = false;
        } else {
            overlap += ",";
        }
        overlap += h.to_string();
    }
    world.log(overlap);

    let hit = world.raycast(0.0, 0.0, 1.0, 0.0, 20.0);
    let eid = hit["entity"];
    let dist = hit["distance"];
    world.log("ray:" + eid.to_string() + "@" + dist.to_string());

    let first_handle = world.spawn_sprite("main", "green", 1.0, 0.0, 0.25, 0.0, 0.0);
    let second_handle = world.spawn_sprite("main", "green", -1.0, 0.0, 0.25, 0.0, 0.0);
    world.set_position(first_handle, 0.5, 0.0);
    world.set_position(second_handle, -0.5, 0.0);
    world.log("spawned:" + first_handle.to_string() + "," + second_handle.to_string());
}

fn process(world, entity, dt) { }
