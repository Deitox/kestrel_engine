use super::{AnimationDelta, AnimationPlan, AnimationTime};
use crate::assets::skeletal::{JointQuatTrack, JointVec3Track, SkeletalClip};
use crate::assets::{ClipInterpolation, ClipKeyframe};
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::{
    BoneTransforms, ClipInstance, ClipSample, PropertyTrackPlayer, SkeletonInstance, Sprite, SpriteAnimation,
    SpriteAnimationLoopMode, SpriteFrameState, Tint, Transform, TransformTrackPlayer,
};
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::{Commands, Entity, Mut, Query, Res, ResMut, Without};
use bevy_ecs::query::{Added, Changed, Or};
use glam::{Mat4, Quat, Vec3};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

#[cfg(feature = "anim_stats")]
use std::time::{Duration, Instant};

#[cfg(feature = "anim_stats")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "anim_stats")]
#[derive(Clone, Copy, Debug, Default)]
pub struct SpriteAnimationStats {
    pub fast_loop_calls: u64,
    pub event_calls: u64,
    pub plain_calls: u64,
    pub fast_loop_binary_searches: u64,
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
    }
}

#[cfg(feature = "anim_stats")]
pub fn reset_sprite_animation_stats() {
    SPRITE_FAST_LOOP_CALLS.store(0, Ordering::Relaxed);
    SPRITE_EVENT_CALLS.store(0, Ordering::Relaxed);
    SPRITE_PLAIN_CALLS.store(0, Ordering::Relaxed);
    SPRITE_FAST_LOOP_BINARY_SEARCHES.store(0, Ordering::Relaxed);
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

pub fn sys_drive_sprite_animations(
    mut profiler: ResMut<SystemProfiler>,
    animation_plan: Res<AnimationPlan>,
    animation_time: Res<AnimationTime>,
    mut events: ResMut<EventBus>,
    mut animations: Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState)>,
) {
    let _span = profiler.scope("sys_drive_sprite_animations");
    let plan = animation_plan.delta;
    if !plan.has_steps() {
        return;
    }
    let has_group_scales = animation_time.has_group_scales();
    let animation_time_ref: &AnimationTime = &*animation_time;
    match plan {
        AnimationDelta::None => {}
        AnimationDelta::Single(delta) => {
            if delta != 0.0 {
                drive_single(delta, has_group_scales, animation_time_ref, &mut events, &mut animations);
            }
        }
        AnimationDelta::Fixed { step, steps } => {
            if steps > 0 && step != 0.0 {
                drive_fixed(step, steps, has_group_scales, animation_time_ref, &mut events, &mut animations);
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::skeletal::{load_skeleton_from_gltf, SkeletonAsset};
    use crate::assets::{AnimationClip, ClipInterpolation, ClipKeyframe, ClipSegment, ClipVec2Track};
    use crate::ecs::SpriteAnimationFrame;
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
        let skeleton_key = Arc::clone(&fixture.skeleton.name);
        let mut instance = SkeletonInstance::new(skeleton_key, Arc::clone(&fixture.skeleton));
        instance.set_active_clip(Some(Arc::clone(&fixture.clip)));
        instance.set_playing(false);
        instance.clear_dirty();

        let sentinel = Mat4::from_scale(Vec3::splat(3.14));
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
        world.insert_resource(SystemProfiler::new());
        world.insert_resource(AnimationPlan { delta: AnimationDelta::Single(0.05) });
        world.insert_resource(AnimationTime::default());
        world.insert_resource(EventBus::default());

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
        let frames = Arc::from(vec![frame].into_boxed_slice());
        let durations = Arc::from(vec![0.1_f32].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            durations,
            offsets,
            0.1,
            SpriteAnimationLoopMode::Loop,
        );

        let sprite =
            Sprite { atlas_key: Arc::from("atlas"), region: Arc::clone(&region), region_id: 7, uv: [0.0; 4] };
        let frame_state = SpriteFrameState::from_sprite(&sprite);
        world.spawn((animation, sprite, frame_state));

        let mut system_state = SystemState::<(
            ResMut<SystemProfiler>,
            Res<AnimationPlan>,
            Res<AnimationTime>,
            ResMut<EventBus>,
            Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState)>,
        )>::new(&mut world);
        {
            let (profiler, plan, time, events, animations) = system_state.get_mut(&mut world);
            sys_drive_sprite_animations(profiler, plan, time, events, animations);
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
    fn sprite_animation_rewinds_with_negative_delta() {
        let region = Arc::from("frame");
        let frames = Arc::from(
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
        let durations = Arc::from(vec![0.2_f32, 0.2, 0.2].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.2, 0.4].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
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
        let frames = Arc::from(
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
        let durations = Arc::from(vec![0.2_f32, 0.2, 0.2].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.2, 0.4].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
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
        let frames = Arc::from(
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
        let durations = Arc::from(vec![0.08_f32, 0.08, 0.08].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.08, 0.16].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
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
        let frames = Arc::from(
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
        let durations = Arc::from(vec![0.08_f32, 0.12].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.08].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
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
        let frames = Arc::from(
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
        let durations = Arc::from(vec![0.08_f32, 0.08, 0.08].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.08, 0.16].into_boxed_slice());
        let mut animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
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
        world.insert_resource(SystemProfiler::new());
        world.insert_resource(AnimationPlan { delta: AnimationDelta::Fixed { step: 0.1, steps: 3 } });
        world.insert_resource(AnimationTime::default());
        world.insert_resource(EventBus::default());

        let region = Arc::from("frame");
        let frames = Arc::from(
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
        let durations = Arc::from(vec![0.1_f32, 0.1].into_boxed_slice());
        let offsets = Arc::from(vec![0.0_f32, 0.1].into_boxed_slice());
        let animation = SpriteAnimation::new(
            Arc::from("timeline"),
            frames,
            durations,
            offsets,
            0.2,
            SpriteAnimationLoopMode::Loop,
        );

        let sprite =
            Sprite { atlas_key: Arc::from("atlas"), region: Arc::clone(&region), region_id: 0, uv: [0.0; 4] };
        let frame_state = SpriteFrameState::from_sprite(&sprite);
        world.spawn((animation, sprite, frame_state));

        let mut system_state = SystemState::<(
            ResMut<SystemProfiler>,
            Res<AnimationPlan>,
            Res<AnimationTime>,
            ResMut<EventBus>,
            Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState)>,
        )>::new(&mut world);

        let _guard = DriveFixedRecorderGuard::enable();
        {
            let (profiler, plan, time, events, animations) = system_state.get_mut(&mut world);
            sys_drive_sprite_animations(profiler, plan, time, events, animations);
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
    let animation_time_ref: &AnimationTime = &*animation_time;
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
    let animation_time_ref: &AnimationTime = &*animation_time;
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
        if !wants_tint {
            sampling_property_player.apply_tint = false;
        } else if tint.is_none() {
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
            if transform_mask.apply_translation && transform_mask.apply_rotation && transform_mask.apply_scale
            {
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
            } else {
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
        offset = (offset + random_fraction * total).rem_euclid(total.max(std::f32::EPSILON));
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
                    animation.frame_index = (animation.frame_index + 1) % len;
                    animation.refresh_current_duration();
                    emit_frame_event = true;
                    changed_this_step = true;
                }
                SpriteAnimationLoopMode::OnceStop => {
                    animation.frame_index = len.saturating_sub(1);
                    animation.refresh_current_duration();
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
                    animation.frame_index = len.saturating_sub(1);
                    animation.refresh_current_duration();
                    if let Some(last) = animation.frame_durations.last() {
                        animation.elapsed_in_frame = *last;
                    }
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
                        animation.refresh_current_duration();
                    } else if animation.forward {
                        if animation.frame_index + 1 < len {
                            animation.frame_index += 1;
                        } else {
                            animation.forward = false;
                            animation.frame_index = (len - 2).min(len - 1);
                        }
                        animation.refresh_current_duration();
                        changed_this_step = true;
                        emit_frame_event = true;
                    } else if animation.frame_index > 0 {
                        animation.frame_index -= 1;
                        animation.refresh_current_duration();
                        changed_this_step = true;
                        emit_frame_event = true;
                    } else {
                        animation.forward = true;
                        animation.frame_index = 1.min(len - 1);
                        animation.refresh_current_duration();
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
                        if animation.frame_index == 0 {
                            animation.frame_index = len - 1;
                        } else {
                            animation.frame_index -= 1;
                        }
                        animation.refresh_current_duration();
                        animation.elapsed_in_frame = animation.current_duration;
                        delta += animation.current_duration;
                        emit_frame_event = true;
                        changed_this_step = true;
                    } else {
                        animation.refresh_current_duration();
                        animation.elapsed_in_frame = animation.current_duration;
                    }
                }
                SpriteAnimationLoopMode::OnceStop => {
                    if animation.frame_index == 0 {
                        animation.elapsed_in_frame = 0.0;
                        animation.refresh_current_duration();
                        if respect_terminal_behavior {
                            animation.playing = false;
                        }
                        delta = 0.0;
                    } else {
                        animation.frame_index -= 1;
                        animation.refresh_current_duration();
                        animation.elapsed_in_frame = animation.current_duration;
                        delta += animation.current_duration;
                        emit_frame_event = true;
                        changed_this_step = true;
                    }
                }
                SpriteAnimationLoopMode::OnceHold => {
                    if animation.frame_index == 0 {
                        animation.elapsed_in_frame = 0.0;
                        animation.refresh_current_duration();
                        if respect_terminal_behavior {
                            animation.playing = false;
                        }
                        delta = 0.0;
                    } else {
                        animation.frame_index -= 1;
                        animation.refresh_current_duration();
                        animation.elapsed_in_frame = animation.current_duration;
                        delta += animation.current_duration;
                        emit_frame_event = true;
                        changed_this_step = true;
                    }
                }
                SpriteAnimationLoopMode::PingPong => {
                    if len <= 1 {
                        animation.forward = true;
                        animation.refresh_current_duration();
                        animation.elapsed_in_frame = animation.current_duration;
                    } else {
                        let bounced = animation.forward != animation.prev_forward;
                        if bounced {
                            if animation.forward {
                                // just bounced from start
                                animation.forward = animation.prev_forward;
                                animation.frame_index = 0;
                            } else {
                                // just bounced from end
                                animation.forward = animation.prev_forward;
                                animation.frame_index = len - 1;
                            }
                        } else if animation.forward {
                            if animation.frame_index > 0 {
                                animation.frame_index -= 1;
                            } else {
                                animation.frame_index = 0;
                            }
                        } else if animation.frame_index + 1 < len {
                            animation.frame_index += 1;
                        } else {
                            animation.frame_index = len - 1;
                        }
                        animation.refresh_current_duration();
                        animation.elapsed_in_frame = animation.current_duration;
                        delta += animation.current_duration;
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
        for name in frame.events.iter() {
            events.push(GameEvent::SpriteAnimationEvent {
                entity,
                timeline: Arc::clone(&animation.timeline),
                event: Arc::clone(name),
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

    let total = total_duration.max(std::f32::EPSILON);
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
        normalized = total - std::f32::EPSILON;
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

fn drive_single(
    delta: f32,
    has_group_scales: bool,
    animation_time: &AnimationTime,
    events: &mut EventBus,
    animations: &mut Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState)>,
) {
    for (entity, mut animation, mut sprite_state) in animations.iter_mut() {
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

        if animation.pending_start_events {
            if animation.has_events {
                emit_sprite_animation_events(entity, &animation, events);
            }
            animation.pending_start_events = false;
        }

        let playback_rate = if animation.playback_rate_dirty {
            let group_scale =
                if has_group_scales { animation_time.group_scale(animation.group.as_deref()) } else { 1.0 };
            animation.ensure_playback_rate(group_scale)
        } else {
            animation.playback_rate
        };

        if playback_rate == 0.0 {
            continue;
        }

        let scaled = delta * playback_rate;
        if scaled == 0.0 {
            continue;
        }
        let Some(advance_delta) = animation.accumulate_delta(scaled, CLIP_TIME_EPSILON) else {
            continue;
        };

        let mut sprite_changed = false;
        if animation.fast_loop {
            if advance_delta > 0.0 {
                let current_duration = animation.current_duration;
                let new_elapsed = animation.elapsed_in_frame + advance_delta;
                if new_elapsed <= current_duration + CLIP_TIME_EPSILON {
                    animation.elapsed_in_frame = new_elapsed.min(current_duration);
                } else if advance_animation_loop_no_events(&mut animation, advance_delta) {
                    sprite_changed = true;
                }
            } else if advance_delta < 0.0 {
                let new_elapsed = animation.elapsed_in_frame + advance_delta;
                if new_elapsed >= -CLIP_TIME_EPSILON {
                    animation.elapsed_in_frame = new_elapsed.max(0.0);
                } else if advance_animation_loop_no_events(&mut animation, advance_delta) {
                    sprite_changed = true;
                }
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
            if let Some(frame) = animation.frames.get(animation.frame_index) {
                sprite_state.update_from_frame(frame);
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
    animations: &mut Query<(Entity, &mut SpriteAnimation, &mut SpriteFrameState)>,
) {
    for (entity, mut animation, mut sprite_state) in animations.iter_mut() {
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

        if animation.pending_start_events {
            if animation.has_events {
                emit_sprite_animation_events(entity, &animation, events);
            }
            animation.pending_start_events = false;
        }

        let playback_rate = if animation.playback_rate_dirty {
            let group_scale =
                if has_group_scales { animation_time.group_scale(animation.group.as_deref()) } else { 1.0 };
            animation.ensure_playback_rate(group_scale)
        } else {
            animation.playback_rate
        };

        if playback_rate == 0.0 || steps == 0 {
            continue;
        }

        let scaled_step = step * playback_rate;
        if scaled_step == 0.0 {
            continue;
        }

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
                if advance_delta > 0.0 {
                    let current_duration = animation.current_duration;
                    let new_elapsed = animation.elapsed_in_frame + advance_delta;
                    if new_elapsed <= current_duration + CLIP_TIME_EPSILON {
                        animation.elapsed_in_frame = new_elapsed.min(current_duration);
                    } else if advance_animation_loop_no_events(&mut animation, advance_delta) {
                        step_changed = true;
                    }
                } else if advance_delta < 0.0 {
                    let new_elapsed = animation.elapsed_in_frame + advance_delta;
                    if new_elapsed >= -CLIP_TIME_EPSILON {
                        animation.elapsed_in_frame = new_elapsed.max(0.0);
                    } else if advance_animation_loop_no_events(&mut animation, advance_delta) {
                        step_changed = true;
                    }
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
            if let Some(frame) = animation.frames.get(animation.frame_index) {
                sprite_state.update_from_frame(frame);
            }
            continue;
        }
    }
}

pub fn sys_apply_sprite_frame_states(
    mut sprites: Query<
        (&mut Sprite, &mut SpriteFrameState),
        Or<(Changed<SpriteFrameState>, Added<SpriteFrameState>)>,
    >,
) {
    for (mut sprite, mut state) in sprites.iter_mut() {
        if let Some(region) = state.pending_region.take() {
            sprite.region = region;
            state.region_initialized = true;
        }
        sprite.region_id = state.region_id;
        sprite.uv = state.uv;
    }
}
