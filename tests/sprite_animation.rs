use bevy_ecs::prelude::Entity;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{EcsWorld, Sprite, Transform, WorldTransform};
use serde_json::json;
use std::borrow::Cow;
use std::io::Write;
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
    let mut temp = NamedTempFile::new().expect("temp atlas");
    let source = std::fs::read("assets/images/atlas.json").expect("read atlas");
    temp.write_all(&source).expect("write copy");
    temp.flush().expect("flush copy");
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
