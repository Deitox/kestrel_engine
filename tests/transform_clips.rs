use glam::{Vec2, Vec4};
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{ClipInstance, EcsWorld, Tint, Transform, WorldTransform};
use std::sync::Arc;

fn approx_vec2(a: Vec2, b: Vec2) -> bool {
    (a - b).length_squared() <= 1e-6
}

fn approx_vec4(a: Vec4, b: Vec4) -> bool {
    (a - b).length_squared() <= 1e-6
}

fn approx_scalar(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5
}

#[derive(Clone, Copy)]
struct FinalPose {
    translation: Vec2,
    rotation: f32,
    scale: Vec2,
    tint: Vec4,
    clip_time: f32,
}

fn simulate_clip_pose(assets: &AssetManager, deltas: &[f32]) -> FinalPose {
    let mut ecs = EcsWorld::new();
    let entity =
        ecs.world.spawn((Transform::default(), WorldTransform::default(), Tint(Vec4::ONE))).id();

    assert!(ecs.set_transform_clip(entity, assets, "slime"), "attach clip for playback");
    for &dt in deltas {
        ecs.update(dt);
    }

    let (translation, rotation, scale) = {
        let transform = ecs.world.get::<Transform>(entity).expect("transform after playback");
        (transform.translation, transform.rotation, transform.scale)
    };

    let tint = ecs.world.get::<Tint>(entity).expect("tint after playback").0;

    let (clip_time, sample) = {
        let instance = ecs.world.get::<ClipInstance>(entity).expect("clip instance missing");
        (instance.time, instance.sample())
    };

    let sample_translation = sample.translation.expect("translation track missing");
    assert!(approx_vec2(sample_translation, translation), "translation mismatch");

    let sample_rotation = sample.rotation.expect("rotation track missing");
    assert!(approx_scalar(sample_rotation, rotation), "rotation mismatch");

    let sample_scale = sample.scale.expect("scale track missing");
    assert!(approx_vec2(sample_scale, scale), "scale mismatch");

    let sample_tint = sample.tint.expect("tint track missing");
    assert!(approx_vec4(sample_tint, tint), "tint mismatch");

    FinalPose { translation, rotation, scale, tint, clip_time }
}

#[test]
fn transform_clip_sampling_matches_golden_values() {
    let mut assets = AssetManager::new();
    assets
        .retain_clip("slime", Some("fixtures/animation_clips/slime_bob.json"))
        .expect("load slime clip");

    let clip_arc = Arc::new(assets.clip("slime").expect("missing clip").clone());
    let clip_key: Arc<str> = Arc::from("slime");
    let instance = ClipInstance::new(Arc::clone(&clip_key), Arc::clone(&clip_arc));

    let cases = [
        (
            0.0,
            Vec2::new(0.0, 0.0),
            0.0,
            Vec2::splat(1.0),
            Vec4::new(1.0, 1.0, 1.0, 1.0),
        ),
        (
            0.125,
            Vec2::new(0.0, 2.0),
            std::f32::consts::FRAC_PI_2,
            Vec2::splat(1.0),
            Vec4::new(0.9, 0.975, 1.0, 1.0),
        ),
        (
            0.25,
            Vec2::new(0.0, 4.0),
            std::f32::consts::PI,
            Vec2::splat(1.0),
            Vec4::new(0.8, 0.95, 1.0, 1.0),
        ),
        (
            0.375,
            Vec2::new(0.0, 2.0),
            std::f32::consts::PI * 1.5,
            Vec2::splat(1.0),
            Vec4::new(0.7, 0.925, 1.0, 1.0),
        ),
        (
            0.5,
            Vec2::new(0.0, 0.0),
            std::f32::consts::TAU,
            Vec2::new(1.2, 0.8),
            Vec4::new(0.6, 0.9, 1.0, 1.0),
        ),
        (
            0.625,
            Vec2::new(0.0, 2.0),
            std::f32::consts::FRAC_PI_2,
            Vec2::splat(1.0),
            Vec4::new(0.9, 0.975, 1.0, 1.0),
        ),
    ];

    for (time, expected_translation, expected_rotation, expected_scale, expected_tint) in cases {
        let sample = instance.sample_at(time);
        let translation = sample.translation.expect("expected translation sample");
        assert!(
            approx_vec2(translation, expected_translation),
            "translation mismatch at t={time}"
        );

        let rotation = sample.rotation.expect("expected rotation sample");
        assert!(
            approx_scalar(rotation, expected_rotation),
            "rotation mismatch at t={time}"
        );

        let scale = sample.scale.expect("expected scale sample");
        assert!(approx_vec2(scale, expected_scale), "scale mismatch at t={time}");

        let tint = sample.tint.expect("expected tint sample");
        assert!(approx_vec4(tint, expected_tint), "tint mismatch at t={time}");
    }
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

#[test]
fn transform_clip_final_pose_consistent_across_update_chunking() {
    let mut assets = AssetManager::new();
    assets
        .retain_clip("slime", Some("fixtures/animation_clips/slime_bob.json"))
        .expect("load slime clip");

    let clip_arc = Arc::new(assets.clip("slime").expect("missing clip").clone());
    let clip_key: Arc<str> = Arc::from("slime");

    let sequences: &[&[f32]] = &[
        &[0.05, 0.05, 0.05, 0.05, 0.05, 0.05, 0.05, 0.05, 0.05, 0.05],
        &[0.2, 0.05, 0.15, 0.1],
        &[0.3, 0.2],
    ];

    let mut baseline: Option<FinalPose> = None;

    for &deltas in sequences {
        let pose = simulate_clip_pose(&assets, deltas);

        assert!(
            approx_vec2(pose.translation, Vec2::ZERO),
            "translation should reset after full loop"
        );
        assert!(approx_scalar(pose.rotation, 0.0), "rotation should reset after full loop");
        assert!(
            approx_vec2(pose.scale, Vec2::splat(1.0)),
            "scale should reset after full loop"
        );
        assert!(
            approx_vec4(pose.tint, Vec4::new(1.0, 1.0, 1.0, 1.0)),
            "tint should reset after full loop"
        );
        assert!(approx_scalar(pose.clip_time, 0.0), "clip time should wrap to zero");

        let mut reference = ClipInstance::new(Arc::clone(&clip_key), Arc::clone(&clip_arc));
        reference.set_time(pose.clip_time);
        let sample = reference.sample();

        let expected_translation = sample.translation.expect("reference translation track missing");
        assert!(approx_vec2(pose.translation, expected_translation), "translation drift detected");

        let expected_rotation = sample.rotation.expect("reference rotation track missing");
        assert!(approx_scalar(pose.rotation, expected_rotation), "rotation drift detected");

        let expected_scale = sample.scale.expect("reference scale track missing");
        assert!(approx_vec2(pose.scale, expected_scale), "scale drift detected");

        let expected_tint = sample.tint.expect("reference tint track missing");
        assert!(approx_vec4(pose.tint, expected_tint), "tint drift detected");

        if let Some(base) = baseline {
            assert!(approx_vec2(pose.translation, base.translation));
            assert!(approx_scalar(pose.rotation, base.rotation));
            assert!(approx_vec2(pose.scale, base.scale));
            assert!(approx_vec4(pose.tint, base.tint));
            assert!(approx_scalar(pose.clip_time, base.clip_time));
        } else {
            baseline = Some(pose);
        }
    }
}
