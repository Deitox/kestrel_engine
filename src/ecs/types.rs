use crate::scene::{MeshLightingData, SceneEntityId};
use bevy_ecs::prelude::*;
use glam::{Mat4, Quat, Vec2, Vec3, Vec4};
use rapier2d::prelude::{ColliderHandle, RigidBodyHandle};
use std::borrow::Cow;

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
    pub atlas_key: Cow<'static, str>,
    pub region: Cow<'static, str>,
}

#[derive(Component, Clone)]
pub struct SpriteAnimation {
    pub timeline: String,
    pub frames: Vec<SpriteAnimationFrame>,
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
}

impl SpriteAnimation {
    pub fn new(
        timeline: String,
        frames: Vec<SpriteAnimationFrame>,
        looped: bool,
        mode: SpriteAnimationLoopMode,
    ) -> Self {
        Self {
            timeline,
            frames,
            frame_index: 0,
            elapsed_in_frame: 0.0,
            playing: true,
            looped,
            forward: true,
            speed: 1.0,
            mode,
            start_offset: 0.0,
            random_start: false,
            group: None,
        }
    }

    pub fn set_mode(&mut self, mode: SpriteAnimationLoopMode) {
        self.mode = mode;
        self.looped = mode.looped();
        self.forward = true;
    }

    pub fn set_start_offset(&mut self, offset: f32) {
        self.start_offset = offset.max(0.0);
    }

    pub fn set_random_start(&mut self, random: bool) {
        self.random_start = random;
    }

    pub fn set_group<S: Into<Option<String>>>(&mut self, group: S) {
        self.group = group.into();
    }

    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }

    pub fn current_region_name(&self) -> Option<&str> {
        self.frames.get(self.frame_index).map(|frame| frame.region.as_str())
    }

    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }

    pub fn total_duration(&self) -> f32 {
        self.frames.iter().map(|frame| frame.duration.max(std::f32::EPSILON)).sum()
    }
}

#[derive(Clone)]
pub struct SpriteAnimationFrame {
    pub region: String,
    pub duration: f32,
    pub events: Vec<String>,
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
pub struct EntityInfo {
    pub scene_id: SceneEntityId,
    pub translation: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
    pub velocity: Option<Vec2>,
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
