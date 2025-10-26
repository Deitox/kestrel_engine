use kestrel_engine::scene::{
    Scene, SceneCameraBookmark, SceneEntityId, SceneMetadata, SceneViewportMode, Vec2Data,
};

#[test]
fn camera_metadata_roundtrip_preserves_bookmarks_and_follow_target() {
    let mut scene = Scene::default();
    scene.metadata.viewport = SceneViewportMode::Perspective3D;
    scene.metadata.camera_bookmarks = vec![SceneCameraBookmark {
        name: "Front".to_string(),
        position: Vec2Data { x: 12.5, y: -4.25 },
        zoom: 1.75,
    }];
    scene.metadata.active_camera_bookmark = Some("Front".to_string());
    let follow_id = SceneEntityId::new();
    scene.metadata.camera_follow_entity = Some(follow_id.clone());

    let json = serde_json::to_string(&scene).expect("scene serializes");
    let restored: Scene = serde_json::from_str(&json).expect("scene deserializes");

    assert_eq!(restored.metadata.camera_bookmarks.len(), 1);
    let restored_bookmark = &restored.metadata.camera_bookmarks[0];
    assert_eq!(restored_bookmark.name, "Front");
    assert!((restored_bookmark.position.x - 12.5).abs() < f32::EPSILON);
    assert!((restored_bookmark.position.y + 4.25).abs() < f32::EPSILON);
    assert!((restored_bookmark.zoom - 1.75).abs() < f32::EPSILON);
    assert_eq!(restored.metadata.active_camera_bookmark.as_deref(), Some("Front"));
    assert_eq!(
        restored.metadata.camera_follow_entity.as_ref().map(|id| id.as_str()),
        Some(follow_id.as_str())
    );
}

#[test]
fn camera_metadata_defaults_are_empty() {
    let metadata = SceneMetadata::default();
    assert!(metadata.camera_bookmarks.is_empty());
    assert!(metadata.active_camera_bookmark.is_none());
    assert!(metadata.camera_follow_entity.is_none());
    assert_eq!(metadata.viewport, SceneViewportMode::Ortho2D);
}
