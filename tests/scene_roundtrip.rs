use glam::Vec3;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::scene::Scene;

#[test]
fn scene_roundtrip_preserves_entity_count() {
    let mut world = EcsWorld::new();
    let _emitter = world.spawn_demo_scene();
    let mut assets = AssetManager::new();
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("main atlas should load before exporting scene");
    let mut mesh_registry = MeshRegistry::new();
    mesh_registry
        .load_from_path("test_triangle", "assets/models/demo_triangle.gltf")
        .expect("demo gltf should load for mesh dependency test");
    let mesh_entity = world.spawn_mesh_entity("test_triangle", Vec3::ZERO, Vec3::ONE);
    assert!(world.entity_exists(mesh_entity));
    let original_count = world.entity_count();

    let scene = world.export_scene_with_mesh_source(&assets, |key| {
        mesh_registry.mesh_source(key).map(|path| path.to_string_lossy().into_owned())
    });
    assert!(scene.dependencies.contains_atlas("main"), "scene should track atlas dependency");
    let main_dep = scene
        .dependencies
        .atlas_dependencies()
        .find(|dep| dep.key() == "main")
        .expect("saved scene should include main atlas dependency");
    assert_eq!(main_dep.path(), Some("assets/images/atlas.json"));
    let mesh_dep = scene
        .dependencies
        .mesh_dependencies()
        .find(|dep| dep.key() == "test_triangle")
        .expect("saved scene should include mesh dependency with path");
    assert_eq!(mesh_dep.path(), Some("assets/models/demo_triangle.gltf"));

    let path = std::path::Path::new("target/test_scene_roundtrip.json");
    world
        .save_scene_to_path_with_mesh_source(&path, &assets, |key| {
            mesh_registry.mesh_source(key).map(|p| p.to_string_lossy().into_owned())
        })
        .expect("scene save should succeed");

    let loaded = Scene::load_from_path(path).expect("scene load should succeed");
    assert_eq!(loaded.entities.len(), scene.entities.len());
    let loaded_mesh_dep = loaded
        .dependencies
        .mesh_dependencies()
        .find(|dep| dep.key() == "test_triangle")
        .expect("loaded scene should preserve mesh dependency path");
    assert_eq!(loaded_mesh_dep.path(), Some("assets/models/demo_triangle.gltf"));

    let mut new_world = EcsWorld::new();
    let mut new_registry = MeshRegistry::new();
    new_world
        .load_scene_with_mesh(&loaded, &assets, |key, path| new_registry.ensure_mesh(key, path))
        .expect("scene load into world");
    assert_eq!(new_world.entity_count(), original_count);
    assert!(new_world.first_emitter().is_some());

    let mut autoload_world = EcsWorld::new();
    let mut autoload_assets = AssetManager::new();
    let mut autoload_registry = MeshRegistry::new();
    let _autoload_scene = autoload_world
        .load_scene_from_path_with_mesh(path, &mut autoload_assets, |key, path| {
            autoload_registry.ensure_mesh(key, path)
        })
        .expect("scene load with auto dependency resolution");
    assert_eq!(autoload_world.entity_count(), original_count);

    let missing_assets = AssetManager::new();
    let mut missing_world = EcsWorld::new();
    assert!(
        missing_world.load_scene(&loaded, &missing_assets).is_err(),
        "loading without required assets should error"
    );

    let save_without_mesh =
        world.save_scene_to_path(path, &assets).expect_err("saving without mesh sources should fail");
    assert!(
        save_without_mesh.to_string().contains("save_scene_to_path_with_mesh_source"),
        "error should mention mesh-aware save helper"
    );
}
