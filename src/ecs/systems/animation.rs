use super::{AnimationDelta, AnimationTime, TimeDelta};
use crate::assets::skeletal::{JointCurve, JointQuatTrack, JointVec3Track, SkeletalClip};
use crate::assets::{ClipInterpolation, ClipKeyframe};
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::{
    BoneTransforms, ClipInstance, ClipSample, PropertyTrackPlayer, SkeletonInstance, Sprite, SpriteAnimation,
    SpriteAnimationLoopMode, Tint, Transform, TransformTrackPlayer,
};
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::{Entity, Mut, Query, Res, ResMut};
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ptr::NonNull;

const CLIP_TIME_EPSILON: f32 = 1e-5;

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

fn evaluate_skeleton_pose(instance: &mut SkeletonInstance, clip: &SkeletalClip, time: f32) {
    let joint_count = instance.joint_count();
    if joint_count == 0 {
        return;
    }

    let mut curve_map: Vec<Option<&JointCurve>> = vec![None; joint_count];
    for curve in clip.channels.iter() {
        let index = curve.joint_index as usize;
        if index < curve_map.len() {
            curve_map[index] = Some(curve);
        }
    }

    let mut children: Vec<Vec<usize>> = vec![Vec::new(); joint_count];
    for (index, joint) in instance.skeleton.joints.iter().enumerate() {
        if let Some(parent) = joint.parent {
            if let Some(list) = children.get_mut(parent as usize) {
                list.push(index);
            }
        }
    }

    for (index, joint) in instance.skeleton.joints.iter().enumerate() {
        let mut translation = joint.rest_translation;
        let mut rotation = joint.rest_rotation;
        let mut scale = joint.rest_scale;
        if let Some(curve) = curve_map[index] {
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

    let mut visited = vec![false; joint_count];

    let root_indices: Vec<usize> = instance
        .skeleton
        .roots
        .iter()
        .map(|&root| root as usize)
        .filter(|&index| index < joint_count)
        .collect();

    for root_index in root_indices {
        propagate_joint(root_index, Mat4::IDENTITY, instance, &children, &mut visited);
    }

    for index in 0..joint_count {
        if !visited[index] {
            propagate_joint(index, Mat4::IDENTITY, instance, &children, &mut visited);
        }
    }
}

fn propagate_joint(
    joint_index: usize,
    parent_model: Mat4,
    instance: &mut SkeletonInstance,
    children: &[Vec<usize>],
    visited: &mut [bool],
) {
    let model = parent_model * instance.local_poses[joint_index];
    instance.model_poses[joint_index] = model;
    let joint = &instance.skeleton.joints[joint_index];
    instance.palette[joint_index] = model * joint.inverse_bind;
    visited[joint_index] = true;
    for &child in &children[joint_index] {
        propagate_joint(child, model, instance, children, visited);
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
        if time >= duration && (time - duration).abs() <= CLIP_TIME_EPSILON {
            duration
        } else {
            let wrapped = time.rem_euclid(duration.max(std::f32::EPSILON));
            if wrapped <= CLIP_TIME_EPSILON && time > 0.0 && (time - duration).abs() <= CLIP_TIME_EPSILON {
                duration
            } else {
                wrapped
            }
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
    for (_entity, mut instance, transform_player, property_player, transform, tint) in clips.iter_mut() {
        if !instance.playing && instance.looped {
            // Looping clips resume automatically; keep advancing even if flagged not playing.
        } else if !instance.playing {
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

        let duration = instance.duration();
        if duration <= 0.0 {
            instance.time = 0.0;
            continue;
        }

        let previous_time = instance.time;
        let mut new_time = previous_time + scaled;
        if instance.looped {
            new_time = new_time.rem_euclid(duration.max(std::f32::EPSILON));
        } else if new_time >= duration {
            new_time = duration;
            instance.playing = false;
        }
        instance.time = new_time;
        let sample = instance.sample();
        apply_clip_sample(&mut instance, transform_player, property_player, transform, tint, sample);
    }
}

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
        if mask.apply_translation {
            if let Some(value) = sample.translation {
                let changed = instance.last_translation.map_or(true, |prev| !approx_eq_vec2(prev, value));
                if changed {
                    transform.translation = value;
                }
            }
        }
        if mask.apply_rotation {
            if let Some(value) = sample.rotation {
                let changed = instance.last_rotation.map_or(true, |prev| !approx_eq_scalar(prev, value));
                if changed {
                    transform.rotation = value;
                }
            }
        }
        if mask.apply_scale {
            if let Some(value) = sample.scale {
                let changed = instance.last_scale.map_or(true, |prev| !approx_eq_vec2(prev, value));
                if changed {
                    transform.scale = value;
                }
            }
        }
    }

    if let Some(mut tint_component) = tint {
        let mask = property_player.copied().unwrap_or_default();
        if mask.apply_tint {
            if let Some(value) = sample.tint {
                let changed = instance.last_tint.map_or(true, |prev| !approx_eq_vec4(prev, value));
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

fn approx_eq_scalar(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5
}

fn approx_eq_vec2(a: Vec2, b: Vec2) -> bool {
    (a - b).length_squared() <= 1e-8
}

fn approx_eq_vec4(a: Vec4, b: Vec4) -> bool {
    (a - b).length_squared() <= 1e-6
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
            if new_elapsed <= current_duration {
                animation.elapsed_in_frame = new_elapsed;
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
            if new_elapsed <= current_duration {
                animation.elapsed_in_frame = new_elapsed;
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
