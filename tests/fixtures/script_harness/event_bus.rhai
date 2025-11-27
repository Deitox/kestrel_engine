fn ready(world, entity) {
    world.listen("ping", "on_ping");
    world.emit("ping", #{ value: 5 });
}

fn on_ping(world, event) {
    let payload = event["payload"]["value"];
    world.log("pong:" + payload.to_string());
}

fn process(world, entity, dt) { }
