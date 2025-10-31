use crate::assets::{
    skeletal::{SkeletalClip, SkeletonAsset},
    AnimationClip, ClipInterpolation, ClipScalarTrack, ClipVec2Track, ClipVec4Track,
};
use crate::scene::{MeshLightingData, SceneEntityId};
use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use rapier2d::prelude::{ColliderHandle, RigidBodyHandle};
use std::sync::Arc;

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
        if self.region_id != frame.region_id || self.region.as_ref() != frame.region.as_ref() {
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
pub struct SpriteAnimation {
    pub timeline: Arc<str>,
    pub frames: Arc<[SpriteAnimationFrame]>,
    pub frame_durations: Arc<[f32]>,
    pub current_duration: f32,
    pub frame_index: usize,
    pub elapsed_in_frame: f32,
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
}

impl SpriteAnimation {
    pub fn new(
        timeline: Arc<str>,
        frames: Arc<[SpriteAnimationFrame]>,
        frame_durations: Arc<[f32]>,
        mode: SpriteAnimationLoopMode,
    ) -> Self {
        let has_events = frames.iter().any(|frame| !frame.events.is_empty());
        let fast_loop = !has_events && matches!(mode, SpriteAnimationLoopMode::Loop);
        let current_duration = frame_durations.first().copied().unwrap_or(0.0);
        Self {
            timeline,
            frames,
            frame_durations,
            current_duration,
            frame_index: 0,
            elapsed_in_frame: 0.0,
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
        }
    }

    pub fn set_mode(&mut self, mode: SpriteAnimationLoopMode) {
        self.mode = mode;
        self.looped = mode.looped();
        self.forward = true;
        self.fast_loop = !self.has_events && matches!(self.mode, SpriteAnimationLoopMode::Loop);
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
        self.frame_durations.iter().copied().sum()
    }

    pub fn mark_playback_rate_dirty(&mut self) {
        self.playback_rate_dirty = true;
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.max(0.0);
        self.playback_rate_dirty = true;
    }

    pub fn ensure_playback_rate(&mut self, group_scale: f32) -> f32 {
        let clamped_group = group_scale.max(0.0);
        let base_speed = self.speed.max(0.0);
        if self.playback_rate_dirty {
            self.playback_rate = (base_speed * clamped_group).max(0.0);
            self.playback_rate_dirty = false;
        }
        self.playback_rate
    }

    #[inline]
    pub fn refresh_current_duration(&mut self) {
        self.current_duration = self.frame_durations.get(self.frame_index).copied().unwrap_or(0.0);
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

#[derive(Clone, Default)]
pub struct ClipSample {
    pub translation: Option<Vec2>,
    pub rotation: Option<f32>,
    pub scale: Option<Vec2>,
    pub tint: Option<Vec4>,
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
}

impl ClipInstance {
    pub fn new(clip_key: Arc<str>, clip: Arc<AnimationClip>) -> Self {
        let version = clip.version;
        let looped = clip.looped;
        Self {
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
        }
    }

    pub fn replace_clip(&mut self, clip_key: Arc<str>, clip: Arc<AnimationClip>) {
        let previous_speed = self.speed;
        let previous_group = self.group.clone();
        self.clip_key = clip_key;
        self.clip = clip;
        self.clip_version = self.clip.version;
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
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.max(0.0);
        self.mark_playback_rate_dirty();
    }

    pub fn set_group(&mut self, group: Option<&str>) {
        self.group = group.map(|g| g.to_string());
        self.mark_playback_rate_dirty();
    }

    pub fn mark_playback_rate_dirty(&mut self) {
        self.playback_rate_dirty = true;
    }

    pub fn ensure_playback_rate(&mut self, group_scale: f32) -> f32 {
        if self.playback_rate_dirty {
            let clamped_group = group_scale.max(0.0);
            let base_speed = self.speed.max(0.0);
            self.playback_rate = (base_speed * clamped_group).max(0.0);
            self.playback_rate_dirty = false;
        }
        self.playback_rate
    }

    pub fn duration(&self) -> f32 {
        self.clip.duration.max(0.0)
    }

    pub fn set_time(&mut self, time: f32) {
        let duration = self.duration();
        if duration > 0.0 {
            if self.looped {
                if time >= duration && (time - duration).abs() <= CLIP_TIME_EPSILON {
                    self.time = duration;
                } else {
                    let wrapped = time.rem_euclid(duration.max(std::f32::EPSILON));
                    self.time = wrapped;
                }
            } else {
                self.time = time.clamp(0.0, duration);
            }
        } else {
            self.time = 0.0;
        }
    }

    pub fn sample(&self) -> ClipSample {
        self.sample_at(self.time)
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

fn sample_vec2_track(track: &ClipVec2Track, time: f32, looped: bool) -> Option<Vec2> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let duration = frames.last().map(|kf| kf.time).unwrap_or(0.0);
    let sample_time = normalize_time(time, duration, looped);
    Some(sample_keyframes(frames, track.interpolation, sample_time, |a, b, t| a + (b - a) * t))
}

fn sample_scalar_track(track: &ClipScalarTrack, time: f32, looped: bool) -> Option<f32> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let duration = frames.last().map(|kf| kf.time).unwrap_or(0.0);
    let sample_time = normalize_time(time, duration, looped);
    Some(sample_keyframes(frames, track.interpolation, sample_time, |a, b, t| a + (b - a) * t))
}

fn sample_vec4_track(track: &ClipVec4Track, time: f32, looped: bool) -> Option<Vec4> {
    let frames = track.keyframes.as_ref();
    if frames.is_empty() {
        return None;
    }
    let duration = frames.last().map(|kf| kf.time).unwrap_or(0.0);
    let sample_time = normalize_time(time, duration, looped);
    Some(sample_keyframes(frames, track.interpolation, sample_time, |a, b, t| a + (b - a) * t))
}

const CLIP_TIME_EPSILON: f32 = 1e-5;

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
    pub dirty: bool,
}

impl SkeletonInstance {
    pub fn new(skeleton_key: Arc<str>, skeleton: Arc<SkeletonAsset>) -> Self {
        let joint_count = skeleton.joints.len();
        let mut local_poses = Vec::with_capacity(joint_count);
        let mut model_poses = Vec::with_capacity(joint_count);
        let mut palette = Vec::with_capacity(joint_count);
        for joint in skeleton.joints.iter() {
            local_poses.push(joint.rest_local);
            model_poses.push(joint.rest_world);
            palette.push(joint.rest_world * joint.inverse_bind);
        }
        Self {
            skeleton_key,
            skeleton,
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
            dirty: false,
        }
    }

    #[inline]
    pub fn joint_count(&self) -> usize {
        self.skeleton.joints.len()
    }

    pub fn set_active_clip(&mut self, clip: Option<Arc<SkeletalClip>>) {
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
        self.speed = speed.max(0.0);
        self.playback_rate_dirty = true;
    }

    pub fn set_group<S: Into<Option<String>>>(&mut self, group: S) {
        self.group = group.into();
        self.playback_rate_dirty = true;
    }

    pub fn ensure_playback_rate(&mut self, group_scale: f32) -> f32 {
        if self.playback_rate_dirty {
            let clamped_group = group_scale.max(0.0);
            let base_speed = self.speed.max(0.0);
            self.playback_rate = (base_speed * clamped_group).max(0.0);
            self.playback_rate_dirty = false;
        }
        self.playback_rate
    }

    pub fn clip_duration(&self) -> f32 {
        self.active_clip.as_ref().map(|clip| clip.duration.max(0.0)).unwrap_or(0.0)
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
    }

    pub fn reset_to_rest_pose(&mut self) {
        for (index, joint) in self.skeleton.joints.iter().enumerate() {
            self.local_poses[index] = joint.rest_local;
            self.model_poses[index] = joint.rest_world;
            self.palette[index] = joint.rest_world * joint.inverse_bind;
        }
        self.dirty = false;
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
    pub model: [[f32; 4]; 4],
    pub uv_rect: [f32; 4],
    pub tint: [f32; 4],
}

#[derive(Clone)]
pub struct SpriteInstance {
    pub atlas: String,
    pub data: InstanceData,
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
