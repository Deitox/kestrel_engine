use glam::Vec2;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{EcsWorld, Transform, WorldTransform};
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
