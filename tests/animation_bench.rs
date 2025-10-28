//! Benchmark harness for animation systems.
//! Sweeps through configured animator counts, steps the ECS schedules, and writes CSV summaries.
//! Marked ignored so it only runs when explicitly requested:
//! `cargo test -- --ignored animation_bench_run`.

use kestrel_engine::ecs::{
    EcsWorld, Sprite, SpriteAnimation, SpriteAnimationFrame, Transform, WorldTransform,
};
use std::borrow::Cow;
use std::fs::{create_dir_all, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const STEPS: u32 = 240; // ~4 seconds at 60 FPS by default
const DT: f32 = 1.0 / 60.0;
const ANIMATOR_SWEEP: &[usize] = &[100, 1_000, 5_000, 10_000];

struct BenchResult {
    animators: usize,
    steps: u32,
    dt: f32,
    elapsed: Duration,
}

#[test]
#[ignore = "benchmark harness – run manually when collecting perf data"]
fn animation_bench_run() {
    let mut results = Vec::new();
    for &count in ANIMATOR_SWEEP {
        let result = run_bench_case(count, STEPS, DT);
        println!(
            "[animation_bench] animators: {:>5} | steps: {:>3} | elapsed: {:>8.3} ms | mean: {:>8.1} ns/step",
            result.animators,
            result.steps,
            result.elapsed.as_secs_f64() * 1_000.0,
            result.elapsed.as_nanos() as f64 / result.steps as f64
        );
        results.push(result);
    }
    if let Err(err) = write_csv(&results) {
        eprintln!("[animation_bench] Failed to write CSV: {err}");
    }
}

fn run_bench_case(animator_count: usize, steps: u32, dt: f32) -> BenchResult {
    let mut world = EcsWorld::new();
    seed_sprite_animators(&mut world, animator_count);

    // Warm-up step to settle any lazy initialization.
    world.update(0.0);

    let start = Instant::now();
    for _ in 0..steps {
        world.update(dt);
    }
    BenchResult { animators: animator_count, steps, dt, elapsed: start.elapsed() }
}

fn seed_sprite_animators(world: &mut EcsWorld, count: usize) {
    let frames = vec![
        SpriteAnimationFrame { region: "frame_a".to_string(), duration: 0.08 },
        SpriteAnimationFrame { region: "frame_b".to_string(), duration: 0.08 },
        SpriteAnimationFrame { region: "frame_c".to_string(), duration: 0.08 },
    ];

    for _ in 0..count {
        world.world.spawn((
            Transform::default(),
            WorldTransform::default(),
            Sprite { atlas_key: Cow::Borrowed("bench"), region: Cow::Borrowed("frame_a") },
            SpriteAnimation::new("bench_cycle".to_string(), frames.clone(), true),
        ));
    }
}

fn write_csv(results: &[BenchResult]) -> std::io::Result<()> {
    if results.is_empty() {
        return Ok(());
    }
    let mut path = target_dir();
    path.push("animation_bench.csv");
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let mut file = File::create(&path)?;
    writeln!(file, "animators,steps,dt,elapsed_ms,mean_ns_per_step")?;
    for result in results {
        let elapsed_ms = result.elapsed.as_secs_f64() * 1_000.0;
        let mean_ns = result.elapsed.as_nanos() as f64 / result.steps as f64;
        writeln!(
            file,
            "{},{},{:.6},{:.3},{:.1}",
            result.animators, result.steps, result.dt, elapsed_ms, mean_ns
        )?;
    }
    println!("[animation_bench] CSV written to {}", path.display());
    Ok(())
}

fn target_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        PathBuf::from(dir)
    } else {
        PathBuf::from("target")
    }
}
