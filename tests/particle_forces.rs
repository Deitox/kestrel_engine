use glam::{Vec2, Vec4};
use kestrel_engine::ecs::{
    EcsWorld, Force, ForceFalloff, ForceFieldKind, Mass, Particle, ParticleTrail, Transform, Velocity,
};

fn step(world: &mut EcsWorld, dt: f32) {
    world.update(dt);
}

#[test]
fn radial_force_field_accelerates_particles() {
    let mut world = EcsWorld::new();
    let field = world.spawn_force_field(Vec2::ZERO, 2.0, 2.0, ForceFieldKind::Radial, ForceFalloff::Linear);
    let _ = field;

    let particle = world
        .world
        .spawn((
            Transform { translation: Vec2::new(1.0, 0.0), rotation: 0.0, scale: Vec2::splat(0.1) },
            Velocity(Vec2::ZERO),
            Force::default(),
            Mass(1.0),
            Particle { lifetime: 5.0, max_lifetime: 5.0 },
            kestrel_engine::ecs::ParticleVisual {
                start_color: Vec4::ONE,
                end_color: Vec4::ONE,
                start_size: 0.1,
                end_size: 0.1,
            },
            kestrel_engine::ecs::Tint(Vec4::ONE),
        ))
        .id();

    step(&mut world, 1.0);

    let vel = world.world.get::<Velocity>(particle).unwrap().0;
    assert!(vel.x > 0.9, "radial field should push outward on +X");
    assert!(vel.y.abs() < 0.1, "field should not add significant Y for symmetrical setup");
}

#[test]
fn trail_scales_with_velocity() {
    let mut world = EcsWorld::new();
    let particle = world
        .world
        .spawn((
            Transform { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::splat(0.1) },
            Velocity(Vec2::new(5.0, 0.0)),
            Force::default(),
            Mass(1.0),
            Particle { lifetime: 5.0, max_lifetime: 5.0 },
            kestrel_engine::ecs::ParticleVisual {
                start_color: Vec4::ONE,
                end_color: Vec4::ONE,
                start_size: 0.1,
                end_size: 0.1,
            },
            kestrel_engine::ecs::Tint(Vec4::ONE),
            ParticleTrail { length_scale: 0.5, min_length: 0.05, max_length: 1.0, width: 0.08, fade: 0.8 },
        ))
        .id();

    step(&mut world, 0.5);

    let transform = world.world.get::<Transform>(particle).unwrap();
    assert!(
        transform.scale.y > transform.scale.x,
        "trail should stretch along velocity, length > width"
    );
}
