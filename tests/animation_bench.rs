//! Placeholder benchmarking harness for animation systems.
//! Intended to sweep entity counts, step the ECS schedules, and emit CSV metrics.
//! Marked ignored so it does not run during ordinary `cargo test` until fully implemented.

use kestrel_engine::ecs::EcsWorld;

#[test]
#[ignore = "benchmark harness stub"]
fn animation_bench_stub() {
    let mut world = EcsWorld::new();
    // Future work: spawn configurable numbers of sprite animators and measure system costs.
    println!(
        "[animation_bench] Stub running – populate fixtures and timing logic in Milestone 1.\n\
         World currently contains {} entities.",
        world.world.entities().len()
    );
}
