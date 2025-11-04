use bevy_ecs::prelude::Entity;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{
    EcsWorld, SceneEntityTag, Sprite, SpriteAnimation, SpriteAnimationLoopMode, Transform, WorldTransform,
};
use kestrel_engine::events::GameEvent;
use kestrel_engine::scene::SceneEntityId;
use serde_json::json;
use std::sync::Arc;
use tempfile::NamedTempFile;

fn sprite_region(world: &EcsWorld, entity: Entity) -> String {
    world.world.get::<Sprite>(entity).expect("sprite component missing").region.to_string()
}

#[test]
fn atlas_timelines_expose_metadata() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");
    let mut names = assets.atlas_timeline_names("main");
    names.sort();
    assert!(names.contains(&"demo_cycle".to_string()), "demo_cycle timeline should be present");
    let timeline = assets.atlas_timeline("main", "demo_cycle").expect("demo_cycle timeline available");
    assert!(timeline.looped);
    assert_eq!(timeline.frames.len(), 3);
    assert!(
        (timeline.frames[0].duration - 0.12).abs() < f32::EPSILON,
        "timeline should preserve frame duration"
    );
}

#[test]
fn sprite_animation_advances_and_resets() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(
        ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")),
        "should attach demo_cycle timeline"
    );
    let initial = sprite_region(&ecs, entity);
    ecs.update(0.2);
    let advanced = sprite_region(&ecs, entity);
    assert_ne!(initial, advanced, "animation should advance after enough time");

    ecs.set_sprite_animation_playing(entity, false);
    ecs.update(1.0);
    let paused = sprite_region(&ecs, entity);
    assert_eq!(advanced, paused, "paused animation should not advance");

    ecs.set_sprite_animation_playing(entity, true);
    ecs.set_sprite_animation_speed(entity, 1.5);
    let mut resumed = paused.clone();
    for _ in 0..10 {
        ecs.update(0.1);
        resumed = sprite_region(&ecs, entity);
        if resumed != paused {
            break;
        }
    }
    assert_ne!(paused, resumed, "animation should advance after resuming");

    ecs.reset_sprite_animation(entity);
    let reset = sprite_region(&ecs, entity);
    assert_eq!(initial, reset, "reset should snap back to first frame");
}

#[test]
fn sprite_animation_seek_updates_frame() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));

    // Seek to the final frame (green) and verify sprite region updates immediately.
    assert!(ecs.seek_sprite_animation_frame(entity, 2));
    let region = sprite_region(&ecs, entity);
    assert_eq!(region, "green");

    // Seeking beyond the available frames should clamp to the last frame.
    assert!(ecs.seek_sprite_animation_frame(entity, 99));
    let clamped = sprite_region(&ecs, entity);
    assert_eq!(clamped, "green");

    // Seek back to the first frame.
    assert!(ecs.seek_sprite_animation_frame(entity, 0));
    let first = sprite_region(&ecs, entity);
    assert_eq!(first, "redorb");
}

#[test]
fn sprite_animation_hot_reload_preserves_frame() {
    let temp = NamedTempFile::new().expect("temp atlas");
    let source = std::fs::read("assets/images/atlas.json").expect("read atlas");
    std::fs::write(temp.path(), &source).expect("write copy");
    let temp_path = temp.path().to_path_buf();

    let mut assets = AssetManager::new();
    assets.retain_atlas("main", temp_path.to_str()).expect("load atlas from temp");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    ecs.update(0.2); // advance into middle frame
    assert_eq!(sprite_region(&ecs, entity), "bluebox");

    let anim_before =
        ecs.world.get::<kestrel_engine::ecs::SpriteAnimation>(entity).expect("animation component");
    let prev_elapsed = anim_before.elapsed_in_frame;
    let prev_duration = anim_before.frames[anim_before.frame_index].duration;
    let prev_forward = anim_before.forward;

    let mut atlas_json: serde_json::Value = serde_json::from_slice(&source).expect("parse atlas");
    atlas_json["animations"]["demo_cycle"]["frames"] = json!([
        { "region": "green", "duration_ms": 90 },
        { "region": "redorb", "duration_ms": 110 },
        { "region": "bluebox", "duration_ms": 170 }
    ]);
    std::fs::write(&temp_path, serde_json::to_vec_pretty(&atlas_json).expect("encode"))
        .expect("write modified atlas");

    assets.reload_atlas("main").expect("hot reload atlas");
    let updated = ecs.refresh_sprite_animations_for_atlas("main", &assets);
    assert_eq!(updated, 1, "one animation should refresh");

    let anim = ecs.world.get::<kestrel_engine::ecs::SpriteAnimation>(entity).expect("animation component");
    assert_eq!(anim.frames.len(), 3);
    assert_eq!(anim.current_region_name(), Some("bluebox"));
    assert_eq!(anim.frame_index, 2, "frame should track region by name");
    let expected_elapsed = {
        let prev_duration = prev_duration.max(std::f32::EPSILON);
        let progress = (prev_elapsed / prev_duration).clamp(0.0, 1.0);
        let new_duration = anim.frames[anim.frame_index].duration;
        progress * new_duration
    };
    assert!(
        (anim.elapsed_in_frame - expected_elapsed).abs() < 1e-6,
        "elapsed time should scale with new frame duration"
    );
    assert_eq!(anim.forward, prev_forward, "playback direction should remain unchanged");

    let sprite = ecs.world.get::<Sprite>(entity).expect("sprite component");
    assert_eq!(sprite.region.as_ref(), "bluebox");
}

#[test]
fn sprite_animation_ping_pong_reverses_direction() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    assert!(ecs.set_sprite_animation_loop_mode(entity, SpriteAnimationLoopMode::PingPong));

    ecs.update(0.12);
    let first_forward = ecs
        .world
        .get::<kestrel_engine::ecs::SpriteAnimation>(entity)
        .expect("animation component")
        .frame_index;
    ecs.update(0.12);
    let reached_end = ecs
        .world
        .get::<kestrel_engine::ecs::SpriteAnimation>(entity)
        .expect("animation component")
        .frame_index;
    ecs.update(0.12);
    let reversed = ecs
        .world
        .get::<kestrel_engine::ecs::SpriteAnimation>(entity)
        .expect("animation component")
        .frame_index;

    assert!(first_forward > 0, "animation should advance forward before reversing");
    assert!(reached_end >= first_forward, "animation should hit the end frame");
    assert!(
        reversed < reached_end,
        "animation should walk backward after reaching the end in ping-pong mode"
    );
}

#[test]
fn sprite_animation_once_hold_stays_on_last_frame() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    assert!(ecs.set_sprite_animation_loop_mode(entity, SpriteAnimationLoopMode::OnceHold));

    ecs.update(1.0);
    let anim = ecs.world.get::<kestrel_engine::ecs::SpriteAnimation>(entity).expect("animation component");
    assert!(!anim.playing, "once_hold should stop playback");
    assert_eq!(
        anim.frame_index,
        anim.frame_count().saturating_sub(1),
        "animation should remain on the last frame"
    );
    let held_region = sprite_region(&ecs, entity);
    assert_eq!(held_region, "green", "last frame should remain active");
}

#[test]
fn sprite_animation_events_emit_on_frame_entry() {
    let temp = NamedTempFile::new().expect("temp atlas");
    let source = std::fs::read("assets/images/atlas.json").expect("read atlas");
    let mut atlas_json: serde_json::Value = serde_json::from_slice(&source).expect("parse atlas");
    atlas_json["animations"]["demo_cycle"]["events"] = json!([{ "frame": 1, "name": "footstep" }]);
    atlas_json["animations"]["demo_cycle"]["loop_mode"] = json!("loop");
    std::fs::write(&temp, serde_json::to_vec_pretty(&atlas_json).expect("encode"))
        .expect("write modified atlas");
    let temp_path = temp.path().to_path_buf();

    let mut assets = AssetManager::new();
    assets.retain_atlas("main", temp_path.to_str()).expect("load atlas with events");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));

    ecs.drain_events();
    ecs.update(0.12);
    let events = ecs.drain_events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, GameEvent::SpriteAnimationEvent { event, .. } if event.as_ref() == "footstep")),
        "animation should emit declared events when entering the frame"
    );
}

#[test]
fn sprite_animation_info_reports_frame_metadata() {
    let temp = NamedTempFile::new().expect("temp atlas");
    let source = std::fs::read("assets/images/atlas.json").expect("read atlas");
    let mut atlas_json: serde_json::Value = serde_json::from_slice(&source).expect("parse atlas");
    atlas_json["animations"]["demo_cycle"]["events"] = json!([{ "frame": 1, "name": "footstep" }]);
    std::fs::write(&temp, serde_json::to_vec_pretty(&atlas_json).expect("encode"))
        .expect("write modified atlas");
    let temp_path = temp.path().to_path_buf();

    let mut assets = AssetManager::new();
    assets.retain_atlas("main", temp_path.to_str()).expect("load atlas with metadata");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
            SceneEntityTag::new(SceneEntityId::new()),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    assert!(ecs.seek_sprite_animation_frame(entity, 1));

    let info = ecs.entity_info(entity).expect("entity info available");
    let sprite_info = info.sprite.expect("sprite info");
    let anim_info = sprite_info.animation.expect("animation info");

    assert_eq!(anim_info.frame_index, 1);
    assert_eq!(anim_info.frame_region.as_deref(), Some("bluebox"));
    assert!((anim_info.frame_duration - 0.12).abs() < f32::EPSILON);
    assert!(anim_info.frame_elapsed.abs() < f32::EPSILON);
    assert!(
        anim_info.frame_events.iter().any(|event| event == "footstep"),
        "frame metadata should surface declared events"
    );
}

#[test]
fn sprite_animation_hot_reload_handles_duplicate_regions() {
    let temp = NamedTempFile::new().expect("temp atlas");
    let source = std::fs::read("assets/images/atlas.json").expect("read atlas");
    let mut atlas_json: serde_json::Value = serde_json::from_slice(&source).expect("parse atlas");
    atlas_json["animations"]["demo_cycle"]["frames"] = json!([
        { "region": "redorb", "duration_ms": 100 },
        { "region": "bluebox", "duration_ms": 100 },
        { "region": "redorb", "duration_ms": 100 },
        { "region": "green", "duration_ms": 100 }
    ]);
    std::fs::write(temp.path(), serde_json::to_vec_pretty(&atlas_json).expect("encode"))
        .expect("write initial atlas");
    let temp_path = temp.path().to_path_buf();

    let mut assets = AssetManager::new();
    assets.retain_atlas("main", temp_path.to_str()).expect("load atlas with duplicates");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    assert!(ecs.seek_sprite_animation_frame(entity, 2)); // second occurrence of redorb
    {
        let mut anim =
            ecs.world.get_mut::<kestrel_engine::ecs::SpriteAnimation>(entity).expect("animation component");
        anim.forward = false;
        anim.elapsed_in_frame = 0.05;
    }

    let mut atlas_json = serde_json::from_slice::<serde_json::Value>(&source).expect("parse atlas");
    atlas_json["animations"]["demo_cycle"]["frames"] = json!([
        { "region": "redorb", "duration_ms": 80 },
        { "region": "green", "duration_ms": 90 },
        { "region": "redorb", "duration_ms": 120 },
        { "region": "bluebox", "duration_ms": 70 },
        { "region": "redorb", "duration_ms": 150 }
    ]);
    std::fs::write(&temp_path, serde_json::to_vec_pretty(&atlas_json).expect("encode"))
        .expect("write modified atlas");

    assets.reload_atlas("main").expect("reload atlas");
    let refreshed = ecs.refresh_sprite_animations_for_atlas("main", &assets);
    assert_eq!(refreshed, 1);

    let anim = ecs.world.get::<kestrel_engine::ecs::SpriteAnimation>(entity).expect("animation component");
    assert_eq!(anim.frames.len(), 5);
    assert_eq!(anim.current_region_name(), Some("redorb"));
    assert_eq!(anim.frame_index, 2, "should remain on the matching occurrence");
    let expected_elapsed = {
        let progress = (0.05f32 / 0.1f32).clamp(0.0, 1.0);
        let new_duration = anim.frames[anim.frame_index].duration;
        progress * new_duration
    };
    assert!(
        (anim.elapsed_in_frame - expected_elapsed).abs() < 1e-6,
        "elapsed time should scale with new duration"
    );
    assert!(!anim.forward, "playback direction should persist");

    let sprite = ecs.world.get::<Sprite>(entity).expect("sprite component");
    assert_eq!(sprite.region.as_ref(), "redorb");
}

#[test]
fn sprite_animation_hot_reload_prefers_frame_names() {
    let temp = NamedTempFile::new().expect("temp atlas");
    let source = std::fs::read("assets/images/atlas.json").expect("read atlas");
    let mut atlas_json: serde_json::Value = serde_json::from_slice(&source).expect("parse atlas");
    atlas_json["animations"]["demo_cycle"]["frames"] = json!([
        { "name": "idle_a", "region": "redorb", "duration_ms": 160 },
        { "name": "idle_b", "region": "bluebox", "duration_ms": 180 },
        { "name": "idle_c", "region": "green", "duration_ms": 140 }
    ]);
    std::fs::write(temp.path(), serde_json::to_vec_pretty(&atlas_json).expect("encode"))
        .expect("write initial atlas");
    let temp_path = temp.path().to_path_buf();

    let mut assets = AssetManager::new();
    assets.retain_atlas("main", temp_path.to_str()).expect("load atlas with names");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    assert!(ecs.seek_sprite_animation_frame(entity, 1));
    {
        let mut anim =
            ecs.world.get_mut::<kestrel_engine::ecs::SpriteAnimation>(entity).expect("animation component");
        anim.forward = false;
        anim.elapsed_in_frame = 0.09;
    }

    let mut modified = atlas_json.clone();
    modified["animations"]["demo_cycle"]["frames"] = json!([
        { "name": "idle_c", "region": "green", "duration_ms": 140 },
        { "name": "idle_b", "region": "checker", "duration_ms": 150 },
        { "name": "idle_a", "region": "redorb", "duration_ms": 160 },
        { "name": "idle_d", "region": "bluebox", "duration_ms": 110 }
    ]);
    std::fs::write(&temp_path, serde_json::to_vec_pretty(&modified).expect("encode"))
        .expect("write modified atlas");

    assets.reload_atlas("main").expect("reload atlas");
    let refreshed = ecs.refresh_sprite_animations_for_atlas("main", &assets);
    assert_eq!(refreshed, 1);

    let anim = ecs.world.get::<kestrel_engine::ecs::SpriteAnimation>(entity).expect("animation component");
    assert_eq!(anim.frames.len(), 4);
    assert_eq!(anim.frame_index, 1, "should stay aligned with the frame name");
    assert_eq!(anim.frames[anim.frame_index].name.as_ref(), "idle_b");
    assert_eq!(anim.current_region_name(), Some("checker"));
    let expected_elapsed = {
        let prev_duration = 0.18_f32;
        let progress = (0.09_f32 / prev_duration).clamp(0.0, 1.0);
        let new_duration = anim.frames[anim.frame_index].duration;
        progress * new_duration
    };
    assert!(
        (anim.elapsed_in_frame - expected_elapsed).abs() < 1e-6,
        "elapsed time should scale with new duration"
    );
    assert!(!anim.forward, "playback direction should persist");

    let sprite = ecs.world.get::<Sprite>(entity).expect("sprite component");
    assert_eq!(sprite.region.as_ref(), "checker");
}

#[test]
fn sprite_animation_respects_start_offset() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    assert!(ecs.set_sprite_animation_start_offset(entity, 0.24));
    let region = sprite_region(&ecs, entity);
    assert_eq!(region, "green", "start_offset should pre-advance the animation before playback begins");
}

#[test]
fn sprite_animation_random_start_is_stable() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");

    let mut world_a = EcsWorld::new();
    let entity_a = world_a
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(world_a.set_sprite_timeline(entity_a, &assets, Some("demo_cycle")));
    assert!(world_a.set_sprite_animation_random_start(entity_a, true));
    let region_a = sprite_region(&world_a, entity_a);

    let mut world_b = EcsWorld::new();
    let entity_b = world_b
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(world_b.set_sprite_timeline(entity_b, &assets, Some("demo_cycle")));
    assert!(world_b.set_sprite_animation_random_start(entity_b, true));
    let region_b = sprite_region(&world_b, entity_b);

    assert_eq!(
        region_a, region_b,
        "deterministic random_start should produce repeatable phase offsets for identical entities"
    );
}

#[test]
fn animation_time_scales_and_gates_playback() {
    let mut assets = AssetManager::new();
    assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("load main atlas");
    let mut ecs = EcsWorld::new();
    let entity = ecs
        .world
        .spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));

    {
        let mut anim_time = ecs.world.resource_mut::<kestrel_engine::ecs::AnimationTime>();
        anim_time.scale = 0.5;
    }
    ecs.update(0.25);
    let region_scaled = sprite_region(&ecs, entity);
    assert_eq!(
        region_scaled, "bluebox",
        "half-speed playback should advance as though delta time were halved"
    );

    {
        let mut anim_time = ecs.world.resource_mut::<kestrel_engine::ecs::AnimationTime>();
        anim_time.paused = true;
    }
    ecs.update(1.0);
    let region_paused = sprite_region(&ecs, entity);
    assert_eq!(region_paused, "bluebox", "paused animation time should prevent frame advancement");

    {
        let mut anim_time = ecs.world.resource_mut::<kestrel_engine::ecs::AnimationTime>();
        anim_time.paused = false;
        anim_time.scale = 1.0;
        anim_time.set_fixed_step(Some(0.12));
        anim_time.remainder = 0.0;
    }
    {
        if let Some(mut animation) = ecs.world.entity_mut(entity).get_mut::<SpriteAnimation>() {
            animation.elapsed_in_frame = 0.0;
            animation.current_duration = 0.12;
        }
    }
    ecs.update(0.06);
    let interim = sprite_region(&ecs, entity);
    assert_eq!(
        interim, "bluebox",
        "fixed-step playback should avoid advancing until the step threshold is reached"
    );
    ecs.update(0.12);
    let still_blue = sprite_region(&ecs, entity);
    assert_eq!(
        still_blue, "bluebox",
        "fixed-step playback should consume exactly one step without advancing"
    );
    ecs.update(0.12);
    let stepped = sprite_region(&ecs, entity);
    assert_ne!(stepped, "bluebox", "fixed-step playback should advance once subsequent steps accumulate");
    {
        let mut anim_time = ecs.world.resource_mut::<kestrel_engine::ecs::AnimationTime>();
        anim_time.set_fixed_step(None);
    }
}
