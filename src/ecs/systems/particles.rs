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
        for _ in 0..to_spawn {
            let angle = rng.gen_range(-emitter.spread..=emitter.spread);
            let dir = Vec2::from_angle(transform.rotation + std::f32::consts::FRAC_PI_2 + angle);
            let velocity = dir * emitter.speed;
            let lifetime = emitter.lifetime;
            let start_size = emitter.start_size.max(0.01);
            let mut entity = commands.spawn((
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
            if let Some(trail) = emitter.trail {
                entity.insert(trail);
            }
        }
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
        Option<&mut Force>,
        Option<&Mass>,
        Option<&ParticleTrail>,
    )>,
    force_fields: Query<(&Transform, &ForceField), Without<Particle>>,
    attractors: Query<(&Transform, &ParticleAttractor), Without<Particle>>,
    dt: Res<TimeDelta>,
    mut particle_state: ResMut<ParticleState>,
) {
    let _span = profiler.scope("sys_update_particles");
    let mut field_cache = Vec::new();
    for (transform, field) in force_fields.iter() {
        field_cache.push((transform.translation, *field));
    }
    let mut attractor_cache = Vec::new();
    for (transform, attractor) in attractors.iter() {
        attractor_cache.push((transform.translation, *attractor));
    }

    let mut active_particles = 0u32;
    for (entity, mut particle, mut transform, velocity, visual, mut tint, aabb, force, mass, trail) in
        &mut particles
    {
        particle.lifetime -= dt.0;
        if particle.lifetime <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        active_particles = active_particles.saturating_add(1);
        let life_ratio = (particle.lifetime / particle.max_lifetime).clamp(0.0, 1.0);
        let progress = 1.0 - life_ratio;
        let visual_size = visual.start_size + (visual.end_size - visual.start_size) * progress;

        let mut net_force = Vec2::ZERO;
        let mut velocity_snapshot = None;
        if let Some(mut vel) = velocity {
            let mut accel = Vec2::ZERO;
            let inv_mass = mass.and_then(|m| if m.0 > 0.0 { Some(1.0 / m.0) } else { None }).unwrap_or(1.0);

            for (origin, field) in field_cache.iter() {
                let mut dir = match field.kind {
                    ForceFieldKind::Radial => transform.translation - *origin,
                    ForceFieldKind::Directional => field.direction,
                };
                if dir.length_squared() <= f32::EPSILON && matches!(field.kind, ForceFieldKind::Radial) {
                    continue;
                }
                dir = dir.normalize_or_zero();
                let distance = (transform.translation - *origin).length();
                let falloff = match field.falloff {
                    ForceFalloff::None => 1.0,
                    ForceFalloff::Linear => {
                        if field.radius <= 0.0 {
                            0.0
                        } else {
                            (1.0 - (distance / field.radius)).clamp(0.0, 1.0)
                        }
                    }
                };
                if falloff <= 0.0 && matches!(field.kind, ForceFieldKind::Radial) && distance > field.radius {
                    continue;
                }
                accel += dir * field.strength * falloff;
            }

            for (origin, attractor) in attractor_cache.iter() {
                let to_origin = *origin - transform.translation;
                let dist = to_origin.length();
                if dist <= attractor.min_distance {
                    continue;
                }
                if dist > attractor.radius {
                    continue;
                }
                let dir = to_origin / dist;
                let falloff = match attractor.falloff {
                    ForceFalloff::None => 1.0,
                    ForceFalloff::Linear => {
                        if attractor.radius <= 0.0 {
                            0.0
                        } else {
                            (1.0 - (dist / attractor.radius)).clamp(0.0, 1.0)
                        }
                    }
                };
                if falloff <= 0.0 {
                    continue;
                }
                let mut push = dir * attractor.strength * falloff;
                if attractor.max_acceleration > 0.0 {
                    let max = attractor.max_acceleration;
                    let len_sq = push.length_squared();
                    if len_sq > max * max {
                        push = push.normalize_or_zero() * max;
                    }
                }
                accel += push;
            }

            if accel != Vec2::ZERO {
                vel.0 += accel * inv_mass * dt.0;
                net_force = accel;
            }
            vel.0 *= 0.98;
            velocity_snapshot = Some(vel.0);
        }
        if let Some(mut force) = force {
            force.0 = net_force;
        }

        let mut width = visual_size.max(0.01);
        let mut length = width;
        let mut rotation = transform.rotation;
        let mut fade = 1.0;
        if let Some(trail) = trail {
            let velocity = velocity_snapshot.unwrap_or(Vec2::ZERO);
            let speed = velocity.length();
            let desired_length =
                (speed * trail.length_scale).clamp(trail.min_length, trail.max_length.max(trail.min_length));
            length = desired_length.max(0.01);
            width = width.max(trail.width.max(0.01));
            if speed > f32::EPSILON {
                rotation = velocity.y.atan2(velocity.x) - std::f32::consts::FRAC_PI_2;
            }
            fade = trail.fade.clamp(0.0, 1.0);
        }
        transform.rotation = rotation;
        transform.scale = Vec2::new(width, length);
        if let Some(mut half) = aabb {
            half.half = Vec2::new((width * 0.5).max(0.01), (length * 0.5).max(0.01));
        }
        let mut color = visual.start_color + (visual.end_color - visual.start_color) * progress;
        color.w *= fade;
        tint.0 = color;
    }
    particle_state.active_particles = active_particles;
}
