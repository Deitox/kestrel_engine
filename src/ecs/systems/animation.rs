use super::{AnimationDelta, AnimationTime, TimeDelta};
use crate::assets::skeletal::{JointQuatTrack, JointVec3Track, SkeletalClip};
use crate::assets::{ClipInterpolation, ClipKeyframe};
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::{
    BoneTransforms, ClipInstance, ClipSample, PropertyTrackPlayer, SkeletonInstance, Sprite, SpriteAnimation,
    SpriteAnimationLoopMode, Tint, Transform, TransformTrackPlayer,
};
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::{Entity, Mut, Query, Res, ResMut};
use glam::{Mat4, Quat, Vec3};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ptr::NonNull;

#[cfg(feature = "anim_stats")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "anim_stats")]
#[derive(Clone, Copy, Debug, Default)]
pub struct SpriteAnimationStats {
    pub fast_loop_calls: u64,
    pub event_calls: u64,
    pub plain_calls: u64,
}

#[cfg(feature = "anim_stats")]
#[derive(Clone, Copy, Debug, Default)]
pub struct TransformClipStats {
    pub advance_calls: u64,
    pub zero_delta_calls: u64,
    pub skipped_clips: u64,
    pub looped_resume_clips: u64,
    pub zero_duration_clips: u64,
}

#[cfg(feature = "anim_stats")]
static SPRITE_FAST_LOOP_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_EVENT_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_PLAIN_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_ADVANCE_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_ZERO_DELTA_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_SKIPPED: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_LOOPED_RESUME: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_ZERO_DURATION: AtomicU64 = AtomicU64::new(0);

const CLIP_TIME_EPSILON: f32 = 1e-5;

#[cfg(feature = "anim_stats")]
pub fn sprite_animation_stats_snapshot() -> SpriteAnimationStats {
    SpriteAnimationStats {
        fast_loop_calls: SPRITE_FAST_LOOP_CALLS.load(Ordering::Relaxed),
        event_calls: SPRITE_EVENT_CALLS.load(Ordering::Relaxed),
        plain_calls: SPRITE_PLAIN_CALLS.load(Ordering::Relaxed),
    }
}

#[cfg(feature = "anim_stats")]
pub fn reset_sprite_animation_stats() {
    SPRITE_FAST_LOOP_CALLS.store(0, Ordering::Relaxed);
    SPRITE_EVENT_CALLS.store(0, Ordering::Relaxed);
    SPRITE_PLAIN_CALLS.store(0, Ordering::Relaxed);
}

#[cfg(feature = "anim_stats")]
pub fn transform_clip_stats_snapshot() -> TransformClipStats {
    TransformClipStats {
        advance_calls: TRANSFORM_CLIP_ADVANCE_CALLS.load(Ordering::Relaxed),
        zero_delta_calls: TRANSFORM_CLIP_ZERO_DELTA_CALLS.load(Ordering::Relaxed),
        skipped_clips: TRANSFORM_CLIP_SKIPPED.load(Ordering::Relaxed),
        looped_resume_clips: TRANSFORM_CLIP_LOOPED_RESUME.load(Ordering::Relaxed),
        zero_duration_clips: TRANSFORM_CLIP_ZERO_DURATION.load(Ordering::Relaxed),
    }
}

#[cfg(feature = "anim_stats")]
pub fn reset_transform_clip_stats() {
    TRANSFORM_CLIP_ADVANCE_CALLS.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_ZERO_DELTA_CALLS.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_SKIPPED.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_LOOPED_RESUME.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_ZERO_DURATION.store(0, Ordering::Relaxed);
}

#[cfg(feature = "anim_stats")]
fn record_fast_loop_call(count: u64) {
    SPRITE_FAST_LOOP_CALLS.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_fast_loop_call(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_event_call(count: u64) {
    SPRITE_EVENT_CALLS.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_event_call(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_plain_call(count: u64) {
    SPRITE_PLAIN_CALLS.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_plain_call(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_advance(count: u64) {
    TRANSFORM_CLIP_ADVANCE_CALLS.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_advance(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_zero_delta(count: u64) {
    TRANSFORM_CLIP_ZERO_DELTA_CALLS.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_zero_delta(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_skipped(count: u64) {
    TRANSFORM_CLIP_SKIPPED.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_skipped(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_looped_resume(count: u64) {
    TRANSFORM_CLIP_LOOPED_RESUME.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_looped_resume(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_zero_duration(count: u64) {
    TRANSFORM_CLIP_ZERO_DURATION.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_zero_duration(_count: u64) {}

pub fn sys_drive_sprite_animations(
    mut profiler: ResMut<SystemProfiler>,
    dt: Res<TimeDelta>,
    mut animation_time: ResMut<AnimationTime>,
    mut events: ResMut<EventBus>,
    mut animations: Query<(Entity, &mut SpriteAnimation, &mut Sprite)>,
) {
    let _span = profiler.scope("sys_drive_sprite_animations");
    let plan = animation_time.consume(dt.0);
    if !plan.has_steps() {
        return;
    }
    let has_group_scales = animation_time.has_group_scales();
    let animation_time_ref: &AnimationTime = &*animation_time;
    match plan {
        AnimationDelta::None => {}
        AnimationDelta::Single(delta) => {
            if delta > 0.0 {
                drive_single(delta, has_group_scales, animation_time_ref, &mut events, &mut animations);
            }
        }
        AnimationDelta::Fixed { step, steps } => {
            if steps > 0 {
                drive_fixed(step, steps, has_group_scales, animation_time_ref, &mut events, &mut animations);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::skeletal::{load_skeleton_from_gltf, SkeletonAsset};
    use anyhow::Result;
    use glam::{Mat4, Quat, Vec3};
    use std::path::Path;
    use std::sync::Arc;

    struct SkeletalFixture {
        skeleton: Arc<SkeletonAsset>,
        clip: Arc<SkeletalClip>,
    }

    impl SkeletalFixture {
        fn load() -> Result<Self> {
            let path = Path::new("fixtures/gltf/skeletons/slime_rig.gltf");
            let import = load_skeleton_from_gltf(path)?;
            let skeleton = Arc::new(import.skeleton);
            let clip = Arc::new(
                import.clips.into_iter().next().ok_or_else(|| anyhow::anyhow!("Fixture clip missing"))?,
            );
            Ok(Self { skeleton, clip })
        }

        fn sample(&self, time: f32) -> SkeletonInstance {
            let skeleton_key = Arc::clone(&self.skeleton.name);
            let mut instance = SkeletonInstance::new(skeleton_key, Arc::clone(&self.skeleton));
            instance.reset_to_rest_pose();
            evaluate_skeleton_pose(&mut instance, self.clip.as_ref(), time);
            instance
        }
    }

    #[test]
    fn slime_rig_pose_matches_keyframes() -> Result<()> {
        let fixture = SkeletalFixture::load()?;

        let at_start = fixture.sample(0.0);
        let expected_root_start = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
        let expected_child_local_start = Mat4::from_translation(Vec3::new(0.0, 2.0, 0.0));
        let expected_child_model_start = expected_root_start * expected_child_local_start;
        assert_mat4_approx(at_start.local_poses[0], expected_root_start, "root local @ t=0");
        assert_mat4_approx(at_start.model_poses[0], expected_root_start, "root model @ t=0");
        assert_mat4_approx(at_start.palette[0], expected_root_start, "root palette @ t=0");
        assert_mat4_approx(at_start.local_poses[1], expected_child_local_start, "child local @ t=0");
        assert_mat4_approx(at_start.model_poses[1], expected_child_model_start, "child model @ t=0");
        assert_mat4_approx(at_start.palette[1], expected_child_model_start, "child palette @ t=0");

        let at_mid = fixture.sample(0.5);
        let expected_root_mid = Mat4::from_translation(Vec3::new(0.0, 1.05, 0.0));
        let expected_child_local_mid = Mat4::from_scale_rotation_translation(
            Vec3::new(1.05, 0.95, 1.0),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
            Vec3::new(0.0, 2.1, 0.0),
        );
        let expected_child_model_mid = expected_root_mid * expected_child_local_mid;
        assert_mat4_approx(at_mid.local_poses[0], expected_root_mid, "root local @ t=0.5");
        assert_mat4_approx(at_mid.model_poses[0], expected_root_mid, "root model @ t=0.5");
        assert_mat4_approx(at_mid.palette[0], expected_root_mid, "root palette @ t=0.5");
        assert_mat4_approx(at_mid.local_poses[1], expected_child_local_mid, "child local @ t=0.5");
        assert_mat4_approx(at_mid.model_poses[1], expected_child_model_mid, "child model @ t=0.5");
        assert_mat4_approx(at_mid.palette[1], expected_child_model_mid, "child palette @ t=0.5");

        let at_end = fixture.sample(1.0);
        let expected_root_end = Mat4::from_translation(Vec3::new(0.0, 1.1, 0.0));
        let expected_child_local_end = Mat4::from_scale_rotation_translation(
            Vec3::new(1.1, 0.9, 1.0),
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Vec3::new(0.0, 2.2, 0.0),
        );
        let expected_child_model_end = expected_root_end * expected_child_local_end;
        assert_mat4_approx(at_end.local_poses[0], expected_root_end, "root local @ t=1.0");
        assert_mat4_approx(at_end.model_poses[0], expected_root_end, "root model @ t=1.0");
        assert_mat4_approx(at_end.palette[0], expected_root_end, "root palette @ t=1.0");
        assert_mat4_approx(at_end.local_poses[1], expected_child_local_end, "child local @ t=1.0");
        assert_mat4_approx(at_end.model_poses[1], expected_child_model_end, "child model @ t=1.0");
        assert_mat4_approx(at_end.palette[1], expected_child_model_end, "child palette @ t=1.0");

        Ok(())
    }

    #[test]
    fn slime_rig_pose_wraps_time() -> Result<()> {
        let fixture = SkeletalFixture::load()?;
        let early = fixture.sample(0.25);
        let late = fixture.sample(1.25);

        assert_mat4_approx(early.model_poses[0], late.model_poses[0], "root model wrap");
        assert_mat4_approx(early.palette[0], late.palette[0], "root palette wrap");
        assert_mat4_approx(early.model_poses[1], late.model_poses[1], "child model wrap");
        assert_mat4_approx(early.palette[1], late.palette[1], "child palette wrap");

        Ok(())
    }

    fn assert_mat4_approx(actual: Mat4, expected: Mat4, label: &str) {
        let actual = actual.to_cols_array();
        let expected = expected.to_cols_array();
        for (index, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
            let delta = (a - e).abs();
            assert!(
                delta < 1e-4,
                "{label} mismatch at element {index}: expected {e}, got {a}, delta {delta}"
            );
        }
    }
}

pub fn sys_drive_transform_clips(
    mut profiler: ResMut<SystemProfiler>,
    dt: Res<TimeDelta>,
    mut animation_time: ResMut<AnimationTime>,
    mut clips: Query<(
        Entity,
        &mut ClipInstance,
        Option<&TransformTrackPlayer>,
        Option<&PropertyTrackPlayer>,
        Option<Mut<Transform>>,
        Option<Mut<Tint>>,
    )>,
) {
    let _span = profiler.scope("sys_drive_transform_clips");
    let plan = animation_time.consume(dt.0);
    if !plan.has_steps() {
        return;
    }
    let has_group_scales = animation_time.has_group_scales();
    let animation_time_ref: &AnimationTime = &*animation_time;
    let delta = match plan {
        AnimationDelta::None => return,
        AnimationDelta::Single(amount) => amount,
        AnimationDelta::Fixed { step, steps } => step * steps as f32,
    };
    if delta <= 0.0 {
        return;
    }
    drive_transform_clips(delta, has_group_scales, animation_time_ref, &mut clips);
}

pub fn sys_drive_skeletal_clips(
    mut profiler: ResMut<SystemProfiler>,
    dt: Res<TimeDelta>,
    mut animation_time: ResMut<AnimationTime>,
    mut skeletons: Query<(Entity, &mut SkeletonInstance, Option<Mut<BoneTransforms>>)>,
) {
    let _span = profiler.scope("sys_drive_skeletal_clips");
    let plan = animation_time.consume(dt.0);
    if !plan.has_steps() {
        return;
    }
    let has_group_scales = animation_time.has_group_scales();
    let animation_time_ref: &AnimationTime = &*animation_time;
    let delta = match plan {
        AnimationDelta::None => return,
        AnimationDelta::Single(amount) => amount,
        AnimationDelta::Fixed { step, steps } => step * steps as f32,
    };
    if delta <= 0.0 {
        return;
    }
    drive_skeletal_clips(delta, has_group_scales, animation_time_ref, &mut skeletons);
}

fn drive_skeletal_clips(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    skeletons: &mut Query<(Entity, &mut SkeletonInstance, Option<Mut<BoneTransforms>>)>,
) {
    for (_entity, mut instance, bone_transforms) in skeletons.iter_mut() {
        instance.ensure_capacity();
        let clip = match instance.active_clip.clone() {
            Some(clip) => clip,
            None => {
                instance.reset_to_rest_pose();
                if let Some(mut bones) = bone_transforms {
                    bones.ensure_joint_count(instance.joint_count());
                    bones.model.copy_from_slice(&instance.model_poses);
                    bones.palette.copy_from_slice(&instance.palette);
                }
                continue;
            }
        };

        let group_scale =
            if has_group_scales { animation_time.group_scale(instance.group.as_deref()) } else { 1.0 };
        let playback_rate = if instance.playback_rate_dirty {
            instance.ensure_playback_rate(group_scale)
        } else {
            instance.playback_rate
        };
        if playback_rate <= 0.0 {
            continue;
        }

        let scaled = delta * playback_rate;
        if scaled <= 0.0 {
            continue;
        }

        if instance.playing {
            let mut next_time = instance.time + scaled;
            let duration = clip.duration.max(0.0);
            if duration > 0.0 {
                if instance.looped {
                    if next_time >= duration && (next_time - duration).abs() <= CLIP_TIME_EPSILON {
                        next_time = duration;
                    } else {
                        next_time = next_time.rem_euclid(duration.max(std::f32::EPSILON));
                    }
                } else if next_time >= duration {
                    next_time = duration;
                    instance.playing = false;
                }
            } else {
                next_time = 0.0;
            }
            instance.time = next_time;
        }

        let pose_time = instance.time;
        evaluate_skeleton_pose(&mut instance, &clip, pose_time);

        if let Some(mut bones) = bone_transforms {
            bones.ensure_joint_count(instance.joint_count());
            bones.model.copy_from_slice(&instance.model_poses);
            bones.palette.copy_from_slice(&instance.palette);
        }
    }
}

pub(crate) fn evaluate_skeleton_pose(instance: &mut SkeletonInstance, clip: &SkeletalClip, time: f32) {
    let joint_count = instance.joint_count();
    if joint_count == 0 {
        return;
    }

    instance.ensure_capacity();
    let channel_map = &mut instance.joint_channel_map;
    for slot in channel_map.iter_mut() {
        *slot = None;
    }
    for (curve_index, curve) in clip.channels.iter().enumerate() {
        let index = curve.joint_index as usize;
        if index < channel_map.len() {
            channel_map[index] = Some(curve_index);
        }
    }

    for (index, joint) in instance.skeleton.joints.iter().enumerate() {
        let mut translation = joint.rest_translation;
        let mut rotation = joint.rest_rotation;
        let mut scale = joint.rest_scale;
        if let Some(curve_index) = channel_map[index] {
            let curve = &clip.channels[curve_index];
            if let Some(track) = &curve.translation {
                translation = sample_vec3_track(track, time, clip.looped);
            }
            if let Some(track) = &curve.rotation {
                rotation = sample_quat_track(track, time, clip.looped);
            }
            if let Some(track) = &curve.scale {
                scale = sample_vec3_track(track, time, clip.looped);
            }
        }
        instance.local_poses[index] = Mat4::from_scale_rotation_translation(scale, rotation, translation);
    }

    let mut visited = std::mem::take(&mut instance.joint_visited);
    if visited.len() != joint_count {
        visited.resize(joint_count, false);
    }
    for flag in visited.iter_mut() {
        *flag = false;
    }

    let roots = instance.skeleton.roots.clone();
    for &root in roots.iter() {
        let root_index = root as usize;
        if root_index < joint_count {
            propagate_joint(root_index, Mat4::IDENTITY, instance, &mut visited);
        }
    }

    for index in 0..joint_count {
        if !visited[index] {
            propagate_joint(index, Mat4::IDENTITY, instance, &mut visited);
        }
    }
    instance.joint_visited = visited;
}

fn propagate_joint(
    joint_index: usize,
    parent_model: Mat4,
    instance: &mut SkeletonInstance,
    visited: &mut [bool],
) {
    let model = parent_model * instance.local_poses[joint_index];
    instance.model_poses[joint_index] = model;
    let joint = &instance.skeleton.joints[joint_index];
    instance.palette[joint_index] = model * joint.inverse_bind;
    visited[joint_index] = true;
    debug_assert!(joint_index < instance.joint_children.len());
    let (child_ptr, child_len) = unsafe {
        let child_vec = instance.joint_children.get_unchecked(joint_index);
        (child_vec.as_ptr(), child_vec.len())
    };
    for idx in 0..child_len {
        let child = unsafe { *child_ptr.add(idx) };
        propagate_joint(child, model, instance, visited);
    }
}

fn sample_vec3_track(track: &JointVec3Track, time: f32, looped: bool) -> Vec3 {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return Vec3::ZERO;
    }
    let duration = frames.last().map(|kf| kf.time).unwrap_or(0.0);
    let sample_time = normalize_time(time, duration, looped);
    sample_frames(frames, track.interpolation, sample_time, |a, b, t| a + (b - a) * t)
}

fn sample_quat_track(track: &JointQuatTrack, time: f32, looped: bool) -> Quat {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return Quat::IDENTITY;
    }
    let duration = frames.last().map(|kf| kf.time).unwrap_or(0.0);
    let sample_time = normalize_time(time, duration, looped);
    sample_frames(frames, track.interpolation, sample_time, |a, b, t| a.slerp(b, t)).normalize()
}

fn sample_frames<T, L>(frames: &[ClipKeyframe<T>], mode: ClipInterpolation, time: f32, lerp: L) -> T
where
    T: Copy,
    L: Fn(T, T, f32) -> T,
{
    if frames.len() == 1 || time <= frames[0].time {
        return frames[0].value;
    }
    if matches!(mode, ClipInterpolation::Step) {
        for window in frames.windows(2) {
            if time < window[1].time {
                return window[0].value;
            }
        }
        return frames.last().unwrap().value;
    }
    for window in frames.windows(2) {
        let start = &window[0];
        let end = &window[1];
        if time <= end.time {
            let span = (end.time - start.time).max(std::f32::EPSILON);
            let alpha = ((time - start.time) / span).clamp(0.0, 1.0);
            return lerp(start.value, end.value, alpha);
        }
    }
    frames.last().unwrap().value
}

fn normalize_time(time: f32, duration: f32, looped: bool) -> f32 {
    if duration <= 0.0 {
        return 0.0;
    }
    if looped {
        if (time - duration).abs() <= CLIP_TIME_EPSILON {
            return duration;
        }
        if time >= 0.0 && time < duration {
            return time;
        }
        let wrapped = time.rem_euclid(duration.max(std::f32::EPSILON));
        if wrapped <= CLIP_TIME_EPSILON && time > 0.0 && (time - duration).abs() <= CLIP_TIME_EPSILON {
            duration
        } else {
            wrapped
        }
    } else {
        time.clamp(0.0, duration)
    }
}

fn drive_transform_clips(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    clips: &mut Query<(
        Entity,
        &mut ClipInstance,
        Option<&TransformTrackPlayer>,
        Option<&PropertyTrackPlayer>,
        Option<Mut<Transform>>,
        Option<Mut<Tint>>,
    )>,
) {
    for (_entity, mut instance, transform_player, property_player, mut transform, tint) in clips.iter_mut() {
        if !instance.playing && instance.looped {
            // Looping clips resume automatically; keep advancing even if flagged not playing.
            #[cfg(feature = "anim_stats")]
            record_transform_looped_resume(1);
        } else if !instance.playing {
            #[cfg(feature = "anim_stats")]
            record_transform_skipped(1);
            continue;
        }

        let group_scale =
            if has_group_scales { animation_time.group_scale(instance.group.as_deref()) } else { 1.0 };
        let playback_rate = if instance.playback_rate_dirty {
            instance.ensure_playback_rate(group_scale)
        } else {
            instance.playback_rate
        };
        if playback_rate <= 0.0 {
            continue;
        }

        let scaled = delta * playback_rate;
        if scaled <= 0.0 {
            continue;
        }

        if instance.duration() <= 0.0 {
            #[cfg(feature = "anim_stats")]
            record_transform_zero_duration(1);
            instance.time = 0.0;
            continue;
        }

        let transform_mask = transform_player.copied().unwrap_or_default();
        let property_mask = property_player.copied().unwrap_or_default();
        let wants_tint = property_mask.apply_tint;
        let transform_available = transform.is_some();
        let tint_available = !wants_tint || tint.is_some();
        let can_fast_path = transform_available
            && transform_mask.apply_translation
            && transform_mask.apply_rotation
            && transform_mask.apply_scale
            && tint_available;

        if can_fast_path {
            let applied = instance.advance_time(scaled);
            record_transform_advance(1);
            #[cfg(feature = "anim_stats")]
            {
                if applied <= 0.0 {
                    record_transform_zero_delta(1);
                }
            }
            #[cfg(not(feature = "anim_stats"))]
            let _ = applied;

            let sample = instance.sample_cached();
            let transform_ref =
                transform.as_deref_mut().expect("transform missing in fast path despite guard");
            if let Some(value) = sample.translation {
                if instance.last_translation.map_or(true, |prev| prev != value) {
                    transform_ref.translation = value;
                }
            }
            if let Some(value) = sample.rotation {
                if instance.last_rotation.map_or(true, |prev| prev != value) {
                    transform_ref.rotation = value;
                }
            }
            if let Some(value) = sample.scale {
                if instance.last_scale.map_or(true, |prev| prev != value) {
                    transform_ref.scale = value;
                }
            }

            if wants_tint {
                if let Some(mut tint_ref) = tint {
                    if let Some(value) = sample.tint {
                        if instance.last_tint.map_or(true, |prev| prev != value) {
                            tint_ref.0 = value;
                        }
                    }
                }
            }

            instance.last_translation = sample.translation;
            instance.last_rotation = sample.rotation;
            instance.last_scale = sample.scale;
            instance.last_tint = sample.tint;

            continue;
        }

        let applied = instance.advance_time(scaled);
        record_transform_advance(1);
        #[cfg(feature = "anim_stats")]
        {
            if applied <= 0.0 {
                record_transform_zero_delta(1);
            }
        }
        #[cfg(not(feature = "anim_stats"))]
        let _ = applied;
        let sample = instance.sample_cached();
        apply_clip_sample(&mut instance, transform_player, property_player, transform, tint, sample);
    }
}

#[inline(always)]
fn apply_clip_sample(
    instance: &mut ClipInstance,
    transform_player: Option<&TransformTrackPlayer>,
    property_player: Option<&PropertyTrackPlayer>,
    transform: Option<Mut<Transform>>,
    tint: Option<Mut<Tint>>,
    sample: ClipSample,
) {
    if let Some(mut transform) = transform {
        let mask = transform_player.copied().unwrap_or_default();
        if mask.apply_translation && mask.apply_rotation && mask.apply_scale {
            if let Some(value) = sample.translation {
                if instance.last_translation.map_or(true, |prev| prev != value) {
                    transform.translation = value;
                }
            }
            if let Some(value) = sample.rotation {
                if instance.last_rotation.map_or(true, |prev| prev != value) {
                    transform.rotation = value;
                }
            }
            if let Some(value) = sample.scale {
                if instance.last_scale.map_or(true, |prev| prev != value) {
                    transform.scale = value;
                }
            }
        } else {
            if mask.apply_translation {
                if let Some(value) = sample.translation {
                    if instance.last_translation.map_or(true, |prev| prev != value) {
                        transform.translation = value;
                    }
                }
            }
            if mask.apply_rotation {
                if let Some(value) = sample.rotation {
                    if instance.last_rotation.map_or(true, |prev| prev != value) {
                        transform.rotation = value;
                    }
                }
            }
            if mask.apply_scale {
                if let Some(value) = sample.scale {
                    if instance.last_scale.map_or(true, |prev| prev != value) {
                        transform.scale = value;
                    }
                }
            }
        }
    }

    if let Some(mut tint_component) = tint {
        let mask = property_player.copied().unwrap_or_default();
        if mask.apply_tint {
            if let Some(value) = sample.tint {
                let changed = instance.last_tint.map_or(true, |prev| prev != value);
                if changed {
                    tint_component.0 = value;
                }
            }
        }
    }

    instance.last_translation = sample.translation;
    instance.last_rotation = sample.rotation;
    instance.last_scale = sample.scale;
    instance.last_tint = sample.tint;
}

pub(crate) fn initialize_animation_phase(animation: &mut SpriteAnimation, entity: Entity) -> bool {
    if animation.frames.is_empty() {
        return false;
    }
    animation.frame_index = 0;
    animation.elapsed_in_frame = 0.0;
    animation.forward = true;
    animation.refresh_current_duration();

    let mut offset = animation.start_offset.max(0.0);
    let total = animation.total_duration();
    if animation.random_start && total > 0.0 {
        let random_fraction = stable_random_fraction(entity, animation.timeline.as_ref());
        offset = (offset + random_fraction * total).rem_euclid(total.max(std::f32::EPSILON));
    }

    if !animation.mode.looped() && total > 0.0 {
        offset = offset.min(total);
    }

    if offset <= 0.0 {
        return true;
    }

    let was_playing = animation.playing;
    let changed = advance_animation(animation, offset, entity, None, false);
    animation.playing = was_playing;
    changed
}

pub(crate) fn advance_animation(
    animation: &mut SpriteAnimation,
    mut delta: f32,
    entity: Entity,
    mut events: Option<&mut EventBus>,
    respect_terminal_behavior: bool,
) -> bool {
    if delta <= 0.0 {
        return false;
    }
    let frames = animation.frames.as_ref();
    if frames.is_empty() {
        return false;
    }

    #[cfg(feature = "anim_stats")]
    if events.as_ref().is_some() {
        record_event_call(1);
    } else {
        record_plain_call(1);
    }

    let len = frames.len();
    let mut frame_changed = false;
    while delta > 0.0 && animation.playing {
        let frame_duration = unsafe { *animation.frame_durations.get_unchecked(animation.frame_index) };
        let time_left = frame_duration - animation.elapsed_in_frame;
        if delta < time_left {
            animation.elapsed_in_frame += delta;
            delta = 0.0;
            continue;
        }

        delta -= time_left;
        animation.elapsed_in_frame = 0.0;
        let mut emit_frame_event = false;

        match animation.mode {
            SpriteAnimationLoopMode::Loop => {
                animation.frame_index = (animation.frame_index + 1) % len;
                animation.current_duration =
                    unsafe { *animation.frame_durations.get_unchecked(animation.frame_index) };
                emit_frame_event = true;
                frame_changed = true;
            }
            SpriteAnimationLoopMode::OnceStop => {
                animation.frame_index = len.saturating_sub(1);
                animation.current_duration = animation.frame_durations.last().copied().unwrap_or(0.0);
                frame_changed = true;
                if let Some(events) = events.as_deref_mut() {
                    emit_sprite_animation_events(entity, animation, events);
                }
                if respect_terminal_behavior {
                    animation.playing = false;
                }
                break;
            }
            SpriteAnimationLoopMode::OnceHold => {
                animation.frame_index = len.saturating_sub(1);
                animation.current_duration = animation.frame_durations.last().copied().unwrap_or(0.0);
                if let Some(last) = animation.frame_durations.last() {
                    animation.elapsed_in_frame = *last;
                }
                frame_changed = true;
                if let Some(events) = events.as_deref_mut() {
                    emit_sprite_animation_events(entity, animation, events);
                }
                if respect_terminal_behavior {
                    animation.playing = false;
                }
                break;
            }
            SpriteAnimationLoopMode::PingPong => {
                if len <= 1 {
                    animation.forward = true;
                    animation.current_duration = animation.frame_durations.first().copied().unwrap_or(0.0);
                } else if animation.forward {
                    if animation.frame_index + 1 < len {
                        animation.frame_index += 1;
                    } else {
                        animation.forward = false;
                        animation.frame_index = (len - 2).min(len - 1);
                    }
                    animation.current_duration =
                        unsafe { *animation.frame_durations.get_unchecked(animation.frame_index) };
                    frame_changed = true;
                    emit_frame_event = true;
                } else if animation.frame_index > 0 {
                    animation.frame_index -= 1;
                    animation.current_duration =
                        unsafe { *animation.frame_durations.get_unchecked(animation.frame_index) };
                    frame_changed = true;
                    emit_frame_event = true;
                } else {
                    animation.forward = true;
                    animation.frame_index = 1.min(len - 1);
                    animation.current_duration =
                        animation.frame_durations.get(animation.frame_index).copied().unwrap_or(0.0);
                    frame_changed = len > 1;
                    emit_frame_event = len > 1;
                }
            }
        }

        if emit_frame_event {
            if let Some(events) = events.as_deref_mut() {
                emit_sprite_animation_events(entity, animation, events);
            }
        }
    }

    frame_changed
}

fn emit_sprite_animation_events(entity: Entity, animation: &SpriteAnimation, events: &mut EventBus) {
    if let Some(frame) = animation.frames.get(animation.frame_index) {
        for name in frame.events.iter() {
            events.push(GameEvent::SpriteAnimationEvent {
                entity,
                timeline: animation.timeline.as_ref().to_string(),
                event: name.as_ref().to_string(),
            });
        }
    }
}

fn stable_random_fraction(entity: Entity, timeline: &str) -> f32 {
    let mut hasher = DefaultHasher::new();
    entity.hash(&mut hasher);
    timeline.hash(&mut hasher);
    let bits = hasher.finish();
    const SCALE: f64 = 1.0 / (u64::MAX as f64 + 1.0);
    (bits as f64 * SCALE) as f32
}
#[inline(always)]
fn advance_animation_loop_no_events(animation: &mut SpriteAnimation, mut delta: f32) -> bool {
    if delta <= 0.0 || !animation.playing {
        return false;
    }
    let len = animation.frame_durations.len();
    if len == 0 {
        return false;
    }

    #[cfg(feature = "anim_stats")]
    record_fast_loop_call(1);

    let mut index = animation.frame_index;
    let mut elapsed = animation.elapsed_in_frame;
    let mut frame_changed = false;

    while delta > 0.0 {
        let frame_duration = unsafe { *animation.frame_durations.get_unchecked(index) };
        let time_left = frame_duration - elapsed;
        if delta <= time_left {
            elapsed += delta;
            break;
        }

        delta -= time_left;
        elapsed = 0.0;
        index += 1;
        if index == len {
            index = 0;
        }
        frame_changed = true;
    }

    animation.frame_index = index;
    animation.elapsed_in_frame = elapsed;
    animation.current_duration = animation.frame_durations.get(animation.frame_index).copied().unwrap_or(0.0);
    frame_changed
}
fn drive_single(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    events: &mut EventBus,
    animations: &mut Query<(Entity, &mut SpriteAnimation, &mut Sprite)>,
) {
    for (entity, mut animation, mut sprite) in animations.iter_mut() {
        let frame_count = animation.frames.len();
        if frame_count == 0 {
            continue;
        }
        if animation.frame_index >= frame_count {
            animation.frame_index = 0;
            animation.elapsed_in_frame = 0.0;
            animation.refresh_current_duration();
        }
        if !animation.playing {
            continue;
        }

        let playback_rate = if animation.playback_rate_dirty {
            let group_scale =
                if has_group_scales { animation_time.group_scale(animation.group.as_deref()) } else { 1.0 };
            animation.ensure_playback_rate(group_scale)
        } else {
            animation.playback_rate
        };

        if playback_rate <= 0.0 {
            continue;
        }

        let scaled = delta * playback_rate;
        if scaled <= 0.0 {
            continue;
        }

        let mut sprite_changed = false;
        if animation.fast_loop {
            let current_duration = animation.current_duration;
            let new_elapsed = animation.elapsed_in_frame + scaled;
            if new_elapsed <= current_duration + CLIP_TIME_EPSILON {
                animation.elapsed_in_frame = new_elapsed.min(current_duration);
            } else if advance_animation_loop_no_events(&mut animation, scaled) {
                sprite_changed = true;
            }
        } else if animation.has_events {
            let events_ref = &mut *events;
            if advance_animation(&mut animation, scaled, entity, Some(events_ref), true) {
                sprite_changed = true;
            }
        } else if advance_animation(&mut animation, scaled, entity, None, true) {
            sprite_changed = true;
        }

        if sprite_changed {
            let frame_ptr = NonNull::from(&animation.frames[animation.frame_index]);
            drop(animation);
            unsafe {
                sprite.apply_frame(frame_ptr.as_ref());
            }
            continue;
        }
    }
}

fn drive_fixed(
    step: f32,
    steps: u32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    events: &mut EventBus,
    animations: &mut Query<(Entity, &mut SpriteAnimation, &mut Sprite)>,
) {
    for (entity, mut animation, mut sprite) in animations.iter_mut() {
        let frame_count = animation.frames.len();
        if frame_count == 0 {
            continue;
        }
        if animation.frame_index >= frame_count {
            animation.frame_index = 0;
            animation.elapsed_in_frame = 0.0;
            animation.refresh_current_duration();
        }
        if !animation.playing {
            continue;
        }

        let playback_rate = if animation.playback_rate_dirty {
            let group_scale =
                if has_group_scales { animation_time.group_scale(animation.group.as_deref()) } else { 1.0 };
            animation.ensure_playback_rate(group_scale)
        } else {
            animation.playback_rate
        };

        if playback_rate <= 0.0 {
            continue;
        }

        let scaled_step = step * playback_rate;
        if scaled_step <= 0.0 {
            continue;
        }

        let mut sprite_changed = false;
        if animation.fast_loop {
            let total = scaled_step * steps as f32;
            let current_duration = animation.current_duration;
            let new_elapsed = animation.elapsed_in_frame + total;
            if new_elapsed <= current_duration + CLIP_TIME_EPSILON {
                animation.elapsed_in_frame = new_elapsed.min(current_duration);
            } else if advance_animation_loop_no_events(&mut animation, total) {
                sprite_changed = true;
            }
        } else if animation.has_events {
            let events_ref = &mut *events;
            for _ in 0..steps {
                if !animation.playing {
                    break;
                }
                if advance_animation(&mut animation, scaled_step, entity, Some(events_ref), true) {
                    sprite_changed = true;
                }
            }
        } else {
            for _ in 0..steps {
                if !animation.playing {
                    break;
                }
                if advance_animation(&mut animation, scaled_step, entity, None, true) {
                    sprite_changed = true;
                }
            }
        }

        if sprite_changed {
            let frame_ptr = NonNull::from(&animation.frames[animation.frame_index]);
            drop(animation);
            unsafe {
                sprite.apply_frame(frame_ptr.as_ref());
            }
            continue;
        }
    }
}
