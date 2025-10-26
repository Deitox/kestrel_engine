use bevy_ecs::prelude::Entity;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{EcsWorld, Sprite, Transform, WorldTransform};
use std::borrow::Cow;

fn sprite_region(world: &EcsWorld, entity: Entity) -> String {
    world
        .world
        .get::<Sprite>(entity)
        .expect("sprite component missing")
        .region
        .to_string()
}

#[test]
fn atlas_timelines_expose_metadata() {
    let mut assets = AssetManager::new();
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("load main atlas");
    let mut names = assets.atlas_timeline_names("main");
    names.sort();
    assert!(
        names.contains(&"demo_cycle".to_string()),
        "demo_cycle timeline should be present"
    );
    let timeline = assets
        .atlas_timeline("main", "demo_cycle")
        .expect("demo_cycle timeline available");
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
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("load main atlas");
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
