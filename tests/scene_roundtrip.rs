use glam::Vec3;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{EcsWorld, MeshLighting, MeshRef, MeshSurface};
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
    assert!(assets.has_atlas("main"), "atlas retain should keep atlas loaded");
    let mut mesh_registry = MeshRegistry::new();
    mesh_registry
        .load_from_path("test_triangle", "assets/models/demo_triangle.gltf")
        .expect("demo gltf should load for mesh dependency test");
    mesh_registry
        .retain_mesh("test_triangle", Some("assets/models/demo_triangle.gltf"))
        .expect("retaining mesh should succeed");
    let mesh_entity = world.spawn_mesh_entity("test_triangle", Vec3::ZERO, Vec3::ONE);
    assert!(world.entity_exists(mesh_entity));
    assert!(mesh_registry.has("test_triangle"), "mesh registry should contain retained mesh");
    assert_eq!(
        mesh_registry.mesh_ref_count("test_triangle"),
        Some(1),
        "retain_mesh should increment ref count"
    );
    world.world.entity_mut(mesh_entity).insert(MeshSurface {
        material: Some("materials/bronze.mat".to_string()),
        lighting: MeshLighting {
            cast_shadows: true,
            receive_shadows: true,
            base_color: Vec3::new(0.25, 0.55, 0.85),
            emissive: Some(Vec3::new(0.1, 0.2, 0.3)),
            metallic: 0.35,
            roughness: 0.42,
        },
    });
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
    let saved_mesh_entity = scene
        .entities
        .iter()
        .find(|entity| entity.mesh.as_ref().map(|mesh| mesh.key.as_str()) == Some("test_triangle"))
        .expect("mesh entity should be serialized");
    let saved_mesh = saved_mesh_entity.mesh.as_ref().expect("mesh data present");
    assert_eq!(saved_mesh.material.as_deref(), Some("materials/bronze.mat"));
    assert!(saved_mesh.lighting.cast_shadows);
    assert!(saved_mesh.lighting.receive_shadows);
    let saved_base_color = Vec3::from(saved_mesh.lighting.base_color.clone());
    assert!((saved_base_color.x - 0.25).abs() < f32::EPSILON);
    assert!((saved_base_color.y - 0.55).abs() < f32::EPSILON);
    assert!((saved_base_color.z - 0.85).abs() < f32::EPSILON);
    assert!((saved_mesh.lighting.metallic - 0.35).abs() < f32::EPSILON);
    assert!((saved_mesh.lighting.roughness - 0.42).abs() < f32::EPSILON);
    let emissive_vec = saved_mesh
        .lighting
        .emissive
        .as_ref()
        .map(|data| Vec3::from(data.clone()))
        .expect("emissive color captured");
    assert!((emissive_vec.x - 0.1).abs() < f32::EPSILON);
    assert!((emissive_vec.y - 0.2).abs() < f32::EPSILON);
    assert!((emissive_vec.z - 0.3).abs() < f32::EPSILON);

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
    let loaded_mesh_entity = loaded
        .entities
        .iter()
        .find(|entity| entity.mesh.as_ref().map(|mesh| mesh.key.as_str()) == Some("test_triangle"))
        .expect("loaded scene should include mesh entity data");
    let loaded_mesh = loaded_mesh_entity.mesh.as_ref().expect("loaded mesh data");
    assert_eq!(loaded_mesh.material.as_deref(), Some("materials/bronze.mat"));
    assert!(loaded_mesh.lighting.cast_shadows);
    assert!(loaded_mesh.lighting.receive_shadows);
    let loaded_base_color = Vec3::from(loaded_mesh.lighting.base_color.clone());
    assert!((loaded_base_color.x - 0.25).abs() < f32::EPSILON);
    assert!((loaded_base_color.y - 0.55).abs() < f32::EPSILON);
    assert!((loaded_base_color.z - 0.85).abs() < f32::EPSILON);
    assert!((loaded_mesh.lighting.metallic - 0.35).abs() < f32::EPSILON);
    assert!((loaded_mesh.lighting.roughness - 0.42).abs() < f32::EPSILON);
    let loaded_emissive = loaded_mesh
        .lighting
        .emissive
        .as_ref()
        .map(|data| Vec3::from(data.clone()))
        .expect("loaded emissive retained");
    assert!((loaded_emissive.x - 0.1).abs() < f32::EPSILON);
    assert!((loaded_emissive.y - 0.2).abs() < f32::EPSILON);
    assert!((loaded_emissive.z - 0.3).abs() < f32::EPSILON);

    let mut new_world = EcsWorld::new();
    let mut new_registry = MeshRegistry::new();
    new_world
        .load_scene_with_mesh(&loaded, &assets, |key, path| new_registry.ensure_mesh(key, path))
        .expect("scene load into world");
    assert!(
        new_registry.has("test_triangle"),
        "mesh registry used during load should register mesh dependencies"
    );
    assert_eq!(
        new_registry.mesh_ref_count("test_triangle"),
        Some(0),
        "ensure_mesh prepares mesh without incrementing ref count"
    );
    {
        let mut query = new_world.world.query::<(&MeshSurface, &MeshRef)>();
        let mut matched = false;
        for (surface, mesh_ref) in query.iter(&new_world.world) {
            if mesh_ref.key == "test_triangle" {
                assert_eq!(surface.material.as_deref(), Some("materials/bronze.mat"));
                assert!(surface.lighting.cast_shadows);
                assert!(surface.lighting.receive_shadows);
                assert!((surface.lighting.base_color.x - 0.25).abs() < f32::EPSILON);
                assert!((surface.lighting.base_color.y - 0.55).abs() < f32::EPSILON);
                assert!((surface.lighting.base_color.z - 0.85).abs() < f32::EPSILON);
                assert!((surface.lighting.metallic - 0.35).abs() < f32::EPSILON);
                assert!((surface.lighting.roughness - 0.42).abs() < f32::EPSILON);
                let emissive = surface.lighting.emissive.expect("emissive should exist");
                assert!((emissive.x - 0.1).abs() < f32::EPSILON);
                assert!((emissive.y - 0.2).abs() < f32::EPSILON);
                assert!((emissive.z - 0.3).abs() < f32::EPSILON);
                matched = true;
            }
        }
        assert!(matched, "mesh surface metadata should survive load");
    }
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
    assert!(autoload_assets.has_atlas("main"), "auto-load should populate required atlases");
    assert!(
        autoload_registry.has("test_triangle"),
        "auto-load should ensure mesh dependencies are registered"
    );
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
