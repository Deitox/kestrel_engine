use super::{AnimationDelta, AnimationTime, TimeDelta};
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::{Sprite, SpriteAnimation, SpriteAnimationLoopMode};
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::{Entity, Query, Res, ResMut};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

pub fn sys_drive_sprite_animations(
    mut profiler: ResMut<SystemProfiler>,
    dt: Res<TimeDelta>,
    mut animation_time: ResMut<AnimationTime>,
    mut events: ResMut<EventBus>,
    mut query: Query<(Entity, &mut Sprite, &mut SpriteAnimation)>,
) {
    let _span = profiler.scope("sys_drive_sprite_animations");
    let plan = animation_time.consume(dt.0);
    let has_steps = plan.has_steps();

    for (entity, mut sprite, mut animation) in query.iter_mut() {
        if animation.frames.is_empty() {
            continue;
        }
        if animation.frame_index >= animation.frames.len() {
            animation.frame_index = 0;
            animation.elapsed_in_frame = 0.0;
        }
        if !animation.playing {
            continue;
        }
        if !has_steps {
            continue;
        }
        let group_scale = animation_time.group_scale(animation.group());
        if group_scale <= 0.0 {
            continue;
        }

        let mut sprite_changed = false;
        match plan {
            AnimationDelta::None => {}
            AnimationDelta::Single(delta) => {
                let scaled = delta * animation.speed.max(0.0) * group_scale;
                if scaled > 0.0 {
                    if animation.has_events {
                        let events_ref = &mut *events;
                        if advance_animation(&mut animation, scaled, entity, Some(events_ref), true) {
                            sprite_changed = true;
                        }
                    } else if advance_animation(&mut animation, scaled, entity, None, true) {
                        sprite_changed = true;
                    }
                }
            }
            AnimationDelta::Fixed { step, steps } => {
                if steps == 0 {
                    continue;
                }
                let scaled_step = step * animation.speed.max(0.0) * group_scale;
                if scaled_step <= 0.0 {
                    continue;
                }
                if animation.has_events {
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
            }
        }

        if sprite_changed {
            if let Some(frame) = animation.current_frame() {
                sprite.apply_frame(frame);
            }
        }
    }
}

pub(crate) fn initialize_animation_phase(animation: &mut SpriteAnimation, entity: Entity) -> bool {
    if animation.frames.is_empty() {
        return false;
    }
    animation.frame_index = 0;
    animation.elapsed_in_frame = 0.0;
    animation.forward = true;

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
    if animation.frames.is_empty() {
        return false;
    }

    let len = animation.frames.len();
    let mut frame_changed = false;
    while delta > 0.0 && animation.playing {
        let frame_duration = animation.frames[animation.frame_index].duration;
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
                emit_frame_event = true;
                frame_changed = true;
            }
            SpriteAnimationLoopMode::OnceStop => {
                animation.frame_index = len.saturating_sub(1);
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
                if let Some(last) = animation.frames.last() {
                    animation.elapsed_in_frame = last.duration;
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
                } else if animation.forward {
                    if animation.frame_index + 1 < len {
                        animation.frame_index += 1;
                    } else {
                        animation.forward = false;
                        animation.frame_index = (len - 2).min(len - 1);
                    }
                    frame_changed = true;
                    emit_frame_event = true;
                } else if animation.frame_index > 0 {
                    animation.frame_index -= 1;
                    frame_changed = true;
                    emit_frame_event = true;
                } else {
                    animation.forward = true;
                    animation.frame_index = 1.min(len - 1);
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
