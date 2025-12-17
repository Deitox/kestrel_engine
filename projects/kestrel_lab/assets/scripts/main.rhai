fn init(world) {
    world.log("Kestrel Lab: init");
    world.rand_seed(1);
    world.stat_set("score", 0);
    spawn_wave(world, 8);
}

fn update(world, _dt) {
    let targets = world.handles_with_tag("target");
    if targets.len() == 0 {
        spawn_wave(world, 8);
    }
}

fn spawn_wave(world, count) {
    for i in 0..count {
        let target = world.spawn_template_safe("target", "target");
        if target == () {
            world.log("spawn_wave: target spawn failed");
            return;
        }
        let x = world.rand(-1.1, 1.1);
        let y = world.rand(-0.75, 0.75);
        world.set_position(target, x, y);

        let vx = world.rand(-0.45, 0.45);
        let vy = world.rand(-0.45, 0.45);
        world.set_velocity(target, vx, vy);
    }
}
