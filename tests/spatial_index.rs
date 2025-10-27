use glam::Vec2;
use kestrel_engine::ecs::{Aabb, EcsWorld, SpatialMode, Transform, Velocity, WorldTransform};

fn spawn_particle(world: &mut EcsWorld, position: Vec2) {
    world.world.spawn((
        Transform { translation: position, rotation: 0.0, scale: Vec2::splat(0.1) },
        WorldTransform::default(),
        Aabb { half: Vec2::splat(0.05) },
        Velocity(Vec2::ZERO),
    ));
}

#[test]
fn quadtree_fallback_triggers_under_density() {
    let mut world = EcsWorld::new();
    world.set_spatial_quadtree_enabled(true);
    world.set_spatial_density_threshold(1.0);
    for _ in 0..12 {
        spawn_particle(&mut world, Vec2::new(0.0, 0.0));
    }
    world.fixed_step(1.0 / 60.0);
    let metrics = world.spatial_metrics();
    assert_eq!(metrics.mode, SpatialMode::Quadtree);
    assert!(metrics.quadtree_nodes > 0);
}

#[test]
fn spatial_metrics_cover_grid_usage() {
    let mut world = EcsWorld::new();
    world.set_spatial_quadtree_enabled(false);
    world.set_spatial_cell(0.1);
    spawn_particle(&mut world, Vec2::new(-0.5, 0.0));
    spawn_particle(&mut world, Vec2::new(0.6, 0.0));
    world.fixed_step(1.0 / 60.0);
    let metrics = world.spatial_metrics();
    assert_eq!(metrics.mode, SpatialMode::Grid);
    assert!(metrics.occupied_cells >= 2);
    assert!(metrics.average_occupancy >= 1.0);
}
