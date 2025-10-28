use super::TimeDelta;
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::{Sprite, SpriteAnimation, SpriteAnimationLoopMode};
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::{Entity, Query, Res, ResMut};
use std::borrow::Cow;

pub fn sys_drive_sprite_animations(
    mut profiler: ResMut<SystemProfiler>,
    dt: Res<TimeDelta>,
    mut events: ResMut<EventBus>,
    mut query: Query<(Entity, &mut Sprite, &mut SpriteAnimation)>,
) {
    let _span = profiler.scope("sys_drive_sprite_animations");
    let delta = dt.0;
    if delta <= 0.0 {
        return;
    }
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
        let mut remaining = delta * animation.speed.max(0.0);
        if remaining <= 0.0 {
            continue;
        }
        let len = animation.frames.len();
        while remaining > 0.0 && animation.playing {
            let frame_duration = animation.frames[animation.frame_index].duration.max(std::f32::EPSILON);
            let time_left = frame_duration - animation.elapsed_in_frame;
            if remaining < time_left {
                animation.elapsed_in_frame += remaining;
                remaining = 0.0;
            } else {
                remaining -= time_left;
                animation.elapsed_in_frame = 0.0;
                let mut emit_frame_event = false;
                match animation.mode {
                    SpriteAnimationLoopMode::Loop => {
                        if animation.frame_index + 1 < len {
                            animation.frame_index += 1;
                        } else {
                            animation.frame_index = 0;
                        }
                        emit_frame_event = true;
                    }
                    SpriteAnimationLoopMode::OnceStop => {
                        animation.frame_index = len.saturating_sub(1);
                        emit_sprite_animation_events(entity, &animation, &mut events);
                        animation.playing = false;
                        break;
                    }
                    SpriteAnimationLoopMode::OnceHold => {
                        animation.frame_index = len.saturating_sub(1);
                        if let Some(last) = animation.frames.last() {
                            animation.elapsed_in_frame = last.duration.max(std::f32::EPSILON);
                        }
                        emit_sprite_animation_events(entity, &animation, &mut events);
                        animation.playing = false;
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
                            emit_frame_event = true;
                        } else if animation.frame_index > 0 {
                            animation.frame_index -= 1;
                            emit_frame_event = true;
                        } else {
                            animation.forward = true;
                            animation.frame_index = 1.min(len - 1);
                            emit_frame_event = len > 1;
                        }
                    }
                }
                if emit_frame_event {
                    emit_sprite_animation_events(entity, &animation, &mut events);
                }
            }
        }
        if let Some(region) = animation.current_region_name() {
            if sprite.region.as_ref() != region {
                sprite.region = Cow::Owned(region.to_string());
            }
        }
    }
}

fn emit_sprite_animation_events(entity: Entity, animation: &SpriteAnimation, events: &mut EventBus) {
    if let Some(frame) = animation.frames.get(animation.frame_index) {
        for name in &frame.events {
            events.push(GameEvent::SpriteAnimationEvent {
                entity,
                timeline: animation.timeline.clone(),
                event: name.clone(),
            });
        }
    }
}
