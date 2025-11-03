#[cfg(feature = "anim_stats")]
use kestrel_engine::ecs::{
    reset_sprite_animation_stats, reset_transform_clip_stats, sprite_animation_stats_snapshot,
    transform_clip_stats_snapshot, SpriteAnimationStats, TransformClipStats,
};
use kestrel_engine::ecs::{
    EcsWorld, Sprite, SpriteAnimation, SpriteAnimationFrame, SpriteAnimationLoopMode, SystemProfiler,
};
use std::sync::Arc;

#[test]
#[ignore = "manual profiling harness"]
fn animation_profile_snapshot() {
    let count = std::env::var("ANIMATION_PROFILE_COUNT")
        .ok()
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(10_000);
    let steps =
        std::env::var("ANIMATION_PROFILE_STEPS").ok().and_then(|raw| raw.parse::<u32>().ok()).unwrap_or(240);
    let warmup =
        std::env::var("ANIMATION_PROFILE_WARMUP").ok().and_then(|raw| raw.parse::<u32>().ok()).unwrap_or(16);
    let dt = std::env::var("ANIMATION_PROFILE_DT")
        .ok()
        .and_then(|raw| raw.parse::<f32>().ok())
        .unwrap_or(1.0 / 60.0);

    let frame_duration = std::env::var("ANIMATION_PROFILE_FRAME_DURATION")
        .ok()
        .and_then(|raw| raw.parse::<f32>().ok())
        .unwrap_or(0.08);

    let mut world = EcsWorld::new();
    seed_sprite_animators(&mut world, count, frame_duration);

    for _ in 0..warmup {
        world.update(dt);
    }

    {
        let mut profiler = world.world.resource_mut::<SystemProfiler>();
        *profiler = SystemProfiler::new();
    }

    #[cfg(feature = "anim_stats")]
    {
        reset_sprite_animation_stats();
        reset_transform_clip_stats();
    }

    let mut per_step = Vec::with_capacity(steps as usize);
    #[cfg(feature = "anim_stats")]
    let mut sprite_stats_per_step = Vec::with_capacity(steps as usize);
    #[cfg(feature = "anim_stats")]
    let mut transform_stats_per_step = Vec::with_capacity(steps as usize);
    #[cfg(feature = "anim_stats")]
    let mut prev_sprite_stats = sprite_animation_stats_snapshot();
    #[cfg(feature = "anim_stats")]
    let mut prev_transform_stats = transform_clip_stats_snapshot();

    for _ in 0..steps {
        world.update(dt);
        if let Some(timing) =
            world.system_timings().into_iter().find(|timing| timing.name == "sys_drive_sprite_animations")
        {
            per_step.push(timing.last_ms as f64);
        }
        #[cfg(feature = "anim_stats")]
        {
            let current_sprite = sprite_animation_stats_snapshot();
            sprite_stats_per_step.push(SpriteAnimationStats {
                fast_loop_calls: current_sprite.fast_loop_calls - prev_sprite_stats.fast_loop_calls,
                event_calls: current_sprite.event_calls - prev_sprite_stats.event_calls,
                plain_calls: current_sprite.plain_calls - prev_sprite_stats.plain_calls,
            });
            prev_sprite_stats = current_sprite;

            let current_transform = transform_clip_stats_snapshot();
            transform_stats_per_step.push(TransformClipStats {
                advance_calls: current_transform.advance_calls - prev_transform_stats.advance_calls,
                zero_delta_calls: current_transform.zero_delta_calls - prev_transform_stats.zero_delta_calls,
                skipped_clips: current_transform.skipped_clips - prev_transform_stats.skipped_clips,
                looped_resume_clips: current_transform.looped_resume_clips
                    - prev_transform_stats.looped_resume_clips,
                zero_duration_clips: current_transform.zero_duration_clips
                    - prev_transform_stats.zero_duration_clips,
                fast_path_clips: current_transform.fast_path_clips - prev_transform_stats.fast_path_clips,
                slow_path_clips: current_transform.slow_path_clips - prev_transform_stats.slow_path_clips,
            });
            prev_transform_stats = current_transform;
        }
    }

    let timings = world.system_timings();
    println!("[animation_profile] animators={count} steps={steps} dt={:.6}", dt);
    if timings.is_empty() {
        println!("[animation_profile] no system timings captured");
    } else {
        for timing in timings {
            println!(
                "[animation_profile] {:<32} last={:>8.4} ms avg={:>8.4} ms max={:>8.4} ms samples={}",
                timing.name, timing.last_ms, timing.average_ms, timing.max_ms, timing.samples
            );
        }
    }

    if !per_step.is_empty() {
        let step_count = per_step.len() as f64;
        let mean_step = per_step.iter().sum::<f64>() / step_count;
        let max_step =
            per_step.iter().copied().fold(0.0_f64, |acc, value| if value > acc { value } else { acc });
        let mut sorted = per_step.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let p95_index = ((sorted.len() as f64) * 0.95).floor() as usize;
        let p95_value = sorted[p95_index.min(sorted.len() - 1)];

        let mut per_step_with_index: Vec<(usize, f64)> = per_step.iter().copied().enumerate().collect();
        let spike_threshold = 0.35_f64;
        let mut steady_sum = 0.0_f64;
        let mut steady_count = 0_usize;
        let mut spike_sum = 0.0_f64;
        let mut spike_count = 0_usize;
        for &(_, value) in &per_step_with_index {
            if value > spike_threshold {
                spike_sum += value;
                spike_count += 1;
            } else {
                steady_sum += value;
                steady_count += 1;
            }
        }
        let steady_mean = if steady_count > 0 { steady_sum / steady_count as f64 } else { 0.0 };
        let spike_mean = if spike_count > 0 { spike_sum / spike_count as f64 } else { 0.0 };

        println!(
            "[animation_profile] sys_drive per-step stats: mean={:.4} ms p95={:.4} ms max={:.4} ms steady_mean={:.4} ms steady_samples={} spike_mean={:.4} ms spike_samples={}",
            mean_step, p95_value, max_step, steady_mean, steady_count, spike_mean, spike_count
        );

        per_step_with_index.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        println!("[animation_profile] top step costs:");
        for &(index, value) in per_step_with_index.iter().take(6) {
            println!("[animation_profile]   step {:>4} -> {:>8.4} ms", index, value);
        }

        #[cfg(feature = "anim_stats")]
        {
            let mut total_sprite = SpriteAnimationStats::default();
            for stats in &sprite_stats_per_step {
                total_sprite.fast_loop_calls += stats.fast_loop_calls;
                total_sprite.event_calls += stats.event_calls;
                total_sprite.plain_calls += stats.plain_calls;
            }

            let mut total_transform = TransformClipStats::default();
            for stats in &transform_stats_per_step {
                total_transform.advance_calls += stats.advance_calls;
                total_transform.zero_delta_calls += stats.zero_delta_calls;
                total_transform.skipped_clips += stats.skipped_clips;
                total_transform.looped_resume_clips += stats.looped_resume_clips;
                total_transform.zero_duration_clips += stats.zero_duration_clips;
            }

            println!(
                "[animation_profile] anim_stats sprite totals: fast_loop={} event={} plain={}",
                total_sprite.fast_loop_calls, total_sprite.event_calls, total_sprite.plain_calls
            );
            println!(
                "[animation_profile] anim_stats transform totals: advance={} zero_delta={} skipped={} loop_resume={} zero_duration={}",
                total_transform.advance_calls,
                total_transform.zero_delta_calls,
                total_transform.skipped_clips,
                total_transform.looped_resume_clips,
                total_transform.zero_duration_clips
            );

            println!("[animation_profile] anim_stats top step mix:");
            for &(index, value) in per_step_with_index.iter().take(6) {
                let sprite_step = sprite_stats_per_step.get(index).copied().unwrap_or_default();
                let transform_step = transform_stats_per_step.get(index).copied().unwrap_or_default();
                println!(
                    "[animation_profile]   step {:>4} -> {:>8.4} ms | sprite(fast={} event={} plain={}) transform(adv={} zero={} skipped={} loop_resume={} zero_duration={})",
                    index,
                    value,
                    sprite_step.fast_loop_calls,
                    sprite_step.event_calls,
                    sprite_step.plain_calls,
                    transform_step.advance_calls,
                    transform_step.zero_delta_calls,
                    transform_step.skipped_clips,
                    transform_step.looped_resume_clips,
                    transform_step.zero_duration_clips
                );
            }
        }
    }
}

fn seed_sprite_animators(world: &mut EcsWorld, count: usize, frame_duration: f32) {
    let empty_events: Arc<[Arc<str>]> = Arc::from(Vec::<Arc<str>>::new());
    let frame_template: Arc<[SpriteAnimationFrame]> = Arc::from(vec![
        SpriteAnimationFrame {
            name: Arc::from("frame_a"),
            region: Arc::from("frame_a"),
            region_id: 0,
            duration: frame_duration,
            uv: [0.0; 4],
            events: Arc::clone(&empty_events),
        },
        SpriteAnimationFrame {
            name: Arc::from("frame_b"),
            region: Arc::from("frame_b"),
            region_id: 1,
            duration: frame_duration,
            uv: [0.0; 4],
            events: Arc::clone(&empty_events),
        },
        SpriteAnimationFrame {
            name: Arc::from("frame_c"),
            region: Arc::from("frame_c"),
            region_id: 2,
            duration: frame_duration,
            uv: [0.0; 4],
            events: Arc::clone(&empty_events),
        },
    ]);
    let timeline_name = Arc::from("bench_cycle");
    let frame_durations: Arc<[f32]> = Arc::from(vec![frame_duration; frame_template.len()]);

    for _ in 0..count {
        world.world.spawn((
            Sprite {
                atlas_key: Arc::from("bench"),
                region: Arc::from("frame_a"),
                region_id: 0,
                uv: [0.0; 4],
            },
            SpriteAnimation::new(
                Arc::clone(&timeline_name),
                Arc::clone(&frame_template),
                Arc::clone(&frame_durations),
                SpriteAnimationLoopMode::Loop,
            ),
        ));
    }
}
