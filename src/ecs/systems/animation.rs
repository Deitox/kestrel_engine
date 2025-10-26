use super::TimeDelta;
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::{Sprite, SpriteAnimation};
use bevy_ecs::prelude::{Query, Res, ResMut};
use std::borrow::Cow;

pub fn sys_drive_sprite_animations(
    mut profiler: ResMut<SystemProfiler>,
    dt: Res<TimeDelta>,
    mut query: Query<(&mut Sprite, &mut SpriteAnimation)>,
) {
    let _span = profiler.scope("sys_drive_sprite_animations");
    let delta = dt.0;
    if delta <= 0.0 {
        return;
    }
    for (mut sprite, mut animation) in query.iter_mut() {
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
        while remaining > 0.0 && animation.playing {
            let frame_duration = animation.frames[animation.frame_index].duration.max(std::f32::EPSILON);
            let time_left = frame_duration - animation.elapsed_in_frame;
            if remaining < time_left {
                animation.elapsed_in_frame += remaining;
                remaining = 0.0;
            } else {
                remaining -= time_left;
                animation.elapsed_in_frame = 0.0;
                if animation.frame_index + 1 < animation.frames.len() {
                    animation.frame_index += 1;
                } else if animation.looped {
                    animation.frame_index = 0;
                } else {
                    animation.playing = false;
                    break;
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
