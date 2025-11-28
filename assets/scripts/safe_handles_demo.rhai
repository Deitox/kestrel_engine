// Demonstrates safe spawn/despawn and reload hygiene for handles.
let handles = #{
    enemies: [],
};

fn ready(world, _entity) {
    cleanup_handles(world);
    spawn_enemy_safe(world);
}

fn process(world, _entity, dt) {
    if dt <= 0.0 {
        return;
    }
    cleanup_handles(world);
    for enemy in handles.enemies.clone() {
        if world.handle_is_alive(enemy) {
            // Move each enemy slightly to the right to show activity.
            world.set_position(enemy, 0.25, 0.0);
        }
    }
}

fn spawn_enemy_safe(world) {
    let h = world.spawn_enemy_safe("enemy", "enemy");
    if h == () {
        world.log("[demo] enemy spawn failed");
        return;
    }
    if world.handle_is_alive(h) {
        handles.enemies.push(h);
        world.log("[demo] spawned enemy handle: " + h.to_string());
    }
}

fn cleanup_handles(world) {
    // Drop any dead handles on hot reload or after despawns.
    handles.enemies = handles.enemies.filter(|h| world.handle_validate(*h) != ());
}
