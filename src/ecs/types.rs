use crate::assets::{
    skeletal::{SkeletalClip, SkeletonAsset},
    AnimationClip, ClipInterpolation, ClipKeyframe, ClipScalarTrack, ClipVec2Track, ClipVec4Track,
};
#[cfg(feature = "anim_stats")]
use crate::ecs::systems::{record_transform_advance_time, record_transform_segment_crosses};
use crate::scene::{MeshLightingData, SceneEntityId};
use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use rapier2d::prelude::{ColliderHandle, RigidBodyHandle};
use std::sync::Arc;
#[cfg(feature = "anim_stats")]
use std::time::Instant;

#[derive(Component, Clone, Copy)]
pub struct Transform {
    pub translation: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
}
impl Default for Transform {
    fn default() -> Self {
        Self { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::splat(1.0) }
    }
}

impl Transform {
    #[inline]
    pub fn to_mat4(&self) -> Mat4 {
        let (sx, sy) = (self.scale.x, self.scale.y);
        let (s, c) = self.rotation.sin_cos();
        Mat4::from_cols_array(&[
            c * sx,
            s * sx,
            0.0,
            0.0,
            -s * sy,
            c * sy,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            self.translation.x,
            self.translation.y,
            0.0,
            1.0,
        ])
    }
}
#[derive(Component, Clone, Copy, Default)]
pub struct WorldTransform(pub Mat4);
#[derive(Component, Clone, Copy, Default)]
pub struct WorldTransform3D(pub Mat4);
#[derive(Component, Clone, Copy)]
pub struct Parent(pub Entity);
#[derive(Component, Default)]
pub struct Children(pub Vec<Entity>);

#[derive(Component, Clone)]
pub struct SceneEntityTag {
    pub id: SceneEntityId,
}

impl SceneEntityTag {
    pub fn new(id: SceneEntityId) -> Self {
        Self { id }
    }
}
#[derive(Component)]
pub struct Spin {
    pub speed: f32,
}
#[derive(Component, Clone)]
pub struct Sprite {
    pub atlas_key: Arc<str>,
    pub region: Arc<str>,
    pub region_id: u16,
    pub uv: [f32; 4],
}

impl Sprite {
    pub const UNINITIALIZED_REGION: u16 = u16::MAX;

    pub fn uninitialized(atlas_key: Arc<str>, region: Arc<str>) -> Self {
        Self { atlas_key, region, region_id: Self::UNINITIALIZED_REGION, uv: [0.0; 4] }
    }

    pub fn apply_frame(&mut self, frame: &SpriteAnimationFrame) {
        if self.region_id == Self::UNINITIALIZED_REGION {
            // Keep the human-readable region name in sync the first time we touch the sprite.
            // Subsequent animation-driven updates only mutate `region_id`/`uv` to avoid an Arc clone per frame.
            self.region = frame.region.clone();
        }
        self.region_id = frame.region_id;
        self.uv = frame.uv;
    }
}

impl Sprite {
    #[inline]
    pub fn is_initialized(&self) -> bool {
        self.region_id != Self::UNINITIALIZED_REGION
    }
}

#[derive(Component, Clone)]
pub struct SpriteFrameState {
    pub region_id: u16,
    pub uv: [f32; 4],
    pub pending_region: Option<Arc<str>>,
    pub region_initialized: bool,
    pub queued_for_apply: bool,
}

impl SpriteFrameState {
    pub fn new_uninitialized() -> Self {
        Self {
            region_id: Sprite::UNINITIALIZED_REGION,
            uv: [0.0; 4],
            pending_region: None,
            region_initialized: false,
            queued_for_apply: false,
        }
    }

    pub fn from_sprite(sprite: &Sprite) -> Self {
        Self {
            region_id: sprite.region_id,
            uv: sprite.uv,
            pending_region: None,
            region_initialized: sprite.is_initialized(),
            queued_for_apply: false,
        }
    }

    pub fn update_from_frame(&mut self, frame: &SpriteAnimationFrame) {
        self.region_id = frame.region_id;
        self.uv = frame.uv;
        self.pending_region = Some(frame.region.clone());
    }

    pub fn update_from_hot_frame(&mut self, frame: &SpriteFrameHotData, region: Option<&Arc<str>>) {
        self.region_id = frame.region_id;
        self.uv = frame.uv;
        if let Some(region) = region {
            self.pending_region = Some(Arc::clone(region));
        }
    }

    pub fn sync_from_sprite(&mut self, sprite: &Sprite) {
        self.region_id = sprite.region_id;
        self.uv = sprite.uv;
        self.region_initialized = sprite.is_initialized();
        if self.region_initialized {
            self.pending_region = None;
        }
        self.queued_for_apply = false;
    }
}

#[derive(Component, Clone)]
pub struct SpriteAnimation {
    pub timeline: Arc<str>,
    pub frames: Arc<[SpriteAnimationFrame]>,
    pub frame_hot_data: Arc<[SpriteFrameHotData]>,
    pub frame_durations: Arc<[f32]>,
    pub frame_offsets: Arc<[f32]>,
    pub total_duration: f32,
    pub total_duration_inv: f32,
    pub current_duration: f32,
    pub current_frame_offset: f32,
    pub frame_index: usize,
    pub elapsed_in_frame: f32,
    pub pending_small_delta: f32,
    pub playing: bool,
    pub looped: bool,
    pub mode: SpriteAnimationLoopMode,
    pub forward: bool,
    pub speed: f32,
    pub start_offset: f32,
    pub random_start: bool,
    pub group: Option<String>,
    pub has_events: bool,
    pub playback_rate: f32,
    pub playback_rate_dirty: bool,
    pub fast_loop: bool,
    pub pending_start_events: bool,
    pub prev_forward: bool,
}

/// Marker used to route animators through the fast-path update loop.
#[derive(Component, Default)]
pub struct FastSpriteAnimator;

impl SpriteAnimation {
    pub fn new(
        timeline: Arc<str>,
        frames: Arc<[SpriteAnimationFrame]>,
        frame_hot_data: Arc<[SpriteFrameHotData]>,
        frame_durations: Arc<[f32]>,
        frame_offsets: Arc<[f32]>,
        total_duration: f32,
        mode: SpriteAnimationLoopMode,
    ) -> Self {
        let duration_inv = if total_duration > 0.0 { 1.0 / total_duration } else { 0.0 };
        let has_events = frames.iter().any(|frame| !frame.events.is_empty());
        let fast_loop = !has_events && matches!(mode, SpriteAnimationLoopMode::Loop);
        let current_duration = frame_durations.first().copied().unwrap_or(0.0);
        let current_frame_offset = frame_offsets.first().copied().unwrap_or(0.0);
        let mut animation = Self {
            timeline,
            frames,
            frame_hot_data,
            frame_durations,
            frame_offsets,
            total_duration,
            total_duration_inv: duration_inv,
            current_duration,
            current_frame_offset,
            frame_index: 0,
            elapsed_in_frame: 0.0,
            pending_small_delta: 0.0,
            playing: true,
            looped: mode.looped(),
            forward: true,
            speed: 1.0,
            mode,
            start_offset: 0.0,
            random_start: false,
            group: None,
            has_events,
            playback_rate: 0.0,
            playback_rate_dirty: true,
            fast_loop,
            pending_start_events: false,
            prev_forward: true,
        };
        animation.refresh_pending_start_events();
        animation
    }

    pub fn set_mode(&mut self, mode: SpriteAnimationLoopMode) {
        self.mode = mode;
        self.looped = mode.looped();
        self.forward = true;
        self.prev_forward = true;
        self.fast_loop = !self.has_events && matches!(self.mode, SpriteAnimationLoopMode::Loop);
        self.refresh_pending_start_events();
    }

    pub fn set_start_offset(&mut self, offset: f32) {
        self.start_offset = offset.max(0.0);
    }

    pub fn set_random_start(&mut self, random: bool) {
        self.random_start = random;
    }

    pub fn set_group<S: Into<Option<String>>>(&mut self, group: S) {
        self.group = group.into();
        self.mark_playback_rate_dirty();
    }

    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    pub fn current_region_name(&self) -> Option<&str> {
        self.frames.get(self.frame_index).map(|frame| frame.region.as_ref())
    }

    pub fn current_region_handle(&self) -> Option<Arc<str>> {
        self.frames.get(self.frame_index).map(|frame| frame.region.clone())
    }

    pub fn current_region_id(&self) -> Option<u16> {
        self.frames.get(self.frame_index).map(|frame| frame.region_id)
    }

    pub fn current_frame(&self) -> Option<&SpriteAnimationFrame> {
        self.frames.get(self.frame_index)
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn total_duration(&self) -> f32 {
        self.total_duration
    }

    pub fn mark_playback_rate_dirty(&mut self) {
        self.playback_rate_dirty = true;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        self.playback_rate_dirty = true;
    }

    pub fn ensure_playback_rate(&mut self, group_scale: f32) -> f32 {
        if self.playback_rate_dirty {
            self.playback_rate = self.speed * group_scale;
            self.playback_rate_dirty = false;
        }
        self.playback_rate
    }

    #[inline]
    pub fn refresh_current_duration(&mut self) {
        if self.frame_index < self.frame_durations.len() {
            self.set_frame_metrics_unchecked(self.frame_index);
        } else {
            self.current_duration = 0.0;
            self.current_frame_offset = 0.0;
        }
    }

    #[inline(always)]
    pub(crate) fn set_frame_metrics_unchecked(&mut self, index: usize) {
        debug_assert!(index < self.frame_durations.len());
        self.frame_index = index;
        unsafe {
            self.current_duration = *self.frame_durations.get_unchecked(index);
            self.current_frame_offset = *self.frame_offsets.get_unchecked(index);
        }
    }

    #[inline]
    pub fn refresh_pending_start_events(&mut self) {
        self.pending_start_events =
            self.has_events && self.current_frame().map(|frame| !frame.events.is_empty()).unwrap_or(false);
    }

    #[inline]
    pub(crate) fn accumulate_delta(&mut self, delta: f32, epsilon: f32) -> Option<f32> {
        if delta == 0.0 {
            return None;
        }
        let mut pending = self.pending_small_delta;
        if pending != 0.0 && pending.signum() != delta.signum() {
            pending = 0.0;
        }
        let total = pending + delta;
        if total.abs() < epsilon {
            self.pending_small_delta = total;
            None
        } else {
            self.pending_small_delta = 0.0;
            Some(total)
        }
    }
}

#[derive(Clone)]
pub struct SpriteAnimationFrame {
    pub name: Arc<str>,
    pub region: Arc<str>,
    pub region_id: u16,
    pub duration: f32,
    pub uv: [f32; 4],
    pub events: Arc<[Arc<str>]>,
}

#[derive(Clone, Copy)]
pub struct SpriteFrameHotData {
    pub region_id: u16,
    pub uv: [f32; 4],
}

#[derive(Clone, Copy, Default)]
pub struct ClipSample {
    pub translation: Option<Vec2>,
    pub rotation: Option<f32>,
    pub scale: Option<Vec2>,
    pub tint: Option<Vec4>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClipChannelMask {
    translation: bool,
    rotation: bool,
    scale: bool,
    tint: bool,
}

impl ClipChannelMask {
    const fn new(translation: bool, rotation: bool, scale: bool, tint: bool) -> Self {
        Self { translation, rotation, scale, tint }
    }

    const fn all() -> Self {
        Self::new(true, true, true, true)
    }

    fn from_players(
        transform: Option<&TransformTrackPlayer>,
        property: Option<&PropertyTrackPlayer>,
    ) -> Self {
        let transform_mask = transform.copied().unwrap_or_default();
        let property_mask = property.copied().unwrap_or_default();
        Self::new(
            transform_mask.apply_translation,
            transform_mask.apply_rotation,
            transform_mask.apply_scale,
            property_mask.apply_tint,
        )
    }

    fn from_clip(clip: &AnimationClip) -> Self {
        Self::new(
            clip.translation.is_some(),
            clip.rotation.is_some(),
            clip.scale.is_some(),
            clip.tint.is_some(),
        )
    }

    fn intersect(self, other: Self) -> Self {
        Self::new(
            self.translation && other.translation,
            self.rotation && other.rotation,
            self.scale && other.scale,
            self.tint && other.tint,
        )
    }

    fn is_empty(self) -> bool {
        !(self.translation || self.rotation || self.scale || self.tint)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct TrackAdvanceResult {
    advanced: bool,
    segment_changed: bool,
    offset_delta: f32,
}

#[derive(Component, Clone)]
pub struct ClipInstance {
    pub clip_key: Arc<str>,
    pub clip: Arc<AnimationClip>,
    pub clip_version: u32,
    pub time: f32,
    pub playing: bool,
    pub looped: bool,
    pub speed: f32,
    pub group: Option<String>,
    pub playback_rate: f32,
    pub playback_rate_dirty: bool,
    pub last_translation: Option<Vec2>,
    pub last_rotation: Option<f32>,
    pub last_scale: Option<Vec2>,
    pub last_tint: Option<Vec4>,
    pub current_sample: ClipSample,
    pub translation_sample_dirty: bool,
    pub rotation_sample_dirty: bool,
    pub scale_sample_dirty: bool,
    pub tint_sample_dirty: bool,
    pub translation_segment_start: Vec2,
    pub translation_segment_slope: Vec2,
    pub rotation_segment_start: f32,
    pub rotation_segment_slope: f32,
    pub scale_segment_start: Vec2,
    pub scale_segment_slope: Vec2,
    pub tint_segment_start: Vec4,
    pub tint_segment_slope: Vec4,
    pub translation_segment_cached_index: usize,
    pub rotation_segment_cached_index: usize,
    pub scale_segment_cached_index: usize,
    pub tint_segment_cached_index: usize,
    pub translation_segment_cache_valid: bool,
    pub rotation_segment_cache_valid: bool,
    pub scale_segment_cache_valid: bool,
    pub tint_segment_cache_valid: bool,
    pub translation_state_current: bool,
    pub rotation_state_current: bool,
    pub scale_state_current: bool,
    pub tint_state_current: bool,
    pub translation_cursor: usize,
    pub rotation_cursor: usize,
    pub scale_cursor: usize,
    pub tint_cursor: usize,
    pub translation_segment_time: f32,
    pub rotation_segment_time: f32,
    pub scale_segment_time: f32,
    pub tint_segment_time: f32,
    pub translation_segment_span: f32,
    pub rotation_segment_span: f32,
    pub scale_segment_span: f32,
    pub tint_segment_span: f32,
    clip_channels: ClipChannelMask,
}

impl ClipInstance {
    pub fn new(clip_key: Arc<str>, clip: Arc<AnimationClip>) -> Self {
        let version = clip.version;
        let looped = clip.looped;
        let clip_channels = ClipChannelMask::from_clip(clip.as_ref());
        let mut instance = Self {
            clip_key,
            clip,
            clip_version: version,
            time: 0.0,
            playing: true,
            looped,
            speed: 1.0,
            group: None,
            playback_rate: 0.0,
            playback_rate_dirty: true,
            last_translation: None,
            last_rotation: None,
            last_scale: None,
            last_tint: None,
            current_sample: ClipSample::default(),
            translation_sample_dirty: true,
            rotation_sample_dirty: true,
            scale_sample_dirty: true,
            tint_sample_dirty: true,
            translation_segment_start: Vec2::ZERO,
            translation_segment_slope: Vec2::ZERO,
            rotation_segment_start: 0.0,
            rotation_segment_slope: 0.0,
            scale_segment_start: Vec2::ZERO,
            scale_segment_slope: Vec2::ZERO,
            tint_segment_start: Vec4::ZERO,
            tint_segment_slope: Vec4::ZERO,
            translation_segment_cached_index: usize::MAX,
            rotation_segment_cached_index: usize::MAX,
            scale_segment_cached_index: usize::MAX,
            tint_segment_cached_index: usize::MAX,
            translation_segment_cache_valid: false,
            rotation_segment_cache_valid: false,
            scale_segment_cache_valid: false,
            tint_segment_cache_valid: false,
            translation_state_current: true,
            rotation_state_current: true,
            scale_state_current: true,
            tint_state_current: true,
            translation_cursor: 0,
            rotation_cursor: 0,
            scale_cursor: 0,
            tint_cursor: 0,
            translation_segment_time: 0.0,
            rotation_segment_time: 0.0,
            scale_segment_time: 0.0,
            tint_segment_time: 0.0,
            translation_segment_span: 0.0,
            rotation_segment_span: 0.0,
            scale_segment_span: 0.0,
            tint_segment_span: 0.0,
            clip_channels,
        };
        instance.rebuild_track_cursors();
        let initial_mask = ClipChannelMask::all().intersect(instance.clip_channels);
        instance.advance_track_states(0.0, initial_mask);
        instance
    }

    pub fn replace_clip(&mut self, clip_key: Arc<str>, clip: Arc<AnimationClip>) {
        let previous_speed = self.speed;
        let previous_group = self.group.clone();
        self.clip_key = clip_key;
        self.clip = clip;
        self.clip_version = self.clip.version;
        self.clip_channels = ClipChannelMask::from_clip(self.clip.as_ref());
        self.looped = self.clip.looped;
        self.time = 0.0;
        self.playing = true;
        self.speed = previous_speed;
        self.group = previous_group;
        self.playback_rate = 0.0;
        self.playback_rate_dirty = true;
        self.last_translation = None;
        self.last_rotation = None;
        self.last_scale = None;
        self.last_tint = None;
        self.clear_current_sample();
        self.translation_segment_start = Vec2::ZERO;
        self.translation_segment_slope = Vec2::ZERO;
        self.rotation_segment_start = 0.0;
        self.rotation_segment_slope = 0.0;
        self.scale_segment_start = Vec2::ZERO;
        self.scale_segment_slope = Vec2::ZERO;
        self.tint_segment_start = Vec4::ZERO;
        self.tint_segment_slope = Vec4::ZERO;
        self.translation_segment_cached_index = usize::MAX;
        self.rotation_segment_cached_index = usize::MAX;
        self.scale_segment_cached_index = usize::MAX;
        self.tint_segment_cached_index = usize::MAX;
        self.translation_segment_cache_valid = false;
        self.rotation_segment_cache_valid = false;
        self.scale_segment_cache_valid = false;
        self.tint_segment_cache_valid = false;
        self.reset_cursors();
        self.rebuild_track_cursors();
        let initial_mask = ClipChannelMask::all().intersect(self.clip_channels);
        self.advance_track_states(0.0, initial_mask);
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    pub fn reset(&mut self) {
        self.time = 0.0;
        self.playing = true;
        self.last_translation = None;
        self.last_rotation = None;
        self.last_scale = None;
        self.last_tint = None;
        self.clear_current_sample();
        self.translation_segment_start = Vec2::ZERO;
        self.translation_segment_slope = Vec2::ZERO;
        self.rotation_segment_start = 0.0;
        self.rotation_segment_slope = 0.0;
        self.scale_segment_start = Vec2::ZERO;
        self.scale_segment_slope = Vec2::ZERO;
        self.tint_segment_start = Vec4::ZERO;
        self.tint_segment_slope = Vec4::ZERO;
        self.translation_segment_cached_index = usize::MAX;
        self.rotation_segment_cached_index = usize::MAX;
        self.scale_segment_cached_index = usize::MAX;
        self.tint_segment_cached_index = usize::MAX;
        self.translation_segment_cache_valid = false;
        self.rotation_segment_cache_valid = false;
        self.scale_segment_cache_valid = false;
        self.tint_segment_cache_valid = false;
        self.reset_cursors();
        self.rebuild_track_cursors();
        let initial_mask = ClipChannelMask::all().intersect(self.clip_channels);
        self.advance_track_states(0.0, initial_mask);
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        self.mark_playback_rate_dirty();
    }

    pub fn set_group(&mut self, group: Option<&str>) {
        self.group = group.map(|g| g.to_string());
        self.mark_playback_rate_dirty();
    }

    pub fn mark_playback_rate_dirty(&mut self) {
        self.playback_rate_dirty = true;
    }

    #[inline(always)]
    fn clear_current_sample(&mut self) {
        self.current_sample = ClipSample::default();
        self.translation_sample_dirty = true;
        self.rotation_sample_dirty = true;
        self.scale_sample_dirty = true;
        self.tint_sample_dirty = true;
    }

    pub fn ensure_playback_rate(&mut self, group_scale: f32) -> f32 {
        if self.playback_rate_dirty {
            self.playback_rate = self.speed * group_scale;
            self.playback_rate_dirty = false;
        }
        self.playback_rate
    }

    #[inline]
    pub fn has_tint_channel(&self) -> bool {
        self.clip_channels.tint
    }

    pub fn advance_time(&mut self, delta: f32) -> f32 {
        self.advance_time_with_mask(delta, ClipChannelMask::all())
    }

    pub fn advance_time_masked(
        &mut self,
        delta: f32,
        transform_mask: Option<&TransformTrackPlayer>,
        property_mask: Option<&PropertyTrackPlayer>,
    ) -> f32 {
        let channel_mask = ClipChannelMask::from_players(transform_mask, property_mask);
        self.advance_time_with_mask(delta, channel_mask)
    }

    fn advance_time_with_mask(&mut self, delta: f32, channel_mask: ClipChannelMask) -> f32 {
        if delta <= 0.0 {
            return 0.0;
        }
        let duration = self.duration();
        if duration <= 0.0 {
            self.time = 0.0;
            let effective_mask = channel_mask.intersect(self.clip_channels);
            self.advance_track_states(0.0, effective_mask);
            return 0.0;
        }

        let previous_time = self.time;
        let applied = if self.looped {
            let mut next = self.time + delta;
            if !next.is_finite() {
                next = 0.0;
            }
            if next >= 0.0 && next < (duration - CLIP_TIME_EPSILON) {
                self.time = next;
                delta
            } else {
                let duration_inv = self.clip.duration_inv;
                let mut wrapped = wrap_time_looped(next, duration, duration_inv);
                if wrapped <= CLIP_TIME_EPSILON || (duration - wrapped) <= CLIP_TIME_EPSILON {
                    wrapped = 0.0;
                }
                self.time = wrapped;
                delta
            }
        } else {
            let mut next = (self.time + delta).min(duration);
            if next >= duration - CLIP_TIME_EPSILON {
                next = duration;
                self.playing = false;
            }
            let applied = (next - self.time).max(0.0);
            self.time = next;
            applied
        };

        if applied <= 0.0 {
            return 0.0;
        }

        let effective_mask = channel_mask.intersect(self.clip_channels);
        if !effective_mask.is_empty() && self.try_fast_channel_advance(previous_time, applied, effective_mask)
        {
            return applied;
        }
        self.advance_track_states(applied, effective_mask);
        applied
    }

    pub fn duration(&self) -> f32 {
        self.clip.duration.max(0.0)
    }

    pub fn set_time(&mut self, time: f32) {
        let duration = self.duration();
        if duration > 0.0 {
            if self.looped {
                let step = duration.max(std::f32::EPSILON);
                let mut wrapped = time.rem_euclid(step);
                if (wrapped - duration).abs() <= CLIP_TIME_EPSILON
                    || (duration - wrapped).abs() <= CLIP_TIME_EPSILON
                {
                    wrapped = duration;
                }
                self.time = wrapped;
            } else {
                let mut clamped = time.clamp(0.0, duration);
                if clamped >= duration - CLIP_TIME_EPSILON {
                    clamped = duration;
                    self.playing = false;
                }
                self.time = clamped;
            }
        } else {
            self.time = 0.0;
        }
        self.reset_cursors();
        self.rebuild_track_cursors();
        let reset_mask = ClipChannelMask::all().intersect(self.clip_channels);
        self.advance_track_states(0.0, reset_mask);
    }

    pub fn sample(&self) -> ClipSample {
        self.sample_at(self.time)
    }

    #[inline(always)]
    pub fn sample_cached(&mut self) -> ClipSample {
        self.current_sample_full()
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    #[inline(always)]
    fn sample_all_tracks(&self) -> ClipSample {
        let translation = self.clip.translation.as_ref().and_then(|track| {
            sample_vec2_track_from_state(track, self.translation_cursor, self.translation_segment_time)
        });
        let rotation = self.clip.rotation.as_ref().and_then(|track| {
            sample_scalar_track_from_state(track, self.rotation_cursor, self.rotation_segment_time)
        });
        let scale = self.clip.scale.as_ref().and_then(|track| {
            sample_vec2_track_from_state(track, self.scale_cursor, self.scale_segment_time)
        });
        let tint =
            self.clip.tint.as_ref().and_then(|track| {
                sample_vec4_track_from_state(track, self.tint_cursor, self.tint_segment_time)
            });
        ClipSample { translation, rotation, scale, tint }
    }

    #[inline(always)]
    pub fn sample_with_masks(
        &mut self,
        transform_mask: Option<TransformTrackPlayer>,
        property_mask: Option<PropertyTrackPlayer>,
    ) -> ClipSample {
        let sample = self.current_sample_masked(transform_mask.as_ref(), property_mask.as_ref());
        #[cfg(debug_assertions)]
        {
            let reference = self.sample_at(self.time);
            let transform_mask = transform_mask.unwrap_or_default();
            let property_mask = property_mask.unwrap_or_default();
            if transform_mask.apply_translation {
                if let (Some(actual), Some(expected)) = (sample.translation, reference.translation) {
                    debug_assert!(
                        (actual - expected).length_squared() <= 1e-5,
                        "current sample translation mismatch: {:?} vs {:?}",
                        actual,
                        expected
                    );
                }
            }
            if transform_mask.apply_rotation {
                if let (Some(actual), Some(expected)) = (sample.rotation, reference.rotation) {
                    debug_assert!(
                        (actual - expected).abs() <= 1e-5,
                        "current sample rotation mismatch: {:?} vs {:?}",
                        actual,
                        expected
                    );
                }
            }
            if transform_mask.apply_scale {
                if let (Some(actual), Some(expected)) = (sample.scale, reference.scale) {
                    debug_assert!(
                        (actual - expected).length_squared() <= 1e-5,
                        "current sample scale mismatch: {:?} vs {:?}",
                        actual,
                        expected
                    );
                }
            }
            if property_mask.apply_tint {
                if let (Some(actual), Some(expected)) = (sample.tint, reference.tint) {
                    debug_assert!(
                        (actual - expected).length_squared() <= 1e-5,
                        "current sample tint mismatch: {:?} vs {:?}",
                        actual,
                        expected
                    );
                }
            }
        }
        sample
    }

    pub fn sample_at(&self, time: f32) -> ClipSample {
        let translation =
            self.clip.translation.as_ref().and_then(|track| sample_vec2_track(track, time, self.looped));
        let rotation =
            self.clip.rotation.as_ref().and_then(|track| sample_scalar_track(track, time, self.looped));
        let scale = self.clip.scale.as_ref().and_then(|track| sample_vec2_track(track, time, self.looped));
        let tint = self.clip.tint.as_ref().and_then(|track| sample_vec4_track(track, time, self.looped));
        ClipSample { translation, rotation, scale, tint }
    }

    #[inline(always)]
    fn reset_cursors(&mut self) {
        self.translation_cursor = 0;
        self.rotation_cursor = 0;
        self.scale_cursor = 0;
        self.tint_cursor = 0;
        self.translation_segment_time = 0.0;
        self.rotation_segment_time = 0.0;
        self.scale_segment_time = 0.0;
        self.tint_segment_time = 0.0;
        self.translation_segment_span = 0.0;
        self.rotation_segment_span = 0.0;
        self.scale_segment_span = 0.0;
        self.tint_segment_span = 0.0;
        self.translation_segment_start = Vec2::ZERO;
        self.translation_segment_slope = Vec2::ZERO;
        self.rotation_segment_start = 0.0;
        self.rotation_segment_slope = 0.0;
        self.scale_segment_start = Vec2::ZERO;
        self.scale_segment_slope = Vec2::ZERO;
        self.tint_segment_start = Vec4::ZERO;
        self.tint_segment_slope = Vec4::ZERO;
        self.translation_segment_cached_index = usize::MAX;
        self.rotation_segment_cached_index = usize::MAX;
        self.scale_segment_cached_index = usize::MAX;
        self.tint_segment_cached_index = usize::MAX;
        self.translation_segment_cache_valid = false;
        self.rotation_segment_cache_valid = false;
        self.scale_segment_cache_valid = false;
        self.tint_segment_cache_valid = false;
        self.translation_state_current = false;
        self.rotation_state_current = false;
        self.scale_state_current = false;
        self.tint_state_current = false;
    }

    fn rebuild_track_cursors(&mut self) {
        let time = self.time;
        self.sync_translation_state_to_time(time);
        self.sync_rotation_state_to_time(time);
        self.sync_scale_state_to_time(time);
        self.sync_tint_state_to_time(time);
    }

    fn sync_translation_state_to_time(&mut self, time: f32) {
        if let Some(track) = self.clip.translation.as_ref() {
            let (cursor, offset) = rebuild_vec2_cursor(track, time, self.looped);
            self.translation_cursor = cursor;
            self.translation_segment_time = offset;
            self.translation_segment_span =
                track.segments.get(cursor).map(|segment| segment.span).unwrap_or(0.0).max(0.0);
            self.translation_segment_start =
                track.keyframes.get(cursor).map(|kf| kf.value).unwrap_or(Vec2::ZERO);
            if let Some(segment) = track.segments.get(cursor) {
                self.translation_segment_slope = segment.slope;
                self.translation_segment_cached_index = cursor;
                self.translation_segment_cache_valid = true;
            } else {
                self.translation_segment_slope = Vec2::ZERO;
                self.translation_segment_cached_index = usize::MAX;
                self.translation_segment_cache_valid = false;
            }
        } else {
            self.translation_cursor = 0;
            self.translation_segment_time = 0.0;
            self.translation_segment_span = 0.0;
            self.translation_segment_start = Vec2::ZERO;
            self.translation_segment_slope = Vec2::ZERO;
            self.translation_segment_cached_index = usize::MAX;
            self.translation_segment_cache_valid = false;
        }
        self.translation_state_current = true;
        self.translation_sample_dirty = true;
    }

    fn sync_rotation_state_to_time(&mut self, time: f32) {
        if let Some(track) = self.clip.rotation.as_ref() {
            let (cursor, offset) = rebuild_scalar_cursor(track, time, self.looped);
            self.rotation_cursor = cursor;
            self.rotation_segment_time = offset;
            self.rotation_segment_span =
                track.segments.get(cursor).map(|segment| segment.span).unwrap_or(0.0).max(0.0);
            self.rotation_segment_start = track.keyframes.get(cursor).map(|kf| kf.value).unwrap_or(0.0);
            if let Some(segment) = track.segments.get(cursor) {
                self.rotation_segment_slope = segment.slope;
                self.rotation_segment_cached_index = cursor;
                self.rotation_segment_cache_valid = true;
            } else {
                self.rotation_segment_slope = 0.0;
                self.rotation_segment_cached_index = usize::MAX;
                self.rotation_segment_cache_valid = false;
            }
        } else {
            self.rotation_cursor = 0;
            self.rotation_segment_time = 0.0;
            self.rotation_segment_span = 0.0;
            self.rotation_segment_start = 0.0;
            self.rotation_segment_slope = 0.0;
            self.rotation_segment_cached_index = usize::MAX;
            self.rotation_segment_cache_valid = false;
        }
        self.rotation_state_current = true;
        self.rotation_sample_dirty = true;
    }

    fn sync_scale_state_to_time(&mut self, time: f32) {
        if let Some(track) = self.clip.scale.as_ref() {
            let (cursor, offset) = rebuild_vec2_cursor(track, time, self.looped);
            self.scale_cursor = cursor;
            self.scale_segment_time = offset;
            self.scale_segment_span =
                track.segments.get(cursor).map(|segment| segment.span).unwrap_or(0.0).max(0.0);
            self.scale_segment_start = track.keyframes.get(cursor).map(|kf| kf.value).unwrap_or(Vec2::ZERO);
            if let Some(segment) = track.segments.get(cursor) {
                self.scale_segment_slope = segment.slope;
                self.scale_segment_cached_index = cursor;
                self.scale_segment_cache_valid = true;
            } else {
                self.scale_segment_slope = Vec2::ZERO;
                self.scale_segment_cached_index = usize::MAX;
                self.scale_segment_cache_valid = false;
            }
        } else {
            self.scale_cursor = 0;
            self.scale_segment_time = 0.0;
            self.scale_segment_span = 0.0;
            self.scale_segment_start = Vec2::ZERO;
            self.scale_segment_slope = Vec2::ZERO;
            self.scale_segment_cached_index = usize::MAX;
            self.scale_segment_cache_valid = false;
        }
        self.scale_state_current = true;
        self.scale_sample_dirty = true;
    }

    fn sync_tint_state_to_time(&mut self, time: f32) {
        if let Some(track) = self.clip.tint.as_ref() {
            let (cursor, offset) = rebuild_vec4_cursor(track, time, self.looped);
            self.tint_cursor = cursor;
            self.tint_segment_time = offset;
            self.tint_segment_span =
                track.segments.get(cursor).map(|segment| segment.span).unwrap_or(0.0).max(0.0);
            self.tint_segment_start = track.keyframes.get(cursor).map(|kf| kf.value).unwrap_or(Vec4::ZERO);
            if let Some(segment) = track.segments.get(cursor) {
                self.tint_segment_slope = segment.slope;
                self.tint_segment_cached_index = cursor;
                self.tint_segment_cache_valid = true;
            } else {
                self.tint_segment_slope = Vec4::ZERO;
                self.tint_segment_cached_index = usize::MAX;
                self.tint_segment_cache_valid = false;
            }
        } else {
            self.tint_cursor = 0;
            self.tint_segment_time = 0.0;
            self.tint_segment_span = 0.0;
            self.tint_segment_start = Vec4::ZERO;
            self.tint_segment_slope = Vec4::ZERO;
            self.tint_segment_cached_index = usize::MAX;
            self.tint_segment_cache_valid = false;
        }
        self.tint_state_current = true;
        self.tint_sample_dirty = true;
    }

    #[inline(always)]
    fn advance_vec2_track_state(
        track: &ClipVec2Track,
        mut delta: f32,
        looped: bool,
        cursor: &mut usize,
        segment_time: &mut f32,
        segment_span: &mut f32,
        segment_start: &mut Vec2,
        segment_slope: &mut Vec2,
        cached_index: &mut usize,
        cache_valid: &mut bool,
    ) -> TrackAdvanceResult {
        let mut result = TrackAdvanceResult::default();
        let keyframes = track.keyframes.as_ref();
        let segments = track.segments.as_ref();
        let segment_offsets = track.segment_offsets.as_ref();
        let frame_count = keyframes.len();
        if frame_count == 0 {
            *cursor = 0;
            *segment_time = 0.0;
            *segment_span = 0.0;
            *cached_index = usize::MAX;
            *cache_valid = false;
            return result;
        }
        if frame_count == 1 || segments.is_empty() {
            let value = keyframes[0].value;
            *cursor = 0;
            *segment_time = 0.0;
            *segment_span = 0.0;
            *segment_start = value;
            *segment_slope = Vec2::ZERO;
            *cached_index = 0;
            *cache_valid = true;
            return result;
        }

        let segment_count = segments.len();
        let last_segment = segment_count.saturating_sub(1);
        let index = (*cursor).min(last_segment);
        *cursor = index;

        if !*cache_valid || *cached_index != index {
            unsafe {
                let start_kf = keyframes.get_unchecked(index);
                let segment = segments.get_unchecked(index);
                *segment_start = start_kf.value;
                *segment_slope = segment.slope;
            }
            *cached_index = index;
            *cache_valid = true;
        }

        let mut span = if *segment_span > 0.0 {
            (*segment_span).max(0.0)
        } else {
            segments.get(index).map(|seg| seg.span).unwrap_or(0.0).max(0.0)
        };
        if !span.is_finite() {
            span = 0.0;
        }
        *segment_span = span;

        let mut offset = (*segment_time).clamp(0.0, span);
        let initial_cursor = index;
        let initial_offset = offset;

        if delta <= 0.0 {
            *segment_time = offset;
            return result;
        }

        if looped && track.duration > 0.0 && delta >= track.duration {
            let duration_eps = track.duration.max(std::f32::EPSILON);
            delta = delta.rem_euclid(duration_eps);
            if delta <= 0.0 {
                *segment_time = offset;
                return result;
            }
        }

        result.advanced = true;
        let remaining = if span > offset { span - offset } else { 0.0 };
        if span <= 0.0 || delta <= remaining {
            if span > 0.0 {
                offset = (offset + delta).min(span);
            } else {
                offset = 0.0;
            }
            *segment_time = offset;
            result.segment_changed = index != initial_cursor;
            if !result.segment_changed {
                result.offset_delta = offset - initial_offset;
            }
            return result;
        }

        let start_time = segment_offsets
            .get(index)
            .copied()
            .unwrap_or_else(|| keyframes.get(index).map(|kf| kf.time).unwrap_or(0.0));
        let current_absolute = start_time + offset;
        let original_delta = delta;
        let (target_time, target_cycle) = resolve_target_time(
            current_absolute + original_delta,
            track.duration,
            track.duration_inv,
            looped,
        );
        #[cfg(not(feature = "anim_stats"))]
        let _ = target_cycle;

        let new_index = locate_segment_index(segment_offsets, target_time, last_segment);
        let new_start = segment_offsets
            .get(new_index)
            .copied()
            .unwrap_or_else(|| keyframes.get(new_index).map(|kf| kf.time).unwrap_or(0.0));
        let mut new_span = segments.get(new_index).map(|seg| seg.span).unwrap_or(0.0).max(0.0);
        if !new_span.is_finite() {
            new_span = 0.0;
        }
        let mut new_offset = (target_time - new_start).clamp(0.0, new_span);
        if new_span <= 0.0 {
            new_offset = 0.0;
        }

        unsafe {
            let start_kf = keyframes.get_unchecked(new_index);
            let segment = segments.get_unchecked(new_index);
            *segment_start = start_kf.value;
            *segment_slope = segment.slope;
        }
        *cached_index = new_index;
        *cache_valid = true;
        *cursor = new_index;
        *segment_span = new_span;
        *segment_time = new_offset;

        result.segment_changed = new_index != initial_cursor;
        if !result.segment_changed {
            result.offset_delta = new_offset - initial_offset;
        }

        #[cfg(feature = "anim_stats")]
        {
            let crosses =
                compute_segment_crosses(segment_count, initial_cursor, new_index, target_cycle, looped);
            if crosses > 0 {
                record_transform_segment_crosses(crosses);
            }
        }

        result
    }

    #[inline(always)]
    fn advance_scalar_track_state(
        track: &ClipScalarTrack,
        mut delta: f32,
        looped: bool,
        cursor: &mut usize,
        segment_time: &mut f32,
        segment_span: &mut f32,
        segment_start: &mut f32,
        segment_slope: &mut f32,
        cached_index: &mut usize,
        cache_valid: &mut bool,
    ) -> TrackAdvanceResult {
        let mut result = TrackAdvanceResult::default();
        let keyframes = track.keyframes.as_ref();
        let segments = track.segments.as_ref();
        let segment_offsets = track.segment_offsets.as_ref();
        let frame_count = keyframes.len();
        if frame_count == 0 {
            *cursor = 0;
            *segment_time = 0.0;
            *segment_span = 0.0;
            *cached_index = usize::MAX;
            *cache_valid = false;
            return result;
        }
        if frame_count == 1 || segments.is_empty() {
            let value = keyframes[0].value;
            *cursor = 0;
            *segment_time = 0.0;
            *segment_span = 0.0;
            *segment_start = value;
            *segment_slope = 0.0;
            *cached_index = 0;
            *cache_valid = true;
            return result;
        }

        let segment_count = segments.len();
        let last_segment = segment_count.saturating_sub(1);
        let index = (*cursor).min(last_segment);
        *cursor = index;

        if !*cache_valid || *cached_index != index {
            unsafe {
                let start_kf = keyframes.get_unchecked(index);
                let segment = segments.get_unchecked(index);
                *segment_start = start_kf.value;
                *segment_slope = segment.slope;
            }
            *cached_index = index;
            *cache_valid = true;
        }

        let mut span = if *segment_span > 0.0 {
            (*segment_span).max(0.0)
        } else {
            segments.get(index).map(|seg| seg.span).unwrap_or(0.0).max(0.0)
        };
        if !span.is_finite() {
            span = 0.0;
        }
        *segment_span = span;

        let mut offset = (*segment_time).clamp(0.0, span);
        let initial_cursor = index;
        let initial_offset = offset;

        if delta <= 0.0 {
            *segment_time = offset;
            return result;
        }

        if looped && track.duration > 0.0 && delta >= track.duration {
            let duration_eps = track.duration.max(std::f32::EPSILON);
            delta = delta.rem_euclid(duration_eps);
            if delta <= 0.0 {
                *segment_time = offset;
                return result;
            }
        }

        result.advanced = true;
        let remaining = if span > offset { span - offset } else { 0.0 };
        if span <= 0.0 || delta <= remaining {
            if span > 0.0 {
                offset = (offset + delta).min(span);
            } else {
                offset = 0.0;
            }
            *segment_time = offset;
            result.segment_changed = index != initial_cursor;
            if !result.segment_changed {
                result.offset_delta = offset - initial_offset;
            }
            return result;
        }

        let start_time = segment_offsets
            .get(index)
            .copied()
            .unwrap_or_else(|| keyframes.get(index).map(|kf| kf.time).unwrap_or(0.0));
        let current_absolute = start_time + offset;
        let original_delta = delta;
        let (target_time, target_cycle) = resolve_target_time(
            current_absolute + original_delta,
            track.duration,
            track.duration_inv,
            looped,
        );
        #[cfg(not(feature = "anim_stats"))]
        let _ = target_cycle;

        let new_index = locate_segment_index(segment_offsets, target_time, last_segment);
        let new_start = segment_offsets
            .get(new_index)
            .copied()
            .unwrap_or_else(|| keyframes.get(new_index).map(|kf| kf.time).unwrap_or(0.0));
        let mut new_span = segments.get(new_index).map(|seg| seg.span).unwrap_or(0.0).max(0.0);
        if !new_span.is_finite() {
            new_span = 0.0;
        }
        let mut new_offset = (target_time - new_start).clamp(0.0, new_span);
        if new_span <= 0.0 {
            new_offset = 0.0;
        }

        unsafe {
            let start_kf = keyframes.get_unchecked(new_index);
            let segment = segments.get_unchecked(new_index);
            *segment_start = start_kf.value;
            *segment_slope = segment.slope;
        }
        *cached_index = new_index;
        *cache_valid = true;
        *cursor = new_index;
        *segment_span = new_span;
        *segment_time = new_offset;

        result.segment_changed = new_index != initial_cursor;
        if !result.segment_changed {
            result.offset_delta = new_offset - initial_offset;
        }

        #[cfg(feature = "anim_stats")]
        {
            let crosses =
                compute_segment_crosses(segment_count, initial_cursor, new_index, target_cycle, looped);
            if crosses > 0 {
                record_transform_segment_crosses(crosses);
            }
        }

        result
    }

    #[inline(always)]
    fn advance_vec4_track_state(
        track: &ClipVec4Track,
        mut delta: f32,
        looped: bool,
        cursor: &mut usize,
        segment_time: &mut f32,
        segment_span: &mut f32,
        segment_start: &mut Vec4,
        segment_slope: &mut Vec4,
        cached_index: &mut usize,
        cache_valid: &mut bool,
    ) -> TrackAdvanceResult {
        let mut result = TrackAdvanceResult::default();
        let keyframes = track.keyframes.as_ref();
        let segments = track.segments.as_ref();
        let segment_offsets = track.segment_offsets.as_ref();
        let frame_count = keyframes.len();
        if frame_count == 0 {
            *cursor = 0;
            *segment_time = 0.0;
            *segment_span = 0.0;
            *cached_index = usize::MAX;
            *cache_valid = false;
            return result;
        }
        if frame_count == 1 || segments.is_empty() {
            let value = keyframes[0].value;
            *cursor = 0;
            *segment_time = 0.0;
            *segment_span = 0.0;
            *segment_start = value;
            *segment_slope = Vec4::ZERO;
            *cached_index = 0;
            *cache_valid = true;
            return result;
        }

        let segment_count = segments.len();
        let last_segment = segment_count.saturating_sub(1);
        let index = (*cursor).min(last_segment);
        *cursor = index;

        if !*cache_valid || *cached_index != index {
            unsafe {
                let start_kf = keyframes.get_unchecked(index);
                let segment = segments.get_unchecked(index);
                *segment_start = start_kf.value;
                *segment_slope = segment.slope;
            }
            *cached_index = index;
            *cache_valid = true;
        }

        let mut span = if *segment_span > 0.0 {
            (*segment_span).max(0.0)
        } else {
            segments.get(index).map(|seg| seg.span).unwrap_or(0.0).max(0.0)
        };
        if !span.is_finite() {
            span = 0.0;
        }
        *segment_span = span;

        let mut offset = (*segment_time).clamp(0.0, span);
        let initial_cursor = index;
        let initial_offset = offset;

        if delta <= 0.0 {
            *segment_time = offset;
            return result;
        }

        if looped && track.duration > 0.0 && delta >= track.duration {
            let duration_eps = track.duration.max(std::f32::EPSILON);
            delta = delta.rem_euclid(duration_eps);
            if delta <= 0.0 {
                *segment_time = offset;
                return result;
            }
        }

        result.advanced = true;
        let remaining = if span > offset { span - offset } else { 0.0 };
        if span <= 0.0 || delta <= remaining {
            if span > 0.0 {
                offset = (offset + delta).min(span);
            } else {
                offset = 0.0;
            }
            *segment_time = offset;
            result.segment_changed = index != initial_cursor;
            if !result.segment_changed {
                result.offset_delta = offset - initial_offset;
            }
            return result;
        }

        let start_time = segment_offsets
            .get(index)
            .copied()
            .unwrap_or_else(|| keyframes.get(index).map(|kf| kf.time).unwrap_or(0.0));
        let current_absolute = start_time + offset;
        let original_delta = delta;
        let (target_time, target_cycle) = resolve_target_time(
            current_absolute + original_delta,
            track.duration,
            track.duration_inv,
            looped,
        );
        #[cfg(not(feature = "anim_stats"))]
        let _ = target_cycle;

        let new_index = locate_segment_index(segment_offsets, target_time, last_segment);
        let new_start = segment_offsets
            .get(new_index)
            .copied()
            .unwrap_or_else(|| keyframes.get(new_index).map(|kf| kf.time).unwrap_or(0.0));
        let mut new_span = segments.get(new_index).map(|seg| seg.span).unwrap_or(0.0).max(0.0);
        if !new_span.is_finite() {
            new_span = 0.0;
        }
        let mut new_offset = (target_time - new_start).clamp(0.0, new_span);
        if new_span <= 0.0 {
            new_offset = 0.0;
        }

        unsafe {
            let start_kf = keyframes.get_unchecked(new_index);
            let segment = segments.get_unchecked(new_index);
            *segment_start = start_kf.value;
            *segment_slope = segment.slope;
        }
        *cached_index = new_index;
        *cache_valid = true;
        *cursor = new_index;
        *segment_span = new_span;
        *segment_time = new_offset;

        result.segment_changed = new_index != initial_cursor;
        if !result.segment_changed {
            result.offset_delta = new_offset - initial_offset;
        }

        #[cfg(feature = "anim_stats")]
        {
            let crosses =
                compute_segment_crosses(segment_count, initial_cursor, new_index, target_cycle, looped);
            if crosses > 0 {
                record_transform_segment_crosses(crosses);
            }
        }

        result
    }

    #[inline(always)]
    fn sample_vec2_from_cached(
        track: &ClipVec2Track,
        index: usize,
        offset: f32,
        span: f32,
        start: Vec2,
        slope: Vec2,
    ) -> Vec2 {
        if matches!(track.interpolation, ClipInterpolation::Step) {
            if span <= 0.0 || offset >= span {
                track.keyframes.as_ref().get(index + 1).map(|kf| kf.value).unwrap_or(start)
            } else {
                start
            }
        } else if span > 0.0 {
            start + slope * offset.clamp(0.0, span)
        } else {
            start
        }
    }

    #[inline(always)]
    fn sample_scalar_from_cached(
        track: &ClipScalarTrack,
        index: usize,
        offset: f32,
        span: f32,
        start: f32,
        slope: f32,
    ) -> f32 {
        if matches!(track.interpolation, ClipInterpolation::Step) {
            if span <= 0.0 || offset >= span {
                track.keyframes.as_ref().get(index + 1).map(|kf| kf.value).unwrap_or(start)
            } else {
                start
            }
        } else if span > 0.0 {
            start + slope * offset.clamp(0.0, span)
        } else {
            start
        }
    }

    #[inline(always)]
    fn sample_vec4_from_cached(
        track: &ClipVec4Track,
        index: usize,
        offset: f32,
        span: f32,
        start: Vec4,
        slope: Vec4,
    ) -> Vec4 {
        if matches!(track.interpolation, ClipInterpolation::Step) {
            if span <= 0.0 || offset >= span {
                track.keyframes.as_ref().get(index + 1).map(|kf| kf.value).unwrap_or(start)
            } else {
                start
            }
        } else if span > 0.0 {
            start + slope * offset.clamp(0.0, span)
        } else {
            start
        }
    }

    #[inline(always)]
    fn sample_vec2_from_state_cached(
        track: &ClipVec2Track,
        cursor: usize,
        segment_time: f32,
        segment_span: f32,
        segment_start: Vec2,
        segment_slope: Vec2,
        cached_index: usize,
        cache_valid: bool,
    ) -> Option<Vec2> {
        if cache_valid && cached_index == cursor {
            let span = segment_span.max(0.0);
            let offset = if span > 0.0 { segment_time.clamp(0.0, span) } else { 0.0 };
            Some(Self::sample_vec2_from_cached(track, cursor, offset, span, segment_start, segment_slope))
        } else {
            sample_vec2_track_from_state(track, cursor, segment_time)
        }
    }

    #[inline(always)]
    fn sample_scalar_from_state_cached(
        track: &ClipScalarTrack,
        cursor: usize,
        segment_time: f32,
        segment_span: f32,
        segment_start: f32,
        segment_slope: f32,
        cached_index: usize,
        cache_valid: bool,
    ) -> Option<f32> {
        if cache_valid && cached_index == cursor {
            let span = segment_span.max(0.0);
            let offset = if span > 0.0 { segment_time.clamp(0.0, span) } else { 0.0 };
            Some(Self::sample_scalar_from_cached(track, cursor, offset, span, segment_start, segment_slope))
        } else {
            sample_scalar_track_from_state(track, cursor, segment_time)
        }
    }

    #[inline(always)]
    fn sample_vec4_from_state_cached(
        track: &ClipVec4Track,
        cursor: usize,
        segment_time: f32,
        segment_span: f32,
        segment_start: Vec4,
        segment_slope: Vec4,
        cached_index: usize,
        cache_valid: bool,
    ) -> Option<Vec4> {
        if cache_valid && cached_index == cursor {
            let span = segment_span.max(0.0);
            let offset = if span > 0.0 { segment_time.clamp(0.0, span) } else { 0.0 };
            Some(Self::sample_vec4_from_cached(track, cursor, offset, span, segment_start, segment_slope))
        } else {
            sample_vec4_track_from_state(track, cursor, segment_time)
        }
    }

    fn advance_track_states(&mut self, delta: f32, mask: ClipChannelMask) {
        #[cfg(feature = "anim_stats")]
        let advance_timer = Instant::now();
        let looped = self.looped;
        {
            if !self.clip_channels.translation {
                self.translation_cursor = 0;
                self.translation_segment_time = 0.0;
                self.translation_segment_span = 0.0;
                self.translation_segment_start = Vec2::ZERO;
                self.translation_segment_slope = Vec2::ZERO;
                self.translation_segment_cached_index = usize::MAX;
                self.translation_segment_cache_valid = false;
                self.translation_state_current = true;
                self.translation_sample_dirty = true;
                self.current_sample.translation = None;
            } else if let Some(track) = self.clip.translation.as_ref() {
                if mask.translation {
                    if !self.translation_state_current {
                        self.sync_translation_state_to_time(self.time);
                    } else if delta > 0.0 {
                        let sample_was_clean = !self.translation_sample_dirty;
                        let result = Self::advance_vec2_track_state(
                            track,
                            delta,
                            looped,
                            &mut self.translation_cursor,
                            &mut self.translation_segment_time,
                            &mut self.translation_segment_span,
                            &mut self.translation_segment_start,
                            &mut self.translation_segment_slope,
                            &mut self.translation_segment_cached_index,
                            &mut self.translation_segment_cache_valid,
                        );
                        if result.advanced {
                            let mut kept_clean = false;
                            if sample_was_clean && !result.segment_changed {
                                if let Some(value) = self.current_sample.translation.as_mut() {
                                    if matches!(track.interpolation, ClipInterpolation::Step) {
                                        kept_clean = true;
                                    } else if result.offset_delta != 0.0 {
                                        *value += self.translation_segment_slope * result.offset_delta;
                                        kept_clean = true;
                                    } else {
                                        kept_clean = true;
                                    }
                                }
                            }
                            self.translation_sample_dirty = !kept_clean;
                        }
                    }
                } else if delta > 0.0 {
                    self.translation_state_current = false;
                    self.translation_sample_dirty = true;
                }
            } else {
                self.translation_cursor = 0;
                self.translation_segment_time = 0.0;
                self.translation_segment_span = 0.0;
                self.translation_segment_start = Vec2::ZERO;
                self.translation_segment_slope = Vec2::ZERO;
                self.translation_segment_cached_index = usize::MAX;
                self.translation_segment_cache_valid = false;
                self.translation_state_current = true;
                self.translation_sample_dirty = true;
                self.current_sample.translation = None;
            }

            if !self.clip_channels.rotation {
                self.rotation_cursor = 0;
                self.rotation_segment_time = 0.0;
                self.rotation_segment_span = 0.0;
                self.rotation_segment_start = 0.0;
                self.rotation_segment_slope = 0.0;
                self.rotation_segment_cached_index = usize::MAX;
                self.rotation_segment_cache_valid = false;
                self.rotation_state_current = true;
                self.rotation_sample_dirty = true;
                self.current_sample.rotation = None;
            } else if let Some(track) = self.clip.rotation.as_ref() {
                if mask.rotation {
                    if !self.rotation_state_current {
                        self.sync_rotation_state_to_time(self.time);
                    } else if delta > 0.0 {
                        let sample_was_clean = !self.rotation_sample_dirty;
                        let result = Self::advance_scalar_track_state(
                            track,
                            delta,
                            looped,
                            &mut self.rotation_cursor,
                            &mut self.rotation_segment_time,
                            &mut self.rotation_segment_span,
                            &mut self.rotation_segment_start,
                            &mut self.rotation_segment_slope,
                            &mut self.rotation_segment_cached_index,
                            &mut self.rotation_segment_cache_valid,
                        );
                        if result.advanced {
                            let mut kept_clean = false;
                            if sample_was_clean && !result.segment_changed {
                                if let Some(value) = self.current_sample.rotation.as_mut() {
                                    if matches!(track.interpolation, ClipInterpolation::Step) {
                                        kept_clean = true;
                                    } else if result.offset_delta != 0.0 {
                                        *value += self.rotation_segment_slope * result.offset_delta;
                                        kept_clean = true;
                                    } else {
                                        kept_clean = true;
                                    }
                                }
                            }
                            self.rotation_sample_dirty = !kept_clean;
                        }
                    }
                } else if delta > 0.0 {
                    self.rotation_state_current = false;
                    self.rotation_sample_dirty = true;
                }
            } else {
                self.rotation_cursor = 0;
                self.rotation_segment_time = 0.0;
                self.rotation_segment_span = 0.0;
                self.rotation_segment_start = 0.0;
                self.rotation_segment_slope = 0.0;
                self.rotation_segment_cached_index = usize::MAX;
                self.rotation_segment_cache_valid = false;
                self.rotation_state_current = true;
                self.rotation_sample_dirty = true;
                self.current_sample.rotation = None;
            }

            if !self.clip_channels.scale {
                self.scale_cursor = 0;
                self.scale_segment_time = 0.0;
                self.scale_segment_span = 0.0;
                self.scale_segment_start = Vec2::ZERO;
                self.scale_segment_slope = Vec2::ZERO;
                self.scale_segment_cached_index = usize::MAX;
                self.scale_segment_cache_valid = false;
                self.scale_state_current = true;
                self.scale_sample_dirty = true;
                self.current_sample.scale = None;
            } else if let Some(track) = self.clip.scale.as_ref() {
                if mask.scale {
                    if !self.scale_state_current {
                        self.sync_scale_state_to_time(self.time);
                    } else if delta > 0.0 {
                        let sample_was_clean = !self.scale_sample_dirty;
                        let result = Self::advance_vec2_track_state(
                            track,
                            delta,
                            looped,
                            &mut self.scale_cursor,
                            &mut self.scale_segment_time,
                            &mut self.scale_segment_span,
                            &mut self.scale_segment_start,
                            &mut self.scale_segment_slope,
                            &mut self.scale_segment_cached_index,
                            &mut self.scale_segment_cache_valid,
                        );
                        if result.advanced {
                            let mut kept_clean = false;
                            if sample_was_clean && !result.segment_changed {
                                if let Some(value) = self.current_sample.scale.as_mut() {
                                    if matches!(track.interpolation, ClipInterpolation::Step) {
                                        kept_clean = true;
                                    } else if result.offset_delta != 0.0 {
                                        *value += self.scale_segment_slope * result.offset_delta;
                                        kept_clean = true;
                                    } else {
                                        kept_clean = true;
                                    }
                                }
                            }
                            self.scale_sample_dirty = !kept_clean;
                        }
                    }
                } else if delta > 0.0 {
                    self.scale_state_current = false;
                    self.scale_sample_dirty = true;
                }
            } else {
                self.scale_cursor = 0;
                self.scale_segment_time = 0.0;
                self.scale_segment_span = 0.0;
                self.scale_segment_start = Vec2::ZERO;
                self.scale_segment_slope = Vec2::ZERO;
                self.scale_segment_cached_index = usize::MAX;
                self.scale_segment_cache_valid = false;
                self.scale_state_current = true;
                self.scale_sample_dirty = true;
                self.current_sample.scale = None;
            }

            if !self.clip_channels.tint {
                self.tint_cursor = 0;
                self.tint_segment_time = 0.0;
                self.tint_segment_span = 0.0;
                self.tint_segment_start = Vec4::ZERO;
                self.tint_segment_slope = Vec4::ZERO;
                self.tint_segment_cached_index = usize::MAX;
                self.tint_segment_cache_valid = false;
                self.tint_state_current = true;
                self.tint_sample_dirty = true;
                self.current_sample.tint = None;
            } else if let Some(track) = self.clip.tint.as_ref() {
                if mask.tint {
                    if !self.tint_state_current {
                        self.sync_tint_state_to_time(self.time);
                    } else if delta > 0.0 {
                        let sample_was_clean = !self.tint_sample_dirty;
                        let result = Self::advance_vec4_track_state(
                            track,
                            delta,
                            looped,
                            &mut self.tint_cursor,
                            &mut self.tint_segment_time,
                            &mut self.tint_segment_span,
                            &mut self.tint_segment_start,
                            &mut self.tint_segment_slope,
                            &mut self.tint_segment_cached_index,
                            &mut self.tint_segment_cache_valid,
                        );
                        if result.advanced {
                            let mut kept_clean = false;
                            if sample_was_clean && !result.segment_changed {
                                if let Some(value) = self.current_sample.tint.as_mut() {
                                    if matches!(track.interpolation, ClipInterpolation::Step) {
                                        kept_clean = true;
                                    } else if result.offset_delta != 0.0 {
                                        *value += self.tint_segment_slope * result.offset_delta;
                                        kept_clean = true;
                                    } else {
                                        kept_clean = true;
                                    }
                                }
                            }
                            self.tint_sample_dirty = !kept_clean;
                        }
                    }
                } else if delta > 0.0 {
                    self.tint_state_current = false;
                    self.tint_sample_dirty = true;
                }
            } else {
                self.tint_cursor = 0;
                self.tint_segment_time = 0.0;
                self.tint_segment_span = 0.0;
                self.tint_segment_start = Vec4::ZERO;
                self.tint_segment_slope = Vec4::ZERO;
                self.tint_segment_cached_index = usize::MAX;
                self.tint_segment_cache_valid = false;
                self.tint_state_current = true;
                self.tint_sample_dirty = true;
                self.current_sample.tint = None;
            }
        }

        #[cfg(debug_assertions)]
        self.debug_verify_current_values();

        #[cfg(feature = "anim_stats")]
        record_transform_advance_time(advance_timer.elapsed());
    }

    fn try_fast_channel_advance(&mut self, previous_time: f32, delta: f32, mask: ClipChannelMask) -> bool {
        if delta <= 0.0 {
            return true;
        }
        if mask.is_empty() {
            self.invalidate_unmasked_channels(delta, mask);
            return true;
        }
        let duration = self.duration();
        if duration <= 0.0 {
            return true;
        }
        let target_time = previous_time + delta;
        if !target_time.is_finite() || target_time >= duration - CLIP_TIME_EPSILON {
            return false;
        }
        if !self.fast_channel_segments_available(mask, delta) {
            return false;
        }
        self.apply_fast_channel_delta(delta, mask);
        true
    }

    fn fast_channel_segments_available(&self, mask: ClipChannelMask, delta: f32) -> bool {
        if mask.translation && self.clip_channels.translation {
            if !self.translation_state_current
                || !Self::fast_segment_has_room(
                    self.translation_segment_span,
                    self.translation_segment_time,
                    delta,
                )
            {
                return false;
            }
        }
        if mask.rotation && self.clip_channels.rotation {
            if !self.rotation_state_current
                || !Self::fast_segment_has_room(self.rotation_segment_span, self.rotation_segment_time, delta)
            {
                return false;
            }
        }
        if mask.scale && self.clip_channels.scale {
            if !self.scale_state_current
                || !Self::fast_segment_has_room(self.scale_segment_span, self.scale_segment_time, delta)
            {
                return false;
            }
        }
        if mask.tint && self.clip_channels.tint {
            if !self.tint_state_current
                || !Self::fast_segment_has_room(self.tint_segment_span, self.tint_segment_time, delta)
            {
                return false;
            }
        }
        true
    }

    #[inline(always)]
    fn fast_segment_has_room(span: f32, time: f32, delta: f32) -> bool {
        let span = span.max(0.0);
        if span <= 0.0 {
            return false;
        }
        let offset = time.clamp(0.0, span);
        let remaining = (span - offset).max(0.0);
        remaining > CLIP_TIME_EPSILON && delta <= remaining - CLIP_TIME_EPSILON
    }

    fn apply_fast_channel_delta(&mut self, delta: f32, mask: ClipChannelMask) {
        if mask.translation && self.clip_channels.translation {
            self.translation_segment_time =
                Self::fast_new_offset(self.translation_segment_time, self.translation_segment_span, delta);
            self.fast_update_translation_value(delta);
        }
        if mask.rotation && self.clip_channels.rotation {
            self.rotation_segment_time =
                Self::fast_new_offset(self.rotation_segment_time, self.rotation_segment_span, delta);
            self.fast_update_rotation_value(delta);
        }
        if mask.scale && self.clip_channels.scale {
            self.scale_segment_time =
                Self::fast_new_offset(self.scale_segment_time, self.scale_segment_span, delta);
            self.fast_update_scale_value(delta);
        }
        if mask.tint && self.clip_channels.tint {
            self.tint_segment_time =
                Self::fast_new_offset(self.tint_segment_time, self.tint_segment_span, delta);
            self.fast_update_tint_value(delta);
        }
        self.invalidate_unmasked_channels(delta, mask);
    }

    fn invalidate_unmasked_channels(&mut self, delta: f32, mask: ClipChannelMask) {
        if delta <= 0.0 {
            return;
        }
        if self.clip_channels.translation && !mask.translation {
            self.translation_state_current = false;
            self.translation_sample_dirty = true;
        }
        if self.clip_channels.rotation && !mask.rotation {
            self.rotation_state_current = false;
            self.rotation_sample_dirty = true;
        }
        if self.clip_channels.scale && !mask.scale {
            self.scale_state_current = false;
            self.scale_sample_dirty = true;
        }
        if self.clip_channels.tint && !mask.tint {
            self.tint_state_current = false;
            self.tint_sample_dirty = true;
        }
    }

    #[inline(always)]
    fn fast_new_offset(current: f32, span: f32, delta: f32) -> f32 {
        let span = span.max(0.0);
        if span <= 0.0 {
            return 0.0;
        }
        let offset = current.clamp(0.0, span);
        (offset + delta).min(span)
    }

    fn fast_update_translation_value(&mut self, delta: f32) {
        if self.translation_sample_dirty {
            return;
        }
        let Some(track) = self.clip.translation.as_ref() else {
            self.translation_sample_dirty = true;
            return;
        };
        if matches!(track.interpolation, ClipInterpolation::Step) {
            return;
        }
        if let Some(value) = self.current_sample.translation.as_mut() {
            *value += self.translation_segment_slope * delta;
        } else {
            self.translation_sample_dirty = true;
        }
    }

    fn fast_update_rotation_value(&mut self, delta: f32) {
        if self.rotation_sample_dirty {
            return;
        }
        let Some(track) = self.clip.rotation.as_ref() else {
            self.rotation_sample_dirty = true;
            return;
        };
        if matches!(track.interpolation, ClipInterpolation::Step) {
            return;
        }
        if let Some(value) = self.current_sample.rotation.as_mut() {
            *value += self.rotation_segment_slope * delta;
        } else {
            self.rotation_sample_dirty = true;
        }
    }

    fn fast_update_scale_value(&mut self, delta: f32) {
        if self.scale_sample_dirty {
            return;
        }
        let Some(track) = self.clip.scale.as_ref() else {
            self.scale_sample_dirty = true;
            return;
        };
        if matches!(track.interpolation, ClipInterpolation::Step) {
            return;
        }
        if let Some(value) = self.current_sample.scale.as_mut() {
            *value += self.scale_segment_slope * delta;
        } else {
            self.scale_sample_dirty = true;
        }
    }

    fn fast_update_tint_value(&mut self, delta: f32) {
        if self.tint_sample_dirty {
            return;
        }
        let Some(track) = self.clip.tint.as_ref() else {
            self.tint_sample_dirty = true;
            return;
        };
        if matches!(track.interpolation, ClipInterpolation::Step) {
            return;
        }
        if let Some(value) = self.current_sample.tint.as_mut() {
            *value += self.tint_segment_slope * delta;
        } else {
            self.tint_sample_dirty = true;
        }
    }

    #[cfg(debug_assertions)]
    fn debug_verify_current_values(&mut self) {
        let sample = self.current_sample_full();
        let reference = self.sample_all_tracks();
        if let (Some(actual), Some(expected)) = (sample.translation, reference.translation) {
            debug_assert!(
                (actual - expected).length_squared() <= 1e-5,
                "translation mismatch: {:?} vs {:?}",
                actual,
                expected
            );
        }
        if let (Some(actual), Some(expected)) = (sample.rotation, reference.rotation) {
            debug_assert!(
                (actual - expected).abs() <= 1e-5,
                "rotation mismatch: actual={} expected={} time={} cursor={} seg_time={} seg_span={} cache_valid={} cached_index={} dirty={} slope={} start={}",
                actual,
                expected,
                self.time,
                self.rotation_cursor,
                self.rotation_segment_time,
                self.rotation_segment_span,
                self.rotation_segment_cache_valid,
                self.rotation_segment_cached_index,
                self.rotation_sample_dirty,
                self.rotation_segment_slope,
                self.rotation_segment_start
            );
        }
        if let (Some(actual), Some(expected)) = (sample.scale, reference.scale) {
            debug_assert!(
                (actual - expected).length_squared() <= 1e-5,
                "scale mismatch: {:?} vs {:?}",
                actual,
                expected
            );
        }
        if let (Some(actual), Some(expected)) = (sample.tint, reference.tint) {
            debug_assert!(
                (actual - expected).length_squared() <= 1e-5,
                "tint mismatch: {:?} vs {:?}",
                actual,
                expected
            );
        }
    }

    pub(crate) fn current_sample_full(&mut self) -> ClipSample {
        self.ensure_current_sample_channels(ClipChannelMask::all());
        self.current_sample
    }

    pub(crate) fn current_sample_masked(
        &mut self,
        transform_player: Option<&TransformTrackPlayer>,
        property_player: Option<&PropertyTrackPlayer>,
    ) -> ClipSample {
        let channel_mask = ClipChannelMask::from_players(transform_player, property_player);
        self.ensure_current_sample_channels(channel_mask);
        let transform_mask = transform_player.copied().unwrap_or_default();
        let property_mask = property_player.copied().unwrap_or_default();
        ClipSample {
            translation: if transform_mask.apply_translation {
                self.current_sample.translation
            } else {
                None
            },
            rotation: if transform_mask.apply_rotation { self.current_sample.rotation } else { None },
            scale: if transform_mask.apply_scale { self.current_sample.scale } else { None },
            tint: if property_mask.apply_tint { self.current_sample.tint } else { None },
        }
    }

    fn ensure_current_sample_channels(&mut self, mask: ClipChannelMask) {
        if mask.translation {
            self.ensure_translation_sample();
        }
        if mask.rotation {
            self.ensure_rotation_sample();
        }
        if mask.scale {
            self.ensure_scale_sample();
        }
        if mask.tint {
            self.ensure_tint_sample();
        }
    }

    fn ensure_translation_sample(&mut self) {
        if !self.translation_state_current {
            self.sync_translation_state_to_time(self.time);
        }
        if self.translation_sample_dirty {
            self.current_sample.translation = self.clip.translation.as_ref().and_then(|track| {
                Self::sample_vec2_from_state_cached(
                    track,
                    self.translation_cursor,
                    self.translation_segment_time,
                    self.translation_segment_span,
                    self.translation_segment_start,
                    self.translation_segment_slope,
                    self.translation_segment_cached_index,
                    self.translation_segment_cache_valid,
                )
            });
            self.translation_sample_dirty = false;
        }
    }

    fn ensure_rotation_sample(&mut self) {
        if !self.rotation_state_current {
            self.sync_rotation_state_to_time(self.time);
        }
        if self.rotation_sample_dirty {
            self.current_sample.rotation = self.clip.rotation.as_ref().and_then(|track| {
                Self::sample_scalar_from_state_cached(
                    track,
                    self.rotation_cursor,
                    self.rotation_segment_time,
                    self.rotation_segment_span,
                    self.rotation_segment_start,
                    self.rotation_segment_slope,
                    self.rotation_segment_cached_index,
                    self.rotation_segment_cache_valid,
                )
            });
            self.rotation_sample_dirty = false;
        }
    }

    fn ensure_scale_sample(&mut self) {
        if !self.scale_state_current {
            self.sync_scale_state_to_time(self.time);
        }
        if self.scale_sample_dirty {
            self.current_sample.scale = self.clip.scale.as_ref().and_then(|track| {
                Self::sample_vec2_from_state_cached(
                    track,
                    self.scale_cursor,
                    self.scale_segment_time,
                    self.scale_segment_span,
                    self.scale_segment_start,
                    self.scale_segment_slope,
                    self.scale_segment_cached_index,
                    self.scale_segment_cache_valid,
                )
            });
            self.scale_sample_dirty = false;
        }
    }

    fn ensure_tint_sample(&mut self) {
        if !self.tint_state_current {
            self.sync_tint_state_to_time(self.time);
        }
        if self.tint_sample_dirty {
            self.current_sample.tint = self.clip.tint.as_ref().and_then(|track| {
                Self::sample_vec4_from_state_cached(
                    track,
                    self.tint_cursor,
                    self.tint_segment_time,
                    self.tint_segment_span,
                    self.tint_segment_start,
                    self.tint_segment_slope,
                    self.tint_segment_cached_index,
                    self.tint_segment_cache_valid,
                )
            });
            self.tint_sample_dirty = false;
        }
    }
}

#[derive(Component, Clone, Copy)]
pub struct TransformTrackPlayer {
    pub apply_translation: bool,
    pub apply_rotation: bool,
    pub apply_scale: bool,
}

impl Default for TransformTrackPlayer {
    fn default() -> Self {
        Self { apply_translation: true, apply_rotation: true, apply_scale: true }
    }
}

#[derive(Component, Clone, Copy)]
pub struct PropertyTrackPlayer {
    pub apply_tint: bool,
}

impl Default for PropertyTrackPlayer {
    fn default() -> Self {
        Self { apply_tint: true }
    }
}

impl PropertyTrackPlayer {
    pub fn new(apply_tint: bool) -> Self {
        Self { apply_tint }
    }
}

#[inline(always)]
fn sample_vec2_track(track: &ClipVec2Track, time: f32, looped: bool) -> Option<Vec2> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let sample_time = normalize_time(time, track.duration, track.duration_inv, looped);
    Some(sample_keyframes(frames, track.interpolation, sample_time, |a, b, t| a + (b - a) * t))
}

#[inline(always)]
fn sample_scalar_track(track: &ClipScalarTrack, time: f32, looped: bool) -> Option<f32> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let sample_time = normalize_time(time, track.duration, track.duration_inv, looped);
    Some(sample_keyframes(frames, track.interpolation, sample_time, |a, b, t| a + (b - a) * t))
}

#[inline(always)]
fn sample_vec4_track(track: &ClipVec4Track, time: f32, looped: bool) -> Option<Vec4> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let sample_time = normalize_time(time, track.duration, track.duration_inv, looped);
    Some(sample_keyframes(frames, track.interpolation, sample_time, |a, b, t| a + (b - a) * t))
}

#[inline(always)]
fn sample_vec2_track_from_state(track: &ClipVec2Track, cursor: usize, segment_time: f32) -> Option<Vec2> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let len = frames.len();
    if len == 1 || cursor >= len - 1 {
        return Some(unsafe { frames.get_unchecked(len - 1) }.value);
    }
    let start = unsafe { frames.get_unchecked(cursor) };
    let end = unsafe { frames.get_unchecked(cursor + 1) };
    let segments = track.segments.as_ref();
    let segment = segments.get(cursor);
    if matches!(track.interpolation, ClipInterpolation::Step) {
        let (span, inv) =
            if let Some(seg) = segment { (seg.span.max(0.0), seg.inv_span) } else { (0.0, 0.0) };
        if span <= 0.0 || segment_time * inv >= 1.0 {
            Some(end.value)
        } else {
            Some(start.value)
        }
    } else if let Some(seg) = segment {
        let span = seg.span.max(0.0);
        let t = if span > 0.0 { segment_time.clamp(0.0, span) } else { 0.0 };
        Some(start.value + seg.slope * t)
    } else {
        Some(start.value)
    }
}

#[inline(always)]
fn sample_scalar_track_from_state(track: &ClipScalarTrack, cursor: usize, segment_time: f32) -> Option<f32> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let len = frames.len();
    if len == 1 || cursor >= len - 1 {
        return Some(unsafe { frames.get_unchecked(len - 1) }.value);
    }
    let start = unsafe { frames.get_unchecked(cursor) };
    let end = unsafe { frames.get_unchecked(cursor + 1) };
    let segments = track.segments.as_ref();
    let segment = segments.get(cursor);
    if matches!(track.interpolation, ClipInterpolation::Step) {
        let (span, inv) =
            if let Some(seg) = segment { (seg.span.max(0.0), seg.inv_span) } else { (0.0, 0.0) };
        if span <= 0.0 || segment_time * inv >= 1.0 {
            Some(end.value)
        } else {
            Some(start.value)
        }
    } else if let Some(seg) = segment {
        let span = seg.span.max(0.0);
        let t = if span > 0.0 { segment_time.clamp(0.0, span) } else { 0.0 };
        Some(start.value + seg.slope * t)
    } else {
        Some(start.value)
    }
}

#[inline(always)]
fn sample_vec4_track_from_state(track: &ClipVec4Track, cursor: usize, segment_time: f32) -> Option<Vec4> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let len = frames.len();
    if len == 1 || cursor >= len - 1 {
        return Some(unsafe { frames.get_unchecked(len - 1) }.value);
    }
    let start = unsafe { frames.get_unchecked(cursor) };
    let end = unsafe { frames.get_unchecked(cursor + 1) };
    let segments = track.segments.as_ref();
    let segment = segments.get(cursor);
    if matches!(track.interpolation, ClipInterpolation::Step) {
        let (span, inv) =
            if let Some(seg) = segment { (seg.span.max(0.0), seg.inv_span) } else { (0.0, 0.0) };
        if span <= 0.0 || segment_time * inv >= 1.0 {
            Some(end.value)
        } else {
            Some(start.value)
        }
    } else if let Some(seg) = segment {
        let span = seg.span.max(0.0);
        let t = if span > 0.0 { segment_time.clamp(0.0, span) } else { 0.0 };
        Some(start.value + seg.slope * t)
    } else {
        Some(start.value)
    }
}

fn rebuild_vec2_cursor(track: &ClipVec2Track, clip_time: f32, looped: bool) -> (usize, f32) {
    rebuild_cursor_impl(track.keyframes.as_ref(), track.duration, track.duration_inv, clip_time, looped)
}

fn rebuild_scalar_cursor(track: &ClipScalarTrack, clip_time: f32, looped: bool) -> (usize, f32) {
    rebuild_cursor_impl(track.keyframes.as_ref(), track.duration, track.duration_inv, clip_time, looped)
}

fn rebuild_vec4_cursor(track: &ClipVec4Track, clip_time: f32, looped: bool) -> (usize, f32) {
    rebuild_cursor_impl(track.keyframes.as_ref(), track.duration, track.duration_inv, clip_time, looped)
}

fn rebuild_cursor_impl<T>(
    frames: &[ClipKeyframe<T>],
    duration: f32,
    duration_inv: f32,
    clip_time: f32,
    looped: bool,
) -> (usize, f32) {
    if frames.is_empty() {
        return (0, 0.0);
    }
    if frames.len() == 1 {
        return (0, 0.0);
    }
    let sample_time = normalize_time(clip_time, duration, duration_inv, looped);
    cursor_from_time(frames, sample_time)
}

fn cursor_from_time<T>(frames: &[ClipKeyframe<T>], sample_time: f32) -> (usize, f32) {
    if frames.is_empty() {
        return (0, 0.0);
    }
    if frames.len() == 1 {
        return (0, 0.0);
    }
    let last_index = frames.len() - 1;
    if sample_time <= frames[0].time {
        return (0, 0.0);
    }
    let mut insertion = frames.partition_point(|frame| sample_time >= frame.time);
    if insertion == 0 {
        insertion = 1;
    }
    if insertion > last_index {
        insertion = last_index;
    }
    let mut index = insertion - 1;
    if index >= last_index {
        index = last_index - 1;
    }
    let start = &frames[index];
    let end = &frames[index + 1];
    let span = (end.time - start.time).max(0.0);
    let offset = (sample_time - start.time).clamp(0.0, span);
    (index, offset)
}

const CLIP_TIME_EPSILON: f32 = 1e-5;

#[inline(always)]
fn locate_segment_index(offsets: &[f32], target: f32, last_segment: usize) -> usize {
    if offsets.is_empty() {
        return 0usize.min(last_segment);
    }
    let idx = offsets.partition_point(|start| *start <= target);
    if idx == 0 {
        0
    } else {
        (idx - 1).min(last_segment)
    }
}

#[inline(always)]
fn resolve_target_time(unwrapped: f32, duration: f32, duration_inv: f32, looped: bool) -> (f32, u64) {
    if !looped {
        if duration <= 0.0 {
            return (0.0, 0);
        }
        let mut clamped = unwrapped.clamp(0.0, duration);
        if clamped >= duration - CLIP_TIME_EPSILON {
            clamped = duration;
        }
        return (clamped, 0);
    }
    if duration <= 0.0 {
        return (0.0, 0);
    }
    let mut wrapped = wrap_time_looped(unwrapped, duration, duration_inv);
    if wrapped <= CLIP_TIME_EPSILON || (duration - wrapped) <= CLIP_TIME_EPSILON {
        wrapped = 0.0;
    }
    let cycles =
        if unwrapped >= 0.0 && duration > 0.0 { (unwrapped / duration).floor().max(0.0) as u64 } else { 0 };
    (wrapped, cycles)
}

#[inline(always)]
#[cfg(feature = "anim_stats")]
fn compute_segment_crosses(
    segment_count: usize,
    start_index: usize,
    end_index: usize,
    target_cycle: u64,
    looped: bool,
) -> u64 {
    if segment_count == 0 {
        return 0;
    }
    let start_ordinal = start_index as u64;
    let end_ordinal = if looped {
        target_cycle.saturating_mul(segment_count as u64).saturating_add(end_index as u64)
    } else {
        end_index as u64
    };
    end_ordinal.saturating_sub(start_ordinal).saturating_sub(1)
}

#[inline(always)]
fn wrap_time_looped(time: f32, duration: f32, duration_inv: f32) -> f32 {
    if !time.is_finite() || duration <= 0.0 {
        return 0.0;
    }
    let duration_eps = duration.max(std::f32::EPSILON);
    let mut wrapped = if duration_inv > 0.0 {
        let scaled = time * duration_inv;
        if scaled.is_finite() {
            let loops = scaled.floor();
            let remainder = time - loops * duration;
            if remainder.is_finite() {
                remainder
            } else {
                time.rem_euclid(duration_eps)
            }
        } else {
            time.rem_euclid(duration_eps)
        }
    } else {
        time.rem_euclid(duration_eps)
    };
    if !wrapped.is_finite() {
        wrapped = 0.0;
    }
    if wrapped < 0.0 {
        wrapped += duration_eps;
    }
    if wrapped >= duration_eps {
        wrapped -= duration_eps;
    }
    wrapped
}

#[inline(always)]
fn normalize_time(time: f32, duration: f32, duration_inv: f32, looped: bool) -> f32 {
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
        let wrapped = wrap_time_looped(time, duration, duration_inv);
        if wrapped <= CLIP_TIME_EPSILON && time > 0.0 && (time - duration).abs() <= CLIP_TIME_EPSILON {
            duration
        } else {
            wrapped
        }
    } else {
        time.clamp(0.0, duration)
    }
}

#[inline(always)]
fn sample_keyframes<T, L>(
    frames: &[crate::assets::ClipKeyframe<T>],
    mode: ClipInterpolation,
    time: f32,
    lerp: L,
) -> T
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpriteAnimationLoopMode {
    Loop,
    OnceHold,
    OnceStop,
    PingPong,
}

impl SpriteAnimationLoopMode {
    pub fn from_str(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "once_hold" | "oncehold" => Self::OnceHold,
            "once_stop" | "oncestop" | "once" => Self::OnceStop,
            "pingpong" | "ping_pong" => Self::PingPong,
            _ => Self::Loop,
        }
    }

    pub fn looped(self) -> bool {
        matches!(self, Self::Loop | Self::PingPong)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::OnceHold => "once_hold",
            Self::OnceStop => "once_stop",
            Self::PingPong => "pingpong",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::skeletal::{SkeletalClip, SkeletonAsset, SkeletonJoint};
    use crate::assets::{AnimationClip, ClipSegment};
    use std::f32::consts::TAU;

    #[test]
    fn clip_instance_set_time_wraps_negative_looped() {
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
        instance.set_time(-0.25);
        assert!((instance.time - 0.75).abs() < 1e-6);
    }

    #[test]
    fn skeleton_instance_set_time_handles_negative() {
        let joint = SkeletonJoint {
            name: Arc::from("root"),
            parent: None,
            rest_local: Mat4::IDENTITY,
            rest_world: Mat4::IDENTITY,
            rest_translation: Vec3::ZERO,
            rest_rotation: Quat::IDENTITY,
            rest_scale: Vec3::ONE,
            inverse_bind: Mat4::IDENTITY,
        };
        let skeleton = Arc::new(SkeletonAsset {
            name: Arc::from("skel"),
            joints: Arc::from(vec![joint].into_boxed_slice()),
            roots: Arc::from(vec![0_u32].into_boxed_slice()),
        });
        let mut instance = SkeletonInstance::new(Arc::from("skel"), Arc::clone(&skeleton));
        let clip = Arc::new(SkeletalClip {
            name: Arc::from("clip"),
            skeleton: Arc::from("skel"),
            duration: 1.0,
            channels: Arc::new([]),
            looped: true,
        });
        instance.set_active_clip(None, Some(clip));
        let time = instance.set_time(-0.4);
        assert!((time - 0.6).abs() < 1e-6);
    }

    #[test]
    fn linear_rotation_clip_cached_sample_stays_in_sync() {
        fn rotation_clip() -> Arc<AnimationClip> {
            let keyframes: Arc<[ClipKeyframe<f32>]> = Arc::from(
                vec![ClipKeyframe { time: 0.0, value: 0.0 }, ClipKeyframe { time: 0.5, value: TAU }]
                    .into_boxed_slice(),
            );
            let span = (keyframes[1].time - keyframes[0].time).max(std::f32::EPSILON);
            let inv_span = 1.0 / span;
            let delta = keyframes[1].value - keyframes[0].value;
            let segments =
                Arc::from(vec![ClipSegment { slope: delta * inv_span, span, inv_span }].into_boxed_slice());
            let offsets = Arc::from(vec![keyframes[0].time].into_boxed_slice());
            let deltas = Arc::from(vec![delta].into_boxed_slice());
            Arc::new(AnimationClip {
                name: Arc::from("rotation_bench"),
                duration: span,
                duration_inv: inv_span,
                translation: None,
                rotation: Some(ClipScalarTrack {
                    interpolation: ClipInterpolation::Linear,
                    keyframes,
                    duration: span,
                    duration_inv: inv_span,
                    segment_deltas: deltas,
                    segments,
                    segment_offsets: offsets,
                }),
                scale: None,
                tint: None,
                looped: true,
                version: 1,
            })
        }

        let clip = rotation_clip();
        let mut instance = ClipInstance::new(Arc::from("clip"), clip);
        let mask = TransformTrackPlayer::default();
        let dt = 1.0 / 60.0;

        for step in 0..240 {
            instance.advance_time_masked(dt, Some(&mask), None);
            let cached = instance.sample_cached().rotation.unwrap();
            let reference = instance.sample_at(instance.time).rotation.unwrap();
            let diff = (cached - reference).abs();
            assert!(
                diff <= 1e-5,
                "post-step mismatch at step {step}: time={} cached={} reference={} diff={}",
                instance.time,
                cached,
                reference,
                diff
            );
        }
    }
}
#[derive(Component, Clone)]
pub struct MeshRef {
    pub key: String,
}
#[derive(Component, Clone)]
pub struct MeshSurface {
    pub material: Option<String>,
    pub lighting: MeshLighting,
}
impl Default for MeshSurface {
    fn default() -> Self {
        Self { material: None, lighting: MeshLighting::default() }
    }
}
#[derive(Clone)]
pub struct MeshLighting {
    pub cast_shadows: bool,
    pub receive_shadows: bool,
    pub base_color: Vec3,
    pub emissive: Option<Vec3>,
    pub metallic: f32,
    pub roughness: f32,
}
impl Default for MeshLighting {
    fn default() -> Self {
        Self {
            cast_shadows: false,
            receive_shadows: true,
            base_color: Vec3::splat(1.0),
            emissive: None,
            metallic: 0.0,
            roughness: 0.5,
        }
    }
}
#[derive(Component, Clone, Copy)]
pub struct Transform3D {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
impl Default for Transform3D {
    fn default() -> Self {
        Self { translation: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
}

#[derive(Component, Clone)]
pub struct SkeletonInstance {
    pub skeleton_key: Arc<str>,
    pub skeleton: Arc<SkeletonAsset>,
    pub active_clip_key: Option<Arc<str>>,
    pub active_clip: Option<Arc<SkeletalClip>>,
    pub time: f32,
    pub playing: bool,
    pub looped: bool,
    pub speed: f32,
    pub group: Option<String>,
    pub playback_rate: f32,
    pub playback_rate_dirty: bool,
    pub local_poses: Vec<Mat4>,
    pub model_poses: Vec<Mat4>,
    pub palette: Vec<Mat4>,
    pub joint_channel_map: Vec<Option<usize>>,
    pub joint_children: Vec<Vec<usize>>,
    pub joint_visited: Vec<bool>,
    pub dirty: bool,
}

impl SkeletonInstance {
    pub fn new(skeleton_key: Arc<str>, skeleton: Arc<SkeletonAsset>) -> Self {
        let joint_count = skeleton.joints.len();
        let mut local_poses = Vec::with_capacity(joint_count);
        let mut model_poses = Vec::with_capacity(joint_count);
        let mut palette = Vec::with_capacity(joint_count);
        let mut joint_children = vec![Vec::new(); joint_count];
        for (index, joint) in skeleton.joints.iter().enumerate() {
            local_poses.push(joint.rest_local);
            model_poses.push(joint.rest_world);
            palette.push(joint.rest_world * joint.inverse_bind);
            if let Some(parent) = joint.parent {
                let parent_index = parent as usize;
                if parent_index < joint_children.len() {
                    joint_children[parent_index].push(index);
                }
            }
        }
        let joint_channel_map = vec![None; joint_count];
        let joint_visited = vec![false; joint_count];
        Self {
            skeleton_key,
            skeleton,
            active_clip_key: None,
            active_clip: None,
            time: 0.0,
            playing: true,
            looped: true,
            speed: 1.0,
            group: None,
            playback_rate: 0.0,
            playback_rate_dirty: true,
            local_poses,
            model_poses,
            palette,
            joint_channel_map,
            joint_children,
            joint_visited,
            dirty: false,
        }
    }

    #[inline]
    pub fn joint_count(&self) -> usize {
        self.skeleton.joints.len()
    }

    pub fn set_active_clip(&mut self, clip_key: Option<Arc<str>>, clip: Option<Arc<SkeletalClip>>) {
        self.active_clip_key = clip_key;
        if let Some(ref clip) = clip {
            self.looped = clip.looped;
        }
        self.active_clip = clip;
        self.time = 0.0;
        self.playing = true;
        self.playback_rate = 0.0;
        self.playback_rate_dirty = true;
        self.dirty = true;
    }

    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
        self.playback_rate_dirty = true;
    }

    pub fn set_group<S: Into<Option<String>>>(&mut self, group: S) {
        self.group = group.into();
        self.playback_rate_dirty = true;
    }

    pub fn set_time(&mut self, time: f32) -> f32 {
        let mut clamped = time;
        if let Some(clip) = self.active_clip.as_ref() {
            let duration = clip.duration.max(0.0);
            if duration <= 0.0 {
                clamped = 0.0;
            } else if self.looped {
                let step = duration.max(std::f32::EPSILON);
                clamped = time.rem_euclid(step);
                if (step - clamped).abs() <= CLIP_TIME_EPSILON {
                    clamped = duration;
                }
            } else if clamped <= 0.0 {
                clamped = 0.0;
            } else if clamped >= duration {
                clamped = duration;
                self.playing = false;
            }
        } else {
            clamped = 0.0;
        }
        self.time = clamped;
        self.dirty = true;
        clamped
    }

    pub fn ensure_playback_rate(&mut self, group_scale: f32) -> f32 {
        if self.playback_rate_dirty {
            self.playback_rate = self.speed * group_scale;
            self.playback_rate_dirty = false;
        }
        self.playback_rate
    }

    pub fn clip_duration(&self) -> f32 {
        self.active_clip.as_ref().map(|clip| clip.duration.max(0.0)).unwrap_or(0.0)
    }

    pub fn active_clip_key(&self) -> Option<String> {
        self.active_clip.as_ref().map(|clip| format!("{}::{}", self.skeleton_key, clip.name.as_ref()))
    }

    pub fn has_clip(&self) -> bool {
        self.active_clip.is_some()
    }

    #[inline]
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    #[inline]
    pub fn clear_dirty(&mut self) {
        self.dirty = false;
    }

    pub fn ensure_capacity(&mut self) {
        let joint_count = self.joint_count();
        if self.local_poses.len() != joint_count {
            self.local_poses.resize(joint_count, Mat4::IDENTITY);
        }
        if self.model_poses.len() != joint_count {
            self.model_poses.resize(joint_count, Mat4::IDENTITY);
        }
        if self.palette.len() != joint_count {
            self.palette.resize(joint_count, Mat4::IDENTITY);
        }
        if self.joint_channel_map.len() != joint_count {
            self.joint_channel_map.resize(joint_count, None);
        }
        if self.joint_visited.len() != joint_count {
            self.joint_visited.resize(joint_count, false);
        }
        if self.joint_children.len() != joint_count {
            self.rebuild_joint_children();
        }
    }

    pub fn reset_to_rest_pose(&mut self) {
        for (index, joint) in self.skeleton.joints.iter().enumerate() {
            self.local_poses[index] = joint.rest_local;
            self.model_poses[index] = joint.rest_world;
            self.palette[index] = joint.rest_world * joint.inverse_bind;
        }
        self.dirty = false;
    }

    fn rebuild_joint_children(&mut self) {
        let joint_count = self.joint_count();
        self.joint_children.clear();
        self.joint_children.resize_with(joint_count, Vec::new);
        for (index, joint) in self.skeleton.joints.iter().enumerate() {
            if let Some(parent) = joint.parent {
                let parent_index = parent as usize;
                if parent_index < self.joint_children.len() {
                    self.joint_children[parent_index].push(index);
                }
            }
        }
    }
}

#[derive(Component, Clone)]
pub struct BoneTransforms {
    pub model: Vec<Mat4>,
    pub palette: Vec<Mat4>,
}

impl BoneTransforms {
    pub fn new(joint_count: usize) -> Self {
        Self { model: vec![Mat4::IDENTITY; joint_count], palette: vec![Mat4::IDENTITY; joint_count] }
    }

    pub fn ensure_joint_count(&mut self, joint_count: usize) {
        if self.model.len() != joint_count {
            self.model.resize(joint_count, Mat4::IDENTITY);
        }
        if self.palette.len() != joint_count {
            self.palette.resize(joint_count, Mat4::IDENTITY);
        }
    }
}

#[derive(Component, Clone)]
pub struct SkinMesh {
    pub skeleton_entity: Option<Entity>,
    pub mesh_key: Option<Arc<str>>,
    pub joint_count: u32,
}

impl SkinMesh {
    pub fn new(joint_count: usize) -> Self {
        Self { skeleton_entity: None, mesh_key: None, joint_count: joint_count as u32 }
    }

    pub fn set_skeleton(&mut self, skeleton: Entity) {
        self.skeleton_entity = Some(skeleton);
    }

    pub fn clear_skeleton(&mut self) {
        self.skeleton_entity = None;
    }

    pub fn set_mesh_key(&mut self, key: Arc<str>) {
        self.mesh_key = Some(key);
    }

    pub fn clear_mesh_key(&mut self) {
        self.mesh_key = None;
    }

    #[inline]
    pub fn joints(&self) -> usize {
        self.joint_count as usize
    }
}
#[derive(Component, Clone, Copy)]
pub struct Velocity(pub Vec2);
#[derive(Component, Clone, Copy)]
pub struct Aabb {
    pub half: Vec2,
}
#[derive(Component, Clone, Copy)]
pub struct Tint(pub Vec4);
#[derive(Component, Clone, Copy, Default)]
pub struct Mass(pub f32);
#[derive(Component, Clone, Copy, Default)]
pub struct Force(pub Vec2);
#[derive(Component)]
pub struct ParticleEmitter {
    pub rate: f32,
    pub spread: f32,
    pub speed: f32,
    pub lifetime: f32,
    pub accumulator: f32,
    pub start_color: Vec4,
    pub end_color: Vec4,
    pub start_size: f32,
    pub end_size: f32,
    pub atlas: Arc<str>,
    pub region: Arc<str>,
    pub source: Option<Arc<str>>,
}
#[derive(Component)]
pub struct Particle {
    pub lifetime: f32,
    pub max_lifetime: f32,
}
#[derive(Component)]
pub struct ParticleVisual {
    pub start_color: Vec4,
    pub end_color: Vec4,
    pub start_size: f32,
    pub end_size: f32,
}

#[derive(Clone, Copy, Resource)]
pub struct ParticleCaps {
    pub max_spawn_per_frame: u32,
    pub max_total: u32,
    pub max_emitter_backlog: f32,
}

impl Default for ParticleCaps {
    fn default() -> Self {
        Self { max_spawn_per_frame: 256, max_total: 2_000, max_emitter_backlog: 64.0 }
    }
}

impl ParticleCaps {
    pub fn new(max_spawn_per_frame: u32, max_total: u32, max_emitter_backlog: f32) -> Self {
        let backlog = max_emitter_backlog.max(0.0);
        let spawn = max_spawn_per_frame.min(max_total);
        Self { max_spawn_per_frame: spawn, max_total, max_emitter_backlog: backlog }
    }
}

#[derive(Resource, Clone, Copy, Default)]
pub struct ParticleState {
    pub active_particles: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ParticleBudgetMetrics {
    pub active_particles: u32,
    pub available_spawn_this_frame: u32,
    pub max_total: u32,
    pub max_spawn_per_frame: u32,
    pub total_emitters: u32,
    pub emitter_backlog_total: f32,
    pub emitter_backlog_max_observed: f32,
    pub emitter_backlog_limit: f32,
}

impl ParticleBudgetMetrics {
    pub fn cap_utilization(&self) -> f32 {
        if self.max_total == 0 {
            0.0
        } else {
            self.active_particles as f32 / self.max_total as f32
        }
    }

    pub fn average_backlog(&self) -> f32 {
        if self.total_emitters == 0 {
            0.0
        } else {
            self.emitter_backlog_total / self.total_emitters as f32
        }
    }
}

#[derive(Component, Clone, Copy)]
pub struct RapierBody {
    pub handle: RigidBodyHandle,
}

#[derive(Component, Clone, Copy)]
pub struct RapierCollider {
    pub handle: ColliderHandle,
}

#[derive(Component, Clone, Copy)]
pub struct OrbitController {
    pub center: Vec2,
    pub angular_speed: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceData {
    pub axis_x: [f32; 4],
    pub axis_y: [f32; 4],
    pub translation: [f32; 4],
    pub uv_rect: [f32; 4],
    pub tint: [f32; 4],
}

#[derive(Clone)]
pub struct SpriteInstance {
    pub atlas: Arc<str>,
    pub transform: SpriteInstanceTransform,
    pub uv_rect: [f32; 4],
    pub tint: [f32; 4],
    pub world_half_extent: Vec2,
}

impl SpriteInstance {
    pub fn into_gpu(self) -> (Arc<str>, InstanceData) {
        let data = InstanceData {
            axis_x: self.transform.axis_x.extend(0.0).to_array(),
            axis_y: self.transform.axis_y.extend(0.0).to_array(),
            translation: self.transform.translation.extend(1.0).to_array(),
            uv_rect: self.uv_rect,
            tint: self.tint,
        };
        (self.atlas, data)
    }
}

#[derive(Clone, Copy)]
pub struct SpriteInstanceTransform {
    pub axis_x: Vec3,
    pub axis_y: Vec3,
    pub translation: Vec3,
}

impl SpriteInstanceTransform {
    pub fn from_mat4(model: Mat4) -> Self {
        Self {
            axis_x: Vec3::new(model.x_axis.x, model.x_axis.y, model.x_axis.z),
            axis_y: Vec3::new(model.y_axis.x, model.y_axis.y, model.y_axis.z),
            translation: Vec3::new(model.w_axis.x, model.w_axis.y, model.w_axis.z),
        }
    }

    pub fn half_extent_2d(&self) -> Vec2 {
        let half_x = Vec2::new(self.axis_x.x, self.axis_x.y) * 0.5;
        let half_y = Vec2::new(self.axis_y.x, self.axis_y.y) * 0.5;
        Vec2::new(half_x.x.abs() + half_y.x.abs(), half_x.y.abs() + half_y.y.abs())
    }
}

#[derive(Clone)]
pub struct TransformClipInfo {
    pub clip_key: String,
    pub playing: bool,
    pub looped: bool,
    pub speed: f32,
    pub time: f32,
    pub duration: f32,
    pub group: Option<String>,
    pub has_translation: bool,
    pub has_rotation: bool,
    pub has_scale: bool,
    pub has_tint: bool,
    pub sample_translation: Option<Vec2>,
    pub sample_rotation: Option<f32>,
    pub sample_scale: Option<Vec2>,
    pub sample_tint: Option<Vec4>,
}

#[derive(Clone)]
pub struct SkeletonClipInfo {
    pub clip_key: String,
    pub playing: bool,
    pub looped: bool,
    pub speed: f32,
    pub time: f32,
    pub duration: f32,
    pub group: Option<String>,
}

#[derive(Clone)]
pub struct SkeletonInfo {
    pub skeleton_key: String,
    pub joint_count: usize,
    pub has_bone_transforms: bool,
    pub palette_joint_count: usize,
    pub clip: Option<SkeletonClipInfo>,
}

#[derive(Clone)]
pub struct SkinMeshInfo {
    pub joint_count: usize,
    pub skeleton_entity: Option<Entity>,
    pub skeleton_scene_id: Option<SceneEntityId>,
    pub mesh_key: Option<String>,
}

#[derive(Clone)]
pub struct EntityInfo {
    pub scene_id: SceneEntityId,
    pub translation: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
    pub velocity: Option<Vec2>,
    pub transform_clip: Option<TransformClipInfo>,
    pub transform_tracks: Option<TransformTrackPlayer>,
    pub property_tracks: Option<PropertyTrackPlayer>,
    pub sprite: Option<SpriteInfo>,
    pub mesh: Option<MeshInfo>,
    pub mesh_transform: Option<Transform3DInfo>,
    pub tint: Option<Vec4>,
    pub skeleton: Option<SkeletonInfo>,
    pub skin_mesh: Option<SkinMeshInfo>,
}

#[derive(Clone)]
pub struct SpriteInfo {
    pub atlas: String,
    pub region: String,
    pub animation: Option<SpriteAnimationInfo>,
}

#[derive(Clone)]
pub struct SpriteAnimationInfo {
    pub timeline: String,
    pub playing: bool,
    pub looped: bool,
    pub loop_mode: String,
    pub speed: f32,
    pub frame_index: usize,
    pub frame_count: usize,
    pub frame_elapsed: f32,
    pub frame_duration: f32,
    pub frame_region: Option<String>,
    pub frame_region_id: Option<u16>,
    pub frame_uv: Option<[f32; 4]>,
    pub frame_events: Vec<String>,
    pub start_offset: f32,
    pub random_start: bool,
    pub group: Option<String>,
}

#[derive(Clone)]
pub struct MeshInfo {
    pub key: String,
    pub material: Option<String>,
    pub lighting: MeshLightingInfo,
}

#[derive(Clone)]
pub struct MeshLightingInfo {
    pub cast_shadows: bool,
    pub receive_shadows: bool,
    pub base_color: Vec3,
    pub emissive: Option<Vec3>,
    pub metallic: f32,
    pub roughness: f32,
}
impl Default for MeshLightingInfo {
    fn default() -> Self {
        Self {
            cast_shadows: false,
            receive_shadows: true,
            base_color: Vec3::splat(1.0),
            emissive: None,
            metallic: 0.0,
            roughness: 0.5,
        }
    }
}

#[derive(Clone)]
pub struct MeshInstance {
    pub key: String,
    pub model: Mat4,
    pub material: Option<String>,
    pub lighting: MeshLightingInfo,
    pub skin: Option<MeshSkinInstance>,
}

#[derive(Clone)]
pub struct MeshSkinInstance {
    pub palette: Arc<[Mat4]>,
}

impl From<&MeshLighting> for MeshLightingInfo {
    fn from(value: &MeshLighting) -> Self {
        Self {
            cast_shadows: value.cast_shadows,
            receive_shadows: value.receive_shadows,
            base_color: value.base_color,
            emissive: value.emissive,
            metallic: value.metallic,
            roughness: value.roughness,
        }
    }
}

impl From<MeshLightingData> for MeshLighting {
    fn from(value: MeshLightingData) -> Self {
        Self {
            cast_shadows: value.cast_shadows,
            receive_shadows: value.receive_shadows,
            emissive: value.emissive.map(Into::into),
            base_color: value.base_color.into(),
            metallic: value.metallic,
            roughness: value.roughness,
        }
    }
}

impl From<&MeshLighting> for MeshLightingData {
    fn from(value: &MeshLighting) -> Self {
        MeshLightingData {
            cast_shadows: value.cast_shadows,
            receive_shadows: value.receive_shadows,
            emissive: value.emissive.map(Into::into),
            base_color: value.base_color.into(),
            metallic: value.metallic,
            roughness: value.roughness,
        }
    }
}

#[derive(Clone)]
pub struct Transform3DInfo {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
