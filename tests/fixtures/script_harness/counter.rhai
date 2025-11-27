fn ready(world, entity) {
    let c = world.state_get("count");
    if type_of(c) == "()" { c = 0; }
    world.log("count:" + c.to_string());
    world.state_set("count", c + 1);
}

fn process(world, entity, dt) { }
