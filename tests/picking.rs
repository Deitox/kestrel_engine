use glam::{Mat4, Quat, Vec3};
use kestrel_engine::ecs::{EcsWorld, Transform3D, WorldTransform3D};
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

    {
        let mut transform = world.world.get_mut::<Transform3D>(entity).expect("transform");
        transform.translation = Vec3::new(1.5, 0.0, 0.0);
        transform.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        transform.scale = Vec3::new(0.25, 1.0, 0.5);
    }
    if let Some(mut world_tx) = world.world.get_mut::<WorldTransform3D>(entity) {
        let scale = Vec3::new(0.25, 1.0, 0.5);
        let rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4);
        let translation = Vec3::new(1.5, 0.0, 0.0);
        world_tx.0 = Mat4::from_scale_rotation_translation(scale, rotation, translation);
    }
    let origin_rotated = Vec3::new(1.5, 0.25, 4.0);
    let direction_rotated = Vec3::new(0.05, -0.05, -1.0).normalize();
    let picked_rotated = world.pick_entity_3d(origin_rotated, direction_rotated, &registry);
    assert_eq!(picked_rotated, Some(entity));
}
