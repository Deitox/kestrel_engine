use glam::{Mat4, Quat, Vec2, Vec3};
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{EcsWorld, Transform, Transform3D, WorldTransform, WorldTransform3D};
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
