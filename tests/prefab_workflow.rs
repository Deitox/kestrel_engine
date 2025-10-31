use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{
    EcsWorld, PropertyTrackPlayer, Tint, Transform, Transform3D, TransformTrackPlayer, WorldTransform,
    WorldTransform3D,
};
use kestrel_engine::scene::Scene;

#[test]
fn prefab_export_and_instantiate_roundtrip() {
    let mut ecs = EcsWorld::new();
    let assets = AssetManager::new();

    let entity = ecs
        .world
        .spawn((
            Transform { translation: Vec2::new(2.0, -4.0), rotation: 0.0, scale: Vec2::splat(1.0) },
            WorldTransform::default(),
        ))
        .id();

    let scene = ecs.export_prefab(entity, &assets).expect("export prefab");
    assert_eq!(scene.entities.len(), 1);

    let temp_dir = tempfile::tempdir().expect("temp dir");
    let prefab_path = temp_dir.path().join("prefab.json");
    scene.save_to_path(&prefab_path).expect("save prefab");

    let loaded = Scene::load_from_path(&prefab_path).expect("load prefab");
    let mut instance = loaded.with_fresh_entity_ids();
    instance.offset_entities_2d(Vec2::new(-2.0, 4.0));

    let spawned = ecs.instantiate_prefab(&instance, &assets).expect("instantiate prefab");
    assert_eq!(spawned.len(), 1);

    let info = ecs.entity_info(spawned[0]).expect("spawned info");
    assert!((info.translation.x - 0.0).abs() < 1e-4);
    assert!((info.translation.y - 0.0).abs() < 1e-4);
}

#[test]
fn prefab_offset_entities_3d_aligns_with_drop_target() {
    let mut ecs = EcsWorld::new();
    let assets = AssetManager::new();

    let translation2d = Vec2::new(1.5, -3.25);
    let translation3d = Vec3::new(1.5, -3.25, 0.75);
    let entity = ecs
        .world
        .spawn((
            Transform { translation: translation2d, rotation: 0.0, scale: Vec2::splat(1.0) },
            WorldTransform::default(),
            Transform3D { translation: translation3d, rotation: Quat::IDENTITY, scale: Vec3::ONE },
            WorldTransform3D(Mat4::from_translation(translation3d)),
        ))
        .id();

    let mut scene = ecs.export_prefab(entity, &assets).expect("export prefab");
    let root = scene.entities.first().expect("prefab root");
    let current =
        root.transform3d.as_ref().map(|tx| Vec3::from(tx.translation.clone())).unwrap_or_else(|| {
            let base: Vec2 = root.transform.translation.clone().into();
            Vec3::new(base.x, base.y, 0.0)
        });

    let target = Vec3::new(-2.5, 4.0, 1.25);
    scene.offset_entities_3d(target - current);

    let updated = scene.entities.first().expect("updated root");
    let updated2d: Vec2 = updated.transform.translation.clone().into();
    let updated3d =
        updated.transform3d.as_ref().map(|tx| Vec3::from(tx.translation.clone())).expect("3d transform");

    assert!((updated2d.x - target.x).abs() < 1e-4);
    assert!((updated2d.y - target.y).abs() < 1e-4);
    assert!((updated3d.x - target.x).abs() < 1e-4);
    assert!((updated3d.y - target.y).abs() < 1e-4);
    assert!((updated3d.z - target.z).abs() < 1e-4);
}

#[test]
fn prefab_roundtrip_preserves_transform_clip_binding() {
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    assets
        .retain_clip("slime", Some("fixtures/animation_clips/slime_bob.json"))
        .expect("load transform clip");

    let entity = ecs
        .world
        .spawn((
            Transform { translation: Vec2::new(0.0, 0.0), rotation: 0.25, scale: Vec2::splat(1.0) },
            WorldTransform::default(),
            Tint(Vec4::new(1.0, 1.0, 1.0, 1.0)),
        ))
        .id();

    assert!(ecs.set_transform_clip(entity, &assets, "slime"));
    let _ = ecs.set_transform_clip_group(entity, Some("ui.fx"));
    let _ = ecs.set_transform_clip_speed(entity, 1.5);
    let _ = ecs.set_transform_clip_time(entity, 0.375);
    let _ = ecs.set_transform_clip_playing(entity, false);

    {
        let mut mask = ecs.world.get_mut::<TransformTrackPlayer>(entity).expect("mask present");
        mask.apply_translation = true;
        mask.apply_rotation = false;
        mask.apply_scale = false;
    }
    {
        let mut property =
            ecs.world.get_mut::<PropertyTrackPlayer>(entity).expect("property mask present");
        property.apply_tint = false;
    }
    {
        let mut transform = ecs.world.get_mut::<Transform>(entity).expect("transform present");
        transform.rotation = 0.42;
        transform.scale = Vec2::new(1.8, 0.65);
    }
    {
        let mut tint = ecs.world.get_mut::<Tint>(entity).expect("tint present");
        tint.0 = Vec4::new(0.4, 0.6, 0.8, 1.0);
    }

    let prefab = ecs.export_prefab(entity, &assets).expect("export prefab with clip");
    assert!(prefab.dependencies.contains_clip("slime"), "clip dependency should be recorded");

    let mut clone_world = EcsWorld::new();
    let spawned = clone_world.instantiate_prefab(&prefab, &assets).expect("instantiate prefab");
    assert_eq!(spawned.len(), 1);

    let clone_entity = spawned[0];
    let info = clone_world.entity_info(clone_entity).expect("entity info after instantiate");
    let clip = info.transform_clip.expect("transform clip info present");
    assert_eq!(clip.clip_key, "slime");
    assert!(!clip.playing);
    assert!((clip.speed - 1.5).abs() < 1e-6);
    assert!((clip.time - 0.375).abs() < 1e-6);
    assert_eq!(clip.group.as_deref(), Some("ui.fx"));
    let sample_translation = clip.sample_translation.expect("sample translation");
    assert!((sample_translation.x).abs() < 1e-6);
    assert!((sample_translation.y - 2.0).abs() < 1e-4);

    let mask = info.transform_tracks.expect("transform mask info");
    assert!(mask.apply_translation);
    assert!(!mask.apply_rotation);
    assert!(!mask.apply_scale);
    let property = info.property_tracks.expect("property mask info");
    assert!(!property.apply_tint);

    let transform = clone_world.world.get::<Transform>(clone_entity).expect("transform restored");
    assert!((transform.translation.y - 2.0).abs() < 1e-4);
    assert!((transform.rotation - 0.42).abs() < 1e-6);
    assert!((transform.scale.x - 1.8).abs() < 1e-6);
    assert!((transform.scale.y - 0.65).abs() < 1e-6);

    let tint = clone_world.world.get::<Tint>(clone_entity).expect("tint restored").0;
    assert!((tint.x - 0.4).abs() < 1e-6);
    assert!((tint.y - 0.6).abs() < 1e-6);
    assert!((tint.z - 0.8).abs() < 1e-6);
}
