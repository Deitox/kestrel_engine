use glam::{Vec2, Vec4};
use kestrel_engine::ecs::{
    EcsWorld, Particle, ParticleCaps, ParticleEmitter, ParticleState, Transform, Velocity,
};
use std::f32::consts::PI;

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
    assert_eq!(metrics.available_spawn_this_frame, 8);
    assert_eq!(metrics.total_emitters, 2);
    assert!((metrics.emitter_backlog_total - 16.0).abs() < f32::EPSILON);
    assert!((metrics.average_backlog() - 8.0).abs() < f32::EPSILON);
    assert!((metrics.emitter_backlog_max_observed - 12.0).abs() < f32::EPSILON);
    assert_eq!(metrics.emitter_backlog_limit, 32.0);
}

#[test]
fn particle_spawns_respect_global_frame_cap() {
    let mut world = EcsWorld::new();
    world.set_particle_caps(ParticleCaps::new(4, 100, 64.0));

    world.spawn_particle_emitter(Vec2::ZERO, 50.0, 0.2, 2.0, 2.0, Vec4::ONE, Vec4::ONE, 0.2, 0.1);
    world.spawn_particle_emitter(Vec2::new(1.0, 0.0), 50.0, 0.2, 2.0, 2.0, Vec4::ONE, Vec4::ONE, 0.2, 0.1);

    world.update(0.5);

    let mut particle_query = world.world.query::<&Particle>();
    let count = particle_query.iter(&world.world).count() as u32;
    assert_eq!(count, 4, "per-frame spawn cap should be enforced globally");
}

#[test]
fn emitters_accumulate_when_total_cap_hit() {
    let mut world = EcsWorld::new();
    world.set_particle_caps(ParticleCaps::new(8, 4, 64.0));

    let emitter_a =
        world.spawn_particle_emitter(Vec2::ZERO, 20.0, 0.5, 1.0, 2.0, Vec4::ONE, Vec4::ONE, 0.3, 0.3);
    let emitter_b = world.spawn_particle_emitter(
        Vec2::new(0.5, 0.0),
        20.0,
        0.5,
        1.0,
        2.0,
        Vec4::ONE,
        Vec4::ONE,
        0.3,
        0.3,
    );

    {
        let mut state = world.world.resource_mut::<ParticleState>();
        state.active_particles = 4;
    }

    {
        world.update(0.25);
    }

    let acc_a = world.world.get::<ParticleEmitter>(emitter_a).unwrap().accumulator;
    let acc_b = world.world.get::<ParticleEmitter>(emitter_b).unwrap().accumulator;
    assert!(acc_a > 0.0 && acc_b > 0.0, "emitters should continue to accumulate backlog even when capped");
}

#[test]
fn emitter_rotation_controls_spawn_direction() {
    let mut world = EcsWorld::new();

    let emitter =
        world.spawn_particle_emitter(Vec2::ZERO, 60.0, 0.0, 3.0, 5.0, Vec4::ONE, Vec4::ONE, 0.2, 0.2);

    {
        let mut transform = world.world.get_mut::<Transform>(emitter).unwrap();
        transform.rotation = PI;
    }

    world.update(0.5);

    let mut velocity_query = world.world.query::<&Velocity>();
    let velocities: Vec<_> = velocity_query.iter(&world.world).map(|vel| vel.0).collect();
    assert!(!velocities.is_empty(), "emitter should have spawned at least one particle");
    for vel in velocities {
        assert!(vel.y < -0.5, "particles should travel downward when emitter faces down");
        assert!(
            vel.x.abs() < 0.3,
            "rotation-controlled emission should keep horizontal slack tight when spread=0"
        );
    }
}
