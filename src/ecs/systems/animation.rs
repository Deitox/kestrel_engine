use super::{AnimationDelta, AnimationPlan, AnimationTime};
use crate::assets::skeletal::{JointQuatTrack, JointVec3Track, SkeletalClip};
use crate::assets::{ClipInterpolation, ClipKeyframe};
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::{
    BoneTransforms, ClipInstance, ClipSample, FastSpriteAnimator, PropertyTrackPlayer, SkeletonInstance,
    Sprite, SpriteAnimation, SpriteAnimationLoopMode, SpriteFrameState, Tint, Transform,
    TransformTrackPlayer,
};
#[cfg(feature = "sprite_anim_soa")]
use crate::ecs::{SpriteAnimationFrame, SpriteFrameHotData};
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::{
    Added, Changed, Commands, Entity, Mut, Or, Query, Res, ResMut, Resource, With, Without,
};
use glam::{Mat4, Quat, Vec3};
use std::cell::Cell;
#[cfg(feature = "sprite_anim_soa")]
use std::collections::HashMap;
use std::collections::{hash_map::DefaultHasher, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
#[cfg(feature = "anim_stats")]
use std::time::Duration;
#[cfg(any(feature = "anim_stats", feature = "sprite_anim_simd"))]
use std::time::Instant;
#[cfg(feature = "sprite_anim_simd")]
use wide::f32x8;

#[derive(Default, Resource)]
pub struct SpriteFrameApplyQueue {
    entities: Vec<Entity>,
}

impl SpriteFrameApplyQueue {
    pub fn push(&mut self, entity: Entity) {
        self.entities.push(entity);
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn take(&mut self) -> Vec<Entity> {
        std::mem::take(&mut self.entities)
    }

    pub fn restore(&mut self, mut entities: Vec<Entity>) {
        entities.clear();
        self.entities = entities;
    }
}

#[cfg(feature = "sprite_anim_soa")]
#[derive(Default, Resource)]
pub struct SpriteAnimatorSoa {
    entities: Vec<Entity>,
    frame_index: Vec<u32>,
    elapsed_in_frame: Vec<f32>,
    current_duration: Vec<f32>,
    current_frame_offset: Vec<f32>,
    next_frame_duration: Vec<f32>,
    #[cfg(feature = "sprite_anim_fixed_point")]
    next_frame_duration_fp: Vec<u32>,
    #[cfg(feature = "sprite_anim_simd")]
    const_dt_duration: Vec<f32>,
    #[cfg(all(feature = "sprite_anim_simd", feature = "sprite_anim_fixed_point"))]
    const_dt_duration_fp: Vec<u32>,
    #[cfg(feature = "sprite_anim_simd")]
    const_dt_frame_count: Vec<u32>,
    #[cfg(feature = "sprite_anim_fixed_point")]
    elapsed_in_frame_fp: Vec<u32>,
    #[cfg(feature = "sprite_anim_fixed_point")]
    current_duration_fp: Vec<u32>,
    #[cfg(feature = "sprite_anim_fixed_point")]
    current_frame_offset_fp: Vec<u32>,
    pending_delta: Vec<f32>,
    playback_rate: Vec<f32>,
    speed: Vec<f32>,
    total_duration: Vec<f32>,
    #[cfg(feature = "sprite_anim_fixed_point")]
    total_duration_fp: Vec<u32>,
    total_duration_inv: Vec<f32>,
    frames: Vec<Arc<[SpriteAnimationFrame]>>,
    frame_hot_data: Vec<Arc<[SpriteFrameHotData]>>,
    frame_durations: Vec<Arc<[f32]>>,
    frame_offsets: Vec<Arc<[f32]>>,
    #[cfg(feature = "sprite_anim_fixed_point")]
    frame_durations_fp: Vec<Arc<[u32]>>,
    #[cfg(feature = "sprite_anim_fixed_point")]
    frame_offsets_fp: Vec<Arc<[u32]>>,
    group: Vec<Option<Arc<str>>>,
    flags: Vec<SpriteAnimatorFlags>,
    entity_to_slot: HashMap<Entity, u32>,
}

#[cfg(feature = "sprite_anim_soa")]
type SpriteStateUpdate = (Entity, usize);

#[cfg(feature = "sprite_anim_soa")]
impl SpriteAnimatorSoa {
    pub fn len(&self) -> usize {
        self.entities.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    pub fn contains(&self, entity: Entity) -> bool {
        self.entity_to_slot.contains_key(&entity)
    }

    pub fn slot_index(&self, entity: Entity) -> Option<usize> {
        self.entity_to_slot.get(&entity).copied().map(|slot| slot as usize)
    }

    pub fn entity(&self, slot: usize) -> Entity {
        self.entities[slot]
    }

    pub fn upsert(&mut self, entity: Entity, animation: &SpriteAnimation) {
        if let Some(slot) = self.slot_index(entity) {
            self.write_slot(slot, animation);
        } else {
            self.push_slot(entity, animation);
        }
    }

    pub fn remove(&mut self, entity: Entity) -> bool {
        let Some(slot) = self.entity_to_slot.remove(&entity) else {
            return false;
        };
        self.swap_remove_slot(slot as usize);
        true
    }

    pub fn retain_entities<F>(&mut self, mut keep: F)
    where
        F: FnMut(Entity) -> bool,
    {
        let mut idx = 0;
        while idx < self.entities.len() {
            let entity = self.entities[idx];
            if keep(entity) {
                idx += 1;
            } else {
                self.remove(entity);
            }
        }
    }

    fn push_slot(&mut self, entity: Entity, animation: &SpriteAnimation) {
        let slot = self.entities.len() as u32;
        self.entity_to_slot.insert(entity, slot);
        self.entities.push(entity);
        self.frame_index.push(animation.frame_index as u32);
        self.elapsed_in_frame.push(animation.elapsed_in_frame);
        self.current_duration.push(animation.current_duration);
        self.current_frame_offset.push(animation.current_frame_offset);
        let next_duration =
            next_duration_from_slice(animation.frame_durations.as_ref(), animation.frame_index);
        self.next_frame_duration.push(next_duration);
        #[cfg(feature = "sprite_anim_fixed_point")]
        self.next_frame_duration_fp.push(fp_from_f32(next_duration));
        #[cfg(feature = "sprite_anim_simd")]
        let const_dt_duration = detect_const_dt_duration(animation);
        #[cfg(feature = "sprite_anim_simd")]
        {
            self.const_dt_duration.push(const_dt_duration.unwrap_or(0.0));
            self.const_dt_frame_count.push(animation.frame_durations.len().max(1) as u32);
        }
        #[cfg(all(feature = "sprite_anim_simd", feature = "sprite_anim_fixed_point"))]
        self.const_dt_duration_fp.push(const_dt_duration.map(fp_from_f32).unwrap_or(0));
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            self.elapsed_in_frame_fp.push(fp_from_f32(animation.elapsed_in_frame));
            self.current_duration_fp.push(fp_from_f32(animation.current_duration));
            self.current_frame_offset_fp.push(fp_from_f32(animation.current_frame_offset));
            self.total_duration_fp.push(fp_from_f32(animation.total_duration));
            self.frame_durations_fp.push(convert_slice_to_fp(animation.frame_durations.as_ref()));
            self.frame_offsets_fp.push(convert_slice_to_fp(animation.frame_offsets.as_ref()));
        }
        self.pending_delta.push(animation.pending_small_delta);
        self.playback_rate.push(animation.playback_rate);
        self.speed.push(animation.speed);
        self.total_duration.push(animation.total_duration);
        self.total_duration_inv.push(animation.total_duration_inv);
        self.frames.push(Arc::clone(&animation.frames));
        self.frame_hot_data.push(Arc::clone(&animation.frame_hot_data));
        self.frame_durations.push(Arc::clone(&animation.frame_durations));
        self.frame_offsets.push(Arc::clone(&animation.frame_offsets));
        self.group.push(animation.group.as_ref().map(|g| Arc::<str>::from(g.as_str())));
        let mut flags = SpriteAnimatorFlags::from_animation(animation);
        #[cfg(feature = "sprite_anim_simd")]
        flags.set_const_dt(const_dt_duration.is_some());
        self.flags.push(flags);
    }

    fn write_slot(&mut self, slot: usize, animation: &SpriteAnimation) {
        self.frame_index[slot] = animation.frame_index as u32;
        self.elapsed_in_frame[slot] = animation.elapsed_in_frame;
        self.current_duration[slot] = animation.current_duration;
        self.current_frame_offset[slot] = animation.current_frame_offset;
        let next_duration =
            next_duration_from_slice(animation.frame_durations.as_ref(), animation.frame_index);
        self.next_frame_duration[slot] = next_duration;
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            self.next_frame_duration_fp[slot] = fp_from_f32(next_duration);
        }
        #[cfg(feature = "sprite_anim_simd")]
        let const_dt_duration = detect_const_dt_duration(animation);
        #[cfg(feature = "sprite_anim_simd")]
        {
            self.const_dt_duration[slot] = const_dt_duration.unwrap_or(0.0);
            self.const_dt_frame_count[slot] = animation.frame_durations.len().max(1) as u32;
        }
        #[cfg(all(feature = "sprite_anim_simd", feature = "sprite_anim_fixed_point"))]
        {
            self.const_dt_duration_fp[slot] = const_dt_duration.map(fp_from_f32).unwrap_or(0);
        }
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            self.elapsed_in_frame_fp[slot] = fp_from_f32(animation.elapsed_in_frame);
            self.current_duration_fp[slot] = fp_from_f32(animation.current_duration);
            self.current_frame_offset_fp[slot] = fp_from_f32(animation.current_frame_offset);
            self.total_duration_fp[slot] = fp_from_f32(animation.total_duration);
            self.frame_durations_fp[slot] = convert_slice_to_fp(animation.frame_durations.as_ref());
            self.frame_offsets_fp[slot] = convert_slice_to_fp(animation.frame_offsets.as_ref());
        }
        self.pending_delta[slot] = animation.pending_small_delta;
        self.playback_rate[slot] = animation.playback_rate;
        self.speed[slot] = animation.speed;
        self.total_duration[slot] = animation.total_duration;
        self.total_duration_inv[slot] = animation.total_duration_inv;
        self.frames[slot] = Arc::clone(&animation.frames);
        self.frame_hot_data[slot] = Arc::clone(&animation.frame_hot_data);
        self.frame_durations[slot] = Arc::clone(&animation.frame_durations);
        self.frame_offsets[slot] = Arc::clone(&animation.frame_offsets);
        self.group[slot] = animation.group.as_ref().map(|g| Arc::<str>::from(g.as_str()));
        let mut flags = SpriteAnimatorFlags::from_animation(animation);
        #[cfg(feature = "sprite_anim_simd")]
        flags.set_const_dt(const_dt_duration.is_some());
        self.flags[slot] = flags;
    }

    fn swap_remove_slot(&mut self, slot: usize) {
        if self.entities.is_empty() {
            return;
        }
        debug_assert!(slot < self.entities.len());
        let last = self.entities.len() - 1;
        if slot != last {
            self.entities.swap(slot, last);
            self.frame_index.swap(slot, last);
            self.elapsed_in_frame.swap(slot, last);
            self.current_duration.swap(slot, last);
            self.current_frame_offset.swap(slot, last);
            self.next_frame_duration.swap(slot, last);
            #[cfg(feature = "sprite_anim_fixed_point")]
            self.next_frame_duration_fp.swap(slot, last);
            #[cfg(feature = "sprite_anim_simd")]
            {
                self.const_dt_duration.swap(slot, last);
                self.const_dt_frame_count.swap(slot, last);
            }
            #[cfg(all(feature = "sprite_anim_simd", feature = "sprite_anim_fixed_point"))]
            self.const_dt_duration_fp.swap(slot, last);
            #[cfg(feature = "sprite_anim_fixed_point")]
            {
                self.elapsed_in_frame_fp.swap(slot, last);
                self.current_duration_fp.swap(slot, last);
                self.current_frame_offset_fp.swap(slot, last);
                self.total_duration_fp.swap(slot, last);
                self.frame_durations_fp.swap(slot, last);
                self.frame_offsets_fp.swap(slot, last);
            }
            self.pending_delta.swap(slot, last);
            self.playback_rate.swap(slot, last);
            self.flags.swap(slot, last);
            self.flags[slot].set_needs_prep(true);
            self.speed.swap(slot, last);
            self.total_duration.swap(slot, last);
            self.total_duration_inv.swap(slot, last);
            self.frames.swap(slot, last);
            self.frame_hot_data.swap(slot, last);
            self.frame_durations.swap(slot, last);
            self.frame_offsets.swap(slot, last);
            self.group.swap(slot, last);
            if let Some(entry) = self.entity_to_slot.get_mut(&self.entities[slot]) {
                *entry = slot as u32;
            }
        }
        self.entities.pop();
        self.frame_index.pop();
        self.elapsed_in_frame.pop();
        self.current_duration.pop();
        self.current_frame_offset.pop();
        self.next_frame_duration.pop();
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            self.next_frame_duration_fp.pop();
        }
        #[cfg(feature = "sprite_anim_simd")]
        {
            self.const_dt_duration.pop();
            self.const_dt_frame_count.pop();
        }
        #[cfg(all(feature = "sprite_anim_simd", feature = "sprite_anim_fixed_point"))]
        {
            self.const_dt_duration_fp.pop();
        }
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            self.elapsed_in_frame_fp.pop();
            self.current_duration_fp.pop();
            self.current_frame_offset_fp.pop();
            self.total_duration_fp.pop();
            self.frame_durations_fp.pop();
            self.frame_offsets_fp.pop();
        }
        self.pending_delta.pop();
        self.playback_rate.pop();
        self.flags.pop();
        self.speed.pop();
        self.total_duration.pop();
        self.total_duration_inv.pop();
        self.frames.pop();
        self.frame_hot_data.pop();
        self.frame_durations.pop();
        self.frame_offsets.pop();
        self.group.pop();
    }
}

#[cfg(feature = "sprite_anim_soa")]
fn next_duration_from_slice(durations: &[f32], frame_index: usize) -> f32 {
    if durations.is_empty() {
        0.0
    } else if durations.len() == 1 {
        durations[0]
    } else {
        let next = (frame_index + 1) % durations.len();
        durations.get(next).copied().unwrap_or(durations[0])
    }
}

#[cfg(feature = "sprite_anim_simd")]
fn detect_const_dt_duration(animation: &SpriteAnimation) -> Option<f32> {
    let durations = animation.frame_durations.as_ref();
    if durations.is_empty() {
        return None;
    }
    let first = durations[0];
    if first <= 0.0 {
        return None;
    }
    if durations.iter().all(|value| (value - first).abs() <= CLIP_TIME_EPSILON) {
        Some(first)
    } else {
        None
    }
}

#[cfg(feature = "sprite_anim_soa")]
fn refresh_next_frame_duration_slot(runtime: &mut SpriteAnimatorSoa, slot: usize) {
    let durations = runtime.frame_durations[slot].as_ref();
    let index = runtime.frame_index[slot] as usize;
    let next = next_duration_from_slice(durations, index);
    runtime.next_frame_duration[slot] = next;
    #[cfg(feature = "sprite_anim_fixed_point")]
    {
        runtime.next_frame_duration_fp[slot] = fp_from_f32(next);
    }
}

#[cfg(feature = "sprite_anim_soa")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SpriteAnimatorFlags(u16);

#[cfg(feature = "sprite_anim_soa")]
impl SpriteAnimatorFlags {
    const PLAYING: u16 = 1 << 0;
    const LOOPED: u16 = 1 << 1;
    const FORWARD: u16 = 1 << 2;
    const PREV_FORWARD: u16 = 1 << 3;
    const PENDING_START: u16 = 1 << 4;
    const PLAYBACK_DIRTY: u16 = 1 << 5;
    const NEEDS_PREP: u16 = 1 << 6;
    const CONST_DT: u16 = 1 << 7;

    fn set_bit(&mut self, mask: u16, enabled: bool) {
        if enabled {
            self.0 |= mask;
        } else {
            self.0 &= !mask;
        }
    }

    fn from_animation(animation: &SpriteAnimation) -> Self {
        let mut flags = Self::default();
        flags.set_bit(Self::PLAYING, animation.playing);
        flags.set_bit(Self::LOOPED, animation.looped);
        flags.set_bit(Self::FORWARD, animation.forward);
        flags.set_bit(Self::PREV_FORWARD, animation.prev_forward);
        flags.set_bit(Self::PENDING_START, animation.pending_start_events);
        flags.set_bit(Self::PLAYBACK_DIRTY, animation.playback_rate_dirty);
        flags.set_needs_prep(true);
        flags
    }

    fn playing(self) -> bool {
        self.0 & Self::PLAYING != 0
    }

    fn playback_dirty(self) -> bool {
        self.0 & Self::PLAYBACK_DIRTY != 0
    }

    fn set_playback_dirty(&mut self, value: bool) {
        self.set_bit(Self::PLAYBACK_DIRTY, value);
    }

    fn needs_prep(self) -> bool {
        self.0 & Self::NEEDS_PREP != 0
    }

    fn set_needs_prep(&mut self, value: bool) {
        self.set_bit(Self::NEEDS_PREP, value);
    }

    fn const_dt(self) -> bool {
        self.0 & Self::CONST_DT != 0
    }

    fn set_const_dt(&mut self, value: bool) {
        self.set_bit(Self::CONST_DT, value);
    }
}

#[cfg(feature = "sprite_anim_fixed_point")]
const FP_SHIFT: u32 = 16;
#[cfg(feature = "sprite_anim_fixed_point")]
const FP_ONE: u32 = 1 << FP_SHIFT;
#[cfg(feature = "sprite_anim_fixed_point")]
const FP_CLIP_EPSILON: u32 = 1;
#[cfg(feature = "sprite_anim_simd")]
const SPRITE_SIMD_WIDTH: usize = 8;

#[cfg(feature = "sprite_anim_simd")]
#[derive(Default)]
struct SimdMixStats {
    processed: u64,
    lanes8: u32,
    tail_scalar: u32,
}

#[cfg(feature = "sprite_anim_simd")]
struct PendingSimdSlot {
    slot: usize,
    delta: f32,
}

#[cfg(feature = "sprite_anim_fixed_point")]
#[inline]
fn fp_from_f32(value: f32) -> u32 {
    if value <= 0.0 {
        0
    } else {
        (value * FP_ONE as f32).round().max(0.0).min(u32::MAX as f32) as u32
    }
}

#[cfg(feature = "sprite_anim_fixed_point")]
#[inline]
fn f32_from_fp(value: u32) -> f32 {
    (value as f32) / (FP_ONE as f32)
}

#[cfg(feature = "sprite_anim_fixed_point")]
fn convert_slice_to_fp(values: &[f32]) -> Arc<[u32]> {
    Arc::from(values.iter().map(|&v| fp_from_f32(v)).collect::<Vec<u32>>().into_boxed_slice())
}

#[cfg(feature = "anim_stats")]
use std::time::Duration;

#[cfg(feature = "anim_stats")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "anim_stats")]
#[derive(Clone, Copy, Debug, Default)]
pub struct SpriteAnimationStats {
    pub fast_loop_calls: u64,
    pub event_calls: u64,
    pub plain_calls: u64,
    pub fast_loop_binary_searches: u64,
    pub fast_bucket_entities: u64,
    pub general_bucket_entities: u64,
    pub fast_bucket_frames: u64,
    pub general_bucket_frames: u64,
    pub frame_apply_count: u64,
    pub state_flush_calls: u64,
    pub state_flush_entities: u64,
    pub frame_apply_queue_drains: u64,
    pub frame_apply_queue_len: u64,
}

#[cfg(feature = "anim_stats")]
#[derive(Clone, Copy, Debug, Default)]
pub struct TransformClipStats {
    pub advance_calls: u64,
    pub zero_delta_calls: u64,
    pub skipped_clips: u64,
    pub looped_resume_clips: u64,
    pub zero_duration_clips: u64,
    pub fast_path_clips: u64,
    pub slow_path_clips: u64,
    pub segment_crosses: u64,
    pub advance_time_ns: u64,
    pub sample_time_ns: u64,
    pub apply_time_ns: u64,
}

#[cfg(feature = "anim_stats")]
#[derive(Default)]
struct TransformClipStatAccumulator {
    advance_calls: u64,
    zero_delta_calls: u64,
    skipped_clips: u64,
    zero_duration_clips: u64,
    fast_path_clips: u64,
    slow_path_clips: u64,
    sample_time: Duration,
    apply_time: Duration,
}

#[cfg(feature = "anim_stats")]
impl TransformClipStatAccumulator {
    fn flush(self) {
        if self.advance_calls > 0 {
            record_transform_advance(self.advance_calls);
        }
        if self.zero_delta_calls > 0 {
            record_transform_zero_delta(self.zero_delta_calls);
        }
        if self.skipped_clips > 0 {
            record_transform_skipped(self.skipped_clips);
        }
        if self.zero_duration_clips > 0 {
            record_transform_zero_duration(self.zero_duration_clips);
        }
        if self.fast_path_clips > 0 {
            record_transform_fast_path(self.fast_path_clips);
        }
        if self.slow_path_clips > 0 {
            record_transform_slow_path(self.slow_path_clips);
        }
        if self.sample_time > Duration::ZERO {
            record_transform_sample_time(self.sample_time);
        }
        if self.apply_time > Duration::ZERO {
            record_transform_apply_time(self.apply_time);
        }
    }
}

#[cfg(feature = "anim_stats")]
static SPRITE_FAST_LOOP_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_EVENT_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_PLAIN_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_FAST_LOOP_BINARY_SEARCHES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_FAST_BUCKET_ENTITIES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_GENERAL_BUCKET_ENTITIES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_FAST_BUCKET_FRAMES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_GENERAL_BUCKET_FRAMES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_FRAME_APPLY_COUNT: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_STATE_FLUSH_CALLS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_STATE_FLUSH_ENTITIES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_FRAME_QUEUE_DRAINS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static SPRITE_FRAME_QUEUE_LEN: AtomicU64 = AtomicU64::new(0);
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
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_FAST_PATH: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_SLOW_PATH: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_SEGMENT_CROSSES: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_ADVANCE_TIME_NS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_SAMPLE_TIME_NS: AtomicU64 = AtomicU64::new(0);
#[cfg(feature = "anim_stats")]
static TRANSFORM_CLIP_APPLY_TIME_NS: AtomicU64 = AtomicU64::new(0);

const CLIP_TIME_EPSILON: f32 = 1e-5;

#[cfg(feature = "anim_stats")]
pub fn sprite_animation_stats_snapshot() -> SpriteAnimationStats {
    SpriteAnimationStats {
        fast_loop_calls: SPRITE_FAST_LOOP_CALLS.load(Ordering::Relaxed),
        event_calls: SPRITE_EVENT_CALLS.load(Ordering::Relaxed),
        plain_calls: SPRITE_PLAIN_CALLS.load(Ordering::Relaxed),
        fast_loop_binary_searches: SPRITE_FAST_LOOP_BINARY_SEARCHES.load(Ordering::Relaxed),
        fast_bucket_entities: SPRITE_FAST_BUCKET_ENTITIES.load(Ordering::Relaxed),
        general_bucket_entities: SPRITE_GENERAL_BUCKET_ENTITIES.load(Ordering::Relaxed),
        fast_bucket_frames: SPRITE_FAST_BUCKET_FRAMES.load(Ordering::Relaxed),
        general_bucket_frames: SPRITE_GENERAL_BUCKET_FRAMES.load(Ordering::Relaxed),
        frame_apply_count: SPRITE_FRAME_APPLY_COUNT.load(Ordering::Relaxed),
        state_flush_calls: SPRITE_STATE_FLUSH_CALLS.load(Ordering::Relaxed),
        state_flush_entities: SPRITE_STATE_FLUSH_ENTITIES.load(Ordering::Relaxed),
        frame_apply_queue_drains: SPRITE_FRAME_QUEUE_DRAINS.load(Ordering::Relaxed),
        frame_apply_queue_len: SPRITE_FRAME_QUEUE_LEN.load(Ordering::Relaxed),
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SpriteAnimPerfSample {
    pub frame_index: u64,
    pub delta_kind: SpriteAnimDeltaKind,
    pub fast_animators: u32,
    pub slow_animators: u32,
    pub fast_bucket_frames: u32,
    pub general_bucket_frames: u32,
    pub var_dt_animators: u32,
    pub const_dt_animators: u32,
    pub ping_pong_animators: u32,
    pub events_heavy_animators: u32,
    pub mod_or_div_calls: u32,
    pub simd_lanes_8: u32,
    pub simd_lanes_4: u32,
    pub simd_tail_scalar: u32,
    pub simd_chunk_time_ns: u64,
    pub simd_scalar_time_ns: u64,
    pub events_emitted: u32,
    pub events_coalesced: u32,
    pub slow_ratio_streak: u32,
    pub tail_scalar_streak: u32,
    pub simd_supported: bool,
}

impl SpriteAnimPerfSample {
    fn record_fast_bucket_frame(&mut self) {
        self.fast_bucket_frames = self.fast_bucket_frames.saturating_add(1);
    }

    fn record_general_bucket_frame(&mut self) {
        self.general_bucket_frames = self.general_bucket_frames.saturating_add(1);
    }

    fn record_fast_animator(&mut self, step_kind: SpriteAnimStepKind) {
        self.fast_animators = self.fast_animators.saturating_add(1);
        self.record_step_kind(step_kind);
    }

    fn record_general_animator(&mut self, animation: &SpriteAnimation, step_kind: SpriteAnimStepKind) {
        self.slow_animators = self.slow_animators.saturating_add(1);
        self.record_step_kind(step_kind);
        if animation.mode == SpriteAnimationLoopMode::PingPong {
            self.ping_pong_animators = self.ping_pong_animators.saturating_add(1);
        }
        if animation.has_events {
            self.events_heavy_animators = self.events_heavy_animators.saturating_add(1);
        }
    }

    fn record_events(&mut self, emitted: u32, coalesced: u32) {
        self.events_emitted = self.events_emitted.saturating_add(emitted);
        self.events_coalesced = self.events_coalesced.saturating_add(coalesced);
    }

    fn record_mod_call(&mut self) {
        self.mod_or_div_calls = self.mod_or_div_calls.saturating_add(1);
    }

    #[cfg(feature = "sprite_anim_simd")]
    fn record_simd_mix(&mut self, lanes8: u32, lanes4: u32, tail: u32) {
        self.simd_lanes_8 = self.simd_lanes_8.saturating_add(lanes8);
        self.simd_lanes_4 = self.simd_lanes_4.saturating_add(lanes4);
        self.simd_tail_scalar = self.simd_tail_scalar.saturating_add(tail);
    }

    fn record_step_kind(&mut self, step_kind: SpriteAnimStepKind) {
        match step_kind {
            SpriteAnimStepKind::Variable => {
                self.var_dt_animators = self.var_dt_animators.saturating_add(1);
            }
            SpriteAnimStepKind::Fixed => {
                self.const_dt_animators = self.const_dt_animators.saturating_add(1);
            }
        }
    }

    pub fn total_animators(&self) -> u32 {
        self.fast_animators.saturating_add(self.slow_animators)
    }

    pub fn slow_ratio(&self) -> f32 {
        let total = self.total_animators();
        if total == 0 {
            0.0
        } else {
            self.slow_animators as f32 / total as f32
        }
    }

    pub fn tail_scalar_ratio(&self) -> f32 {
        if self.fast_animators == 0 {
            0.0
        } else {
            self.simd_tail_scalar as f32 / self.fast_animators as f32
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum SpriteAnimDeltaKind {
    #[default]
    None,
    Variable,
    Fixed {
        step: f32,
        steps: u32,
    },
}

impl From<AnimationDelta> for SpriteAnimDeltaKind {
    fn from(value: AnimationDelta) -> Self {
        match value {
            AnimationDelta::None => SpriteAnimDeltaKind::None,
            AnimationDelta::Single(_) => SpriteAnimDeltaKind::Variable,
            AnimationDelta::Fixed { step, steps } => SpriteAnimDeltaKind::Fixed { step, steps },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpriteAnimStepKind {
    Variable,
    Fixed,
}

impl SpriteAnimStepKind {
    const fn as_u8(self) -> u8 {
        match self {
            SpriteAnimStepKind::Variable => 0,
            SpriteAnimStepKind::Fixed => 1,
        }
    }

    const fn from_u8(value: u8) -> Self {
        if value == 1 {
            SpriteAnimStepKind::Fixed
        } else {
            SpriteAnimStepKind::Variable
        }
    }
}

#[derive(Resource)]
pub struct SpriteAnimPerfTelemetry {
    current: SpriteAnimPerfSample,
    history: VecDeque<SpriteAnimPerfSample>,
    capacity: usize,
    frame_counter: u64,
    simd_enabled: bool,
    slow_threshold_streak: u32,
    tail_threshold_streak: u32,
}

impl SpriteAnimPerfTelemetry {
    pub fn new(capacity: usize) -> Self {
        Self {
            current: SpriteAnimPerfSample::default(),
            history: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
            frame_counter: 0,
            simd_enabled: cfg!(feature = "sprite_anim_simd"),
            slow_threshold_streak: 0,
            tail_threshold_streak: 0,
        }
    }

    pub fn start_frame(&mut self, delta: AnimationDelta) -> SpriteAnimPerfFrame<'_> {
        self.current = SpriteAnimPerfSample::default();
        self.current.frame_index = self.frame_counter + 1;
        self.current.delta_kind = SpriteAnimDeltaKind::from(delta);
        self.current.simd_supported = self.simd_enabled;
        SpriteAnimPerfFrame { telemetry: self, finished: false }
    }

    pub fn latest(&self) -> Option<SpriteAnimPerfSample> {
        self.history.back().copied()
    }

    pub fn history(&self) -> impl Iterator<Item = SpriteAnimPerfSample> + '_ {
        self.history.iter().copied()
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
        self.current = SpriteAnimPerfSample::default();
        self.frame_counter = 0;
        self.slow_threshold_streak = 0;
        self.tail_threshold_streak = 0;
    }

    fn finish_frame(&mut self) {
        self.frame_counter += 1;

        let slow_ratio = self.current.slow_ratio();
        if slow_ratio > 0.01 {
            self.slow_threshold_streak = self.slow_threshold_streak.saturating_add(1);
        } else {
            self.slow_threshold_streak = 0;
        }
        self.current.slow_ratio_streak = self.slow_threshold_streak;

        if self.current.simd_supported && self.current.fast_animators > 0 {
            if self.current.tail_scalar_ratio() > 0.05 {
                self.tail_threshold_streak = self.tail_threshold_streak.saturating_add(1);
            } else {
                self.tail_threshold_streak = 0;
            }
        } else {
            self.tail_threshold_streak = 0;
        }
        self.current.tail_scalar_streak = self.tail_threshold_streak;

        if self.history.len() == self.capacity {
            self.history.pop_front();
        }
        self.history.push_back(self.current);
    }
}

pub struct SpriteAnimPerfFrame<'a> {
    telemetry: &'a mut SpriteAnimPerfTelemetry,
    finished: bool,
}

impl<'a> SpriteAnimPerfFrame<'a> {
    pub fn sample_mut(&mut self) -> &mut SpriteAnimPerfSample {
        &mut self.telemetry.current
    }
}

impl Drop for SpriteAnimPerfFrame<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.telemetry.finish_frame();
            self.finished = true;
        }
    }
}

thread_local! {
    static SPRITE_PERF_SAMPLE_PTR: Cell<usize> = const { Cell::new(0) };
    static SPRITE_PERF_STEP_KIND: Cell<u8> = const { Cell::new(SpriteAnimStepKind::Variable.as_u8()) };
}

fn perf_set_sample(sample: Option<*mut SpriteAnimPerfSample>) {
    SPRITE_PERF_SAMPLE_PTR.with(|cell| cell.set(sample.map_or(0, |ptr| ptr as usize)));
}

fn perf_set_step_kind(kind: SpriteAnimStepKind) {
    SPRITE_PERF_STEP_KIND.with(|cell| cell.set(kind.as_u8()));
}

fn perf_current_step_kind() -> SpriteAnimStepKind {
    SPRITE_PERF_STEP_KIND.with(|cell| SpriteAnimStepKind::from_u8(cell.get()))
}

fn perf_with_sample<F: FnOnce(&mut SpriteAnimPerfSample)>(f: F) {
    SPRITE_PERF_SAMPLE_PTR.with(|cell| {
        let ptr = cell.get();
        if ptr == 0 {
            return;
        }
        unsafe {
            f(&mut *(ptr as *mut SpriteAnimPerfSample));
        }
    });
}

fn perf_record_fast_bucket_frame() {
    perf_with_sample(|sample| sample.record_fast_bucket_frame());
}

fn perf_record_general_bucket_frame() {
    perf_with_sample(|sample| sample.record_general_bucket_frame());
}

fn perf_record_fast_animator() {
    let step_kind = perf_current_step_kind();
    perf_with_sample(|sample| sample.record_fast_animator(step_kind));
}

fn perf_record_general_animator(animation: &SpriteAnimation) {
    let step_kind = perf_current_step_kind();
    perf_with_sample(|sample| sample.record_general_animator(animation, step_kind));
}

fn perf_record_events(emitted: u32, coalesced: u32) {
    if emitted == 0 && coalesced == 0 {
        return;
    }
    perf_with_sample(|sample| sample.record_events(emitted, coalesced));
}

fn perf_record_mod_or_div() {
    perf_with_sample(|sample| sample.record_mod_call());
}

#[cfg(feature = "sprite_anim_simd")]
fn perf_record_simd_mix(lanes8: u32, lanes4: u32, tail: u32) {
    if lanes8 == 0 && lanes4 == 0 && tail == 0 {
        return;
    }
    perf_with_sample(|sample| sample.record_simd_mix(lanes8, lanes4, tail));
}

#[cfg(feature = "sprite_anim_simd")]
fn perf_record_simd_chunk_time(time_ns: u64) {
    if time_ns == 0 {
        return;
    }
    perf_with_sample(|sample| sample.simd_chunk_time_ns = sample.simd_chunk_time_ns.saturating_add(time_ns));
}

#[cfg(feature = "sprite_anim_simd")]
fn perf_record_simd_scalar_time(time_ns: u64) {
    if time_ns == 0 {
        return;
    }
    perf_with_sample(|sample| {
        sample.simd_scalar_time_ns = sample.simd_scalar_time_ns.saturating_add(time_ns)
    });
}

#[cfg(feature = "anim_stats")]
pub fn reset_sprite_animation_stats() {
    SPRITE_FAST_LOOP_CALLS.store(0, Ordering::Relaxed);
    SPRITE_EVENT_CALLS.store(0, Ordering::Relaxed);
    SPRITE_PLAIN_CALLS.store(0, Ordering::Relaxed);
    SPRITE_FAST_LOOP_BINARY_SEARCHES.store(0, Ordering::Relaxed);
    SPRITE_FAST_BUCKET_ENTITIES.store(0, Ordering::Relaxed);
    SPRITE_GENERAL_BUCKET_ENTITIES.store(0, Ordering::Relaxed);
    SPRITE_FAST_BUCKET_FRAMES.store(0, Ordering::Relaxed);
    SPRITE_GENERAL_BUCKET_FRAMES.store(0, Ordering::Relaxed);
    SPRITE_FRAME_APPLY_COUNT.store(0, Ordering::Relaxed);
    SPRITE_STATE_FLUSH_CALLS.store(0, Ordering::Relaxed);
    SPRITE_STATE_FLUSH_ENTITIES.store(0, Ordering::Relaxed);
    SPRITE_FRAME_QUEUE_DRAINS.store(0, Ordering::Relaxed);
    SPRITE_FRAME_QUEUE_LEN.store(0, Ordering::Relaxed);
}

#[cfg(feature = "anim_stats")]
pub fn transform_clip_stats_snapshot() -> TransformClipStats {
    TransformClipStats {
        advance_calls: TRANSFORM_CLIP_ADVANCE_CALLS.load(Ordering::Relaxed),
        zero_delta_calls: TRANSFORM_CLIP_ZERO_DELTA_CALLS.load(Ordering::Relaxed),
        skipped_clips: TRANSFORM_CLIP_SKIPPED.load(Ordering::Relaxed),
        looped_resume_clips: TRANSFORM_CLIP_LOOPED_RESUME.load(Ordering::Relaxed),
        zero_duration_clips: TRANSFORM_CLIP_ZERO_DURATION.load(Ordering::Relaxed),
        fast_path_clips: TRANSFORM_CLIP_FAST_PATH.load(Ordering::Relaxed),
        slow_path_clips: TRANSFORM_CLIP_SLOW_PATH.load(Ordering::Relaxed),
        segment_crosses: TRANSFORM_CLIP_SEGMENT_CROSSES.load(Ordering::Relaxed),
        advance_time_ns: TRANSFORM_CLIP_ADVANCE_TIME_NS.load(Ordering::Relaxed),
        sample_time_ns: TRANSFORM_CLIP_SAMPLE_TIME_NS.load(Ordering::Relaxed),
        apply_time_ns: TRANSFORM_CLIP_APPLY_TIME_NS.load(Ordering::Relaxed),
    }
}

#[cfg(feature = "anim_stats")]
pub fn reset_transform_clip_stats() {
    TRANSFORM_CLIP_ADVANCE_CALLS.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_ZERO_DELTA_CALLS.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_SKIPPED.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_LOOPED_RESUME.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_ZERO_DURATION.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_FAST_PATH.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_SLOW_PATH.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_SEGMENT_CROSSES.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_ADVANCE_TIME_NS.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_SAMPLE_TIME_NS.store(0, Ordering::Relaxed);
    TRANSFORM_CLIP_APPLY_TIME_NS.store(0, Ordering::Relaxed);
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
fn record_fast_loop_binary_search(count: u64) {
    SPRITE_FAST_LOOP_BINARY_SEARCHES.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_fast_loop_binary_search(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_fast_bucket_sample(count: u64) {
    SPRITE_FAST_BUCKET_ENTITIES.fetch_add(count, Ordering::Relaxed);
    SPRITE_FAST_BUCKET_FRAMES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_fast_bucket_sample(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_general_bucket_sample(count: u64) {
    SPRITE_GENERAL_BUCKET_ENTITIES.fetch_add(count, Ordering::Relaxed);
    SPRITE_GENERAL_BUCKET_FRAMES.fetch_add(1, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_general_bucket_sample(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_sprite_frame_applies(count: u64) {
    SPRITE_FRAME_APPLY_COUNT.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_sprite_frame_applies(_count: u64) {}

#[cfg(all(feature = "anim_stats", feature = "sprite_anim_soa"))]
fn record_sprite_state_flush(batch_len: usize) {
    SPRITE_STATE_FLUSH_CALLS.fetch_add(1, Ordering::Relaxed);
    SPRITE_STATE_FLUSH_ENTITIES.fetch_add(batch_len as u64, Ordering::Relaxed);
}

#[cfg(not(all(feature = "anim_stats", feature = "sprite_anim_soa")))]
#[allow(dead_code)]
fn record_sprite_state_flush(_batch_len: usize) {}

#[cfg(feature = "anim_stats")]
fn record_sprite_frame_queue_depth(len: usize) {
    SPRITE_FRAME_QUEUE_DRAINS.fetch_add(1, Ordering::Relaxed);
    SPRITE_FRAME_QUEUE_LEN.fetch_add(len as u64, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_sprite_frame_queue_depth(_len: usize) {}

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
pub(crate) fn record_transform_looped_resume(count: u64) {
    TRANSFORM_CLIP_LOOPED_RESUME.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
pub(crate) fn record_transform_looped_resume(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_zero_duration(count: u64) {
    TRANSFORM_CLIP_ZERO_DURATION.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_zero_duration(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_fast_path(count: u64) {
    TRANSFORM_CLIP_FAST_PATH.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_fast_path(_count: u64) {}

#[cfg(feature = "anim_stats")]
fn record_transform_slow_path(count: u64) {
    TRANSFORM_CLIP_SLOW_PATH.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_slow_path(_count: u64) {}

#[cfg(feature = "anim_stats")]
pub(crate) fn record_transform_segment_crosses(count: u64) {
    TRANSFORM_CLIP_SEGMENT_CROSSES.fetch_add(count, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
pub(crate) fn record_transform_segment_crosses(_count: u64) {}

#[cfg(feature = "anim_stats")]
pub(crate) fn record_transform_advance_time(duration: Duration) {
    let nanos = duration.as_nanos().min(u64::MAX as u128) as u64;
    TRANSFORM_CLIP_ADVANCE_TIME_NS.fetch_add(nanos, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
pub(crate) fn record_transform_advance_time(_duration: std::time::Duration) {}

#[cfg(feature = "anim_stats")]
fn record_transform_sample_time(duration: Duration) {
    let nanos = duration.as_nanos().min(u64::MAX as u128) as u64;
    TRANSFORM_CLIP_SAMPLE_TIME_NS.fetch_add(nanos, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_sample_time(_duration: std::time::Duration) {}

#[cfg(feature = "anim_stats")]
fn record_transform_apply_time(duration: Duration) {
    let nanos = duration.as_nanos().min(u64::MAX as u128) as u64;
    TRANSFORM_CLIP_APPLY_TIME_NS.fetch_add(nanos, Ordering::Relaxed);
}

#[cfg(not(feature = "anim_stats"))]
#[allow(dead_code)]
fn record_transform_apply_time(_duration: std::time::Duration) {}

#[allow(clippy::too_many_arguments)]
pub fn sys_drive_sprite_animations(
    mut profiler: ResMut<SystemProfiler>,
    animation_plan: Res<AnimationPlan>,
    animation_time: Res<AnimationTime>,
    mut events: ResMut<EventBus>,
    mut frame_updates: ResMut<SpriteFrameApplyQueue>,
    mut perf: ResMut<SpriteAnimPerfTelemetry>,
    #[cfg(feature = "sprite_anim_soa")] mut runtime: ResMut<SpriteAnimatorSoa>,
    #[cfg(feature = "sprite_anim_soa")] mut fast_sprite_states: Query<
        &mut SpriteFrameState,
        With<FastSpriteAnimator>,
    >,
    #[cfg(not(feature = "sprite_anim_soa"))] mut fast_animations: Query<
        (Entity, &mut SpriteAnimation, &mut SpriteFrameState, &mut Sprite),
        With<FastSpriteAnimator>,
    >,
    mut general_animations: Query<
        (Entity, &mut SpriteAnimation, &mut SpriteFrameState),
        Without<FastSpriteAnimator>,
    >,
) {
    let _span = profiler.scope("sys_drive_sprite_animations");
    debug_assert!(
        frame_updates.is_empty(),
        "SpriteFrameApplyQueue should be empty before driving animations (pending {} entries)",
        frame_updates.len()
    );
    #[cfg(feature = "sprite_anim_soa")]
    let mut state_updates: Vec<SpriteStateUpdate> = Vec::new();
    let plan = animation_plan.delta;
    let mut perf_frame = perf.start_frame(plan);
    if !plan.has_steps() {
        return;
    }
    let sample_ptr = {
        let sample_ref: &mut SpriteAnimPerfSample = perf_frame.sample_mut();
        sample_ref as *mut SpriteAnimPerfSample
    };
    perf_set_sample(Some(sample_ptr));
    let step_kind = match plan {
        AnimationDelta::Fixed { .. } => SpriteAnimStepKind::Fixed,
        _ => SpriteAnimStepKind::Variable,
    };
    perf_set_step_kind(step_kind);
    let has_group_scales = animation_time.has_group_scales();
    let animation_time_ref: &AnimationTime = &animation_time;
    match plan {
        AnimationDelta::None => {}
        AnimationDelta::Single(delta) => {
            if delta != 0.0 {
                #[cfg(feature = "sprite_anim_soa")]
                {
                    drive_fast_single_soa(
                        delta,
                        has_group_scales,
                        animation_time_ref,
                        runtime.as_mut(),
                        &mut state_updates,
                    );
                    flush_sprite_state_updates(
                        runtime.as_ref(),
                        &mut state_updates,
                        frame_updates.as_mut(),
                        &mut fast_sprite_states,
                    );
                }
                #[cfg(not(feature = "sprite_anim_soa"))]
                {
                    drive_fast_single(delta, has_group_scales, animation_time_ref, &mut fast_animations);
                }
                drive_general_single(
                    delta,
                    has_group_scales,
                    animation_time_ref,
                    &mut events,
                    frame_updates.as_mut(),
                    &mut general_animations,
                );
            }
        }
        AnimationDelta::Fixed { step, steps } => {
            if steps > 0 && step != 0.0 {
                #[cfg(feature = "sprite_anim_soa")]
                {
                    drive_fast_fixed_soa(
                        step,
                        steps,
                        has_group_scales,
                        animation_time_ref,
                        runtime.as_mut(),
                        &mut state_updates,
                    );
                    flush_sprite_state_updates(
                        runtime.as_ref(),
                        &mut state_updates,
                        frame_updates.as_mut(),
                        &mut fast_sprite_states,
                    );
                }
                #[cfg(not(feature = "sprite_anim_soa"))]
                {
                    drive_fast_fixed(step, steps, has_group_scales, animation_time_ref, &mut fast_animations);
                }
                drive_general_fixed(
                    step,
                    steps,
                    has_group_scales,
                    animation_time_ref,
                    &mut events,
                    frame_updates.as_mut(),
                    &mut general_animations,
                );
            }
        }
    }
    perf_set_sample(None);
}

pub fn sys_init_sprite_frame_state(
    mut commands: Commands,
    sprites: Query<(Entity, &Sprite), Without<SpriteFrameState>>,
) {
    for (entity, sprite) in sprites.iter() {
        let state = SpriteFrameState::from_sprite(sprite);
        commands.entity(entity).insert(state);
    }
}

#[allow(clippy::type_complexity)]
pub fn sys_flag_fast_sprite_animators(
    mut commands: Commands,
    #[cfg(feature = "sprite_anim_soa")] mut runtime: ResMut<SpriteAnimatorSoa>,
    animations: Query<
        (Entity, &SpriteAnimation, Option<&FastSpriteAnimator>),
        Or<(Added<SpriteAnimation>, Changed<SpriteAnimation>)>,
    >,
) {
    for (entity, animation, marker) in animations.iter() {
        if animation.fast_loop {
            #[cfg(feature = "sprite_anim_soa")]
            runtime.upsert(entity, animation);
            if marker.is_none() {
                commands.entity(entity).insert(FastSpriteAnimator);
            }
        } else if marker.is_some() {
            commands.entity(entity).remove::<FastSpriteAnimator>();
            #[cfg(feature = "sprite_anim_soa")]
            {
                runtime.remove(entity);
            }
        }
    }
}

#[cfg(feature = "sprite_anim_soa")]
pub fn sys_cleanup_sprite_animator_soa(
    mut runtime: ResMut<SpriteAnimatorSoa>,
    active: Query<Entity, (With<SpriteAnimation>, With<FastSpriteAnimator>)>,
) {
    runtime.retain_entities(|entity| active.get(entity).is_ok());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::skeletal::{load_skeleton_from_gltf, SkeletonAsset};
    use crate::assets::{AnimationClip, ClipInterpolation, ClipKeyframe, ClipSegment, ClipVec2Track};
    use crate::ecs::{Sprite, SpriteAnimationFrame, SpriteFrameHotData};
    use anyhow::Result;
    use bevy_ecs::prelude::World;
    use bevy_ecs::system::SystemState;
    use glam::{Mat4, Quat, Vec2, Vec3};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;

    struct SkeletalFixture {
        skeleton: Arc<SkeletonAsset>,
        clip: Arc<SkeletalClip>,
    }

    static DRIVE_FIXED_RECORDING: AtomicBool = AtomicBool::new(false);
    static DRIVE_FIXED_STEP_COUNT: AtomicU32 = AtomicU32::new(0);

    fn hot_frames_from(frames: &[SpriteAnimationFrame]) -> Arc<[SpriteFrameHotData]> {
        let data: Vec<SpriteFrameHotData> = frames
            .iter()
            .map(|frame| SpriteFrameHotData { region_id: frame.region_id, uv: frame.uv })
            .collect();
        Arc::from(data.into_boxed_slice())
    }

    pub(super) fn enable_drive_fixed_step_recording(enabled: bool) {
        DRIVE_FIXED_RECORDING.store(enabled, Ordering::SeqCst);
        if enabled {
            DRIVE_FIXED_STEP_COUNT.store(0, Ordering::SeqCst);
        }
    }

    pub(super) fn record_drive_fixed_step_iteration() {
        if DRIVE_FIXED_RECORDING.load(Ordering::Relaxed) {
            DRIVE_FIXED_STEP_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn drive_fixed_recorded_steps() -> u32 {
        DRIVE_FIXED_STEP_COUNT.load(Ordering::SeqCst)
    }

    struct DriveFixedRecorderGuard;

    impl DriveFixedRecorderGuard {
        fn enable() -> Self {
            enable_drive_fixed_step_recording(true);
            DriveFixedRecorderGuard
        }
    }

    impl Drop for DriveFixedRecorderGuard {
        fn drop(&mut self) {
            enable_drive_fixed_step_recording(false);
        }
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

    #[test]
    fn skeletal_driver_skips_paused_clean_instances() -> Result<()> {
        let fixture = SkeletalFixture::load()?;
        let mut world = World::new();
        world.insert_resource(SpriteAnimPerfTelemetry::new(240));
        let skeleton_key = Arc::clone(&fixture.skeleton.name);
        let mut instance = SkeletonInstance::new(skeleton_key, Arc::clone(&fixture.skeleton));
        instance.set_active_clip(None, Some(Arc::clone(&fixture.clip)));
        instance.set_playing(false);
        instance.clear_dirty();

        let sentinel = Mat4::from_scale(Vec3::splat(std::f32::consts::PI));
        let mut bones = BoneTransforms::new(instance.joint_count());
        for mat in bones.model.iter_mut() {
            *mat = sentinel;
        }
        for mat in bones.palette.iter_mut() {
            *mat = sentinel;
        }

        let entity = world.spawn((instance, bones)).id();

        let mut state: SystemState<Query<(Entity, &mut SkeletonInstance, Option<Mut<BoneTransforms>>)>> =
            SystemState::new(&mut world);
        let animation_time = AnimationTime::default();

        {
            let mut query = state.get_mut(&mut world);
            drive_skeletal_clips(0.1, false, &animation_time, &mut query);
        }
        state.apply(&mut world);

        {
            let bones_after = world.get::<BoneTransforms>(entity).unwrap();
            for mat in bones_after.model.iter().chain(bones_after.palette.iter()) {
                assert_eq!(
                    mat.to_cols_array(),
                    sentinel.to_cols_array(),
                    "paused, clean instance should not rewrite bones"
                );
            }
        }

        {
            let mut instance = world.get_mut::<SkeletonInstance>(entity).unwrap();
            instance.set_time(0.5);
            instance.set_playing(false);
        }
        {
            let mut query = state.get_mut(&mut world);
            drive_skeletal_clips(0.0, false, &animation_time, &mut query);
        }
        state.apply(&mut world);

        {
            let bones_after = world.get::<BoneTransforms>(entity).unwrap();
            assert!(
                bones_after.model.iter().any(|mat| mat.to_cols_array() != sentinel.to_cols_array()),
                "dirty paused instance should refresh bone transforms"
            );
        }
        {
            let instance = world.get::<SkeletonInstance>(entity).unwrap();
            assert!(!instance.dirty, "evaluated instance should clear dirty flag");
        }

        Ok(())
    }

    #[test]
    fn sprite_animation_emits_initial_frame_events() {
        use crate::events::GameEvent;
        use bevy_ecs::system::SystemState;

        let mut world = World::new();
        world.insert_resource(SpriteAnimPerfTelemetry::new(240));
        world.insert_resource(SystemProfiler::new());
        world.insert_resource(AnimationPlan { delta: AnimationDelta::Single(0.05) });
        world.insert_resource(AnimationTime::default());
        world.insert_resource(EventBus::default());
        world.insert_resource(SpriteFrameApplyQueue::default());
        #[cfg(feature = "sprite_anim_soa")]
        world.insert_resource(SpriteAnimatorSoa::default());

        let region = Arc::from("frame0");
        let event_name = Arc::from("spawn");
        let frame = SpriteAnimationFrame {
            name: Arc::clone(&region),
            region: Arc::clone(&region),
            region_id: 7,
            duration: 0.1,
            uv: [0.0; 4],
            events: Arc::from(vec![event_name]),
        };
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(vec![frame].into_boxed_slice());
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.1_f32].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );

        let sprite =
            Sprite { atlas_key: Arc::from("atlas"), region: Arc::clone(&region), region_id: 7, uv: [0.0; 4] };
        let frame_state = SpriteFrameState::from_sprite(&sprite);
        world.spawn((animation, sprite, frame_state));

        #[cfg(feature = "sprite_anim_soa")]
        let mut system_state = SystemState::<(
            ResMut<SystemProfiler>,
            Res<AnimationPlan>,
            Res<AnimationTime>,
            ResMut<EventBus>,
            ResMut<SpriteFrameApplyQueue>,
            ResMut<SpriteAnimPerfTelemetry>,
            ResMut<SpriteAnimatorSoa>,
            Query<&mut SpriteFrameState, With<FastSpriteAnimator>>,
            Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState), Without<FastSpriteAnimator>>,
        )>::new(&mut world);
        #[cfg(not(feature = "sprite_anim_soa"))]
        let mut system_state = SystemState::<(
            ResMut<SystemProfiler>,
            Res<AnimationPlan>,
            Res<AnimationTime>,
            ResMut<EventBus>,
            ResMut<SpriteFrameApplyQueue>,
            ResMut<SpriteAnimPerfTelemetry>,
            Query<
                (Entity, &mut SpriteAnimation, &mut SpriteFrameState, &mut Sprite),
                With<FastSpriteAnimator>,
            >,
            Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState), Without<FastSpriteAnimator>>,
        )>::new(&mut world);
        #[cfg(feature = "sprite_anim_soa")]
        {
            let (profiler, plan, time, events, frame_updates, perf, runtime, fast_states, general_animations) =
                system_state.get_mut(&mut world);
            sys_drive_sprite_animations(
                profiler,
                plan,
                time,
                events,
                frame_updates,
                perf,
                runtime,
                fast_states,
                general_animations,
            );
        }
        #[cfg(not(feature = "sprite_anim_soa"))]
        {
            let (profiler, plan, time, events, frame_updates, perf, fast_animations, general_animations) =
                system_state.get_mut(&mut world);
            sys_drive_sprite_animations(
                profiler,
                plan,
                time,
                events,
                frame_updates,
                perf,
                fast_animations,
                general_animations,
            );
        }
        system_state.apply(&mut world);

        let mut bus = world.resource_mut::<EventBus>();
        let events = bus.drain();
        assert_eq!(events.len(), 1);
        match &events[0] {
            GameEvent::SpriteAnimationEvent { event, .. } => assert_eq!(event.as_ref(), "spawn"),
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn fast_sprite_animators_receive_marker() {
        use bevy_ecs::system::SystemState;

        let mut world = World::new();
        #[cfg(feature = "sprite_anim_soa")]
        world.insert_resource(SpriteAnimatorSoa::default());
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![SpriteAnimationFrame {
                name: Arc::clone(&region),
                region: Arc::clone(&region),
                region_id: 1,
                duration: 0.1,
                uv: [0.0; 4],
                events: Arc::default(),
            }]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.1_f32].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );
        assert!(animation.fast_loop);

        let entity = world.spawn(animation).id();

        #[cfg(feature = "sprite_anim_soa")]
        {
            let mut system_state = SystemState::<(
                Commands,
                ResMut<SpriteAnimatorSoa>,
                Query<
                    (Entity, &SpriteAnimation, Option<&FastSpriteAnimator>),
                    Or<(Added<SpriteAnimation>, Changed<SpriteAnimation>)>,
                >,
            )>::new(&mut world);
            let (commands, runtime, animations) = system_state.get_mut(&mut world);
            sys_flag_fast_sprite_animators(commands, runtime, animations);
            system_state.apply(&mut world);
        }
        #[cfg(not(feature = "sprite_anim_soa"))]
        {
            let mut system_state = SystemState::<(
                Commands,
                Query<
                    (Entity, &SpriteAnimation, Option<&FastSpriteAnimator>),
                    Or<(Added<SpriteAnimation>, Changed<SpriteAnimation>)>,
                >,
            )>::new(&mut world);
            let (commands, animations) = system_state.get_mut(&mut world);
            sys_flag_fast_sprite_animators(commands, animations);
            system_state.apply(&mut world);
        }

        assert!(world.get::<FastSpriteAnimator>(entity).is_some());
    }

    #[cfg(feature = "sprite_anim_soa")]
    #[test]
    fn drive_fast_single_soa_advances_frames() {
        use bevy_ecs::system::SystemState;

        let mut world = World::new();
        let mut runtime = SpriteAnimatorSoa::default();
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            (0..2)
                .map(|idx| SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: idx,
                    duration: 0.05,
                    uv: [0.0; 4],
                    events: Arc::default(),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.05_f32, 0.05_f32].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.05_f32].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );

        let entity = world.spawn((FastSpriteAnimator, SpriteFrameState::new_uninitialized())).id();
        runtime.upsert(entity, &animation);

        let mut frame_updates = SpriteFrameApplyQueue::default();
        let animation_time = AnimationTime::default();
        let mut system_state =
            SystemState::<Query<&mut SpriteFrameState, With<FastSpriteAnimator>>>::new(&mut world);
        let mut state_updates = Vec::new();
        drive_fast_single_soa(0.05, false, &animation_time, &mut runtime, &mut state_updates);
        {
            let mut sprite_states = system_state.get_mut(&mut world);
            flush_sprite_state_updates(&runtime, &mut state_updates, &mut frame_updates, &mut sprite_states);
        }
        system_state.apply(&mut world);

        assert_eq!(runtime.frame_index[runtime.slot_index(entity).unwrap()], 1);
        assert_eq!(frame_updates.len(), 1);
    }

    #[cfg(feature = "sprite_anim_soa")]
    #[test]
    fn sprite_animator_soa_upsert_and_remove_handles_updates() {
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            (0..4)
                .map(|idx| SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: idx as u16,
                    duration: 0.1,
                    uv: [0.0; 4],
                    events: Arc::default(),
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.1_f32; 4].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.1_f32, 0.2_f32, 0.3_f32].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );
        let entity = Entity::from_raw(7);
        animation.frame_index = 3;
        animation.elapsed_in_frame = 0.05;
        animation.current_duration = 0.2;
        animation.pending_small_delta = 0.01;
        animation.playback_rate = 1.5;
        animation.playback_rate_dirty = false;
        animation.forward = false;
        animation.prev_forward = false;
        animation.pending_start_events = true;

        let mut runtime = SpriteAnimatorSoa::default();
        assert!(runtime.is_empty());
        runtime.upsert(entity, &animation);
        assert_eq!(runtime.len(), 1);
        assert!(runtime.contains(entity));

        animation.frame_index = 1;
        animation.elapsed_in_frame = 0.02;
        runtime.upsert(entity, &animation);
        assert_eq!(runtime.len(), 1, "upsert should update existing slot");

        assert!(runtime.remove(entity));
        assert!(runtime.is_empty());
    }

    #[test]
    fn queue_sprite_updates_deduplicate_entities() {
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![SpriteAnimationFrame {
                name: Arc::clone(&region),
                region: Arc::clone(&region),
                region_id: 5,
                duration: 0.1,
                uv: [0.0; 4],
                events: Arc::default(),
            }]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.1_f32].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );

        let entity = Entity::from_raw(42);
        let mut sprite_state = SpriteFrameState::new_uninitialized();
        let mut frame_updates = SpriteFrameApplyQueue::default();

        queue_sprite_frame_update(entity, &animation, &mut sprite_state, &mut frame_updates);
        assert_eq!(frame_updates.len(), 1);
        assert!(sprite_state.queued_for_apply);

        // Second enqueue should collapse into the existing pending update while still mutating the frame state.
        queue_sprite_frame_update(entity, &animation, &mut sprite_state, &mut frame_updates);
        assert_eq!(frame_updates.len(), 1);
    }

    #[test]
    fn sprite_frame_queue_flag_clears_after_apply() {
        let mut world = World::new();
        world.insert_resource(SystemProfiler::new());
        world.insert_resource(SpriteFrameApplyQueue::default());

        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 1,
                    duration: 0.05,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
                SpriteAnimationFrame {
                    name: Arc::from("frame1"),
                    region: Arc::from("frame1"),
                    region_id: 2,
                    duration: 0.05,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
            ]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.05_f32, 0.05_f32].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.05_f32].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );
        let sprite = Sprite {
            atlas_key: Arc::from("atlas"),
            region: Arc::clone(&region),
            region_id: Sprite::UNINITIALIZED_REGION,
            uv: [0.0; 4],
        };
        let frame_state = SpriteFrameState::new_uninitialized();

        let entity = world.spawn((animation, sprite, frame_state)).id();

        let animation_snapshot = world.get::<SpriteAnimation>(entity).unwrap().clone();
        let mut pending_queue = SpriteFrameApplyQueue::default();
        {
            let mut state_ref = world.get_mut::<SpriteFrameState>(entity).unwrap();
            queue_sprite_frame_update(entity, &animation_snapshot, &mut state_ref, &mut pending_queue);
        }
        *world.resource_mut::<SpriteFrameApplyQueue>() = pending_queue;

        let state_after_drive = world.get::<SpriteFrameState>(entity).unwrap();
        assert!(state_after_drive.queued_for_apply);
        assert!(!world.resource::<SpriteFrameApplyQueue>().is_empty());

        {
            let mut apply_state = bevy_ecs::system::SystemState::<(
                ResMut<SystemProfiler>,
                ResMut<SpriteFrameApplyQueue>,
                Query<(&mut Sprite, &mut SpriteFrameState)>,
            )>::new(&mut world);
            let (profiler, frame_updates, sprites) = apply_state.get_mut(&mut world);
            sys_apply_sprite_frame_states(profiler, frame_updates, sprites);
        }

        let state_after_apply = world.get::<SpriteFrameState>(entity).unwrap();
        assert!(!state_after_apply.queued_for_apply);
        assert!(world.resource::<SpriteFrameApplyQueue>().is_empty());
    }

    #[test]
    fn evented_animators_drop_stale_fast_markers() {
        use bevy_ecs::system::SystemState;

        let mut world = World::new();
        #[cfg(feature = "sprite_anim_soa")]
        world.insert_resource(SpriteAnimatorSoa::default());
        let region = Arc::from("frame");
        let event = Arc::from("spawn");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![SpriteAnimationFrame {
                name: Arc::clone(&region),
                region: Arc::clone(&region),
                region_id: 1,
                duration: 0.1,
                uv: [0.0; 4],
                events: Arc::from(vec![Arc::clone(&event)].into_boxed_slice()),
            }]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.1_f32].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );
        assert!(!animation.fast_loop);

        let entity = world.spawn((animation, FastSpriteAnimator)).id();

        #[cfg(feature = "sprite_anim_soa")]
        {
            let mut system_state = SystemState::<(
                Commands,
                ResMut<SpriteAnimatorSoa>,
                Query<
                    (Entity, &SpriteAnimation, Option<&FastSpriteAnimator>),
                    Or<(Added<SpriteAnimation>, Changed<SpriteAnimation>)>,
                >,
            )>::new(&mut world);
            let (commands, runtime, animations) = system_state.get_mut(&mut world);
            sys_flag_fast_sprite_animators(commands, runtime, animations);
            system_state.apply(&mut world);
        }
        #[cfg(not(feature = "sprite_anim_soa"))]
        {
            let mut system_state = SystemState::<(
                Commands,
                Query<
                    (Entity, &SpriteAnimation, Option<&FastSpriteAnimator>),
                    Or<(Added<SpriteAnimation>, Changed<SpriteAnimation>)>,
                >,
            )>::new(&mut world);
            let (commands, animations) = system_state.get_mut(&mut world);
            sys_flag_fast_sprite_animators(commands, animations);
            system_state.apply(&mut world);
        }

        assert!(world.get::<FastSpriteAnimator>(entity).is_none());
    }

    #[test]
    fn sprite_animation_rewinds_with_negative_delta() {
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 0,
                    duration: 0.2,
                    uv: [0.0; 4],
                    events: Arc::default(),
                };
                3
            ]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.2_f32, 0.2, 0.2].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.2, 0.4].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.6,
            SpriteAnimationLoopMode::Loop,
        );
        animation.frame_index = 2;
        animation.elapsed_in_frame = 0.0;

        let changed = advance_animation(&mut animation, -0.25, Entity::from_raw(42), None, true);
        assert!(changed);
        assert_eq!(animation.frame_index, 1);
    }

    #[test]
    fn sprite_animation_ping_pong_rewind_restores_direction() {
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 0,
                    duration: 0.2,
                    uv: [0.0; 4],
                    events: Arc::default(),
                };
                3
            ]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.2_f32, 0.2, 0.2].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.2, 0.4].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.6,
            SpriteAnimationLoopMode::PingPong,
        );
        animation.frame_index = animation.frames.len().saturating_sub(1);
        animation.forward = false;
        animation.prev_forward = true;
        animation.elapsed_in_frame = 0.0;
        animation.refresh_current_duration();

        let changed = advance_animation(&mut animation, -0.3, Entity::from_raw(7), None, true);
        assert!(changed);
        assert!(animation.forward, "rewinding across the end should restore forward direction");
    }

    #[test]
    fn fast_loop_advances_multiple_frames() {
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 0,
                    duration: 0.08,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 1,
                    duration: 0.08,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 2,
                    duration: 0.08,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
            ]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.08_f32, 0.08, 0.08].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.08, 0.16].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.24,
            SpriteAnimationLoopMode::Loop,
        );
        animation.frame_index = 1;
        animation.elapsed_in_frame = 0.06;
        animation.refresh_current_duration();

        let changed = advance_animation_loop_no_events(&mut animation, 0.05);
        assert!(changed, "fast loop should report a change when advancing past the current frame");
        assert_eq!(animation.frame_index, 2);
        assert!(
            (animation.elapsed_in_frame - 0.03).abs() < 1e-4,
            "expected ~0.03s elapsed in frame, got {}",
            animation.elapsed_in_frame
        );
    }

    #[test]
    fn fast_loop_large_delta_wraps_phase() {
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 0,
                    duration: 0.08,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 1,
                    duration: 0.12,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
            ]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.08_f32, 0.12].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.08].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.20,
            SpriteAnimationLoopMode::Loop,
        );
        animation.frame_index = 0;
        animation.elapsed_in_frame = 0.02;
        animation.refresh_current_duration();

        let delta = animation.total_duration * 3.0 + 0.05;
        let changed = advance_animation_loop_no_events(&mut animation, delta);
        assert!(changed, "wrapping multiple cycles should still flag a change");
        assert_eq!(
            animation.frame_index, 0,
            "wrapping an integer number of cycles plus delta should land back in frame 0"
        );
        assert!(
            (animation.elapsed_in_frame - 0.07).abs() < 1e-4,
            "expected ~0.07s elapsed after wrapping, got {}",
            animation.elapsed_in_frame
        );
    }

    #[test]
    fn fast_loop_rewinds_frames() {
        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 0,
                    duration: 0.08,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 1,
                    duration: 0.08,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 2,
                    duration: 0.08,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
            ]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.08_f32, 0.08, 0.08].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.08, 0.16].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.24,
            SpriteAnimationLoopMode::Loop,
        );
        animation.frame_index = 1;
        animation.elapsed_in_frame = 0.02;
        animation.refresh_current_duration();

        let delta = -0.05;
        let changed = advance_animation_loop_no_events(&mut animation, delta);
        assert!(changed, "rewinding past the current frame should report a change");
        assert_eq!(animation.frame_index, 0);
        assert!(
            (animation.elapsed_in_frame - 0.05).abs() < 1e-4,
            "expected ~0.05s elapsed after rewind, got {}",
            animation.elapsed_in_frame
        );
    }

    #[test]
    fn drive_fixed_processes_each_step() {
        use crate::events::EventBus;

        let mut world = World::new();
        world.insert_resource(SpriteAnimPerfTelemetry::new(240));
        world.insert_resource(SystemProfiler::new());
        world.insert_resource(AnimationPlan { delta: AnimationDelta::Fixed { step: 0.1, steps: 3 } });
        world.insert_resource(AnimationTime::default());
        world.insert_resource(EventBus::default());
        world.insert_resource(SpriteFrameApplyQueue::default());

        let region = Arc::from("frame");
        let frames: Arc<[SpriteAnimationFrame]> = Arc::from(
            vec![
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 0,
                    duration: 0.1,
                    uv: [0.0; 4],
                    events: Arc::from(vec![Arc::from("tick")].into_boxed_slice()),
                },
                SpriteAnimationFrame {
                    name: Arc::clone(&region),
                    region: Arc::clone(&region),
                    region_id: 1,
                    duration: 0.1,
                    uv: [0.0; 4],
                    events: Arc::default(),
                },
            ]
            .into_boxed_slice(),
        );
        let hot_frames = hot_frames_from(frames.as_ref());
        let durations = Arc::from(vec![0.1_f32, 0.1].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.1].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            hot_frames,
            durations,
            offsets,
            0.2,
            SpriteAnimationLoopMode::Loop,
        );

        let sprite =
            Sprite { atlas_key: Arc::from("atlas"), region: Arc::clone(&region), region_id: 0, uv: [0.0; 4] };
        let frame_state = SpriteFrameState::from_sprite(&sprite);
        world.spawn((animation, sprite, frame_state));

        #[cfg(feature = "sprite_anim_soa")]
        world.insert_resource(SpriteAnimatorSoa::default());
        #[cfg(feature = "sprite_anim_soa")]
        let mut system_state = SystemState::<(
            ResMut<SystemProfiler>,
            Res<AnimationPlan>,
            Res<AnimationTime>,
            ResMut<EventBus>,
            ResMut<SpriteFrameApplyQueue>,
            ResMut<SpriteAnimPerfTelemetry>,
            ResMut<SpriteAnimatorSoa>,
            Query<&mut SpriteFrameState, With<FastSpriteAnimator>>,
            Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState), Without<FastSpriteAnimator>>,
        )>::new(&mut world);
        #[cfg(not(feature = "sprite_anim_soa"))]
        let mut system_state = SystemState::<(
            ResMut<SystemProfiler>,
            Res<AnimationPlan>,
            Res<AnimationTime>,
            ResMut<EventBus>,
            ResMut<SpriteFrameApplyQueue>,
            ResMut<SpriteAnimPerfTelemetry>,
            Query<
                (Entity, &mut SpriteAnimation, &mut SpriteFrameState, &mut Sprite),
                With<FastSpriteAnimator>,
            >,
            Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState), Without<FastSpriteAnimator>>,
        )>::new(&mut world);

        let _guard = DriveFixedRecorderGuard::enable();
        #[cfg(feature = "sprite_anim_soa")]
        {
            let (profiler, plan, time, events, frame_updates, perf, runtime, fast_states, general_animations) =
                system_state.get_mut(&mut world);
            sys_drive_sprite_animations(
                profiler,
                plan,
                time,
                events,
                frame_updates,
                perf,
                runtime,
                fast_states,
                general_animations,
            );
        }
        #[cfg(not(feature = "sprite_anim_soa"))]
        {
            let (profiler, plan, time, events, frame_updates, perf, fast_animations, general_animations) =
                system_state.get_mut(&mut world);
            sys_drive_sprite_animations(
                profiler,
                plan,
                time,
                events,
                frame_updates,
                perf,
                fast_animations,
                general_animations,
            );
        }
        system_state.apply(&mut world);

        assert_eq!(drive_fixed_recorded_steps(), 3);
    }

    #[test]
    fn clip_sample_waits_for_missing_components() {
        use crate::ecs::types::ClipInstance;

        let clip = Arc::new(AnimationClip {
            name: Arc::from("clip"),
            duration: 1.0,
            duration_inv: 1.0,
            translation: None,
            rotation: None,
            scale: None,
            tint: None,
            looped: true,
            version: 1,
        });
        let mut instance = ClipInstance::new(Arc::from("clip"), clip);
        instance.last_translation = Some(Vec2::ZERO);

        let mut sample = ClipSample::default();
        sample.translation = Some(Vec2::new(1.0, 2.0));

        apply_clip_sample(&mut instance, None, None, None, None, sample);
        assert_eq!(instance.last_translation, Some(Vec2::ZERO));

        let mut world = World::new();
        let entity = world.spawn(Transform::default()).id();
        {
            let mut entity_ref = world.entity_mut(entity);
            let transform = entity_ref.get_mut::<Transform>().unwrap();
            apply_clip_sample(&mut instance, None, None, Some(transform), None, sample);
        }

        let stored = world.entity(entity).get::<Transform>().unwrap();
        assert_eq!(stored.translation, Vec2::new(1.0, 2.0));
    }

    #[test]
    fn zero_duration_transform_clip_applies_sample() {
        use crate::ecs::types::ClipInstance;

        let clip_translation = Vec2::new(3.0, 4.0);
        let clip = zero_duration_translation_clip(clip_translation);
        let mut world = World::new();
        world.insert_resource(SystemProfiler::new());
        world.insert_resource(AnimationPlan { delta: AnimationDelta::Single(0.1) });
        world.insert_resource(AnimationTime::default());

        let entity = world
            .spawn((
                ClipInstance::new(Arc::from("clip"), clip),
                TransformTrackPlayer::default(),
                Transform::default(),
            ))
            .id();

        let mut system_state = SystemState::<(
            ResMut<SystemProfiler>,
            Res<AnimationPlan>,
            Res<AnimationTime>,
            Query<(
                Entity,
                &mut ClipInstance,
                Option<&TransformTrackPlayer>,
                Option<&PropertyTrackPlayer>,
                Option<Mut<Transform>>,
                Option<Mut<Tint>>,
            )>,
        )>::new(&mut world);

        {
            let (profiler, plan, time, clips) = system_state.get_mut(&mut world);
            sys_drive_transform_clips(profiler, plan, time, clips);
        }
        system_state.apply(&mut world);

        let stored = world.get::<Transform>(entity).expect("transform missing");
        assert_eq!(stored.translation, clip_translation);
    }

    fn zero_duration_translation_clip(value: Vec2) -> Arc<AnimationClip> {
        let keyframes = Arc::from(vec![ClipKeyframe { time: 0.0, value }].into_boxed_slice());
        let empty_vec2 = Arc::from(Vec::<Vec2>::new().into_boxed_slice());
        let empty_segments = Arc::from(Vec::<ClipSegment<Vec2>>::new().into_boxed_slice());
        let empty_offsets = Arc::from(Vec::<f32>::new().into_boxed_slice());
        Arc::new(AnimationClip {
            name: Arc::from("zero_duration"),
            duration: 0.0,
            duration_inv: 0.0,
            translation: Some(ClipVec2Track {
                interpolation: ClipInterpolation::Linear,
                keyframes,
                duration: 0.0,
                duration_inv: 0.0,
                segment_deltas: empty_vec2,
                segments: empty_segments,
                segment_offsets: empty_offsets,
            }),
            rotation: None,
            scale: None,
            tint: None,
            looped: false,
            version: 1,
        })
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

#[allow(clippy::type_complexity)]
pub fn sys_drive_transform_clips(
    mut profiler: ResMut<SystemProfiler>,
    animation_plan: Res<AnimationPlan>,
    animation_time: Res<AnimationTime>,
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
    let plan = animation_plan.delta;
    if !plan.has_steps() {
        return;
    }
    let has_group_scales = animation_time.has_group_scales();
    let animation_time_ref: &AnimationTime = &animation_time;
    let delta = match plan {
        AnimationDelta::None => return,
        AnimationDelta::Single(amount) => amount,
        AnimationDelta::Fixed { step, steps } => step * steps as f32,
    };
    if delta == 0.0 {
        return;
    }
    drive_transform_clips(delta, has_group_scales, animation_time_ref, &mut clips);
}

pub fn sys_drive_skeletal_clips(
    mut profiler: ResMut<SystemProfiler>,
    animation_plan: Res<AnimationPlan>,
    animation_time: Res<AnimationTime>,
    mut skeletons: Query<(Entity, &mut SkeletonInstance, Option<Mut<BoneTransforms>>)>,
) {
    let _span = profiler.scope("sys_drive_skeletal_clips");
    let plan = animation_plan.delta;
    if !plan.has_steps() {
        return;
    }
    let has_group_scales = animation_time.has_group_scales();
    let animation_time_ref: &AnimationTime = &animation_time;
    let delta = match plan {
        AnimationDelta::None => return,
        AnimationDelta::Single(amount) => amount,
        AnimationDelta::Fixed { step, steps } => step * steps as f32,
    };
    if delta == 0.0 {
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

        if !instance.playing && !instance.dirty {
            continue;
        }

        let group_scale =
            if has_group_scales { animation_time.group_scale(instance.group.as_deref()) } else { 1.0 };
        let playback_rate = if instance.playback_rate_dirty {
            instance.ensure_playback_rate(group_scale)
        } else {
            instance.playback_rate
        };
        if playback_rate == 0.0 && !instance.dirty {
            continue;
        }

        let scaled = delta * playback_rate;
        if scaled == 0.0 && !instance.dirty {
            continue;
        }

        if instance.playing && scaled != 0.0 {
            let current_time = instance.time;
            instance.set_time(current_time + scaled);
        }

        let pose_time = instance.time;
        evaluate_skeleton_pose(&mut instance, &clip, pose_time);

        if let Some(mut bones) = bone_transforms {
            bones.ensure_joint_count(instance.joint_count());
            bones.model.copy_from_slice(&instance.model_poses);
            bones.palette.copy_from_slice(&instance.palette);
        }

        instance.clear_dirty();
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
            let span = (end.time - start.time).max(f32::EPSILON);
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
        let wrapped = time.rem_euclid(duration.max(f32::EPSILON));
        if wrapped <= CLIP_TIME_EPSILON && time > 0.0 && (time - duration).abs() <= CLIP_TIME_EPSILON {
            duration
        } else {
            wrapped
        }
    } else {
        time.clamp(0.0, duration)
    }
}

#[allow(clippy::type_complexity)]
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
    #[cfg(feature = "anim_stats")]
    let mut stats = TransformClipStatAccumulator::default();
    for (_entity, mut instance, transform_player, property_player, mut transform, mut tint) in
        clips.iter_mut()
    {
        if !instance.playing {
            #[cfg(feature = "anim_stats")]
            {
                stats.skipped_clips += 1;
            }
            continue;
        }

        let group_scale =
            if has_group_scales { animation_time.group_scale(instance.group.as_deref()) } else { 1.0 };
        let playback_rate = if instance.playback_rate_dirty {
            instance.ensure_playback_rate(group_scale)
        } else {
            instance.playback_rate
        };
        if playback_rate == 0.0 {
            continue;
        }

        let scaled = delta * playback_rate;
        if scaled == 0.0 {
            continue;
        }

        let zero_duration_clip = instance.duration() <= 0.0;
        if zero_duration_clip {
            #[cfg(feature = "anim_stats")]
            {
                stats.zero_duration_clips += 1;
            }
            instance.time = 0.0;
        }

        let transform_mask = transform_player.copied().unwrap_or_default();
        let property_mask = property_player.copied().unwrap_or_default();
        let clip_has_tint = instance.has_tint_channel();
        let wants_tint = property_player.is_some() && property_mask.apply_tint && clip_has_tint;
        let transform_available = transform.is_some();
        let tint_available = !wants_tint || tint.is_some();
        let mut sampling_transform_player = transform_mask;
        if !transform_available {
            sampling_transform_player.apply_translation = false;
            sampling_transform_player.apply_rotation = false;
            sampling_transform_player.apply_scale = false;
        }
        let mut sampling_property_player = property_mask;
        if !wants_tint || tint.is_none() {
            sampling_property_player.apply_tint = false;
        }
        let disabled_transform_player =
            TransformTrackPlayer { apply_translation: false, apply_rotation: false, apply_scale: false };
        let disabled_property_player = PropertyTrackPlayer::new(false);
        let transform_player_for_apply =
            if transform_available { transform_player } else { Some(&disabled_transform_player) };
        let property_player_for_apply =
            if wants_tint && tint.is_none() { Some(&disabled_property_player) } else { property_player };
        let has_tint_target = wants_tint && tint.is_some();
        if !transform_available && !has_tint_target {
            if instance.playing {
                let current_time = instance.time;
                instance.set_time(current_time + scaled);
            }
            instance.last_translation = None;
            instance.last_rotation = None;
            instance.last_scale = None;
            if wants_tint {
                instance.last_tint = None;
            }
            continue;
        }
        if zero_duration_clip {
            #[cfg(feature = "anim_stats")]
            let sample_timer = Instant::now();
            let sample =
                instance.sample_with_masks(Some(sampling_transform_player), Some(sampling_property_player));
            #[cfg(feature = "anim_stats")]
            {
                stats.sample_time += sample_timer.elapsed();
            }
            #[cfg(feature = "anim_stats")]
            let apply_timer = Instant::now();
            apply_clip_sample(
                &mut instance,
                transform_player_for_apply,
                property_player_for_apply,
                transform,
                tint,
                sample,
            );
            #[cfg(feature = "anim_stats")]
            {
                stats.apply_time += apply_timer.elapsed();
            }
            continue;
        }
        let can_fast_path = transform_available
            && transform_mask.apply_translation
            && transform_mask.apply_rotation
            && transform_mask.apply_scale
            && tint_available;

        if scaled < 0.0 {
            #[cfg(feature = "anim_stats")]
            {
                stats.slow_path_clips += 1;
            }
            let current_time = instance.time;
            instance.set_time(current_time + scaled);
            #[cfg(feature = "anim_stats")]
            let sample_timer = Instant::now();
            let sample =
                instance.sample_with_masks(Some(sampling_transform_player), Some(sampling_property_player));
            #[cfg(feature = "anim_stats")]
            {
                stats.sample_time += sample_timer.elapsed();
            }
            #[cfg(feature = "anim_stats")]
            let apply_timer = Instant::now();
            apply_clip_sample(
                &mut instance,
                transform_player_for_apply,
                property_player_for_apply,
                transform,
                tint,
                sample,
            );
            #[cfg(feature = "anim_stats")]
            {
                stats.apply_time += apply_timer.elapsed();
            }
            continue;
        }

        if can_fast_path {
            #[cfg(feature = "anim_stats")]
            {
                stats.fast_path_clips += 1;
            }

            let applied = instance.advance_time_masked(scaled, transform_player, property_player);
            #[cfg(feature = "anim_stats")]
            {
                stats.advance_calls += 1;
                if applied <= 0.0 {
                    stats.zero_delta_calls += 1;
                }
            }
            if applied <= 0.0 {
                continue;
            }

            #[cfg(feature = "anim_stats")]
            let sample_timer = Instant::now();
            let sample = instance.sample_cached();
            #[cfg(feature = "anim_stats")]
            {
                stats.sample_time += sample_timer.elapsed();
            }

            #[cfg(feature = "anim_stats")]
            let apply_timer = Instant::now();
            let translation_changed =
                transform_mask.apply_translation && sample.translation != instance.last_translation;
            let rotation_changed = transform_mask.apply_rotation && sample.rotation != instance.last_rotation;
            let scale_changed = transform_mask.apply_scale && sample.scale != instance.last_scale;
            if (translation_changed || rotation_changed || scale_changed) && transform_available {
                if let Some(transform_component) = transform.as_mut() {
                    let transform_component = &mut **transform_component;
                    if translation_changed {
                        if let Some(value) = sample.translation {
                            transform_component.translation = value;
                        }
                    }
                    if rotation_changed {
                        if let Some(value) = sample.rotation {
                            transform_component.rotation = value;
                        }
                    }
                    if scale_changed {
                        if let Some(value) = sample.scale {
                            transform_component.scale = value;
                        }
                    }
                }
            }

            if wants_tint {
                let tint_changed = sample.tint != instance.last_tint;
                if tint_changed {
                    if let (Some(tint_component), Some(value)) = (tint.as_mut(), sample.tint) {
                        let tint_component = &mut **tint_component;
                        tint_component.0 = value;
                    }
                }
            }

            if transform_mask.apply_translation {
                instance.last_translation = if transform_available { sample.translation } else { None };
            } else {
                instance.last_translation = None;
            }
            if transform_mask.apply_rotation {
                instance.last_rotation = if transform_available { sample.rotation } else { None };
            } else {
                instance.last_rotation = None;
            }
            if transform_mask.apply_scale {
                instance.last_scale = if transform_available { sample.scale } else { None };
            } else {
                instance.last_scale = None;
            }
            if wants_tint {
                instance.last_tint = if tint.is_some() { sample.tint } else { None };
            } else {
                instance.last_tint = None;
            }
            #[cfg(feature = "anim_stats")]
            {
                stats.apply_time += apply_timer.elapsed();
            }
            continue;
        }

        let applied = instance.advance_time_masked(scaled, transform_player, property_player);
        #[cfg(feature = "anim_stats")]
        {
            stats.advance_calls += 1;
            if applied <= 0.0 {
                stats.zero_delta_calls += 1;
            }
        }
        if applied <= 0.0 {
            #[cfg(feature = "anim_stats")]
            {
                stats.slow_path_clips += 1;
            }
            continue;
        }
        #[cfg(feature = "anim_stats")]
        let sample_timer = Instant::now();
        let sample =
            instance.sample_with_masks(Some(sampling_transform_player), Some(sampling_property_player));
        #[cfg(feature = "anim_stats")]
        {
            stats.sample_time += sample_timer.elapsed();
        }
        #[cfg(feature = "anim_stats")]
        let apply_timer = Instant::now();
        apply_clip_sample(
            &mut instance,
            transform_player_for_apply,
            property_player_for_apply,
            transform,
            tint,
            sample,
        );
        #[cfg(feature = "anim_stats")]
        {
            stats.apply_time += apply_timer.elapsed();
            stats.slow_path_clips += 1;
        }
    }
    #[cfg(feature = "anim_stats")]
    stats.flush();
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
    let had_transform_component = transform.is_some();
    let had_tint_component = tint.is_some();

    let translation_changed = sample.translation != instance.last_translation;
    let rotation_changed = sample.rotation != instance.last_rotation;
    let scale_changed = sample.scale != instance.last_scale;
    let tint_changed = sample.tint != instance.last_tint;

    let transform_mask = transform_player.copied().unwrap_or_default();
    let needs_translation = transform_mask.apply_translation && translation_changed;
    let needs_rotation = transform_mask.apply_rotation && rotation_changed;
    let needs_scale = transform_mask.apply_scale && scale_changed;
    if (needs_translation || needs_rotation || needs_scale) && transform.is_some() {
        if let Some(mut transform) = transform {
            if needs_translation {
                if let Some(value) = sample.translation {
                    transform.translation = value;
                }
            }
            if needs_rotation {
                if let Some(value) = sample.rotation {
                    transform.rotation = value;
                }
            }
            if needs_scale {
                if let Some(value) = sample.scale {
                    transform.scale = value;
                }
            }
        }
    }

    let tint_mask = property_player.copied().unwrap_or_default();
    if tint_mask.apply_tint && tint_changed {
        if let Some(mut tint_component) = tint {
            if let Some(value) = sample.tint {
                tint_component.0 = value;
            }
        }
    }

    if transform_mask.apply_translation {
        if had_transform_component {
            instance.last_translation = sample.translation;
        }
    } else {
        instance.last_translation = None;
    }
    if transform_mask.apply_rotation {
        if had_transform_component {
            instance.last_rotation = sample.rotation;
        }
    } else {
        instance.last_rotation = None;
    }
    if transform_mask.apply_scale {
        if had_transform_component {
            instance.last_scale = sample.scale;
        }
    } else {
        instance.last_scale = None;
    }
    if tint_mask.apply_tint {
        if had_tint_component {
            instance.last_tint = sample.tint;
        }
    } else {
        instance.last_tint = None;
    }
}

pub(crate) fn initialize_animation_phase(animation: &mut SpriteAnimation, entity: Entity) -> bool {
    if animation.frames.is_empty() {
        return false;
    }
    animation.frame_index = 0;
    animation.elapsed_in_frame = 0.0;
    animation.forward = true;
    animation.prev_forward = true;
    animation.refresh_current_duration();
    animation.refresh_pending_start_events();

    let mut offset = animation.start_offset.max(0.0);
    let total = animation.total_duration();
    if animation.random_start && total > 0.0 {
        let random_fraction = stable_random_fraction(entity, animation.timeline.as_ref());
        offset = (offset + random_fraction * total).rem_euclid(total.max(f32::EPSILON));
    }

    if !animation.mode.looped() && total > 0.0 {
        offset = offset.min(total);
    }

    if offset <= 0.0 {
        return true;
    }

    let was_playing = animation.playing;
    animation.playing = true;
    let changed = advance_animation(animation, offset, entity, None, false);
    animation.playing = was_playing;
    animation.refresh_pending_start_events();
    changed
}

pub(crate) fn advance_animation(
    animation: &mut SpriteAnimation,
    mut delta: f32,
    entity: Entity,
    mut events: Option<&mut EventBus>,
    respect_terminal_behavior: bool,
) -> bool {
    if delta == 0.0 {
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
    while animation.playing && delta.abs() > 0.0 {
        if delta > 0.0 {
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
            let prior_forward = animation.forward;
            let mut changed_this_step = false;

            match animation.mode {
                SpriteAnimationLoopMode::Loop => {
                    let next = if len > 0 { (animation.frame_index + 1) % len } else { 0 };
                    animation.set_frame_metrics_unchecked(next);
                    emit_frame_event = true;
                    changed_this_step = true;
                }
                SpriteAnimationLoopMode::OnceStop => {
                    animation.set_frame_metrics_unchecked(len.saturating_sub(1));
                    frame_changed = true;
                    animation.prev_forward = prior_forward;
                    if let Some(events) = events.as_deref_mut() {
                        emit_sprite_animation_events(entity, animation, events);
                    }
                    if respect_terminal_behavior {
                        animation.playing = false;
                    }
                    break;
                }
                SpriteAnimationLoopMode::OnceHold => {
                    animation.set_frame_metrics_unchecked(len.saturating_sub(1));
                    animation.elapsed_in_frame = animation.current_duration;
                    frame_changed = true;
                    animation.prev_forward = prior_forward;
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
                        animation.set_frame_metrics_unchecked(0);
                    } else if animation.forward {
                        let next = if animation.frame_index + 1 < len {
                            animation.frame_index + 1
                        } else {
                            animation.forward = false;
                            (len - 2).min(len - 1)
                        };
                        animation.set_frame_metrics_unchecked(next);
                        changed_this_step = true;
                        emit_frame_event = true;
                    } else if animation.frame_index > 0 {
                        let prev = animation.frame_index - 1;
                        animation.set_frame_metrics_unchecked(prev);
                        changed_this_step = true;
                        emit_frame_event = true;
                    } else {
                        animation.forward = true;
                        let next = if len > 1 { 1 } else { 0 };
                        animation.set_frame_metrics_unchecked(next);
                        changed_this_step = len > 1;
                        emit_frame_event = len > 1;
                    }
                }
            }

            if changed_this_step {
                frame_changed = true;
                animation.prev_forward = prior_forward;
            }

            if emit_frame_event {
                if let Some(events) = events.as_deref_mut() {
                    emit_sprite_animation_events(entity, animation, events);
                }
            }
        } else {
            let time_spent = animation.elapsed_in_frame;
            if -delta < time_spent {
                animation.elapsed_in_frame += delta;
                delta = 0.0;
                continue;
            }

            delta += time_spent;
            animation.elapsed_in_frame = 0.0;
            let mut emit_frame_event = false;
            let prior_forward = animation.forward;
            let mut changed_this_step = false;

            match animation.mode {
                SpriteAnimationLoopMode::Loop => {
                    if len > 1 {
                        let prev =
                            if animation.frame_index == 0 { len - 1 } else { animation.frame_index - 1 };
                        animation.set_frame_metrics_unchecked(prev);
                        let duration = animation.current_duration;
                        animation.elapsed_in_frame = duration;
                        delta += duration;
                        emit_frame_event = true;
                        changed_this_step = true;
                    } else {
                        animation.set_frame_metrics_unchecked(0);
                        animation.elapsed_in_frame = animation.current_duration;
                    }
                }
                SpriteAnimationLoopMode::OnceStop => {
                    if animation.frame_index == 0 {
                        animation.elapsed_in_frame = 0.0;
                        animation.set_frame_metrics_unchecked(0);
                        if respect_terminal_behavior {
                            animation.playing = false;
                        }
                        delta = 0.0;
                    } else {
                        let prev = animation.frame_index - 1;
                        animation.set_frame_metrics_unchecked(prev);
                        let duration = animation.current_duration;
                        animation.elapsed_in_frame = duration;
                        delta += duration;
                        emit_frame_event = true;
                        changed_this_step = true;
                    }
                }
                SpriteAnimationLoopMode::OnceHold => {
                    if animation.frame_index == 0 {
                        animation.elapsed_in_frame = 0.0;
                        animation.set_frame_metrics_unchecked(0);
                        if respect_terminal_behavior {
                            animation.playing = false;
                        }
                        delta = 0.0;
                    } else {
                        let prev = animation.frame_index - 1;
                        animation.set_frame_metrics_unchecked(prev);
                        let duration = animation.current_duration;
                        animation.elapsed_in_frame = duration;
                        delta += duration;
                        emit_frame_event = true;
                        changed_this_step = true;
                    }
                }
                SpriteAnimationLoopMode::PingPong => {
                    if len <= 1 {
                        animation.forward = true;
                        animation.set_frame_metrics_unchecked(0);
                        animation.elapsed_in_frame = animation.current_duration;
                    } else {
                        let bounced = animation.forward != animation.prev_forward;
                        if bounced {
                            if animation.forward {
                                // just bounced from start
                                animation.forward = animation.prev_forward;
                                animation.set_frame_metrics_unchecked(0);
                            } else {
                                // just bounced from end
                                animation.forward = animation.prev_forward;
                                animation.set_frame_metrics_unchecked(len - 1);
                            }
                        } else if animation.forward {
                            if animation.frame_index > 0 {
                                animation.set_frame_metrics_unchecked(animation.frame_index - 1);
                            } else {
                                animation.set_frame_metrics_unchecked(0);
                            }
                        } else if animation.frame_index + 1 < len {
                            animation.set_frame_metrics_unchecked(animation.frame_index + 1);
                        } else {
                            animation.set_frame_metrics_unchecked(len - 1);
                        }
                        let duration = animation.current_duration;
                        animation.elapsed_in_frame = duration;
                        delta += duration;
                        emit_frame_event = len > 1;
                        changed_this_step = len > 1;
                    }
                }
            }

            if changed_this_step {
                frame_changed = true;
                animation.prev_forward = prior_forward;
            }

            if emit_frame_event {
                if let Some(events) = events.as_deref_mut() {
                    emit_sprite_animation_events(entity, animation, events);
                }
            }
        }
    }

    frame_changed
}

fn emit_sprite_animation_events(entity: Entity, animation: &SpriteAnimation, events: &mut EventBus) {
    if let Some(frame) = animation.frames.get(animation.frame_index) {
        let mut emitted = 0_u32;
        for name in frame.events.iter() {
            events.push(GameEvent::SpriteAnimationEvent {
                entity,
                timeline: Arc::clone(&animation.timeline),
                event: Arc::clone(name),
            });
            emitted = emitted.saturating_add(1);
        }
        perf_record_events(emitted, 0);
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
fn advance_animation_loop_no_events(animation: &mut SpriteAnimation, delta: f32) -> bool {
    if delta == 0.0 || !animation.playing {
        return false;
    }
    let frame_count = animation.frame_durations.len();
    if frame_count == 0 {
        return false;
    }

    #[cfg(feature = "anim_stats")]
    record_fast_loop_call(1);

    let total_duration = animation.total_duration.max(0.0);
    if total_duration <= 0.0 {
        return false;
    }

    let total = total_duration.max(f32::EPSILON);
    let current_duration = animation.current_duration.max(0.0);
    let current_elapsed = animation.elapsed_in_frame;
    let current_offset = animation.current_frame_offset;

    if delta > 0.0 {
        let new_elapsed = current_elapsed + delta;
        if new_elapsed <= current_duration + CLIP_TIME_EPSILON {
            animation.elapsed_in_frame = new_elapsed.min(current_duration);
            return false;
        }
    } else if delta < 0.0 {
        let new_elapsed = current_elapsed + delta;
        if new_elapsed >= -CLIP_TIME_EPSILON {
            animation.elapsed_in_frame = new_elapsed.max(0.0);
            return false;
        }
    }

    let raw_position = current_offset + current_elapsed + delta;
    let mut normalized = raw_position.rem_euclid(total);
    if normalized.is_nan() {
        normalized = 0.0;
    } else if normalized >= total {
        normalized = total - f32::EPSILON;
    } else if normalized < 0.0 {
        normalized = 0.0;
    }

    let wrapped = raw_position < 0.0 || raw_position >= total;

    #[cfg(feature = "anim_stats")]
    if wrapped {
        record_fast_loop_binary_search(1);
    }

    let offsets = animation.frame_offsets.as_ref();
    let durations = animation.frame_durations.as_ref();

    let mut index = if frame_count == 1 {
        0
    } else {
        match offsets
            .binary_search_by(|start| start.partial_cmp(&normalized).unwrap_or(std::cmp::Ordering::Less))
        {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        }
    };

    if index >= frame_count {
        index = frame_count - 1;
    }

    let current_start = offsets.get(index).copied().unwrap_or(0.0);
    let mut new_duration = durations.get(index).copied().unwrap_or(current_duration);
    if new_duration < 0.0 {
        new_duration = 0.0;
    }

    let mut elapsed = normalized - current_start;
    if elapsed < 0.0 {
        elapsed = 0.0;
    } else if elapsed > new_duration {
        elapsed = new_duration;
    }

    let previous_index = animation.frame_index.min(frame_count - 1);

    animation.frame_index = index;
    animation.elapsed_in_frame = elapsed;
    animation.current_duration = new_duration;
    animation.current_frame_offset = current_start;

    wrapped || index != previous_index
}

#[cfg(feature = "sprite_anim_soa")]
fn advance_animation_loop_no_events_slot(runtime: &mut SpriteAnimatorSoa, slot: usize, delta: f32) -> bool {
    if delta == 0.0 || !runtime.flags[slot].playing() {
        return false;
    }
    let frame_count = runtime.frame_durations[slot].len();
    if frame_count == 0 {
        return false;
    }

    #[cfg(feature = "anim_stats")]
    record_fast_loop_call(1);

    let total_duration = runtime.total_duration[slot].max(0.0);
    if total_duration <= 0.0 {
        return false;
    }

    let total = total_duration.max(f32::EPSILON);
    let current_duration = runtime.current_duration[slot].max(0.0);
    let current_elapsed = runtime.elapsed_in_frame[slot];
    let current_offset = runtime.current_frame_offset[slot];

    if delta > 0.0 {
        let new_elapsed = current_elapsed + delta;
        if new_elapsed <= current_duration + CLIP_TIME_EPSILON {
            runtime.elapsed_in_frame[slot] = new_elapsed.min(current_duration);
            return false;
        }
    } else if delta < 0.0 {
        let new_elapsed = current_elapsed + delta;
        if new_elapsed >= -CLIP_TIME_EPSILON {
            runtime.elapsed_in_frame[slot] = new_elapsed.max(0.0);
            return false;
        }
    }

    let raw_position = current_offset + current_elapsed + delta;
    let mut normalized = raw_position.rem_euclid(total);
    if normalized.is_nan() {
        normalized = 0.0;
    } else if normalized >= total {
        normalized = total - f32::EPSILON;
    } else if normalized < 0.0 {
        normalized = 0.0;
    }

    let wrapped = raw_position < 0.0 || raw_position >= total;

    #[cfg(feature = "anim_stats")]
    if wrapped {
        record_fast_loop_binary_search(1);
    }

    let offsets = runtime.frame_offsets[slot].as_ref();
    let durations = runtime.frame_durations[slot].as_ref();

    let mut index = if frame_count == 1 {
        0
    } else {
        match offsets
            .binary_search_by(|start| start.partial_cmp(&normalized).unwrap_or(std::cmp::Ordering::Less))
        {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        }
    };

    if index >= frame_count {
        index = frame_count - 1;
    }

    let current_start = offsets.get(index).copied().unwrap_or(0.0);
    let mut new_duration = durations.get(index).copied().unwrap_or(current_duration);
    if new_duration < 0.0 {
        new_duration = 0.0;
    }

    let mut elapsed = normalized - current_start;
    if elapsed < 0.0 {
        elapsed = 0.0;
    } else if elapsed > new_duration {
        elapsed = new_duration;
    }

    let previous_index = runtime.frame_index[slot].min((frame_count - 1) as u32);

    runtime.frame_index[slot] = index as u32;
    runtime.elapsed_in_frame[slot] = elapsed;
    runtime.current_duration[slot] = new_duration;
    runtime.current_frame_offset[slot] = current_start;
    #[cfg(feature = "sprite_anim_fixed_point")]
    {
        runtime.elapsed_in_frame_fp[slot] = fp_from_f32(elapsed);
        runtime.current_duration_fp[slot] = unsafe { *runtime.frame_durations_fp[slot].get_unchecked(index) };
        runtime.current_frame_offset_fp[slot] =
            unsafe { *runtime.frame_offsets_fp[slot].get_unchecked(index) };
    }
    refresh_next_frame_duration_slot(runtime, slot);

    wrapped || index as u32 != previous_index
}

#[inline(always)]
fn advance_animation_fast_loop(animation: &mut SpriteAnimation, delta: f32) -> bool {
    if delta == 0.0 || !animation.playing {
        return false;
    }
    let frame_count = animation.frame_durations.len();
    if frame_count == 0 {
        return false;
    }

    #[cfg(feature = "anim_stats")]
    record_fast_loop_call(1);

    let total_duration = animation.total_duration.max(f32::EPSILON);
    if delta.abs() >= total_duration * 4.0 {
        perf_record_mod_or_div();
        return advance_animation_loop_no_events(animation, delta);
    }

    let durations = animation.frame_durations.as_ptr();
    let offsets = animation.frame_offsets.as_ptr();
    let max_epsilon = CLIP_TIME_EPSILON;
    let mut index = animation.frame_index;
    debug_assert!(index < frame_count);
    let mut current_duration = animation.current_duration.max(max_epsilon);
    let mut current_offset = animation.current_frame_offset;

    if delta > 0.0 {
        let mut remaining = animation.elapsed_in_frame + delta;
        if remaining <= current_duration {
            animation.elapsed_in_frame = remaining;
            animation.current_frame_offset = current_offset;
            return false;
        }
        remaining -= current_duration;
        let mut threshold;
        loop {
            index += 1;
            if index == frame_count {
                index = 0;
            }
            unsafe {
                current_duration = (*durations.add(index)).max(max_epsilon);
                current_offset = *offsets.add(index);
            }
            threshold = current_duration;
            if remaining <= threshold {
                animation.frame_index = index;
                animation.current_duration = current_duration;
                animation.current_frame_offset = current_offset;
                animation.elapsed_in_frame = remaining;
                return true;
            }
            remaining -= threshold;
        }
    } else {
        let mut remaining = animation.elapsed_in_frame + delta;
        if remaining >= -max_epsilon {
            animation.elapsed_in_frame = remaining.max(0.0);
            animation.current_frame_offset = current_offset;
            return false;
        }
        remaining = -remaining;
        loop {
            index = if index == 0 { frame_count - 1 } else { index - 1 };
            unsafe {
                current_duration = (*durations.add(index)).max(max_epsilon);
                current_offset = *offsets.add(index);
            }
            let threshold = current_duration;
            if remaining <= threshold {
                animation.frame_index = index;
                animation.current_duration = current_duration;
                animation.current_frame_offset = current_offset;
                let new_elapsed = current_duration - remaining;
                animation.elapsed_in_frame = new_elapsed.max(0.0);
                return true;
            }
            remaining -= threshold;
        }
    }
}

#[cfg(not(feature = "sprite_anim_soa"))]
fn drive_fast_single(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    animations: &mut Query<
        (Entity, &mut SpriteAnimation, &mut SpriteFrameState, &mut Sprite),
        With<FastSpriteAnimator>,
    >,
) {
    #[cfg(feature = "anim_stats")]
    let mut processed = 0_u64;

    if delta == 0.0 {
        #[cfg(feature = "anim_stats")]
        record_fast_bucket_sample(0);
        return;
    }

    perf_record_fast_bucket_frame();

    for (_, mut animation, mut sprite_state, mut sprite) in animations.iter_mut() {
        let frame_count = animation.frames.len();
        if !prepare_animation(&mut animation, frame_count) {
            continue;
        }

        let Some(playback_rate) = resolve_playback_rate(&mut animation, has_group_scales, animation_time)
        else {
            continue;
        };
        let scaled = delta * playback_rate;
        if scaled == 0.0 {
            continue;
        }
        let Some(advance_delta) = animation.accumulate_delta(scaled, CLIP_TIME_EPSILON) else {
            continue;
        };

        perf_record_fast_animator();
        debug_assert!(animation.fast_loop);
        if advance_animation_fast_loop(&mut animation, advance_delta) {
            apply_sprite_frame_immediate(&animation, &mut sprite_state, &mut sprite);
        }
        #[cfg(feature = "anim_stats")]
        {
            processed += 1;
        }
    }

    #[cfg(feature = "anim_stats")]
    record_fast_bucket_sample(processed);
}

#[cfg(feature = "sprite_anim_soa")]
fn drive_fast_single_soa(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    runtime: &mut SpriteAnimatorSoa,
    state_updates: &mut Vec<SpriteStateUpdate>,
) {
    let processed =
        drive_fast_single_soa_pass(delta, has_group_scales, animation_time, runtime, state_updates, true);
    #[cfg(feature = "anim_stats")]
    record_fast_bucket_sample(processed);
    #[cfg(not(feature = "anim_stats"))]
    {
        let _ = processed;
    }
}

#[cfg(feature = "sprite_anim_soa")]
fn drive_fast_single_soa_pass(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    runtime: &mut SpriteAnimatorSoa,
    state_updates: &mut Vec<SpriteStateUpdate>,
    record_bucket_frame: bool,
) -> u64 {
    if delta == 0.0 || runtime.is_empty() {
        return 0;
    }

    if record_bucket_frame {
        perf_record_fast_bucket_frame();
    }

    #[cfg(feature = "sprite_anim_simd")]
    if delta > 0.0 && runtime.len() >= SPRITE_SIMD_WIDTH {
        let stats = drive_fast_single_simd(delta, has_group_scales, animation_time, runtime, state_updates);
        perf_record_simd_mix(stats.lanes8, 0, stats.tail_scalar);
        return stats.processed;
    }

    let mut processed = 0_u64;

    for slot in 0..runtime.len() {
        let handled =
            process_fast_slot(slot, delta, has_group_scales, animation_time, runtime, state_updates);
        if handled {
            processed += 1;
        }
    }

    processed
}

#[cfg(feature = "sprite_anim_soa")]
fn drive_fast_fixed_soa(
    step: f32,
    steps: u32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    runtime: &mut SpriteAnimatorSoa,
    state_updates: &mut Vec<SpriteStateUpdate>,
) {
    if step == 0.0 || steps == 0 || runtime.is_empty() {
        return;
    }

    perf_record_fast_bucket_frame();
    let mut processed_total = 0_u64;

    for _ in 0..steps {
        let processed =
            drive_fast_single_soa_pass(step, has_group_scales, animation_time, runtime, state_updates, false);
        processed_total = processed_total.saturating_add(processed);
        #[cfg(test)]
        {
            for _ in 0..processed {
                tests::record_drive_fixed_step_iteration();
            }
        }
    }

    #[cfg(feature = "anim_stats")]
    record_fast_bucket_sample(processed_total);
}

#[cfg(not(feature = "sprite_anim_soa"))]
fn drive_fast_fixed(
    step: f32,
    steps: u32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    animations: &mut Query<
        (Entity, &mut SpriteAnimation, &mut SpriteFrameState, &mut Sprite),
        With<FastSpriteAnimator>,
    >,
) {
    if step == 0.0 || steps == 0 {
        #[cfg(feature = "anim_stats")]
        record_fast_bucket_sample(0);
        return;
    }

    #[cfg(feature = "anim_stats")]
    let mut processed = 0_u64;

    perf_record_fast_bucket_frame();

    for (_, mut animation, mut sprite_state, mut sprite) in animations.iter_mut() {
        let frame_count = animation.frames.len();
        if !prepare_animation(&mut animation, frame_count) {
            continue;
        }

        let Some(playback_rate) = resolve_playback_rate(&mut animation, has_group_scales, animation_time)
        else {
            continue;
        };
        let scaled_step = step * playback_rate;
        if scaled_step == 0.0 {
            continue;
        }
        perf_record_fast_animator();

        let mut sprite_changed = false;
        for _ in 0..steps {
            #[cfg(test)]
            tests::record_drive_fixed_step_iteration();
            let Some(advance_delta) = animation.accumulate_delta(scaled_step, CLIP_TIME_EPSILON) else {
                if !animation.playing {
                    break;
                }
                continue;
            };

            debug_assert!(animation.fast_loop);
            if advance_animation_fast_loop(&mut animation, advance_delta) {
                sprite_changed = true;
            }
            if !animation.playing {
                break;
            }
        }

        if sprite_changed {
            apply_sprite_frame_immediate(&animation, &mut sprite_state, &mut sprite);
        }

        #[cfg(feature = "anim_stats")]
        {
            processed += 1;
        }
    }

    #[cfg(feature = "anim_stats")]
    record_fast_bucket_sample(processed);
}

#[cfg(feature = "sprite_anim_soa")]
fn process_fast_slot(
    slot: usize,
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    runtime: &mut SpriteAnimatorSoa,
    state_updates: &mut Vec<SpriteStateUpdate>,
) -> bool {
    if !prepare_animation_slot(runtime, slot) {
        return false;
    }

    let Some(playback_rate) = resolve_playback_rate_slot(runtime, slot, has_group_scales, animation_time)
    else {
        return false;
    };
    let scaled = delta * playback_rate;
    if scaled == 0.0 {
        return false;
    }
    let Some(advance_delta) = accumulate_delta_slot(runtime, slot, scaled, CLIP_TIME_EPSILON) else {
        return false;
    };

    if advance_animation_fast_loop_slot(runtime, slot, advance_delta) {
        let entity = runtime.entity(slot);
        state_updates.push((entity, slot));
    }
    perf_record_fast_animator();
    true
}

#[cfg(feature = "sprite_anim_simd")]
fn drive_fast_single_simd(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    runtime: &mut SpriteAnimatorSoa,
    state_updates: &mut Vec<SpriteStateUpdate>,
) -> SimdMixStats {
    let len = runtime.len();
    if len == 0 {
        return SimdMixStats::default();
    }
    let mut slot = 0;
    let mut processed = 0_u64;
    let mut simd_lanes = 0_u32;
    let mut pending: Vec<PendingSimdSlot> = Vec::with_capacity(SPRITE_SIMD_WIDTH);
    while slot < len {
        if !prepare_animation_slot(runtime, slot) {
            slot += 1;
            continue;
        }
        let Some(playback_rate) = resolve_playback_rate_slot(runtime, slot, has_group_scales, animation_time)
        else {
            slot += 1;
            continue;
        };
        let scaled = delta * playback_rate;
        if scaled == 0.0 {
            slot += 1;
            continue;
        }
        let Some(advance_delta) = accumulate_delta_slot(runtime, slot, scaled, CLIP_TIME_EPSILON) else {
            slot += 1;
            continue;
        };
        if runtime.flags[slot].const_dt() && advance_delta > 0.0 {
            processed += 1;
            pending.push(PendingSimdSlot { slot, delta: advance_delta });
            if pending.len() == SPRITE_SIMD_WIDTH {
                simd_lanes += SPRITE_SIMD_WIDTH as u32;
                apply_const_dt_simd_chunk(runtime, &pending, state_updates);
                pending.clear();
            }
        } else {
            processed += 1;
            if advance_animation_fast_loop_slot(runtime, slot, advance_delta) {
                let entity = runtime.entity(slot);
                state_updates.push((entity, slot));
            }
            perf_record_fast_animator();
        }
        slot += 1;
    }
    let tail_scalar = pending.len() as u32;
    for entry in pending {
        let scalar_timer = Instant::now();
        if advance_animation_fast_loop_slot(runtime, entry.slot, entry.delta) {
            let entity = runtime.entity(entry.slot);
            state_updates.push((entity, entry.slot));
        }
        perf_record_fast_animator();
        perf_record_simd_scalar_time(scalar_timer.elapsed().as_nanos() as u64);
    }
    SimdMixStats { processed, lanes8: simd_lanes, tail_scalar }
}

#[cfg(feature = "sprite_anim_simd")]
fn apply_const_dt_simd_chunk(
    runtime: &mut SpriteAnimatorSoa,
    pending: &[PendingSimdSlot],
    state_updates: &mut Vec<SpriteStateUpdate>,
) {
    let chunk_timer = Instant::now();
    debug_assert_eq!(pending.len(), SPRITE_SIMD_WIDTH);
    for entry in pending {
        let slot = entry.slot;
        let dt_fp = runtime.const_dt_duration_fp[slot].max(FP_CLIP_EPSILON);
        let delta_fp = fp_from_f32(entry.delta) as u64;
        let total_fp = runtime.elapsed_in_frame_fp[slot] as u64 + delta_fp;
        let steps = total_fp / dt_fp as u64;
        let remainder_fp = (total_fp - steps * dt_fp as u64) as u32;

        let frames = runtime.const_dt_frame_count[slot].max(1) as usize;
        let mut index = runtime.frame_index[slot] as usize;
        if frames > 0 {
            let advance = (steps as usize) % frames;
            index = (index + advance) % frames;
        } else {
            index = 0;
        }
        runtime.frame_index[slot] = index as u32;
        runtime.elapsed_in_frame_fp[slot] = remainder_fp;
        runtime.elapsed_in_frame[slot] = f32_from_fp(remainder_fp);

        let offsets = runtime.frame_offsets[slot].as_ref();
        runtime.current_frame_offset[slot] = offsets.get(index).copied().unwrap_or(0.0);
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            let offsets_fp = runtime.frame_offsets_fp[slot].as_ref();
            runtime.current_frame_offset_fp[slot] = offsets_fp.get(index).copied().unwrap_or(0);
        }

        let durations = runtime.frame_durations[slot].as_ref();
        let duration_f = durations.get(index).copied().unwrap_or(runtime.const_dt_duration[slot]);
        runtime.current_duration[slot] = duration_f;
        runtime.next_frame_duration[slot] = duration_f;
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            let durations_fp = runtime.frame_durations_fp[slot].as_ref();
            let duration_fp = durations_fp.get(index).copied().unwrap_or(dt_fp);
            runtime.current_duration_fp[slot] = duration_fp;
            runtime.next_frame_duration_fp[slot] = duration_fp;
        }

        if steps > 0 {
            let entity = runtime.entity(slot);
            state_updates.push((entity, slot));
        }
        perf_record_fast_animator();
    }
    perf_record_simd_chunk_time(chunk_timer.elapsed().as_nanos() as u64);
}

#[cfg(feature = "sprite_anim_soa")]
fn flush_sprite_state_updates(
    runtime: &SpriteAnimatorSoa,
    state_updates: &mut Vec<SpriteStateUpdate>,
    frame_updates: &mut SpriteFrameApplyQueue,
    sprite_states: &mut Query<&mut SpriteFrameState, With<FastSpriteAnimator>>,
) {
    if state_updates.is_empty() {
        return;
    }
    let flush_len = state_updates.len();
    record_sprite_state_flush(flush_len);
    let mut entities: Vec<Entity> = Vec::with_capacity(state_updates.len());
    let mut slots: Vec<usize> = Vec::with_capacity(state_updates.len());
    for (entity, slot) in state_updates.drain(..) {
        entities.push(entity);
        slots.push(slot);
    }
    let mut iter = sprite_states.iter_many_mut(entities.iter());
    let mut idx = 0usize;
    while let Some(mut sprite_state) = iter.fetch_next() {
        let entity = entities[idx];
        let slot = slots[idx];
        queue_sprite_frame_update_from_soa(entity, slot, runtime, &mut sprite_state, frame_updates);
        idx += 1;
    }
}

fn drive_general_fixed(
    step: f32,
    steps: u32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    events: &mut EventBus,
    frame_updates: &mut SpriteFrameApplyQueue,
    animations: &mut Query<
        (Entity, &mut SpriteAnimation, &mut SpriteFrameState),
        Without<FastSpriteAnimator>,
    >,
) {
    #[cfg(feature = "anim_stats")]
    let mut processed = 0_u64;

    if steps == 0 {
        #[cfg(feature = "anim_stats")]
        record_general_bucket_sample(0);
        return;
    }

    perf_record_general_bucket_frame();

    for (entity, mut animation, mut sprite_state) in animations.iter_mut() {
        let frame_count = animation.frames.len();
        if !prepare_animation(&mut animation, frame_count) {
            continue;
        }

        if animation.pending_start_events {
            if animation.has_events {
                emit_sprite_animation_events(entity, &animation, events);
            }
            animation.pending_start_events = false;
        }

        let Some(playback_rate) = resolve_playback_rate(&mut animation, has_group_scales, animation_time)
        else {
            continue;
        };
        let scaled_step = step * playback_rate;
        if scaled_step == 0.0 {
            continue;
        }

        perf_record_general_animator(&animation);
        let mut sprite_changed = false;
        for _ in 0..steps {
            #[cfg(test)]
            tests::record_drive_fixed_step_iteration();

            let Some(advance_delta) = animation.accumulate_delta(scaled_step, CLIP_TIME_EPSILON) else {
                if !animation.playing {
                    break;
                }
                continue;
            };

            let mut step_changed = false;
            if animation.fast_loop {
                if advance_animation_fast_loop(&mut animation, advance_delta) {
                    step_changed = true;
                }
            } else if animation.has_events {
                let events_ref = &mut *events;
                if advance_animation(&mut animation, advance_delta, entity, Some(events_ref), true) {
                    step_changed = true;
                }
            } else if advance_animation(&mut animation, advance_delta, entity, None, true) {
                step_changed = true;
            }

            if step_changed {
                sprite_changed = true;
            }
            if !animation.playing {
                break;
            }
        }

        if sprite_changed {
            queue_sprite_frame_update(entity, &animation, &mut sprite_state, frame_updates);
        }
        #[cfg(feature = "anim_stats")]
        {
            processed += 1;
        }
    }

    #[cfg(feature = "anim_stats")]
    record_general_bucket_sample(processed);
}

fn prepare_animation(animation: &mut SpriteAnimation, frame_count: usize) -> bool {
    if frame_count == 0 {
        return false;
    }
    if animation.frame_index >= frame_count {
        animation.frame_index = 0;
        animation.elapsed_in_frame = 0.0;
        animation.refresh_current_duration();
    }
    animation.playing
}

#[cfg(feature = "sprite_anim_soa")]
fn prepare_animation_slot(runtime: &mut SpriteAnimatorSoa, slot: usize) -> bool {
    let frame_count = runtime.frame_durations[slot].len();
    if frame_count == 0 {
        runtime.flags[slot].set_needs_prep(true);
        return false;
    }
    let mut flags = runtime.flags[slot];
    if !flags.needs_prep() {
        return flags.playing();
    }
    let index = runtime.frame_index[slot] as usize;
    if index >= frame_count {
        runtime.frame_index[slot] = 0;
        runtime.elapsed_in_frame[slot] = 0.0;
        runtime.current_duration[slot] = runtime.frame_durations[slot].first().copied().unwrap_or(0.0);
        runtime.current_frame_offset[slot] = runtime.frame_offsets[slot].first().copied().unwrap_or(0.0);
        #[cfg(feature = "sprite_anim_fixed_point")]
        {
            runtime.elapsed_in_frame_fp[slot] = 0;
            runtime.current_duration_fp[slot] =
                runtime.frame_durations_fp[slot].first().copied().unwrap_or(0);
            runtime.current_frame_offset_fp[slot] =
                runtime.frame_offsets_fp[slot].first().copied().unwrap_or(0);
        }
    }
    flags.set_needs_prep(false);
    refresh_next_frame_duration_slot(runtime, slot);
    runtime.flags[slot] = flags;
    flags.playing()
}

#[inline(always)]
fn resolve_playback_rate(
    animation: &mut SpriteAnimation,
    has_group_scales: bool,
    animation_time: &AnimationTime,
) -> Option<f32> {
    let playback_rate = if animation.playback_rate_dirty {
        let group_scale =
            if has_group_scales { animation_time.group_scale(animation.group.as_deref()) } else { 1.0 };
        animation.ensure_playback_rate(group_scale)
    } else {
        animation.playback_rate
    };
    if playback_rate == 0.0 {
        None
    } else {
        Some(playback_rate)
    }
}

#[cfg(all(feature = "sprite_anim_soa", not(feature = "sprite_anim_fixed_point")))]
fn advance_animation_fast_loop_slot(runtime: &mut SpriteAnimatorSoa, slot: usize, delta: f32) -> bool {
    if delta == 0.0 || !runtime.flags[slot].playing() {
        return false;
    }
    let frame_count = runtime.frame_durations[slot].len();
    if frame_count == 0 {
        return false;
    }

    #[cfg(feature = "anim_stats")]
    record_fast_loop_call(1);

    let total_duration = runtime.total_duration[slot].max(f32::EPSILON);
    if delta.abs() >= total_duration * 4.0 {
        perf_record_mod_or_div();
        return advance_animation_loop_no_events_slot(runtime, slot, delta);
    }

    let durations = runtime.frame_durations[slot].as_ref();
    let offsets = runtime.frame_offsets[slot].as_ref();
    let mut index = runtime.frame_index[slot] as usize;
    debug_assert!(index < frame_count);
    let mut current_duration = runtime.current_duration[slot];
    let mut current_offset = runtime.current_frame_offset[slot];
    let mut next_duration = runtime.next_frame_duration[slot].max(CLIP_TIME_EPSILON);

    if delta > 0.0 {
        let mut remaining = runtime.elapsed_in_frame[slot] + delta;
        let mut threshold = current_duration.max(CLIP_TIME_EPSILON);
        if remaining <= threshold {
            runtime.elapsed_in_frame[slot] = remaining.min(current_duration);
            runtime.current_frame_offset[slot] = current_offset;
            return false;
        }
        remaining -= threshold;
        loop {
            index += 1;
            if index == frame_count {
                index = 0;
            }
            unsafe {
                current_offset = *offsets.get_unchecked(index);
            }
            current_duration = next_duration;
            threshold = current_duration.max(CLIP_TIME_EPSILON);
            if remaining <= threshold {
                runtime.frame_index[slot] = index as u32;
                runtime.current_duration[slot] = current_duration;
                runtime.current_frame_offset[slot] = current_offset;
                runtime.elapsed_in_frame[slot] = remaining.min(current_duration);
                refresh_next_frame_duration_slot(runtime, slot);
                return true;
            }
            remaining -= threshold;
            let peek = if index + 1 == frame_count { 0 } else { index + 1 };
            unsafe {
                next_duration = *durations.get_unchecked(peek);
            }
        }
    } else {
        let mut remaining = runtime.elapsed_in_frame[slot] + delta;
        if remaining >= -CLIP_TIME_EPSILON {
            runtime.elapsed_in_frame[slot] = remaining.max(0.0);
            runtime.current_frame_offset[slot] = current_offset;
            return false;
        }
        remaining = -remaining;
        loop {
            index = if index == 0 { frame_count - 1 } else { index - 1 };
            unsafe {
                current_duration = *durations.get_unchecked(index);
                current_offset = *offsets.get_unchecked(index);
            }
            let threshold = current_duration.max(CLIP_TIME_EPSILON);
            if remaining <= threshold {
                runtime.frame_index[slot] = index as u32;
                runtime.current_duration[slot] = current_duration;
                runtime.current_frame_offset[slot] = current_offset;
                let new_elapsed = current_duration - remaining;
                runtime.elapsed_in_frame[slot] = new_elapsed.max(0.0);
                refresh_next_frame_duration_slot(runtime, slot);
                return true;
            }
            remaining -= threshold;
        }
    }
}

#[cfg(all(feature = "sprite_anim_soa", feature = "sprite_anim_fixed_point"))]
fn advance_animation_fast_loop_slot(runtime: &mut SpriteAnimatorSoa, slot: usize, delta: f32) -> bool {
    if delta == 0.0 || !runtime.flags[slot].playing() {
        return false;
    }
    let frame_count = runtime.frame_durations_fp[slot].len();
    if frame_count == 0 {
        return false;
    }

    #[cfg(feature = "anim_stats")]
    record_fast_loop_call(1);

    let total_duration = runtime.total_duration[slot].max(f32::EPSILON);
    if delta.abs() >= total_duration * 4.0 {
        perf_record_mod_or_div();
        return advance_animation_loop_no_events_slot(runtime, slot, delta);
    }

    let durations = runtime.frame_durations_fp[slot].as_ref();
    let offsets = runtime.frame_offsets_fp[slot].as_ref();
    let mut index = runtime.frame_index[slot] as usize;
    debug_assert!(index < frame_count);
    let mut current_duration_fp = runtime.current_duration_fp[slot].max(FP_CLIP_EPSILON);
    let mut current_offset_fp = runtime.current_frame_offset_fp[slot];
    let mut next_duration_fp = runtime.next_frame_duration_fp[slot].max(FP_CLIP_EPSILON);

    if delta > 0.0 {
        let delta_fp = fp_from_f32(delta) as u64;
        let mut remaining = runtime.elapsed_in_frame_fp[slot] as u64 + delta_fp;
        let mut threshold = current_duration_fp.max(FP_CLIP_EPSILON) as u64;
        if remaining <= threshold {
            let new_elapsed = remaining.min(current_duration_fp as u64) as u32;
            runtime.elapsed_in_frame_fp[slot] = new_elapsed;
            runtime.elapsed_in_frame[slot] = f32_from_fp(new_elapsed);
            runtime.current_frame_offset_fp[slot] = current_offset_fp;
            runtime.current_frame_offset[slot] = f32_from_fp(current_offset_fp);
            return false;
        }
        remaining -= threshold;
        loop {
            index += 1;
            if index == frame_count {
                index = 0;
            }
            unsafe {
                current_offset_fp = *offsets.get_unchecked(index);
            }
            current_duration_fp = next_duration_fp.max(FP_CLIP_EPSILON);
            threshold = current_duration_fp as u64;
            if remaining <= threshold {
                runtime.frame_index[slot] = index as u32;
                runtime.current_duration_fp[slot] = current_duration_fp;
                runtime.current_duration[slot] = f32_from_fp(current_duration_fp);
                runtime.current_frame_offset_fp[slot] = current_offset_fp;
                runtime.current_frame_offset[slot] = f32_from_fp(current_offset_fp);
                let new_elapsed = remaining.min(current_duration_fp as u64) as u32;
                runtime.elapsed_in_frame_fp[slot] = new_elapsed;
                runtime.elapsed_in_frame[slot] = f32_from_fp(new_elapsed);
                refresh_next_frame_duration_slot(runtime, slot);
                return true;
            }
            remaining -= threshold;
            let peek = if index + 1 == frame_count { 0 } else { index + 1 };
            unsafe {
                next_duration_fp = (*durations.get_unchecked(peek)).max(FP_CLIP_EPSILON);
            }
        }
    } else {
        let delta_fp = fp_from_f32(-delta) as u64;
        let mut remaining = runtime.elapsed_in_frame_fp[slot] as i64 - delta_fp as i64;
        if remaining >= -(FP_CLIP_EPSILON as i64) {
            let clamped = remaining.max(0) as u32;
            runtime.elapsed_in_frame_fp[slot] = clamped;
            runtime.elapsed_in_frame[slot] = f32_from_fp(clamped);
            runtime.current_frame_offset_fp[slot] = current_offset_fp;
            runtime.current_frame_offset[slot] = f32_from_fp(current_offset_fp);
            return false;
        }
        remaining = -remaining;
        loop {
            index = if index == 0 { frame_count - 1 } else { index - 1 };
            unsafe {
                current_duration_fp = (*durations.get_unchecked(index)).max(FP_CLIP_EPSILON);
                current_offset_fp = *offsets.get_unchecked(index);
            }
            let threshold = current_duration_fp as i64;
            if remaining <= threshold {
                runtime.frame_index[slot] = index as u32;
                runtime.current_duration_fp[slot] = current_duration_fp;
                runtime.current_duration[slot] = f32_from_fp(current_duration_fp);
                runtime.current_frame_offset_fp[slot] = current_offset_fp;
                runtime.current_frame_offset[slot] = f32_from_fp(current_offset_fp);
                let new_elapsed = current_duration_fp as i64 - remaining;
                let clamped = new_elapsed.max(0) as u32;
                runtime.elapsed_in_frame_fp[slot] = clamped;
                runtime.elapsed_in_frame[slot] = f32_from_fp(clamped);
                refresh_next_frame_duration_slot(runtime, slot);
                return true;
            }
            remaining -= threshold;
        }
    }
}

#[cfg(feature = "sprite_anim_soa")]
#[inline(always)]
fn resolve_playback_rate_slot(
    runtime: &mut SpriteAnimatorSoa,
    slot: usize,
    has_group_scales: bool,
    animation_time: &AnimationTime,
) -> Option<f32> {
    let mut flags = runtime.flags[slot];
    if flags.playback_dirty() {
        let group_scale =
            if has_group_scales { animation_time.group_scale(runtime.group[slot].as_deref()) } else { 1.0 };
        runtime.playback_rate[slot] = runtime.speed[slot] * group_scale;
        flags.set_playback_dirty(false);
        runtime.flags[slot] = flags;
    }
    let playback_rate = runtime.playback_rate[slot];
    if playback_rate == 0.0 {
        None
    } else {
        Some(playback_rate)
    }
}

#[cfg(feature = "sprite_anim_soa")]
#[inline(always)]
fn accumulate_delta_slot(
    runtime: &mut SpriteAnimatorSoa,
    slot: usize,
    delta: f32,
    epsilon: f32,
) -> Option<f32> {
    if delta == 0.0 {
        return None;
    }
    let mut pending = runtime.pending_delta[slot];
    if pending != 0.0 && (pending < 0.0) != (delta < 0.0) {
        pending = 0.0;
    }
    let total = pending + delta;
    if total > -epsilon && total < epsilon {
        runtime.pending_delta[slot] = total;
        None
    } else {
        runtime.pending_delta[slot] = 0.0;
        Some(total)
    }
}

fn queue_sprite_frame_update(
    entity: Entity,
    animation: &SpriteAnimation,
    sprite_state: &mut SpriteFrameState,
    frame_updates: &mut SpriteFrameApplyQueue,
) {
    debug_assert_eq!(animation.frames.len(), animation.frame_hot_data.len());
    // SAFETY: frame_index already validated against frame_count earlier in the loop.
    let hot = unsafe { animation.frame_hot_data.get_unchecked(animation.frame_index) };
    let frame = unsafe { animation.frames.get_unchecked(animation.frame_index) };
    let region = Some(&frame.region);
    sprite_state.update_from_hot_frame(hot, region);
    if sprite_state.queued_for_apply {
        return;
    }
    sprite_state.queued_for_apply = true;
    frame_updates.push(entity);
}

fn apply_sprite_frame_immediate(
    animation: &SpriteAnimation,
    sprite_state: &mut SpriteFrameState,
    sprite: &mut Sprite,
) {
    debug_assert_eq!(animation.frames.len(), animation.frame_hot_data.len());
    let hot = unsafe { animation.frame_hot_data.get_unchecked(animation.frame_index) };
    let frame = unsafe { animation.frames.get_unchecked(animation.frame_index) };
    let region = Some(&frame.region);
    sprite_state.update_from_hot_frame(hot, region);
    sprite_state.apply_to_sprite(sprite);
}

#[cfg(feature = "sprite_anim_soa")]
fn queue_sprite_frame_update_from_soa(
    entity: Entity,
    slot: usize,
    runtime: &SpriteAnimatorSoa,
    sprite_state: &mut SpriteFrameState,
    frame_updates: &mut SpriteFrameApplyQueue,
) {
    let index = runtime.frame_index[slot] as usize;
    let hot_frames = runtime.frame_hot_data[slot].as_ref();
    let frames = runtime.frames[slot].as_ref();
    let hot = unsafe { hot_frames.get_unchecked(index) };
    let frame = unsafe { frames.get_unchecked(index) };
    let region = Some(&frame.region);
    sprite_state.update_from_hot_frame(hot, region);
    if sprite_state.queued_for_apply {
        return;
    }
    sprite_state.queued_for_apply = true;
    frame_updates.push(entity);
}

pub fn sys_apply_sprite_frame_states(
    mut profiler: ResMut<SystemProfiler>,
    mut frame_updates: ResMut<SpriteFrameApplyQueue>,
    mut sprites: Query<(&mut Sprite, &mut SpriteFrameState)>,
) {
    let pending = frame_updates.take();
    let pending_len = pending.len();
    if pending_len == 0 {
        frame_updates.restore(pending);
        return;
    }
    record_sprite_frame_queue_depth(pending_len);

    let _span = profiler.scope("sys_apply_sprite_frame_states");
    #[cfg(feature = "anim_stats")]
    let mut applied = 0_u64;

    {
        let mut iter = sprites.iter_many_mut(pending.iter());
        while let Some((mut sprite, mut state)) = iter.fetch_next() {
            if let Some(region) = state.pending_region.take() {
                sprite.region = region;
                state.region_initialized = true;
            }
            sprite.region_id = state.region_id;
            sprite.uv = state.uv;
            state.queued_for_apply = false;
            #[cfg(feature = "anim_stats")]
            {
                applied += 1;
            }
        }
    }

    #[cfg(feature = "anim_stats")]
    record_sprite_frame_applies(applied);

    frame_updates.restore(pending);
}
fn drive_general_single(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    events: &mut EventBus,
    frame_updates: &mut SpriteFrameApplyQueue,
    animations: &mut Query<
        (Entity, &mut SpriteAnimation, &mut SpriteFrameState),
        Without<FastSpriteAnimator>,
    >,
) {
    #[cfg(feature = "anim_stats")]
    let mut processed = 0_u64;

    perf_record_general_bucket_frame();

    for (entity, mut animation, mut sprite_state) in animations.iter_mut() {
        let frame_count = animation.frames.len();
        if !prepare_animation(&mut animation, frame_count) {
            continue;
        }

        if animation.pending_start_events {
            if animation.has_events {
                emit_sprite_animation_events(entity, &animation, events);
            }
            animation.pending_start_events = false;
        }

        let Some(playback_rate) = resolve_playback_rate(&mut animation, has_group_scales, animation_time)
        else {
            continue;
        };
        let scaled = delta * playback_rate;
        if scaled == 0.0 {
            continue;
        }
        let Some(advance_delta) = animation.accumulate_delta(scaled, CLIP_TIME_EPSILON) else {
            continue;
        };

        perf_record_general_animator(&animation);
        let mut sprite_changed = false;
        if animation.fast_loop {
            if advance_animation_fast_loop(&mut animation, advance_delta) {
                sprite_changed = true;
            }
        } else if animation.has_events {
            let events_ref = &mut *events;
            if advance_animation(&mut animation, advance_delta, entity, Some(events_ref), true) {
                sprite_changed = true;
            }
        } else if advance_animation(&mut animation, advance_delta, entity, None, true) {
            sprite_changed = true;
        }

        if sprite_changed {
            queue_sprite_frame_update(entity, &animation, &mut sprite_state, frame_updates);
        }
        #[cfg(feature = "anim_stats")]
        {
            processed += 1;
        }
    }

    #[cfg(feature = "anim_stats")]
    record_general_bucket_sample(processed);
}
