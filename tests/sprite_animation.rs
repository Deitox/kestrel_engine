use bevy_ecs::prelude::Entity;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{EcsWorld, Sprite, SpriteAnimationLoopMode, Transform, WorldTransform};
use kestrel_engine::events::GameEvent;
use serde_json::json;
use std::borrow::Cow;
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
            Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
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
            Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
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
            Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));
    ecs.update(0.2); // advance into middle frame
    assert_eq!(sprite_region(&ecs, entity), "bluebox");

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
            Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
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
            Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
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
            Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
        ))
        .id();
    assert!(ecs.set_sprite_timeline(entity, &assets, Some("demo_cycle")));

    ecs.drain_events();
    ecs.update(0.12);
    let events = ecs.drain_events();
    assert!(
        events.iter().any(
            |event| matches!(event, GameEvent::SpriteAnimationEvent { event, .. } if event == "footstep")
        ),
        "animation should emit declared events when entering the frame"
    );
}
