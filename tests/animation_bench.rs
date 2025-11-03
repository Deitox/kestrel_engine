//! Benchmark harness for the animation stack.
//! Sweeps animator counts, runs multiple timed samples per case, and emits CSV summaries that CI can ingest.
//! Marked ignored so it only runs when explicitly requested:
//! `cargo test --release animation_bench_run -- --ignored --nocapture`.

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use kestrel_engine::assets::skeletal::{
    JointCurve, JointQuatTrack, JointVec3Track, SkeletalClip, SkeletonAsset, SkeletonJoint,
};
use kestrel_engine::assets::{
    AnimationClip, ClipInterpolation, ClipKeyframe, ClipScalarTrack, ClipVec2Track, ClipVec4Track,
};
#[cfg(feature = "anim_stats")]
use kestrel_engine::ecs::{
    reset_sprite_animation_stats, reset_transform_clip_stats, sprite_animation_stats_snapshot,
    transform_clip_stats_snapshot, SpriteAnimationStats, TransformClipStats,
};
use kestrel_engine::ecs::{
    BoneTransforms, ClipInstance, EcsWorld, PropertyTrackPlayer, SkeletonInstance, Sprite, SpriteAnimation,
    SpriteAnimationFrame, SpriteAnimationLoopMode, Tint, Transform, TransformTrackPlayer, WorldTransform,
};
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
const DEFAULT_TRANSFORM_SWEEP: &[usize] = &[2_000];
const TRANSFORM_CSV_RELATIVE_PATH: &str = "benchmarks/animation_transform_clips.csv";
const TRANSFORM_BUDGETS_MS: &[(usize, f64)] = &[(2_000, 0.40)];
const DEFAULT_SKELETAL_SWEEP: &[usize] = &[100];
const SKELETAL_CSV_RELATIVE_PATH: &str = "benchmarks/animation_skeletal_clips.csv";
const SKELETAL_BUDGETS_MS: &[(usize, f64)] = &[(100, 1.20)];
const BENCH_SKELETON_BONES: usize = 10;

struct BenchConfig {
    steps: u32,
    dt: f32,
    warmup_steps: u32,
    samples: usize,
    sweep: Vec<usize>,
    transform_sweep: Vec<usize>,
    skeletal_sweep: Vec<usize>,
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
    #[cfg(feature = "anim_stats")]
    sprite_stats: SpriteAnimationStats,
    #[cfg(feature = "anim_stats")]
    transform_stats: TransformClipStats,
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

    run_bench_suite(
        "sprite_timelines",
        &config.sweep,
        &config,
        BUDGETS_MS,
        CSV_RELATIVE_PATH,
        seed_sprite_animators,
    );

    run_bench_suite(
        "transform_clips",
        &config.transform_sweep,
        &config,
        TRANSFORM_BUDGETS_MS,
        TRANSFORM_CSV_RELATIVE_PATH,
        seed_transform_clips,
    );

    run_bench_suite(
        "skeletal_clips",
        &config.skeletal_sweep,
        &config,
        SKELETAL_BUDGETS_MS,
        SKELETAL_CSV_RELATIVE_PATH,
        seed_skeletal_clips,
    );
}

fn run_bench_suite<F>(
    label: &str,
    counts: &[usize],
    config: &BenchConfig,
    budgets: &[(usize, f64)],
    csv_path: &str,
    mut seed_fn: F,
) -> Vec<BenchResult>
where
    F: FnMut(&mut EcsWorld, usize, bool),
{
    if counts.is_empty() {
        println!("[animation_bench][{label}] sweep is empty, skipping");
        return Vec::new();
    }

    let mut results = Vec::with_capacity(counts.len());
    for &count in counts {
        let result = run_bench_case(count, config, &mut seed_fn);
        let budget = budget_for(count, budgets);
        let summary = &result.summary;
        let meets_budget = budget.map(|limit| summary.mean_step_ms <= limit);
        let status = match meets_budget {
            Some(true) => "PASS",
            Some(false) => "FAIL",
            None => "INFO",
        };
        let budget_msg = budget.map_or("-".to_string(), |limit| format!("{:.3} ms", limit));
        println!(
            "[animation_bench][{label}] {:<4} | animators: {:>5} | mean: {:>7.3} ms | min: {:>7.3} ms | \
             max: {:>7.3} ms | mean/anim: {:>7.1} ns | budget: {}",
            status,
            result.animators,
            summary.mean_step_ms,
            summary.min_step_ms,
            summary.max_step_ms,
            summary.mean_ns_per_animator_step,
            budget_msg
        );
        #[cfg(feature = "anim_stats")]
        {
            let denom = (result.steps as f64) * (result.samples as f64).max(1.0);
            let sprite_avg_fast = result.sprite_stats.fast_loop_calls as f64 / denom;
            let sprite_avg_event = result.sprite_stats.event_calls as f64 / denom;
            let sprite_avg_plain = result.sprite_stats.plain_calls as f64 / denom;
            let transform_avg_adv = result.transform_stats.advance_calls as f64 / denom;
            let transform_avg_zero = result.transform_stats.zero_delta_calls as f64 / denom;
            let transform_avg_skipped = result.transform_stats.skipped_clips as f64 / denom;
            let transform_avg_loop = result.transform_stats.looped_resume_clips as f64 / denom;
            let transform_avg_zero_duration = result.transform_stats.zero_duration_clips as f64 / denom;
            println!(
                "[animation_bench][{label}]      anim_stats avg/step -> sprite(fast={:.2} event={:.2} plain={:.2}) \
                 transform(adv={:.2} zero={:.2} skipped={:.2} loop_resume={:.2} zero_duration={:.2})",
                sprite_avg_fast,
                sprite_avg_event,
                sprite_avg_plain,
                transform_avg_adv,
                transform_avg_zero,
                transform_avg_skipped,
                transform_avg_loop,
                transform_avg_zero_duration
            );
        }
        results.push(result);
    }

    if let Err(err) = write_csv(&results, csv_path, budgets) {
        eprintln!("[animation_bench][{label}] Failed to write CSV: {err}");
    }

    results
}

fn run_bench_case<F>(animator_count: usize, config: &BenchConfig, seed_fn: &mut F) -> BenchResult
where
    F: FnMut(&mut EcsWorld, usize, bool),
{
    let mut sample_elapsed = Vec::with_capacity(config.samples);
    #[cfg(feature = "anim_stats")]
    let mut sprite_totals = SpriteAnimationStats::default();
    #[cfg(feature = "anim_stats")]
    let mut transform_totals = TransformClipStats::default();

    for _ in 0..config.samples {
        let mut world = EcsWorld::new();
        seed_fn(&mut world, animator_count, config.randomize_phase);

        for _ in 0..config.warmup_steps {
            world.update(config.dt);
        }

        #[cfg(feature = "anim_stats")]
        {
            reset_sprite_animation_stats();
            reset_transform_clip_stats();
        }

        let start = Instant::now();
        for _ in 0..config.steps {
            world.update(black_box(config.dt));
        }
        let elapsed = start.elapsed();
        sample_elapsed.push(elapsed);

        #[cfg(feature = "anim_stats")]
        {
            let sprite_stats = sprite_animation_stats_snapshot();
            sprite_totals.fast_loop_calls += sprite_stats.fast_loop_calls;
            sprite_totals.event_calls += sprite_stats.event_calls;
            sprite_totals.plain_calls += sprite_stats.plain_calls;

            let transform_stats = transform_clip_stats_snapshot();
            transform_totals.advance_calls += transform_stats.advance_calls;
            transform_totals.zero_delta_calls += transform_stats.zero_delta_calls;
            transform_totals.skipped_clips += transform_stats.skipped_clips;
            transform_totals.looped_resume_clips += transform_stats.looped_resume_clips;
            transform_totals.zero_duration_clips += transform_stats.zero_duration_clips;
        }
        black_box(&world);
    }

    let summary = compute_summary(animator_count, config.steps, &sample_elapsed);
    BenchResult {
        animators: animator_count,
        steps: config.steps,
        dt: config.dt,
        samples: config.samples,
        summary,
        #[cfg(feature = "anim_stats")]
        sprite_stats: sprite_totals,
        #[cfg(feature = "anim_stats")]
        transform_stats: transform_totals,
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
    let transform_sweep =
        parse_sweep("ANIMATION_BENCH_TRANSFORM_SWEEP").unwrap_or_else(|| DEFAULT_TRANSFORM_SWEEP.to_vec());
    let skeletal_sweep =
        parse_sweep("ANIMATION_BENCH_SKELETAL_SWEEP").unwrap_or_else(|| DEFAULT_SKELETAL_SWEEP.to_vec());
    let randomize_phase = parse_env_bool("ANIMATION_BENCH_RANDOMIZE_PHASES").unwrap_or(true);

    BenchConfig { steps, dt, warmup_steps, samples, sweep, transform_sweep, skeletal_sweep, randomize_phase }
}

fn budget_for(animators: usize, budgets: &[(usize, f64)]) -> Option<f64> {
    budgets.iter().find_map(|(count, budget)| (*count == animators).then_some(*budget))
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

fn seed_transform_clips(world: &mut EcsWorld, count: usize, randomize_phase: bool) {
    let clip = bench_transform_clip();
    let clip_key: Arc<str> = Arc::from("bench_transform");

    for index in 0..count {
        let mut instance = ClipInstance::new(Arc::clone(&clip_key), Arc::clone(&clip));
        let duration = instance.duration();
        if randomize_phase && duration > 0.0 {
            let fraction = stable_phase_fraction(index as u64, clip_key.as_ref());
            instance.set_time(fraction * duration);
        }
        let sample = instance.sample_cached();
        instance.last_translation = sample.translation;
        instance.last_rotation = sample.rotation;
        instance.last_scale = sample.scale;
        instance.last_tint = sample.tint;

        let transform = Transform {
            translation: sample.translation.unwrap_or(Vec2::ZERO),
            rotation: sample.rotation.unwrap_or(0.0),
            scale: sample.scale.unwrap_or(Vec2::splat(1.0)),
        };
        let tint = Tint(sample.tint.unwrap_or(Vec4::ONE));

        world.world.spawn((
            transform,
            WorldTransform::default(),
            instance,
            TransformTrackPlayer::default(),
            PropertyTrackPlayer::default(),
            tint,
        ));
    }
}

fn bench_transform_clip() -> Arc<AnimationClip> {
    let translation_keys: Arc<[ClipKeyframe<Vec2>]> = Arc::from(
        vec![
            ClipKeyframe { time: 0.0, value: Vec2::ZERO },
            ClipKeyframe { time: 0.25, value: Vec2::new(0.0, 4.0) },
            ClipKeyframe { time: 0.5, value: Vec2::ZERO },
        ]
        .into_boxed_slice(),
    );
    let rotation_keys: Arc<[ClipKeyframe<f32>]> = Arc::from(
        vec![
            ClipKeyframe { time: 0.0, value: 0.0 },
            ClipKeyframe { time: 0.5, value: std::f32::consts::TAU },
        ]
        .into_boxed_slice(),
    );
    let scale_keys: Arc<[ClipKeyframe<Vec2>]> = Arc::from(
        vec![
            ClipKeyframe { time: 0.0, value: Vec2::splat(1.0) },
            ClipKeyframe { time: 0.5, value: Vec2::new(1.2, 0.8) },
        ]
        .into_boxed_slice(),
    );
    let tint_keys: Arc<[ClipKeyframe<Vec4>]> = Arc::from(
        vec![
            ClipKeyframe { time: 0.0, value: Vec4::ONE },
            ClipKeyframe { time: 0.5, value: Vec4::new(0.6, 0.9, 1.0, 1.0) },
        ]
        .into_boxed_slice(),
    );
    let translation_delta = Arc::from(
        translation_keys
            .as_ref()
            .windows(2)
            .map(|window| window[1].value - window[0].value)
            .collect::<Vec<Vec2>>()
            .into_boxed_slice(),
    );
    let translation_inv = Arc::from(
        translation_keys
            .as_ref()
            .windows(2)
            .map(|window| {
                let span = (window[1].time - window[0].time).max(std::f32::EPSILON);
                1.0 / span
            })
            .collect::<Vec<f32>>()
            .into_boxed_slice(),
    );
    let rotation_delta = Arc::from(
        rotation_keys
            .as_ref()
            .windows(2)
            .map(|window| window[1].value - window[0].value)
            .collect::<Vec<f32>>()
            .into_boxed_slice(),
    );
    let rotation_inv = Arc::from(
        rotation_keys
            .as_ref()
            .windows(2)
            .map(|window| {
                let span = (window[1].time - window[0].time).max(std::f32::EPSILON);
                1.0 / span
            })
            .collect::<Vec<f32>>()
            .into_boxed_slice(),
    );
    let scale_delta = Arc::from(
        scale_keys
            .as_ref()
            .windows(2)
            .map(|window| window[1].value - window[0].value)
            .collect::<Vec<Vec2>>()
            .into_boxed_slice(),
    );
    let scale_inv = Arc::from(
        scale_keys
            .as_ref()
            .windows(2)
            .map(|window| {
                let span = (window[1].time - window[0].time).max(std::f32::EPSILON);
                1.0 / span
            })
            .collect::<Vec<f32>>()
            .into_boxed_slice(),
    );
    let tint_delta = Arc::from(
        tint_keys
            .as_ref()
            .windows(2)
            .map(|window| window[1].value - window[0].value)
            .collect::<Vec<Vec4>>()
            .into_boxed_slice(),
    );
    let tint_inv = Arc::from(
        tint_keys
            .as_ref()
            .windows(2)
            .map(|window| {
                let span = (window[1].time - window[0].time).max(std::f32::EPSILON);
                1.0 / span
            })
            .collect::<Vec<f32>>()
            .into_boxed_slice(),
    );

    Arc::new(AnimationClip {
        name: Arc::from("bench_transform"),
        duration: 0.5,
        translation: Some(ClipVec2Track {
            interpolation: ClipInterpolation::Linear,
            keyframes: translation_keys,
            duration: 0.5,
            segment_deltas: translation_delta,
            segment_inv_durations: translation_inv,
        }),
        rotation: Some(ClipScalarTrack {
            interpolation: ClipInterpolation::Linear,
            keyframes: rotation_keys,
            duration: 0.5,
            segment_deltas: rotation_delta,
            segment_inv_durations: rotation_inv,
        }),
        scale: Some(ClipVec2Track {
            interpolation: ClipInterpolation::Step,
            keyframes: scale_keys,
            duration: 0.5,
            segment_deltas: scale_delta,
            segment_inv_durations: scale_inv,
        }),
        tint: Some(ClipVec4Track {
            interpolation: ClipInterpolation::Linear,
            keyframes: tint_keys,
            duration: 0.5,
            segment_deltas: tint_delta,
            segment_inv_durations: tint_inv,
        }),
        looped: true,
        version: 1,
    })
}

fn seed_skeletal_clips(world: &mut EcsWorld, count: usize, randomize_phase: bool) {
    let skeleton_key: Arc<str> = Arc::from("bench_skeleton");
    let skeleton = bench_skeleton_asset(BENCH_SKELETON_BONES);
    let clip = bench_skeletal_clip(Arc::clone(&skeleton_key), BENCH_SKELETON_BONES);

    for index in 0..count {
        let mut instance = SkeletonInstance::new(Arc::clone(&skeleton_key), Arc::clone(&skeleton));
        instance.set_active_clip(Some(Arc::clone(&clip)));
        if randomize_phase {
            let duration = instance.clip_duration();
            if duration > 0.0 {
                let fraction = stable_phase_fraction(index as u64, skeleton_key.as_ref());
                instance.time = fraction * duration;
            }
        }
        instance.ensure_capacity();
        instance.mark_dirty();

        let mut bone_transforms = BoneTransforms::new(instance.joint_count());
        bone_transforms.ensure_joint_count(instance.joint_count());

        world.world.spawn((instance, bone_transforms));
    }
}

fn bench_skeleton_asset(bone_count: usize) -> Arc<SkeletonAsset> {
    let mut joints: Vec<SkeletonJoint> = Vec::with_capacity(bone_count);
    for index in 0..bone_count {
        let parent = if index == 0 { None } else { Some((index - 1) as u32) };
        let rest_translation = if index == 0 { Vec3::ZERO } else { Vec3::new(0.0, 1.0, 0.0) };
        let rest_rotation = Quat::IDENTITY;
        let rest_scale = Vec3::ONE;
        let rest_local = Mat4::from_scale_rotation_translation(rest_scale, rest_rotation, rest_translation);
        let rest_world = if let Some(parent_index) = parent {
            joints[parent_index as usize].rest_world * rest_local
        } else {
            rest_local
        };
        let inverse_bind = rest_world.inverse();
        joints.push(SkeletonJoint {
            name: Arc::from(format!("bone_{index}")),
            parent,
            rest_local,
            rest_world,
            rest_translation,
            rest_rotation,
            rest_scale,
            inverse_bind,
        });
    }
    let roots = Arc::from(vec![0_u32].into_boxed_slice());
    Arc::new(SkeletonAsset {
        name: Arc::from("bench_skeleton"),
        joints: Arc::from(joints.into_boxed_slice()),
        roots,
    })
}

fn bench_skeletal_clip(skeleton_key: Arc<str>, bone_count: usize) -> Arc<SkeletalClip> {
    let mut curves = Vec::with_capacity(bone_count);
    for joint_index in 0..bone_count {
        let base_height = joint_index as f32;
        let translation_keys = Arc::from(
            vec![
                ClipKeyframe { time: 0.0, value: Vec3::new(0.0, base_height, 0.0) },
                ClipKeyframe { time: 0.5, value: Vec3::new(0.0, base_height + 0.2, 0.0) },
                ClipKeyframe { time: 1.0, value: Vec3::new(0.0, base_height, 0.0) },
            ]
            .into_boxed_slice(),
        );
        let translation =
            Some(JointVec3Track { interpolation: ClipInterpolation::Linear, keyframes: translation_keys });

        let rotation = if joint_index % 2 == 0 {
            let rotation_keys = Arc::from(
                vec![
                    ClipKeyframe { time: 0.0, value: Quat::IDENTITY },
                    ClipKeyframe { time: 0.5, value: Quat::from_axis_angle(Vec3::Z, 0.5) },
                    ClipKeyframe { time: 1.0, value: Quat::IDENTITY },
                ]
                .into_boxed_slice(),
            );
            Some(JointQuatTrack { interpolation: ClipInterpolation::Linear, keyframes: rotation_keys })
        } else {
            None
        };

        let scale = if joint_index % 3 == 0 {
            let scale_keys = Arc::from(
                vec![
                    ClipKeyframe { time: 0.0, value: Vec3::ONE },
                    ClipKeyframe { time: 0.5, value: Vec3::new(1.1, 0.9, 1.0) },
                    ClipKeyframe { time: 1.0, value: Vec3::ONE },
                ]
                .into_boxed_slice(),
            );
            Some(JointVec3Track { interpolation: ClipInterpolation::Linear, keyframes: scale_keys })
        } else {
            None
        };

        curves.push(JointCurve { joint_index: joint_index as u32, translation, rotation, scale });
    }

    Arc::new(SkeletalClip {
        name: Arc::from("bench_skeletal"),
        skeleton: skeleton_key,
        duration: 1.0,
        channels: Arc::from(curves.into_boxed_slice()),
        looped: true,
    })
}

fn stable_phase_fraction(seed: u64, timeline: &str) -> f32 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    timeline.hash(&mut hasher);
    const SCALE: f64 = 1.0 / (u64::MAX as f64 + 1.0);
    (hasher.finish() as f64 * SCALE) as f32
}

fn write_csv(
    results: &[BenchResult],
    csv_relative_path: &str,
    budgets: &[(usize, f64)],
) -> std::io::Result<()> {
    if results.is_empty() {
        return Ok(());
    }

    let mut path = target_dir();
    path.push(csv_relative_path);
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }

    let mut file = File::create(&path)?;
    let mut header = String::from(
        "animators,steps,samples,dt,mean_step_ms,min_step_ms,max_step_ms,mean_ns_per_animator_step,total_elapsed_ms,budget_ms,meets_budget",
    );
    #[cfg(feature = "anim_stats")]
    {
        header.push_str(
            ",sprite_fast_loop_avg,sprite_event_avg,sprite_plain_avg,transform_advance_avg,transform_zero_delta_avg,transform_skipped_avg,transform_loop_resume_avg,transform_zero_duration_avg",
        );
    }
    writeln!(file, "{header}")?;
    for result in results {
        let summary = &result.summary;
        let budget = budget_for(result.animators, budgets);
        let meets_budget = budget.map(|limit| summary.mean_step_ms <= limit);
        #[cfg(feature = "anim_stats")]
        {
            let denom = (result.steps as f64) * (result.samples as f64).max(1.0);
            writeln!(
                file,
                "{},{},{},{:.6},{:.3},{:.3},{:.3},{:.1},{:.3},{},{}\
,{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
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
                meets_budget.map(|pass| if pass { "pass" } else { "fail" }).unwrap_or("info"),
                result.sprite_stats.fast_loop_calls as f64 / denom,
                result.sprite_stats.event_calls as f64 / denom,
                result.sprite_stats.plain_calls as f64 / denom,
                result.transform_stats.advance_calls as f64 / denom,
                result.transform_stats.zero_delta_calls as f64 / denom,
                result.transform_stats.skipped_clips as f64 / denom,
                result.transform_stats.looped_resume_clips as f64 / denom,
                result.transform_stats.zero_duration_clips as f64 / denom
            )?;
        }
        #[cfg(not(feature = "anim_stats"))]
        {
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
