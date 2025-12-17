let state = #{ age: 0.0 };

fn ready(_world, _entity) {
    state.age = 0.0;
}

fn process(world, entity, dt) {
    if dt <= 0.0 {
        return;
    }
    state.age += dt;
    if state.age >= 2.0 {
        despawn_self(world, entity);
        return;
    }

    let pos = world.entity_position(entity);
    if pos.len() < 2 {
        return;
    }
    let scale = world.entity_scale(entity);
    if scale.len() < 2 {
        return;
    }

    let radius = (scale[0].abs().max(scale[1].abs())) * 0.6;
    let hits = world.overlap_circle_hits(pos[0], pos[1], radius);
    for hit in hits {
        let other = hit["entity"];
        if other == entity {
            continue;
        }
        if world.entity_tag(other) == "target" {
            let payload = #{
                x: pos[0],
                y: pos[1],
                target: other
            };
            world.emit_to("target_destroyed", other, payload);
            despawn_target(world, other);
            despawn_self(world, entity);
            return;
        }
    }
}

fn despawn_self(world, entity) {
    let handle = world.entity_handle(entity);
    if handle == () {
        world.entity_despawn(entity);
    } else {
        world.despawn_safe(handle);
    }
}

fn despawn_target(world, entity) {
    let handle = world.entity_handle(entity);
    if handle == () {
        world.entity_despawn(entity);
    } else {
        world.despawn_safe(handle);
    }
}
