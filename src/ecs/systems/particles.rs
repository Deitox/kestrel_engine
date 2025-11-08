use super::TimeDelta;
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::types::*;
use bevy_ecs::prelude::*;
use bevy_ecs::system::{Commands, Res};
use glam::Vec2;
use rand::Rng;
use std::sync::Arc;

pub fn sys_update_emitters(
    mut profiler: ResMut<SystemProfiler>,
    mut commands: Commands,
    mut emitters: Query<(&mut ParticleEmitter, &Transform)>,
    caps: Res<ParticleCaps>,
    mut particle_state: ResMut<ParticleState>,
    dt: Res<TimeDelta>,
) {
    let _span = profiler.scope("sys_update_emitters");
    let mut rng = rand::thread_rng();
    let max_total = caps.max_total as i32;
    let max_spawn_per_frame = caps.max_spawn_per_frame as i32;
    let mut active_particles = particle_state.active_particles.min(caps.max_total) as i32;
    active_particles = active_particles.clamp(0, max_total);
    let mut remaining_headroom = (max_total - active_particles).max(0);
    let mut frame_budget = remaining_headroom.min(max_spawn_per_frame);

    for (mut emitter, transform) in emitters.iter_mut() {
        let spawn_rate = emitter.rate.max(0.0);
        emitter.accumulator = (emitter.accumulator + spawn_rate * dt.0).min(caps.max_emitter_backlog);

        if frame_budget <= 0 || remaining_headroom <= 0 {
            continue;
        }

        let desired = emitter.accumulator.floor() as i32;
        if desired <= 0 {
            continue;
        }
        let to_spawn = desired.min(frame_budget).min(remaining_headroom);
        if to_spawn <= 0 {
            continue;
        }
        emitter.accumulator -= to_spawn as f32;
        let mut batch = Vec::with_capacity(to_spawn as usize);
        for _ in 0..to_spawn {
            let angle = rng.gen_range(-emitter.spread..=emitter.spread);
            let dir = Vec2::from_angle(transform.rotation + std::f32::consts::FRAC_PI_2 + angle);
            let velocity = dir * emitter.speed;
            let lifetime = emitter.lifetime;
            let start_size = emitter.start_size.max(0.01);
            batch.push((
                Transform {
                    translation: transform.translation + dir * 0.05,
                    rotation: 0.0,
                    scale: Vec2::splat(start_size),
                },
                Velocity(velocity),
                Force::default(),
                Mass(0.2),
                Sprite::uninitialized(Arc::clone(&emitter.atlas), Arc::clone(&emitter.region)),
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
        commands.spawn_batch(batch);
        frame_budget -= to_spawn;
        remaining_headroom -= to_spawn;
        active_particles = (active_particles + to_spawn).min(max_total);
    }

    particle_state.active_particles = active_particles.max(0) as u32;
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
    mut particle_state: ResMut<ParticleState>,
) {
    let _span = profiler.scope("sys_update_particles");
    let mut active_particles = 0u32;
    for (entity, mut particle, mut transform, velocity, visual, mut tint, aabb) in &mut particles {
        particle.lifetime -= dt.0;
        if particle.lifetime <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        active_particles = active_particles.saturating_add(1);
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
    particle_state.active_particles = active_particles;
}
