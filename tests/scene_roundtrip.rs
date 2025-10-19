use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::scene::Scene;

#[test]
fn scene_roundtrip_preserves_entity_count() {
    let mut world = EcsWorld::new();
    let _emitter = world.spawn_demo_scene();
    let original_count = world.entity_count();
    let scene = world.export_scene();

    let path = std::path::Path::new("target/test_scene_roundtrip.json");
    scene.save_to_path(path).expect("scene save should succeed");

    let loaded = Scene::load_from_path(path).expect("scene load should succeed");
    assert_eq!(loaded.entities.len(), scene.entities.len());

    let mut assets = AssetManager::new();
    assets
        .load_atlas("main", "assets/images/atlas.json")
        .expect("main atlas should load for scene roundtrip");

    let mut new_world = EcsWorld::new();
    new_world.load_scene(&loaded, &assets).expect("scene load into world");
    assert_eq!(new_world.entity_count(), original_count);
    assert!(new_world.first_emitter().is_some());
}
