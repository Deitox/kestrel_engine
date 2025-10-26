use glam::{Vec2, Vec4};
use kestrel_engine::ecs::{EcsWorld, Particle, ParticleCaps};

fn make_particle(world: &mut EcsWorld) {
    world.world.spawn((Particle { lifetime: 1.0, max_lifetime: 1.0 },));
}

#[test]
fn particle_budget_metrics_reports_counts() {
    let mut world = EcsWorld::new();
    world.set_particle_caps(ParticleCaps::new(8, 100, 32.0));

    let emitter_a =
        world.spawn_particle_emitter(Vec2::ZERO, 10.0, 0.5, 1.0, 1.0, Vec4::ONE, Vec4::ONE, 0.5, 0.5);
    let emitter_b =
        world.spawn_particle_emitter(Vec2::new(1.0, 0.0), 5.0, 0.5, 1.0, 1.0, Vec4::ONE, Vec4::ONE, 0.5, 0.5);
    {
        let mut emitter = world.world.get_mut::<kestrel_engine::ecs::ParticleEmitter>(emitter_a).unwrap();
        emitter.accumulator = 12.0;
    }
    {
        let mut emitter = world.world.get_mut::<kestrel_engine::ecs::ParticleEmitter>(emitter_b).unwrap();
        emitter.accumulator = 4.0;
    }
    for _ in 0..12 {
        make_particle(&mut world);
    }

    let metrics = world.particle_budget_metrics();
    assert_eq!(metrics.active_particles, 12);
    assert_eq!(metrics.max_total, 100);
    assert_eq!(metrics.max_spawn_per_frame, 8);
    assert_eq!(metrics.available_spawn_this_frame, 8.min(100 - 12));
    assert_eq!(metrics.total_emitters, 2);
    assert!((metrics.emitter_backlog_total - 16.0).abs() < f32::EPSILON);
    assert!((metrics.average_backlog() - 8.0).abs() < f32::EPSILON);
    assert!((metrics.emitter_backlog_max_observed - 12.0).abs() < f32::EPSILON);
    assert_eq!(metrics.emitter_backlog_limit, 32.0);
}
