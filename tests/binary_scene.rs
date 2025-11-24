#![cfg(feature = "binary_scene")]

use kestrel_engine::scene::{Scene, SceneEntity, SceneEntityId, SceneViewportMode, TransformData, Vec2Data};
use tempfile::tempdir;

#[test]
fn binary_scene_roundtrip_preserves_entities() {
    let mut scene = Scene::default();
    scene.metadata.viewport = SceneViewportMode::Perspective3D;
    let entity = SceneEntity {
        id: SceneEntityId::new(),
        name: Some("BinaryRoot".to_string()),
        transform: TransformData {
            translation: Vec2Data { x: 1.0, y: -2.0 },
            rotation: 0.25,
            scale: Vec2Data { x: 2.0, y: 0.5 },
        },
        script: None,
        transform_clip: None,
        skeleton: None,
        sprite: None,
        transform3d: None,
        mesh: None,
        tint: None,
        velocity: None,
        mass: None,
        collider: None,
        particle_emitter: None,
        orbit: None,
        spin: Some(1.5),
        parent_id: None,
        parent: None,
    };
    scene.entities.push(entity);

    let dir = tempdir().expect("temp dir");
    let binary_path = dir.path().join("roundtrip.kscene");
    scene.save_to_path(&binary_path).expect("save binary scene");

    let loaded = Scene::load_from_path(&binary_path).expect("load binary scene");
    assert_eq!(loaded.metadata.viewport, SceneViewportMode::Perspective3D);
    assert_eq!(loaded.entities.len(), 1);
    let loaded_entity = &loaded.entities[0];
    assert_eq!(loaded_entity.name.as_deref(), Some("BinaryRoot"));
    assert!((loaded_entity.transform.translation.x - 1.0).abs() < f32::EPSILON);
    assert!((loaded_entity.transform.scale.y - 0.5).abs() < f32::EPSILON);
}
