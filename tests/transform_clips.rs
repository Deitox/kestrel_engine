use glam::{Vec2, Vec4};
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{EcsWorld, Tint, Transform, WorldTransform};

fn approx_vec2(a: Vec2, b: Vec2) -> bool {
    (a - b).length_squared() <= 1e-6
}

fn approx_vec4(a: Vec4, b: Vec4) -> bool {
    (a - b).length_squared() <= 1e-6
}

fn approx_scalar(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5
}

#[test]
fn transform_clip_drives_transform_and_tint() {
    let mut assets = AssetManager::new();
    assets.retain_clip("slime", Some("fixtures/animation_clips/slime_bob.json")).expect("load slime clip");

    let mut ecs = EcsWorld::new();
    let entity = ecs.world.spawn((Transform::default(), WorldTransform::default(), Tint(Vec4::ONE))).id();

    assert!(ecs.set_transform_clip(entity, &assets, "slime"), "attach clip");

    {
        let transform = ecs.world.get::<Transform>(entity).unwrap();
        assert!(approx_vec2(transform.translation, Vec2::ZERO));
        assert!(approx_scalar(transform.rotation, 0.0));
        assert!(approx_vec2(transform.scale, Vec2::splat(1.0)));
    }

    ecs.update(0.25);
    {
        let transform = ecs.world.get::<Transform>(entity).unwrap();
        assert!(approx_vec2(transform.translation, Vec2::new(0.0, 4.0)));
        assert!(approx_scalar(transform.rotation, std::f32::consts::PI));
        assert!(approx_vec2(transform.scale, Vec2::splat(1.0)));
        let tint = ecs.world.get::<Tint>(entity).unwrap().0;
        assert!(approx_vec4(tint, Vec4::new(0.8, 0.95, 1.0, 1.0)), "tint should lerp midway through clip");
    }

    ecs.update(0.30);
    {
        let transform = ecs.world.get::<Transform>(entity).unwrap();
        assert!(approx_vec2(transform.translation, Vec2::new(0.0, 0.8)));
        assert!(approx_scalar(transform.rotation, 0.62831855));
        assert!(approx_vec2(transform.scale, Vec2::splat(1.0)));
        let tint = ecs.world.get::<Tint>(entity).unwrap().0;
        assert!(
            approx_vec4(tint, Vec4::new(0.96, 0.99, 1.0, 1.0)),
            "looped tint should wrap and interpolate"
        );
    }

    ecs.set_transform_clip_playing(entity, false);
    let frozen_before = ecs.world.get::<Transform>(entity).unwrap().translation;
    ecs.update(1.0);
    let frozen_after = ecs.world.get::<Transform>(entity).unwrap().translation;
    assert!(approx_vec2(frozen_before, frozen_after), "paused clip should not advance");

    ecs.set_transform_clip_speed(entity, 2.0);
    ecs.set_transform_clip_playing(entity, true);
    let before_speed = ecs.world.get::<Transform>(entity).unwrap().translation;
    ecs.update(0.125);
    {
        let transform = ecs.world.get::<Transform>(entity).unwrap();
        assert!(
            transform.translation.y > before_speed.y,
            "accelerated clip should advance proportional to speed"
        );
    }
}

#[test]
fn transform_clip_time_seek_applies_sample() {
    let mut assets = AssetManager::new();
    assets.retain_clip("slime", Some("fixtures/animation_clips/slime_bob.json")).expect("load slime clip");

    let mut ecs = EcsWorld::new();
    let entity = ecs.world.spawn((Transform::default(), WorldTransform::default(), Tint(Vec4::ONE))).id();

    assert!(ecs.set_transform_clip(entity, &assets, "slime"));
    assert!(ecs.set_transform_clip_time(entity, 0.5));

    {
        let transform = ecs.world.get::<Transform>(entity).unwrap();
        assert!(approx_vec2(transform.translation, Vec2::ZERO));
        assert!(approx_scalar(transform.rotation, std::f32::consts::TAU));
        assert!(approx_vec2(transform.scale, Vec2::new(1.2, 0.8)));
        let tint = ecs.world.get::<Tint>(entity).unwrap().0;
        assert!(approx_vec4(tint, Vec4::new(0.6, 0.9, 1.0, 1.0)));
    }

    ecs.reset_transform_clip(entity);
    let transform = ecs.world.get::<Transform>(entity).unwrap();
    assert!(approx_vec2(transform.translation, Vec2::ZERO));
    assert!(approx_scalar(transform.rotation, 0.0));
}
