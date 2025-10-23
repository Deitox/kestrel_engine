use glam::{EulerRot, Mat4, Quat, Vec3};
use kestrel_engine::ecs::{EcsWorld, WorldTransform3D};
use kestrel_engine::mesh_registry::MeshRegistry;

#[test]
fn pick_entity_3d_hits_mesh() {
    let mut world = EcsWorld::new();
    let mut registry = MeshRegistry::new();
    let mesh_key = registry.default_key().to_string();
    registry.retain_mesh(&mesh_key, None).expect("default mesh retained");
    let entity = world.spawn_mesh_entity(&mesh_key, Vec3::ZERO, Vec3::splat(1.0));

    let origin = Vec3::new(0.0, 0.0, 5.0);
    let direction = Vec3::new(0.0, 0.0, -1.0);
    let picked = world.pick_entity_3d(origin, direction, &registry);
    assert_eq!(picked, Some(entity));

    let miss_direction = Vec3::new(5.0, 0.0, -1.0);
    let miss = world.pick_entity_3d(origin, miss_direction, &registry);
    assert_ne!(miss, Some(entity));

    let translation = Vec3::new(1.5, 0.0, 0.0);
    let rotation_euler = Vec3::new(0.0, std::f32::consts::FRAC_PI_4, 0.0);
    let scale = Vec3::new(0.25, 1.0, 0.5);
    assert!(world.set_mesh_translation(entity, translation));
    assert!(world.set_mesh_rotation_euler(entity, rotation_euler));
    assert!(world.set_mesh_scale(entity, scale));

    let world_tx = world.world.get::<WorldTransform3D>(entity).expect("world transform updated");
    let expected_rotation = Quat::from_euler(EulerRot::XYZ, rotation_euler.x, rotation_euler.y, rotation_euler.z);
    let expected = Mat4::from_scale_rotation_translation(scale, expected_rotation, translation);
    let actual = world_tx.0.to_cols_array();
    let expected_cols = expected.to_cols_array();
    for (actual_val, expected_val) in actual.iter().zip(expected_cols.iter()) {
        assert!(
            (actual_val - expected_val).abs() < 1e-5,
            "world transform mismatch: actual {actual_val}, expected {expected_val}"
        );
    }

    let origin_rotated = Vec3::new(1.5, 0.25, 4.0);
    let direction_rotated = Vec3::new(0.05, -0.05, -1.0).normalize();
    let picked_rotated = world.pick_entity_3d(origin_rotated, direction_rotated, &registry);
    assert_eq!(picked_rotated, Some(entity));
}
