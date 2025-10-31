use bevy_ecs::prelude::Entity;
use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{
    Aabb, Children, EcsWorld, Mass, MeshLighting, MeshRef, MeshSurface, Parent, ParticleEmitter,
    PropertyTrackPlayer, SceneEntityTag, Sprite, Tint, Transform, Transform3D, TransformTrackPlayer, Velocity,
    WorldTransform, WorldTransform3D,
};
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::scene::{
    EnvironmentDependency, Scene, SceneEntity, SceneEntityId, SceneEnvironment, SceneLightingData,
    SceneShadowData, TransformData, Vec3Data,
};
use std::sync::Arc;
use tempfile::NamedTempFile;

#[test]
fn scene_roundtrip_preserves_entity_count() {
    let mut world = EcsWorld::new();
    let _emitter = world.spawn_demo_scene();
    let mut assets = AssetManager::new();
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("main atlas should load before exporting scene");
    assert!(assets.has_atlas("main"), "atlas retain should keep atlas loaded");
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    mesh_registry
        .load_from_path("test_triangle", "assets/models/demo_triangle.gltf", &mut material_registry)
        .expect("demo gltf should load for mesh dependency test");
    mesh_registry
        .retain_mesh("test_triangle", Some("assets/models/demo_triangle.gltf"), &mut material_registry)
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

    let mut scene = world.export_scene_with_mesh_source(&assets, |key| {
        mesh_registry.mesh_source(key).map(|path| path.to_string_lossy().into_owned())
    });
    const ENVIRONMENT_KEY: &str = "test_environment";
    scene.dependencies.set_environment_dependency(Some(EnvironmentDependency::new(
        ENVIRONMENT_KEY.to_string(),
        Some("assets/environments/test_environment.hdr".to_string()),
    )));
    scene.metadata.environment = Some(SceneEnvironment::new(ENVIRONMENT_KEY.to_string(), 1.75));
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
    let material_dep = scene
        .dependencies
        .material_dependencies()
        .find(|dep| dep.key() == "materials/bronze.mat")
        .expect("scene should track material dependency");
    assert!(material_dep.path().is_none());
    let env_dep =
        scene.dependencies.environment_dependency().expect("scene should track environment dependency");
    assert_eq!(env_dep.key(), ENVIRONMENT_KEY);
    assert_eq!(env_dep.path(), Some("assets/environments/test_environment.hdr"));
    let env_meta = scene.metadata.environment.as_ref().expect("scene should capture environment metadata");
    assert_eq!(env_meta.key.as_str(), ENVIRONMENT_KEY);
    assert!((env_meta.intensity - 1.75).abs() < f32::EPSILON);
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
    scene.save_to_path(&path).expect("scene save should succeed");

    let loaded = Scene::load_from_path(path).expect("scene load should succeed");
    assert_eq!(loaded.entities.len(), scene.entities.len());
    let loaded_env_dep = loaded
        .dependencies
        .environment_dependency()
        .expect("loaded scene should preserve environment dependency");
    assert_eq!(loaded_env_dep.key(), ENVIRONMENT_KEY);
    assert_eq!(loaded_env_dep.path(), Some("assets/environments/test_environment.hdr"));
    let loaded_env_meta =
        loaded.metadata.environment.as_ref().expect("loaded scene should restore environment metadata");
    assert_eq!(loaded_env_meta.key.as_str(), ENVIRONMENT_KEY);
    assert!((loaded_env_meta.intensity - 1.75).abs() < f32::EPSILON);
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
    let loaded_material_dep = loaded
        .dependencies
        .material_dependencies()
        .find(|dep| dep.key() == "materials/bronze.mat")
        .expect("loaded scene should preserve material dependency");
    assert!(loaded_material_dep.path().is_none());
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
    let mut new_registry = MeshRegistry::new(&mut material_registry);
    new_world
        .load_scene_with_mesh(&loaded, &assets, |key, path| {
            new_registry.ensure_mesh(key, path, &mut material_registry)
        })
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
    let mut autoload_registry = MeshRegistry::new(&mut material_registry);
    let _autoload_scene = autoload_world
        .load_scene_from_path_with_mesh(path, &mut autoload_assets, |key, path| {
            autoload_registry.ensure_mesh(key, path, &mut material_registry)
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

#[test]
fn scene_roundtrip_preserves_transforms_and_emitters() {
    let mut world = EcsWorld::new();
    let mut assets = AssetManager::new();
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("main atlas should load before exporting scene");

    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    mesh_registry
        .load_from_path("test_triangle", "assets/models/demo_triangle.gltf", &mut material_registry)
        .expect("demo gltf should load for mesh dependency test");
    mesh_registry
        .retain_mesh("test_triangle", Some("assets/models/demo_triangle.gltf"), &mut material_registry)
        .expect("retaining mesh should succeed");

    let emitter_color_start = Vec4::new(0.9, 0.7, 0.3, 1.0);
    let emitter_color_end = Vec4::new(0.1, 0.2, 0.4, 0.2);
    let parent = world
        .world
        .spawn((
            Transform { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::splat(1.0) },
            WorldTransform::default(),
            ParticleEmitter {
                rate: 18.0,
                spread: 0.75,
                speed: 1.6,
                lifetime: 2.25,
                accumulator: 0.0,
                start_color: emitter_color_start,
                end_color: emitter_color_end,
                start_size: 0.22,
                end_size: 0.06,
            },
        ))
        .id();

    let rotation = Quat::from_euler(EulerRot::XYZ, 0.35, -0.25, 1.1);
    let scale3 = Vec3::new(1.4, 0.8, 1.6);
    let translation3 = Vec3::new(2.5, -1.2, 3.75);
    let child = world
        .world
        .spawn((
            Transform { translation: Vec2::new(0.45, -0.35), rotation: 0.2, scale: Vec2::new(0.9, 1.05) },
            WorldTransform::default(),
            Transform3D { translation: translation3, rotation, scale: scale3 },
            WorldTransform3D::default(),
            MeshRef { key: "test_triangle".to_string() },
            MeshSurface::default(),
            Parent(parent),
        ))
        .id();
    world.world.entity_mut(parent).insert(Children(vec![child]));

    world.update(0.016);
    world.fixed_step(0.016);

    let temp_file = NamedTempFile::new().expect("temp scene file");
    world
        .save_scene_to_path_with_sources(
            temp_file.path(),
            &assets,
            |key| mesh_registry.mesh_source(key).map(|p| p.to_string_lossy().into_owned()),
            |key| material_registry.material_source(key).map(|s| s.to_string()),
        )
        .expect("scene save should succeed");

    let loaded_scene = Scene::load_from_path(temp_file.path()).expect("scene load should succeed");
    assert!(loaded_scene.entities.iter().any(|entity| {
        entity.mesh.as_ref().map(|mesh| mesh.key.as_str() == "test_triangle").unwrap_or(false)
    }));

    let mut new_world = EcsWorld::new();
    let mut new_registry = MeshRegistry::new(&mut material_registry);
    new_world
        .load_scene_with_mesh(&loaded_scene, &assets, |key, path| {
            new_registry.ensure_mesh(key, path, &mut material_registry)
        })
        .expect("scene load into world");
    new_world.update(0.016);
    new_world.fixed_step(0.016);

    let mut mesh_query =
        new_world.world.query::<(Entity, &MeshRef, &Transform, &Transform3D, &WorldTransform3D, &Parent)>();
    let (mesh_entity, _, transform2d, transform3d, world3d, parent_rel) = mesh_query
        .iter(&new_world.world)
        .find(|(_, mesh_ref, _, _, _, _)| mesh_ref.key == "test_triangle")
        .expect("loaded world should contain mesh entity");

    assert!((transform2d.translation - Vec2::new(0.45, -0.35)).length() < 1e-5);
    assert!((transform2d.scale - Vec2::new(0.9, 1.05)).length() < 1e-5);
    assert!((transform2d.rotation - 0.2).abs() < 1e-5);

    assert!((transform3d.translation - translation3).length() < 1e-5);
    assert!((transform3d.scale - scale3).length() < 1e-5);
    let rotation_dot = transform3d.rotation.dot(rotation).abs();
    assert!((rotation_dot - 1.0).abs() < 1e-5);

    assert!(
        world3d.0.to_cols_array().iter().all(|value| value.is_finite()),
        "world transform should remain finite"
    );

    let parent_entity = parent_rel.0;
    let emitter =
        new_world.world.get::<ParticleEmitter>(parent_entity).expect("parent emitter should persist");
    assert!((emitter.rate - 18.0).abs() < f32::EPSILON);
    assert!((emitter.spread - 0.75).abs() < f32::EPSILON);
    assert!((emitter.speed - 1.6).abs() < f32::EPSILON);
    assert!((emitter.lifetime - 2.25).abs() < f32::EPSILON);
    assert!((emitter.start_size - 0.22).abs() < f32::EPSILON);
    assert!((emitter.end_size - 0.06).abs() < f32::EPSILON);
    assert!((emitter.start_color - emitter_color_start).length() < 1e-5);
    assert!((emitter.end_color - emitter_color_end).length() < 1e-5);

    let children =
        new_world.world.get::<Children>(parent_entity).expect("parent should retain children listing");
    assert!(children.0.contains(&mesh_entity));
}

#[test]
fn scene_roundtrip_preserves_scripted_spawns() {
    let mut world = EcsWorld::new();
    let mut assets = AssetManager::new();
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("main atlas should load before scripted spawn");

    let position = Vec2::new(-0.35, 0.42);
    let velocity = Vec2::new(0.55, -0.15);
    let scale = 0.6;

    let scripted_entity = world
        .spawn_scripted_sprite(&assets, "main", "green", position, scale, velocity)
        .expect("script-driven spawn should succeed");
    assert!(world.entity_exists(scripted_entity), "scripted entity should exist in world");

    let scene = world.export_scene(&assets);
    assert_eq!(scene.entities.len(), 1, "scene export should capture scripted entity only");
    assert!(
        scene.dependencies.contains_atlas("main"),
        "scene dependencies should include main atlas for scripted sprite"
    );
    let saved_entity = scene
        .entities
        .iter()
        .find(|entity| entity.sprite.as_ref().map(|sprite| sprite.region.as_str()) == Some("green"))
        .expect("scripted sprite should serialize with sprite data");
    let saved_velocity = saved_entity
        .velocity
        .as_ref()
        .map(|vel| Vec2::from(vel.clone()))
        .expect("scripted sprite should serialize velocity");
    assert!((saved_velocity - velocity).length() < 1e-5);
    let collider = saved_entity
        .collider
        .as_ref()
        .map(|collider| Vec2::from(collider.half_extents.clone()))
        .expect("scripted sprite should serialize collider");
    assert!((collider - Vec2::splat(scale * 0.5)).length() < 1e-5);
    let mass = saved_entity.mass.expect("scripted sprite should serialize mass");
    assert!((mass - 1.0).abs() < 1e-5);

    let mut new_world = EcsWorld::new();
    new_world.load_scene(&scene, &assets).expect("scene load should recreate scripted sprite");
    let mut query = new_world.world.query::<(&Transform, &Sprite, &Velocity, &Aabb, &Mass)>();
    let (transform, sprite, vel, collider_loaded, mass_loaded) = query
        .iter(&new_world.world)
        .find(|(_, sprite, _, _, _)| sprite.region.as_ref() == "green")
        .expect("loaded world should contain scripted sprite");

    assert_eq!(sprite.region.as_ref(), "green");
    assert_eq!(sprite.atlas_key.as_ref(), "main");
    assert!((transform.translation - position).length() < 1e-5);
    assert!((transform.scale - Vec2::splat(scale)).length() < 1e-5);
    assert!((vel.0 - velocity).length() < 1e-5);
    assert!((collider_loaded.half - Vec2::splat(scale * 0.5)).length() < 1e-5);
    assert!((mass_loaded.0 - 1.0).abs() < 1e-5);
}

#[test]
fn lighting_shadow_settings_roundtrip() {
    let lighting = SceneLightingData {
        direction: Vec3Data::from(Vec3::new(0.3, 0.7, 0.2).normalize()),
        color: Vec3Data { x: 1.2, y: 1.1, z: 0.9 },
        ambient: Vec3Data { x: 0.05, y: 0.06, z: 0.07 },
        exposure: 2.5,
        shadow: SceneShadowData { distance: 64.0, bias: 0.0035, strength: 0.65 },
    };
    let serialized = serde_json::to_string(&lighting).expect("serialize lighting");
    let roundtrip: SceneLightingData = serde_json::from_str(&serialized).expect("deserialize lighting");
    assert!((roundtrip.exposure - 2.5).abs() < f32::EPSILON);
    assert!((roundtrip.shadow.distance - 64.0).abs() < f32::EPSILON);
    assert!((roundtrip.shadow.bias - 0.0035).abs() < f32::EPSILON);
    assert!((roundtrip.shadow.strength - 0.65).abs() < f32::EPSILON);
}

#[test]
fn scene_entity_ids_enable_parent_reconstruction() {
    let mut world = EcsWorld::new();
    let mut assets = AssetManager::new();
    assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("atlas should load for ID roundtrip");

    let parent = world.world.spawn((Transform::default(), WorldTransform::default())).id();
    let child = world.world.spawn((Transform::default(), WorldTransform::default(), Parent(parent))).id();
    world.world.entity_mut(parent).insert(Children(vec![child]));

    let scene = world.export_scene(&assets);
    assert!(
        scene.entities.iter().all(|entity| !entity.id.as_str().is_empty()),
        "export should assign IDs to every entity"
    );

    let mut scene_without_indices = scene.clone();
    for entity in &mut scene_without_indices.entities {
        if entity.parent_id.is_some() {
            entity.parent = None;
        }
    }

    let temp_file = NamedTempFile::new().expect("temp scene file for IDs");
    scene_without_indices
        .save_to_path(temp_file.path())
        .expect("scene save without parent indices should succeed");

    let loaded_scene = Scene::load_from_path(temp_file.path()).expect("scene load should succeed");
    assert_eq!(loaded_scene.entities.len(), scene.entities.len());
    for (expected, loaded) in scene.entities.iter().zip(loaded_scene.entities.iter()) {
        assert_eq!(expected.id, loaded.id, "entity IDs should persist across disk roundtrip");
        assert_eq!(expected.parent_id, loaded.parent_id, "parent IDs should persist across disk roundtrip");
    }

    let mut reload_world = EcsWorld::new();
    reload_world
        .load_scene_from_path_with_mesh(temp_file.path(), &mut assets, |_, _| Ok(()))
        .expect("scene load should rebuild hierarchy");

    let mut parent_query = reload_world.world.query::<&Parent>();
    assert!(parent_query.iter(&reload_world.world).count() > 0, "entities should retain parent components");

    let mut children_query = reload_world.world.query::<&Children>();
    let total_children: usize =
        children_query.iter(&reload_world.world).map(|children| children.0.len()).sum();
    assert!(total_children > 0, "parents should retain children listings when only parent IDs are stored");
}

#[test]
fn scene_clone_subtree_includes_descendants() {
    fn make_entity(id: SceneEntityId, parent_id: Option<SceneEntityId>) -> SceneEntity {
        SceneEntity {
            id,
            name: None,
            transform: TransformData::from_components(Vec2::ZERO, 0.0, Vec2::splat(1.0)),
            transform_clip: None,
            sprite: None,
            transform3d: None,
            mesh: None,
            tint: None,
            velocity: None,
            mass: None,
            collider: None,
            particle_emitter: None,
            orbit: None,
            spin: None,
            parent_id,
            parent: None,
        }
    }

    let root_id = SceneEntityId::new();
    let child_id = SceneEntityId::new();
    let grandchild_id = SceneEntityId::new();
    let mut scene = Scene::default();
    scene.entities.push(make_entity(root_id.clone(), None));
    scene.entities.push(make_entity(child_id.clone(), Some(root_id.clone())));
    scene.entities.push(make_entity(grandchild_id.clone(), Some(child_id.clone())));

    assert_eq!(scene.entity_index_by_id(child_id.as_str()), Some(1));
    let subtree = scene.clone_subtree(child_id.as_str()).expect("subtree clone should find child branch");
    assert_eq!(subtree.len(), 2, "child and grandchild should be captured");
    assert!(subtree.iter().any(|entity| entity.id == child_id));
    assert!(subtree.iter().any(|entity| entity.id == grandchild_id));
    assert!(
        !subtree.iter().any(|entity| entity.id == root_id),
        "root entity should be excluded when cloning a child subtree"
    );
}

#[test]
fn scene_roundtrip_captures_hierarchy_dependencies_and_environment_metadata() {
    const MESH_KEY: &str = "test_triangle";
    const MESH_PATH: &str = "assets/models/demo_triangle.gltf";
    const ENVIRONMENT_KEY: &str = "environment::studio";
    const ENVIRONMENT_PATH: &str = "assets/environments/studio.png";
    let env_intensity = 2.0;

    let mut world = EcsWorld::new();
    let mut export_assets = AssetManager::new();
    export_assets
        .retain_atlas("main", Some("assets/images/atlas.json"))
        .expect("export requires sprite atlas");

    let root = world
        .world
        .spawn((
            Transform { translation: Vec2::new(-0.4, 0.25), rotation: 0.35, scale: Vec2::splat(1.15) },
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("checker")),
            Velocity(Vec2::new(0.05, 0.02)),
        ))
        .id();
    world.world.entity_mut(root).insert(SceneEntityTag::new(SceneEntityId::new()));

    let child = world
        .world
        .spawn((
            Transform { translation: Vec2::new(0.5, -0.1), rotation: -0.45, scale: Vec2::splat(0.9) },
            WorldTransform::default(),
            Sprite::uninitialized(Arc::from("main"), Arc::from("redorb")),
            Aabb { half: Vec2::splat(0.3) },
            Mass(1.5),
            Velocity(Vec2::new(-0.1, 0.12)),
        ))
        .id();
    world.world.entity_mut(child).insert(SceneEntityTag::new(SceneEntityId::new()));
    world.world.entity_mut(child).insert(Parent(root));
    world.world.entity_mut(root).insert(Children(vec![child]));

    let grandchild = world
        .world
        .spawn((
            Transform { translation: Vec2::new(0.1, 0.65), rotation: 0.9, scale: Vec2::splat(0.55) },
            WorldTransform::default(),
            ParticleEmitter {
                rate: 18.0,
                spread: 0.6,
                speed: 2.4,
                lifetime: 1.6,
                accumulator: 0.0,
                start_color: Vec4::new(0.2, 0.8, 1.0, 1.0),
                end_color: Vec4::new(1.0, 0.25, 0.15, 0.0),
                start_size: 0.45,
                end_size: 0.1,
            },
        ))
        .id();
    world.world.entity_mut(grandchild).insert(SceneEntityTag::new(SceneEntityId::new()));
    world.world.entity_mut(grandchild).insert(Parent(child));

    let mesh_entity = world.spawn_mesh_entity(MESH_KEY, Vec3::new(0.0, 0.5, 0.0), Vec3::splat(1.35));
    world.world.entity_mut(mesh_entity).insert(Parent(child));
    world.world.entity_mut(mesh_entity).insert(MeshSurface {
        material: Some("materials/brushed_metal.mat".to_string()),
        lighting: MeshLighting {
            cast_shadows: true,
            receive_shadows: false,
            base_color: Vec3::new(0.8, 0.7, 0.55),
            emissive: Some(Vec3::new(0.05, 0.1, 0.2)),
            metallic: 0.9,
            roughness: 0.2,
        },
    });

    world.world.entity_mut(child).insert(Children(vec![grandchild, mesh_entity]));

    let root_id = world.world.get::<SceneEntityTag>(root).unwrap().id.clone();
    let child_id = world.world.get::<SceneEntityTag>(child).unwrap().id.clone();
    let grandchild_id = world.world.get::<SceneEntityTag>(grandchild).unwrap().id.clone();
    let mesh_id = world.world.get::<SceneEntityTag>(mesh_entity).unwrap().id.clone();

    let mut scene = world.export_scene_with_mesh_source(&export_assets, |key| {
        (key == MESH_KEY).then(|| MESH_PATH.to_string())
    });
    scene.dependencies.set_environment_dependency(Some(EnvironmentDependency::new(
        ENVIRONMENT_KEY.to_string(),
        Some(ENVIRONMENT_PATH.to_string()),
    )));
    scene.metadata.environment = Some(SceneEnvironment::new(ENVIRONMENT_KEY.to_string(), env_intensity));

    assert!(scene.dependencies.contains_atlas("main"));
    assert!(scene.dependencies.contains_mesh(MESH_KEY));
    assert!(scene.dependencies.contains_material("materials/brushed_metal.mat"));
    assert!(scene.dependencies.contains_environment(ENVIRONMENT_KEY));
    assert_eq!(
        scene.metadata.environment.as_ref().map(|env| env.key.as_str()),
        Some(ENVIRONMENT_KEY),
        "environment metadata should capture key"
    );
    assert_eq!(scene.metadata.environment.as_ref().map(|env| env.intensity), Some(env_intensity));

    let root_index = scene.entity_index_by_id(root_id.as_str()).expect("root exported");
    let child_index = scene.entity_index_by_id(child_id.as_str()).expect("child exported");
    let grandchild_index = scene.entity_index_by_id(grandchild_id.as_str()).expect("grandchild exported");
    let mesh_index = scene.entity_index_by_id(mesh_id.as_str()).expect("mesh exported");

    assert!(scene.entities[root_index].parent_id.is_none());
    assert_eq!(scene.entities[child_index].parent_id.as_ref().map(|id| id.as_str()), Some(root_id.as_str()));
    assert_eq!(scene.entities[child_index].parent, Some(root_index));
    assert_eq!(
        scene.entities[grandchild_index].parent_id.as_ref().map(|id| id.as_str()),
        Some(child_id.as_str())
    );
    assert_eq!(scene.entities[grandchild_index].parent, Some(child_index));
    assert_eq!(scene.entities[mesh_index].parent_id.as_ref().map(|id| id.as_str()), Some(child_id.as_str()));
    assert_eq!(scene.entities[mesh_index].parent, Some(child_index));

    let serialized = serde_json::to_string_pretty(&scene).expect("serialize scene");
    let roundtrip: Scene = serde_json::from_str(&serialized).expect("deserialize scene");
    let reserialized = serde_json::to_string_pretty(&roundtrip).expect("reserialize scene");
    assert_eq!(serialized, reserialized, "scene JSON should be stable");

    let temp = NamedTempFile::new().expect("temp scene file");
    scene.save_to_path(temp.path()).expect("save scene to disk");
    let saved_scene = Scene::load_from_path(temp.path()).expect("load saved scene");
    assert_eq!(
        serde_json::to_string_pretty(&saved_scene).expect("serialize saved scene"),
        serialized,
        "disk roundtrip must preserve JSON data"
    );

    let mut reload_assets = AssetManager::new();
    reload_assets.retain_atlas("main", Some("assets/images/atlas.json")).expect("retain atlas before reload");
    let mut mesh_loader_invocations = 0usize;
    let mut reload_world = EcsWorld::new();
    reload_world
        .load_scene_from_path_with_mesh(temp.path(), &mut reload_assets, |key, path| {
            mesh_loader_invocations += 1;
            assert_eq!(key, MESH_KEY);
            assert_eq!(path, Some(MESH_PATH));
            Ok(())
        })
        .expect("reload scene into ECS world");
    assert_eq!(mesh_loader_invocations, 1, "mesh dependency should be resolved once");

    let mut coverage_mask = 0u8;
    let mut hierarchy_query =
        reload_world.world.query::<(&SceneEntityTag, Option<&Parent>, Option<&Children>)>();
    for (tag, parent, children) in hierarchy_query.iter(&reload_world.world) {
        if tag.id.as_str() == child_id.as_str() {
            coverage_mask |= 0b001;
            let parent_entity = parent.expect("child missing parent").0;
            let parent_tag = reload_world.world.get::<SceneEntityTag>(parent_entity).unwrap();
            assert_eq!(parent_tag.id.as_str(), root_id.as_str());
            let child_list = children.expect("child should have descendants");
            assert_eq!(child_list.0.len(), 2);
        } else if tag.id.as_str() == grandchild_id.as_str() {
            coverage_mask |= 0b010;
            let parent_entity = parent.expect("grandchild missing parent").0;
            let parent_tag = reload_world.world.get::<SceneEntityTag>(parent_entity).unwrap();
            assert_eq!(parent_tag.id.as_str(), child_id.as_str());
        } else if tag.id.as_str() == mesh_id.as_str() {
            coverage_mask |= 0b100;
            let parent_entity = parent.expect("mesh missing parent").0;
            let parent_tag = reload_world.world.get::<SceneEntityTag>(parent_entity).unwrap();
            assert_eq!(parent_tag.id.as_str(), child_id.as_str());
        }
    }
    assert_eq!(coverage_mask, 0b111, "all hierarchical relationships should reload");

    let mut runtime_assets = AssetManager::new();
    for dep in scene.dependencies.atlas_dependencies() {
        runtime_assets.retain_atlas(dep.key(), dep.path()).expect("retain atlas from dependencies");
    }
    assert_eq!(runtime_assets.atlas_ref_count("main"), 1);
    runtime_assets.release_atlas("main");
    assert_eq!(runtime_assets.atlas_ref_count("main"), 0);

    let mut runtime_materials = MaterialRegistry::new();
    let mut runtime_meshes = MeshRegistry::new(&mut runtime_materials);
    for dep in scene.dependencies.mesh_dependencies() {
        runtime_meshes
            .retain_mesh(dep.key(), dep.path(), &mut runtime_materials)
            .expect("retain mesh from dependencies");
    }
    assert_eq!(runtime_meshes.mesh_ref_count(MESH_KEY).unwrap_or(0), 1);
    runtime_meshes.release_mesh(MESH_KEY);
    assert_eq!(runtime_meshes.mesh_ref_count(MESH_KEY).unwrap_or(0), 0);

    let mut runtime_environments = EnvironmentRegistry::new();
    if let Some(dep) = scene.dependencies.environment_dependency() {
        runtime_environments.retain(dep.key(), dep.path()).expect("retain environment");
        assert_eq!(runtime_environments.ref_count(dep.key()), Some(1));
        runtime_environments.release(dep.key());
        assert_eq!(runtime_environments.ref_count(dep.key()), Some(0));
    }
}

#[test]
fn scene_roundtrip_preserves_transform_clip_binding() {
    let mut world = EcsWorld::new();
    let mut assets = AssetManager::new();
    assets
        .retain_clip("slime", Some("fixtures/animation_clips/slime_bob.json"))
        .expect("load transform clip");

    let entity = world
        .world
        .spawn((
            Transform { translation: Vec2::new(-3.0, 1.5), rotation: 0.0, scale: Vec2::splat(1.0) },
            WorldTransform::default(),
            Tint(Vec4::new(0.9, 0.5, 0.3, 1.0)),
        ))
        .id();

    assert!(world.set_transform_clip(entity, &assets, "slime"));
    let _ = world.set_transform_clip_group(entity, Some("cutscene"));
    let _ = world.set_transform_clip_speed(entity, 0.75);
    let _ = world.set_transform_clip_time(entity, 0.25);
    let _ = world.set_transform_clip_playing(entity, true);
    {
        let mut mask = world.world.get_mut::<TransformTrackPlayer>(entity).expect("mask present");
        mask.apply_translation = true;
        mask.apply_rotation = false;
        mask.apply_scale = false;
    }
    {
        let mut property =
            world.world.get_mut::<PropertyTrackPlayer>(entity).expect("property mask present");
        property.apply_tint = false;
    }
    {
        let mut transform = world.world.get_mut::<Transform>(entity).expect("transform present");
        transform.rotation = 0.15;
        transform.scale = Vec2::new(1.25, 0.95);
    }
    {
        let mut tint = world.world.get_mut::<Tint>(entity).expect("tint present");
        tint.0 = Vec4::new(0.2, 0.4, 0.8, 1.0);
    }

    let scene = world.export_scene(&assets);
    assert!(scene.dependencies.contains_clip("slime"));

    let entity_id = scene
        .entities
        .first()
        .expect("scene entity")
        .id
        .as_str()
        .to_string();

    let mut clone_world = EcsWorld::new();
    clone_world.load_scene(&scene, &assets).expect("load scene");
    let clone_entity = clone_world
        .find_entity_by_scene_id(&entity_id)
        .expect("entity restored by id");
    let info = clone_world.entity_info(clone_entity).expect("entity info");

    let clip = info.transform_clip.expect("transform clip info");
    assert_eq!(clip.clip_key, "slime");
    assert!(clip.playing);
    assert!((clip.speed - 0.75).abs() < 1e-6);
    assert!((clip.time - 0.25).abs() < 1e-6);
    assert_eq!(clip.group.as_deref(), Some("cutscene"));
    let translation = clip.sample_translation.expect("translation sample");
    assert!((translation.y - 4.0).abs() < 1e-4);

    let mask = info.transform_tracks.expect("transform track mask");
    assert!(mask.apply_translation);
    assert!(!mask.apply_rotation);
    assert!(!mask.apply_scale);
    let property = info.property_tracks.expect("property track mask");
    assert!(!property.apply_tint);

    let transform = clone_world.world.get::<Transform>(clone_entity).expect("transform present");
    assert!((transform.translation.y - 4.0).abs() < 1e-4);
    assert!((transform.rotation - 0.15).abs() < 1e-6);
    assert!((transform.scale.x - 1.25).abs() < 1e-6);
    assert!((transform.scale.y - 0.95).abs() < 1e-6);

    let tint = clone_world.world.get::<Tint>(clone_entity).expect("tint present").0;
    assert!((tint.x - 0.2).abs() < 1e-6);
    assert!((tint.y - 0.4).abs() < 1e-6);
    assert!((tint.z - 0.8).abs() < 1e-6);
}
