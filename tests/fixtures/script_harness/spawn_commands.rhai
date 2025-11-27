fn ready(world, entity) {
    world.log("ready");
    world.spawn_template("enemy");
    world.spawn_prefab("assets/prefabs/example.json");
}

fn process(world, entity, dt) { }
