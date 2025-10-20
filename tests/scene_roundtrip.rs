use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::scene::Scene;

#[test]
fn scene_roundtrip_preserves_entity_count() {
    let mut world = EcsWorld::new();
    let _emitter = world.spawn_demo_scene();
    let original_count = world.entity_count();
    let mut assets = AssetManager::new();
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("main atlas should load before exporting scene");
    let scene = world.export_scene(&assets);
    assert!(scene.dependencies.contains_atlas("main"), "scene should track atlas dependency");
    let main_dep = scene
        .dependencies
        .atlas_dependencies()
        .find(|dep| dep.key() == "main")
        .expect("saved scene should include main atlas dependency");
    assert_eq!(main_dep.path(), Some("assets/images/atlas.json"));

    let path = std::path::Path::new("target/test_scene_roundtrip.json");
    scene.save_to_path(path).expect("scene save should succeed");

    let loaded = Scene::load_from_path(path).expect("scene load should succeed");
    assert_eq!(loaded.entities.len(), scene.entities.len());

    let mut new_world = EcsWorld::new();
    new_world.load_scene(&loaded, &assets).expect("scene load into world");
    assert_eq!(new_world.entity_count(), original_count);
    assert!(new_world.first_emitter().is_some());

    let mut autoload_world = EcsWorld::new();
    let mut autoload_assets = AssetManager::new();
    let _autoload_scene = autoload_world
        .load_scene_from_path(path, &mut autoload_assets)
        .expect("scene load with auto dependency resolution");
    assert_eq!(autoload_world.entity_count(), original_count);

    let missing_assets = AssetManager::new();
    let mut missing_world = EcsWorld::new();
    assert!(
        missing_world.load_scene(&loaded, &missing_assets).is_err(),
        "loading without required assets should error"
    );
}
