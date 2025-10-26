use glam::Vec2;
use kestrel_engine::ecs::{Aabb, EcsWorld, Force, Mass, SceneEntityTag, Transform, Velocity, WorldTransform};
use kestrel_engine::scene::SceneEntityId;

const TEST_DT: f32 = 1.0 / 120.0;
const TEST_STEPS: usize = 240;

#[test]
fn fixed_step_remains_deterministic_across_worlds() {
    let (mut world_a, entity_a) = spawn_linear_motion_world();
    let (mut world_b, entity_b) = spawn_linear_motion_world();

    run_fixed_steps(&mut world_a, TEST_STEPS, TEST_DT);
    run_fixed_steps(&mut world_b, TEST_STEPS, TEST_DT);

    let info_a = world_a.entity_info(entity_a).expect("entity exists in world A");
    let info_b = world_b.entity_info(entity_b).expect("entity exists in world B");

    assert_vec2_near(info_a.translation, info_b.translation, 1e-5);
    assert_vec2_near(
        info_a.velocity.expect("velocity in world A"),
        info_b.velocity.expect("velocity in world B"),
        1e-5,
    );
}

fn spawn_linear_motion_world() -> (EcsWorld, bevy_ecs::prelude::Entity) {
    let mut world = EcsWorld::new();
    let entity = world
        .world
        .spawn((
            Transform { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::ONE },
            WorldTransform::default(),
            Velocity(Vec2::new(1.4, 0.5)),
            Force(Vec2::new(0.0, -0.1)),
            Mass(1.0),
            Aabb { half: Vec2::splat(0.1) },
            SceneEntityTag::new(SceneEntityId::new()),
        ))
        .id();
    (world, entity)
}

fn run_fixed_steps(world: &mut EcsWorld, steps: usize, dt: f32) {
    for _ in 0..steps {
        world.fixed_step(dt);
    }
    // Run a zero-delta update so WorldTransform is refreshed for readers.
    world.update(0.0);
}

fn assert_vec2_near(a: Vec2, b: Vec2, epsilon: f32) {
    assert!(
        (a - b).length() <= epsilon,
        "vectors differed: left={:?}, right={:?}, epsilon={}",
        a,
        b,
        epsilon
    );
}
