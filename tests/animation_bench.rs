//! Benchmark harness for the animation stack.
//! Sweeps animator counts, runs multiple timed samples per case, and emits CSV summaries that CI can ingest.
//! Marked ignored so it only runs when explicitly requested:
//! `cargo test --release animation_bench_run -- --ignored --nocapture`.

use kestrel_engine::ecs::{EcsWorld, Sprite, SpriteAnimation, SpriteAnimationFrame, SpriteAnimationLoopMode};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs::{create_dir_all, File};
use std::hash::{Hash, Hasher};
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const DEFAULT_STEPS: u32 = 240; // ~4 seconds at 60 FPS
const DEFAULT_DT: f32 = 1.0 / 60.0;
const DEFAULT_WARMUP_STEPS: u32 = 16;
const DEFAULT_SAMPLES: usize = 5;
const DEFAULT_ANIMATOR_SWEEP: &[usize] = &[100, 1_000, 5_000, 10_000];
const CSV_RELATIVE_PATH: &str = "benchmarks/animation_sprite_timelines.csv";
const BUDGETS_MS: &[(usize, f64)] = &[(10_000, 0.20)];

struct BenchConfig {
    steps: u32,
    dt: f32,
    warmup_steps: u32,
    samples: usize,
    sweep: Vec<usize>,
    randomize_phase: bool,
}

struct BenchSummary {
    mean_step_ms: f64,
    min_step_ms: f64,
    max_step_ms: f64,
    mean_ns_per_animator_step: f64,
    total_elapsed_ms: f64,
}

struct BenchResult {
    animators: usize,
    steps: u32,
    dt: f32,
    samples: usize,
    summary: BenchSummary,
}

#[test]
#[ignore = "benchmark harness - run manually when collecting perf data"]
fn animation_bench_run() {
    let config = bench_config();
    assert!(
        !config.sweep.is_empty(),
        "no animator counts configured; set ANIMATION_BENCH_SWEEP or use defaults"
    );

    if cfg!(debug_assertions) {
        eprintln!(
            "[animation_bench] Warning: running benchmarks with debug assertions enabled. \
             Use `cargo test --release ...` for production numbers."
        );
    }

    println!(
        "[animation_bench] config: steps={} warmup={} samples={} dt={:.6} randomize_phase={}",
        config.steps, config.warmup_steps, config.samples, config.dt, config.randomize_phase
    );

    let mut results = Vec::with_capacity(config.sweep.len());
    for &count in &config.sweep {
        let result = run_bench_case(count, &config);
        let budget = budget_for(count);
        let summary = &result.summary;
        let meets_budget = budget.map(|limit| summary.mean_step_ms <= limit);
        let status = match meets_budget {
            Some(true) => "PASS",
            Some(false) => "FAIL",
            None => "INFO",
        };
        let budget_msg = budget.map_or("-".to_string(), |limit| format!("{:.3} ms", limit));
        println!(
            "[animation_bench] {:<4} | animators: {:>5} | mean: {:>7.3} ms | min: {:>7.3} ms | \
             max: {:>7.3} ms | mean/anim: {:>7.1} ns | budget: {}",
            status,
            result.animators,
            summary.mean_step_ms,
            summary.min_step_ms,
            summary.max_step_ms,
            summary.mean_ns_per_animator_step,
            budget_msg
        );
        results.push(result);
    }

    if let Err(err) = write_csv(&results) {
        eprintln!("[animation_bench] Failed to write CSV: {err}");
    }
}

fn run_bench_case(animator_count: usize, config: &BenchConfig) -> BenchResult {
    let mut sample_elapsed = Vec::with_capacity(config.samples);

    for _ in 0..config.samples {
        let mut world = EcsWorld::new();
        seed_sprite_animators(&mut world, animator_count, config.randomize_phase);

        for _ in 0..config.warmup_steps {
            world.update(config.dt);
        }

        let start = Instant::now();
        for _ in 0..config.steps {
            world.update(black_box(config.dt));
        }
        let elapsed = start.elapsed();
        sample_elapsed.push(elapsed);
        black_box(&world);
    }

    let summary = compute_summary(animator_count, config.steps, &sample_elapsed);
    BenchResult {
        animators: animator_count,
        steps: config.steps,
        dt: config.dt,
        samples: config.samples,
        summary,
    }
}

fn compute_summary(animators: usize, steps: u32, sample_elapsed: &[Duration]) -> BenchSummary {
    let steps_f64 = f64::from(steps.max(1));
    let mut mean_acc = 0.0;
    let mut min_ms = f64::INFINITY;
    let mut max_ms = 0.0;
    let mut total_elapsed_ms = 0.0;

    for elapsed in sample_elapsed {
        let elapsed_ms = elapsed.as_secs_f64() * 1_000.0;
        total_elapsed_ms += elapsed_ms;
        let per_step_ms = elapsed_ms / steps_f64;
        mean_acc += per_step_ms;
        if per_step_ms < min_ms {
            min_ms = per_step_ms;
        }
        if per_step_ms > max_ms {
            max_ms = per_step_ms;
        }
    }

    let sample_count = sample_elapsed.len().max(1) as f64;
    let mean_step_ms = mean_acc / sample_count;
    let mean_ns_per_animator_step =
        if animators == 0 { 0.0 } else { mean_step_ms * 1_000_000.0 / animators as f64 };

    BenchSummary {
        mean_step_ms,
        min_step_ms: min_ms,
        max_step_ms: max_ms,
        mean_ns_per_animator_step,
        total_elapsed_ms,
    }
}

fn bench_config() -> BenchConfig {
    let steps = parse_env::<u32>("ANIMATION_BENCH_STEPS").unwrap_or(DEFAULT_STEPS).max(1);
    let warmup_steps = parse_env::<u32>("ANIMATION_BENCH_WARMUP_STEPS").unwrap_or(DEFAULT_WARMUP_STEPS);
    let dt = parse_env::<f32>("ANIMATION_BENCH_DT").unwrap_or(DEFAULT_DT);
    let samples = parse_env::<usize>("ANIMATION_BENCH_SAMPLES").unwrap_or(DEFAULT_SAMPLES).max(1);
    let sweep = parse_sweep("ANIMATION_BENCH_SWEEP").unwrap_or_else(|| DEFAULT_ANIMATOR_SWEEP.to_vec());
    let randomize_phase = parse_env_bool("ANIMATION_BENCH_RANDOMIZE_PHASES").unwrap_or(true);

    BenchConfig { steps, dt, warmup_steps, samples, sweep, randomize_phase }
}

fn budget_for(animators: usize) -> Option<f64> {
    BUDGETS_MS.iter().find_map(|(count, budget)| (*count == animators).then_some(*budget))
}

fn seed_sprite_animators(world: &mut EcsWorld, count: usize, randomize_phase: bool) {
    let empty_events: Arc<[Arc<str>]> = Arc::from(Vec::<Arc<str>>::new());
    let frame_template: Arc<[SpriteAnimationFrame]> = Arc::from(vec![
        SpriteAnimationFrame {
            name: Arc::from("frame_a"),
            region: Arc::from("frame_a"),
            region_id: 0,
            duration: 0.08,
            uv: [0.0; 4],
            events: Arc::clone(&empty_events),
        },
        SpriteAnimationFrame {
            name: Arc::from("frame_b"),
            region: Arc::from("frame_b"),
            region_id: 1,
            duration: 0.08,
            uv: [0.0; 4],
            events: Arc::clone(&empty_events),
        },
        SpriteAnimationFrame {
            name: Arc::from("frame_c"),
            region: Arc::from("frame_c"),
            region_id: 2,
            duration: 0.08,
            uv: [0.0; 4],
            events: Arc::clone(&empty_events),
        },
    ]);
    let timeline_name = Arc::from("bench_cycle");
    let atlas_key = Arc::from("bench");
    let frame_durations: Arc<[f32]> =
        Arc::from(frame_template.iter().map(|frame| frame.duration).collect::<Vec<_>>());

    for index in 0..count {
        let mut animation = SpriteAnimation::new(
            Arc::clone(&timeline_name),
            Arc::clone(&frame_template),
            Arc::clone(&frame_durations),
            SpriteAnimationLoopMode::Loop,
        );

        if randomize_phase {
            apply_randomized_phase(&mut animation, index as u64, timeline_name.as_ref());
        } else {
        }

        let mut sprite = Sprite::uninitialized(Arc::clone(&atlas_key), Arc::clone(&frame_template[0].region));
        if let Some(frame) = animation.current_frame() {
            sprite.apply_frame(frame);
        }

        world.world.spawn((sprite, animation));
    }
}

fn apply_randomized_phase(animation: &mut SpriteAnimation, seed: u64, timeline: &str) {
    if animation.frames.is_empty() {
        return;
    }
    let total = animation.total_duration();
    if total <= 0.0 {
        return;
    }
    let fraction = stable_phase_fraction(seed, timeline);
    let offset = (fraction * total).rem_euclid(total.max(std::f32::EPSILON));
    apply_phase_offset(animation, offset);
}

fn apply_phase_offset(animation: &mut SpriteAnimation, mut offset: f32) {
    if animation.frames.is_empty() {
        animation.frame_index = 0;
        animation.elapsed_in_frame = 0.0;
        animation.forward = true;
        animation.refresh_current_duration();
        return;
    }
    let total = animation.total_duration().max(std::f32::EPSILON);
    offset = offset.rem_euclid(total);

    animation.frame_index = 0;
    animation.elapsed_in_frame = 0.0;
    animation.forward = true;
    animation.refresh_current_duration();

    let mut accumulated = 0.0;
    for (index, duration) in animation.frame_durations.iter().copied().enumerate() {
        let duration = duration.max(std::f32::EPSILON);
        if offset < accumulated + duration {
            animation.frame_index = index;
            animation.elapsed_in_frame = (offset - accumulated).clamp(0.0, duration);
            animation.refresh_current_duration();
            return;
        }
        accumulated += duration;
    }

    animation.frame_index = animation.frame_durations.len().saturating_sub(1);
    animation.elapsed_in_frame = 0.0;
    animation.refresh_current_duration();
}
fn stable_phase_fraction(seed: u64, timeline: &str) -> f32 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    timeline.hash(&mut hasher);
    const SCALE: f64 = 1.0 / (u64::MAX as f64 + 1.0);
    (hasher.finish() as f64 * SCALE) as f32
}

fn write_csv(results: &[BenchResult]) -> std::io::Result<()> {
    if results.is_empty() {
        return Ok(());
    }

    let mut path = target_dir();
    path.push(CSV_RELATIVE_PATH);
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }

    let mut file = File::create(&path)?;
    writeln!(
        file,
        "animators,steps,samples,dt,mean_step_ms,min_step_ms,max_step_ms,mean_ns_per_animator_step,total_elapsed_ms,budget_ms,meets_budget"
    )?;
    for result in results {
        let summary = &result.summary;
        let budget = budget_for(result.animators);
        let meets_budget = budget.map(|limit| summary.mean_step_ms <= limit);
        writeln!(
            file,
            "{},{},{},{:.6},{:.3},{:.3},{:.3},{:.1},{:.3},{},{}",
            result.animators,
            result.steps,
            result.samples,
            result.dt,
            summary.mean_step_ms,
            summary.min_step_ms,
            summary.max_step_ms,
            summary.mean_ns_per_animator_step,
            summary.total_elapsed_ms,
            budget.map(|value| format!("{:.3}", value)).unwrap_or_else(|| "".to_string()),
            meets_budget.map(|pass| if pass { "pass" } else { "fail" }).unwrap_or("info")
        )?;
    }
    println!("[animation_bench] CSV written to {}", path.display());
    Ok(())
}

fn target_dir() -> PathBuf {
    if let Ok(dir) = env::var("CARGO_TARGET_DIR") {
        PathBuf::from(dir)
    } else {
        PathBuf::from("target")
    }
}

fn parse_env<T>(key: &str) -> Option<T>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let raw = env::var(key).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<T>() {
        Ok(value) => Some(value),
        Err(err) => {
            eprintln!("[animation_bench] Ignoring {key}={raw:?}: {err}");
            None
        }
    }
}

fn parse_sweep(key: &str) -> Option<Vec<usize>> {
    let raw = env::var(key).ok()?;
    let mut counts = Vec::new();
    for token in raw.split(|c: char| matches!(c, ',' | ';' | ' ' | '\t' | '\n' | '\r')) {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        match trimmed.parse::<usize>() {
            Ok(value) if value > 0 => counts.push(value),
            Ok(_) => eprintln!("[animation_bench] Ignoring zero animator count in {key}={:?}", raw),
            Err(err) => {
                eprintln!("[animation_bench] Ignoring invalid animator count in {key}={:?}: {}", raw, err);
                return None;
            }
        }
    }
    if counts.is_empty() {
        None
    } else {
        Some(counts)
    }
}

fn parse_env_bool(key: &str) -> Option<bool> {
    let raw = env::var(key).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        other => {
            eprintln!("[animation_bench] Ignoring {key}={raw:?}: unsupported boolean value '{other}'");
            None
        }
    }
}
