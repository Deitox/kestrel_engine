//! Animation performance checkpoints aligned with the roadmap budgets.
//! Invoke with `cargo test --release animation_targets_measure -- --ignored --nocapture`.

use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use kestrel_engine::analytics::AnimationBudgetSample;
use kestrel_engine::assets::skeletal::{
    JointCurve, JointQuatTrack, JointVec3Track, SkeletalClip, SkeletonAsset, SkeletonJoint,
};
use kestrel_engine::assets::{
    AnimationClip, ClipInterpolation, ClipKeyframe, ClipScalarTrack, ClipSegment, ClipVec2Track,
    ClipVec4Track,
};
use kestrel_engine::ecs::{
    BoneTransforms, ClipInstance, EcsWorld, PropertyTrackPlayer, SkeletonInstance, Sprite,
    SpriteAnimPerfSample, SpriteAnimation, SpriteAnimationFrame, SpriteAnimationLoopMode, SpriteFrameHotData,
    SpriteFrameState, SystemTimingSummary, Tint, Transform, TransformTrackPlayer, WorldTransform,
};
use rustc_version_runtime::version as rustc_version;
use serde::Serialize;
use std::cmp::Ordering;
use std::env;
use std::fs::{create_dir_all, File};
use std::hint::black_box;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DT: f32 = 1.0 / 60.0;
const STEPS: u32 = 240;
const WARMUP_STEPS: u32 = 16;
const SAMPLES: usize = 5;
const BONES_PER_RIG: usize = 10;

#[derive(Clone, Copy)]
struct BudgetCase {
    label: &'static str,
    units: &'static str,
    count: usize,
    budget_ms: f64,
    kind: CaseKind,
}

#[derive(Clone, Copy)]
enum CaseKind {
    Sprite,
    Transform,
    Skeletal { bones_per_rig: usize },
}

#[derive(Serialize)]
struct BenchReport {
    metadata: BenchMetadata,
    animation_budget: AnimationBudgetSample,
    cases: Vec<CaseReport>,
}

#[derive(Serialize)]
struct BenchMetadata {
    warmup_frames: u32,
    measured_frames: u32,
    samples_per_case: usize,
    dt: f32,
    profile: String,
    lto_mode: String,
    target_cpu: String,
    rustc_version: String,
    feature_flags: Vec<&'static str>,
    commit_sha: Option<String>,
    generated_at_unix_ms: u128,
}

#[derive(Serialize)]
struct CaseReport {
    label: &'static str,
    units: &'static str,
    count: usize,
    #[serde(skip_serializing)]
    case_kind: &'static str,
    #[serde(skip_serializing)]
    bones_per_rig: Option<usize>,
    steps: u32,
    samples: usize,
    dt: f32,
    budget_ms: f64,
    summary: TimingSummary,
    status: TargetStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    sprite_perf: Option<SpritePerfReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_timings: Option<Vec<SystemTimingEntry>>,
}

#[derive(Serialize)]
struct SpritePerfReport {
    frames: usize,
    fast_animators_total: u64,
    slow_animators_total: u64,
    slow_ratio_mean: f64,
    slow_ratio_p95: f64,
    slow_ratio_p99: f64,
    slow_ratio_warn_frames: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    tail_scalar_ratio_p95: Option<f64>,
    tail_scalar_warn_frames: usize,
    ping_pong_total: u64,
    events_heavy_total: u64,
    events_emitted_total: u64,
    mod_or_div_calls_total: u64,
    var_dt_animators_total: u64,
    const_dt_animators_total: u64,
    simd_lanes8_total: u64,
    simd_lanes4_total: u64,
    simd_tail_scalar_total: u64,
    simd_chunk_time_ns_total: u64,
    simd_scalar_time_ns_total: u64,
}

#[derive(Serialize)]
struct SystemTimingEntry {
    name: String,
    last_ms: f32,
    average_ms: f32,
    max_ms: f32,
    samples: u64,
}

impl From<SystemTimingSummary> for SystemTimingEntry {
    fn from(summary: SystemTimingSummary) -> Self {
        Self {
            name: summary.name.to_string(),
            last_ms: summary.last_ms,
            average_ms: summary.average_ms,
            max_ms: summary.max_ms,
            samples: summary.samples,
        }
    }
}

#[derive(Serialize)]
struct TimingSummary {
    mean_step_ms: f64,
    min_step_ms: f64,
    max_step_ms: f64,
    median_step_ms: f64,
    p95_step_ms: f64,
    p99_step_ms: f64,
    total_elapsed_ms: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum TargetStatus {
    WithinBudget,
    OverBudget,
}

const CASES: &[BudgetCase] = &[
    BudgetCase {
        label: "sprite_timelines",
        units: "animators",
        count: 10_000,
        budget_ms: 0.30,
        kind: CaseKind::Sprite,
    },
    BudgetCase {
        label: "transform_clips",
        units: "clips",
        count: 2_000,
        budget_ms: 0.40,
        kind: CaseKind::Transform,
    },
    BudgetCase {
        label: "skeletal_clips",
        units: "bones",
        count: 1_000,
        budget_ms: 1.20,
        kind: CaseKind::Skeletal { bones_per_rig: BONES_PER_RIG },
    },
];

#[test]
#[ignore = "perf harness - run manually when collecting animation targets"]
fn animation_targets_measure() {
    println!("[animation_targets] steps={} warmup={} samples={} dt={:.6}", STEPS, WARMUP_STEPS, SAMPLES, DT);

    let mut reports = Vec::with_capacity(CASES.len());
    for case in CASES {
        let report = run_case(case);
        let status_str = match report.status {
            TargetStatus::WithinBudget => "PASS",
            TargetStatus::OverBudget => "WARN",
        };
        println!(
            "[animation_targets][{label}] {status} | count={count} {units} | mean={mean:.3} ms | \
             min={min:.3} ms | max={max:.3} ms | budget={budget:.2} ms",
            label = case.label,
            status = status_str,
            count = case.count,
            units = case.units,
            mean = report.summary.mean_step_ms,
            min = report.summary.min_step_ms,
            max = report.summary.max_step_ms,
            budget = case.budget_ms
        );
        reports.push(report);
    }

    let metadata = BenchMetadata::capture();
    let animation_budget = summarize_animation_budget(&reports);
    let bench_report = BenchReport { metadata, animation_budget, cases: reports };
    if let Err(err) = write_report(&bench_report) {
        eprintln!("[animation_targets] Failed to write report: {err}");
    }
}

fn run_case(case: &BudgetCase) -> CaseReport {
    let mut elapsed = Vec::with_capacity(SAMPLES);
    let track_sprite_perf = matches!(case.kind, CaseKind::Sprite);
    let mut sprite_perf_samples = Vec::new();
    let mut last_system_timings: Option<Vec<SystemTimingEntry>> = None;
    let force_fixed_step =
        std::env::var("ANIMATION_PROFILE_FORCE_FIXED_STEP").map(|value| value != "0").unwrap_or(true);
    for _ in 0..SAMPLES {
        let mut world = EcsWorld::new();
        seed_world(case, &mut world);
        if force_fixed_step {
            world.set_animation_time_fixed_step(Some(DT));
        } else {
            world.set_animation_time_fixed_step(None);
        }

        for _ in 0..WARMUP_STEPS {
            world.update(DT);
        }

        world.reset_sprite_anim_perf_history();

        let start = Instant::now();
        for _ in 0..STEPS {
            world.update(black_box(DT));
        }
        elapsed.push(start.elapsed());
        if track_sprite_perf {
            sprite_perf_samples.extend(world.sprite_anim_perf_history());
        }
        black_box(&world);
        let timings = world.system_timings().into_iter().map(SystemTimingEntry::from).collect::<Vec<_>>();
        if !timings.is_empty() {
            last_system_timings = Some(timings);
        }
    }

    let summary = summarize(&elapsed);
    let status = if summary.mean_step_ms <= case.budget_ms {
        TargetStatus::WithinBudget
    } else {
        TargetStatus::OverBudget
    };
    let sprite_perf = if track_sprite_perf { summarize_sprite_perf(&sprite_perf_samples) } else { None };

    CaseReport {
        label: case.label,
        units: case.units,
        count: case.count,
        case_kind: match case.kind {
            CaseKind::Sprite => "sprite",
            CaseKind::Transform => "transform",
            CaseKind::Skeletal { .. } => "skeletal",
        },
        bones_per_rig: match case.kind {
            CaseKind::Skeletal { bones_per_rig } => Some(bones_per_rig),
            _ => None,
        },
        steps: STEPS,
        samples: SAMPLES,
        dt: DT,
        budget_ms: case.budget_ms,
        summary,
        status,
        sprite_perf,
        system_timings: last_system_timings,
    }
}

fn summarize_animation_budget(reports: &[CaseReport]) -> AnimationBudgetSample {
    let mut snapshot = AnimationBudgetSample::default();
    for report in reports {
        match report.case_kind {
            "sprite" => {
                snapshot.sprite_eval_ms = report.summary.mean_step_ms as f32;
                snapshot.sprite_animators = report.count as u32;
            }
            "transform" => {
                snapshot.transform_eval_ms = report.summary.mean_step_ms as f32;
                snapshot.transform_clip_count = report.count;
            }
            "skeletal" => {
                snapshot.skeletal_eval_ms = report.summary.mean_step_ms as f32;
                snapshot.skeletal_bone_count = report.count;
                let bones_per_rig = report.bones_per_rig.unwrap_or(BONES_PER_RIG);
                let bones_per_rig = bones_per_rig.max(1);
                snapshot.skeletal_instance_count = (report.count / bones_per_rig).max(1);
            }
            _ => {}
        }
    }
    snapshot
}

fn seed_world(case: &BudgetCase, world: &mut EcsWorld) {
    match case.kind {
        CaseKind::Sprite => seed_sprite_animators(world, case.count, true),
        CaseKind::Transform => seed_transform_clips(world, case.count, true),
        CaseKind::Skeletal { bones_per_rig } => {
            let rigs = case.count.div_ceil(bones_per_rig).max(1);
            seed_skeletal_clips(world, rigs, bones_per_rig, true);
        }
    }
}

fn summarize(samples: &[Duration]) -> TimingSummary {
    let steps_f64 = f64::from(STEPS.max(1));
    let mut mean_acc = 0.0;
    let mut min_ms = f64::INFINITY;
    let mut max_ms = 0.0;
    let mut total_elapsed_ms = 0.0;
    let mut per_step_values = Vec::with_capacity(samples.len());

    for elapsed in samples {
        let elapsed_ms = elapsed.as_secs_f64() * 1_000.0;
        total_elapsed_ms += elapsed_ms;
        let per_step = elapsed_ms / steps_f64;
        per_step_values.push(per_step);
        mean_acc += per_step;
        if per_step < min_ms {
            min_ms = per_step;
        }
        if per_step > max_ms {
            max_ms = per_step;
        }
    }

    let sample_count = samples.len().max(1) as f64;
    let mut sorted = per_step_values.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    TimingSummary {
        mean_step_ms: mean_acc / sample_count,
        min_step_ms: min_ms,
        max_step_ms: max_ms,
        median_step_ms: percentile(&mut sorted, 0.5),
        p95_step_ms: percentile(&mut sorted, 0.95),
        p99_step_ms: percentile(&mut sorted, 0.99),
        total_elapsed_ms,
    }
}

fn percentile(values: &mut [f64], pct: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let clamped = pct.clamp(0.0, 1.0);
    let rank = clamped * (values.len().saturating_sub(1) as f64);
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        values[lower]
    } else {
        let weight = rank - lower as f64;
        values[lower] * (1.0 - weight) + values[upper] * weight
    }
}

fn summarize_sprite_perf(samples: &[SpriteAnimPerfSample]) -> Option<SpritePerfReport> {
    if samples.is_empty() {
        return None;
    }
    let mut slow_ratios = Vec::with_capacity(samples.len());
    let mut tail_ratios = Vec::new();
    let mut slow_ratio_warn = 0usize;
    let mut tail_warn = 0usize;
    let mut fast_total = 0_u64;
    let mut slow_total = 0_u64;
    let mut ping_total = 0_u64;
    let mut events_heavy_total = 0_u64;
    let mut events_emitted_total = 0_u64;
    let mut mod_calls_total = 0_u64;
    let mut var_total = 0_u64;
    let mut const_total = 0_u64;
    let mut simd_lanes8_total = 0_u64;
    let mut simd_lanes4_total = 0_u64;
    let mut simd_tail_total = 0_u64;
    let mut simd_chunk_time_total = 0_u64;
    let mut simd_scalar_time_total = 0_u64;

    for sample in samples {
        fast_total += sample.fast_animators as u64;
        slow_total += sample.slow_animators as u64;
        ping_total += sample.ping_pong_animators as u64;
        events_heavy_total += sample.events_heavy_animators as u64;
        events_emitted_total += sample.events_emitted as u64;
        mod_calls_total += sample.mod_or_div_calls as u64;
        var_total += sample.var_dt_animators as u64;
        const_total += sample.const_dt_animators as u64;
        simd_lanes8_total += sample.simd_lanes_8 as u64;
        simd_lanes4_total += sample.simd_lanes_4 as u64;
        simd_tail_total += sample.simd_tail_scalar as u64;
        simd_chunk_time_total += sample.simd_chunk_time_ns;
        simd_scalar_time_total += sample.simd_scalar_time_ns;

        let slow_ratio = sample.slow_ratio() as f64;
        slow_ratios.push(slow_ratio);
        if slow_ratio > 0.01 {
            slow_ratio_warn += 1;
        }
        if sample.simd_supported && sample.fast_animators > 0 {
            let tail_ratio = sample.tail_scalar_ratio();
            tail_ratios.push(tail_ratio as f64);
            if tail_ratio > 0.05 {
                tail_warn += 1;
            }
        }
    }

    let mut slow_sorted = slow_ratios.clone();
    slow_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let slow_ratio_mean = slow_ratios.iter().sum::<f64>() / slow_ratios.len() as f64;
    let slow_ratio_p95 = percentile(&mut slow_sorted, 0.95);
    let slow_ratio_p99 = percentile(&mut slow_sorted, 0.99);

    let tail_scalar_ratio_p95 = if tail_ratios.is_empty() {
        None
    } else {
        let mut tail_sorted = tail_ratios;
        tail_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        Some(percentile(&mut tail_sorted, 0.95))
    };

    Some(SpritePerfReport {
        frames: samples.len(),
        fast_animators_total: fast_total,
        slow_animators_total: slow_total,
        slow_ratio_mean,
        slow_ratio_p95,
        slow_ratio_p99,
        slow_ratio_warn_frames: slow_ratio_warn,
        tail_scalar_ratio_p95,
        tail_scalar_warn_frames: tail_warn,
        ping_pong_total: ping_total,
        events_heavy_total,
        events_emitted_total,
        mod_or_div_calls_total: mod_calls_total,
        var_dt_animators_total: var_total,
        const_dt_animators_total: const_total,
        simd_lanes8_total,
        simd_lanes4_total,
        simd_tail_scalar_total: simd_tail_total,
        simd_chunk_time_ns_total: simd_chunk_time_total,
        simd_scalar_time_ns_total: simd_scalar_time_total,
    })
}

impl BenchMetadata {
    fn capture() -> Self {
        let profile = env::var("ANIMATION_PROFILE_NAME")
            .or_else(|_| env::var("PROFILE"))
            .unwrap_or_else(|_| "dev".to_string());
        Self {
            warmup_frames: WARMUP_STEPS,
            measured_frames: STEPS,
            samples_per_case: SAMPLES,
            dt: DT,
            profile: profile.clone(),
            lto_mode: detect_lto_mode(&profile).to_string(),
            target_cpu: detect_target_cpu(),
            rustc_version: rustc_version().to_string(),
            feature_flags: active_feature_flags(),
            commit_sha: detect_commit_sha(),
            generated_at_unix_ms: current_timestamp_ms(),
        }
    }
}

fn current_timestamp_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|dur| dur.as_millis()).unwrap_or(0)
}

fn detect_target_cpu() -> String {
    if let Ok(flags) = env::var("RUSTFLAGS") {
        if let Some(value) = parse_target_cpu(&flags) {
            return value;
        }
    }
    if let Some(value) = target_cpu_from_config() {
        return value;
    }
    "default".to_string()
}

fn target_cpu_from_config() -> Option<String> {
    let path = Path::new(".cargo").join("config.toml");
    let contents = std::fs::read_to_string(path).ok()?;
    let sanitized = contents.replace(['[', ']', '"', ','], " ");
    parse_target_cpu(&sanitized)
}

fn parse_target_cpu(flags: &str) -> Option<String> {
    let mut tokens = flags.split_whitespace();
    while let Some(token) = tokens.next() {
        if let Some(value) = token.strip_prefix("-Ctarget-cpu=") {
            return Some(value.to_string());
        }
        if token == "-C" {
            if let Some(next) = tokens.next() {
                if let Some(value) = next.strip_prefix("target-cpu=") {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

fn detect_commit_sha() -> Option<String> {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|sha| !sha.is_empty())
}

fn detect_lto_mode(profile: &str) -> &'static str {
    match profile {
        "release-fat" => "fat",
        "release" | "bench" => "thin",
        _ => "none",
    }
}

fn active_feature_flags() -> Vec<&'static str> {
    let mut flags = Vec::new();
    if cfg!(feature = "binary_scene") {
        flags.push("binary_scene");
    }
    if cfg!(feature = "anim_stats") {
        flags.push("anim_stats");
    }
    if cfg!(feature = "sprite_anim_soa") {
        flags.push("sprite_anim_soa");
    }
    if cfg!(feature = "sprite_anim_fixed_point") {
        flags.push("sprite_anim_fixed_point");
    }
    if cfg!(feature = "sprite_anim_simd") {
        flags.push("sprite_anim_simd");
    }
    flags
}

fn write_report(report: &BenchReport) -> std::io::Result<()> {
    if report.cases.is_empty() {
        return Ok(());
    }
    let path = target_dir();
    if let Some(parent) = path.as_path().parent() {
        create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(report).expect("serialize report");
    let mut file = File::create(&path)?;
    file.write_all(json.as_bytes())?;
    println!("[animation_targets] Report written to {}", path.display());
    Ok(())
}

fn target_dir() -> PathBuf {
    let mut dir = if let Ok(raw) = std::env::var("CARGO_TARGET_DIR") {
        PathBuf::from(raw)
    } else {
        PathBuf::from("target")
    };
    dir.push("animation_targets_report.json");
    dir
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
    let frame_durations: Arc<[f32]> =
        Arc::from(frame_template.iter().map(|frame| frame.duration).collect::<Vec<_>>());
    let frame_offsets: Arc<[f32]> = {
        let mut offsets = Vec::with_capacity(frame_template.len());
        let mut accumulated = 0.0_f32;
        for frame in frame_template.iter() {
            offsets.push(accumulated);
            accumulated += frame.duration;
        }
        Arc::from(offsets.into_boxed_slice())
    };
    let timeline_name = Arc::from("bench_cycle");
    let atlas_key = Arc::from("bench");
    let total_duration: f32 = frame_durations.iter().copied().sum();
    let frame_hot_data: Arc<[SpriteFrameHotData]> = Arc::from(
        frame_template
            .iter()
            .map(|frame| SpriteFrameHotData { region_id: frame.region_id, uv: frame.uv })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );

    for index in 0..count {
        let mut animation = SpriteAnimation::new(
            Arc::clone(&timeline_name),
            Arc::clone(&frame_template),
            Arc::clone(&frame_hot_data),
            Arc::clone(&frame_durations),
            Arc::clone(&frame_offsets),
            total_duration,
            SpriteAnimationLoopMode::Loop,
        );
        if randomize_phase {
            apply_phase_offset(&mut animation, stable_phase_fraction(index as u64, timeline_name.as_ref()));
        }
        let mut sprite = Sprite::uninitialized(Arc::clone(&atlas_key), Arc::clone(&frame_template[0].region));
        if let Some(frame) = animation.current_frame() {
            sprite.apply_frame(frame);
        }
        let frame_state = SpriteFrameState::from_sprite(&sprite);
        world.world.spawn((sprite, frame_state, animation));
    }
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

fn seed_skeletal_clips(world: &mut EcsWorld, rigs: usize, bone_count: usize, randomize_phase: bool) {
    let skeleton_key: Arc<str> = Arc::from("bench_skeleton");
    let skeleton = bench_skeleton_asset(bone_count);
    let clip = bench_skeletal_clip(Arc::clone(&skeleton_key), bone_count);
    for index in 0..rigs {
        let mut instance = SkeletonInstance::new(Arc::clone(&skeleton_key), Arc::clone(&skeleton));
        instance.set_active_clip(None, Some(Arc::clone(&clip)));
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
    let (translation_delta, translation_segments, translation_offsets) =
        build_segment_cache_from_keys(translation_keys.as_ref(), |window| window[1].value - window[0].value);
    let (rotation_delta, rotation_segments, rotation_offsets) =
        build_segment_cache_from_keys(rotation_keys.as_ref(), |window| window[1].value - window[0].value);
    let (scale_delta, scale_segments, scale_offsets) =
        build_segment_cache_from_keys(scale_keys.as_ref(), |window| window[1].value - window[0].value);
    let (tint_delta, tint_segments, tint_offsets) =
        build_segment_cache_from_keys(tint_keys.as_ref(), |window| window[1].value - window[0].value);

    Arc::new(AnimationClip {
        name: Arc::from("bench_transform"),
        duration: 0.5,
        duration_inv: 2.0,
        translation: Some(ClipVec2Track {
            interpolation: ClipInterpolation::Linear,
            keyframes: translation_keys,
            duration: 0.5,
            duration_inv: 2.0,
            segment_deltas: translation_delta,
            segments: translation_segments,
            segment_offsets: translation_offsets,
        }),
        rotation: Some(ClipScalarTrack {
            interpolation: ClipInterpolation::Linear,
            keyframes: rotation_keys,
            duration: 0.5,
            duration_inv: 2.0,
            segment_deltas: rotation_delta,
            segments: rotation_segments,
            segment_offsets: rotation_offsets,
        }),
        scale: Some(ClipVec2Track {
            interpolation: ClipInterpolation::Step,
            keyframes: scale_keys,
            duration: 0.5,
            duration_inv: 2.0,
            segment_deltas: scale_delta,
            segments: scale_segments,
            segment_offsets: scale_offsets,
        }),
        tint: Some(ClipVec4Track {
            interpolation: ClipInterpolation::Linear,
            keyframes: tint_keys,
            duration: 0.5,
            duration_inv: 2.0,
            segment_deltas: tint_delta,
            segments: tint_segments,
            segment_offsets: tint_offsets,
        }),
        looped: true,
        version: 1,
    })
}

fn build_segment_cache_from_keys<T, F>(
    frames: &[ClipKeyframe<T>],
    mut delta_fn: F,
) -> (Arc<[T]>, Arc<[ClipSegment<T>]>, Arc<[f32]>)
where
    T: Copy + std::ops::Mul<f32, Output = T>,
    F: FnMut(&[ClipKeyframe<T>]) -> T,
{
    if frames.len() < 2 {
        return (Arc::from([]), Arc::from([]), Arc::from([]));
    }
    let mut deltas = Vec::with_capacity(frames.len() - 1);
    let mut segments = Vec::with_capacity(frames.len() - 1);
    let mut offsets = Vec::with_capacity(frames.len() - 1);
    for window in frames.windows(2) {
        offsets.push(window[0].time);
        let span = (window[1].time - window[0].time).max(f32::EPSILON);
        let inv_span = 1.0 / span;
        let delta = delta_fn(window);
        segments.push(ClipSegment { slope: delta * inv_span, span, inv_span });
        deltas.push(delta);
    }
    (
        Arc::from(deltas.into_boxed_slice()),
        Arc::from(segments.into_boxed_slice()),
        Arc::from(offsets.into_boxed_slice()),
    )
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

fn apply_phase_offset(animation: &mut SpriteAnimation, random_seed: f32) {
    if animation.frames.is_empty() {
        animation.frame_index = 0;
        animation.elapsed_in_frame = 0.0;
        animation.refresh_current_duration();
        return;
    }
    let total = animation.total_duration().max(f32::EPSILON);
    let offset = (random_seed * total).rem_euclid(total);
    animation.frame_index = 0;
    animation.elapsed_in_frame = 0.0;
    animation.refresh_current_duration();

    let mut accumulated = 0.0;
    for (index, duration) in animation.frame_durations.iter().copied().enumerate() {
        let duration = duration.max(f32::EPSILON);
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
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut hasher);
    timeline.hash(&mut hasher);
    const SCALE: f64 = 1.0 / (u64::MAX as f64 + 1.0);
    (hasher.finish() as f64 * SCALE) as f32
}
