use super::TimeDelta;
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::types::*;
use bevy_ecs::prelude::*;
use bevy_ecs::query::With;
use bevy_ecs::system::{Commands, Res};
use glam::Vec2;
use rand::Rng;
use std::sync::Arc;

pub fn sys_update_emitters(
    mut profiler: ResMut<SystemProfiler>,
    mut commands: Commands,
    mut emitters: Query<(&mut ParticleEmitter, &Transform)>,
    particles: Query<Entity, With<Particle>>,
    caps: Res<ParticleCaps>,
    dt: Res<TimeDelta>,
) {
    let _span = profiler.scope("sys_update_emitters");
    let mut rng = rand::thread_rng();
    let existing_particles = particles.iter().count();
    let max_total = caps.max_total as usize;
    let max_spawn_per_frame = caps.max_spawn_per_frame as usize;
    let mut spawn_budget = (max_total.saturating_sub(existing_particles)).min(max_spawn_per_frame) as i32;

    if spawn_budget <= 0 {
        for (mut emitter, _) in emitters.iter_mut() {
            emitter.accumulator = emitter.accumulator.min(caps.max_emitter_backlog);
        }
        return;
    }

    for (mut emitter, transform) in emitters.iter_mut() {
        let spawn_rate = emitter.rate.max(0.0);
        emitter.accumulator = (emitter.accumulator + spawn_rate * dt.0).min(caps.max_emitter_backlog);
        let desired = emitter.accumulator.floor() as i32;
        if desired <= 0 {
            continue;
        }
        let to_spawn = desired.min(spawn_budget);
        if to_spawn <= 0 {
            continue;
        }
        emitter.accumulator -= to_spawn as f32;
        spawn_budget -= to_spawn;

        for _ in 0..to_spawn {
            let angle = rng.gen_range(-emitter.spread..=emitter.spread);
            let dir = Vec2::from_angle(std::f32::consts::FRAC_PI_2 + angle);
            let velocity = dir * emitter.speed;
            let lifetime = emitter.lifetime;
            let start_size = emitter.start_size.max(0.01);
            commands.spawn((
                Transform {
                    translation: transform.translation + dir * 0.05,
                    rotation: 0.0,
                    scale: Vec2::splat(start_size),
                },
                WorldTransform::default(),
                Velocity(velocity),
                Force::default(),
                Mass(0.2),
                Sprite::uninitialized(Arc::from("main"), Arc::from("green")),
                Tint(emitter.start_color),
                Aabb { half: Vec2::splat((start_size * 0.5).max(0.01)) },
                Particle { lifetime, max_lifetime: lifetime },
                ParticleVisual {
                    start_color: emitter.start_color,
                    end_color: emitter.end_color,
                    start_size: emitter.start_size,
                    end_size: emitter.end_size,
                },
            ));
        }

        if spawn_budget <= 0 {
            break;
        }
    }
}

pub fn sys_update_particles(
    mut profiler: ResMut<SystemProfiler>,
    mut commands: Commands,
    mut particles: Query<(
        Entity,
        &mut Particle,
        &mut Transform,
        Option<&mut Velocity>,
        &ParticleVisual,
        &mut Tint,
        Option<&mut Aabb>,
    )>,
    dt: Res<TimeDelta>,
) {
    let _span = profiler.scope("sys_update_particles");
    for (entity, mut particle, mut transform, velocity, visual, mut tint, aabb) in &mut particles {
        particle.lifetime -= dt.0;
        if particle.lifetime <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let life_ratio = (particle.lifetime / particle.max_lifetime).clamp(0.0, 1.0);
        let progress = 1.0 - life_ratio;
        let size = visual.start_size + (visual.end_size - visual.start_size) * progress;
        transform.scale = Vec2::splat(size.max(0.01));
        if let Some(mut half) = aabb {
            half.half = Vec2::splat((size * 0.5).max(0.01));
        }
        let color = visual.start_color + (visual.end_color - visual.start_color) * progress;
        tint.0 = color;
        if let Some(mut vel) = velocity {
            vel.0 *= 0.98;
        }
    }
}
