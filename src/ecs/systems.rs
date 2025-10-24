use super::physics::{CollisionEventKind, ParticleContacts, PhysicsParams, RapierState, SpatialHash, WorldBounds};
use super::types::*;
use crate::events::{EventBus, GameEvent};
use crate::mesh::MeshBounds;
use bevy_ecs::prelude::*;
use bevy_ecs::query::Without;
use bevy_ecs::system::{Commands, Res, ResMut};
use glam::{Mat4, Vec2, Vec3};
use rand::Rng;
use rapier2d::prelude::Vector;
use smallvec::SmallVec;
use std::borrow::Cow;

#[derive(Resource, Clone, Copy)]
pub struct TimeDelta(pub f32);

pub fn sys_apply_spin(mut q: Query<(&mut Transform, &Spin)>, dt: Res<TimeDelta>) {
    for (mut t, s) in &mut q {
        t.rotation += s.speed * dt.0;
    }
}

pub fn sys_solve_forces(
    mut q: Query<(&mut Velocity, &mut Force, &Mass, Option<&RapierBody>)>,
    params: Res<PhysicsParams>,
    dt: Res<TimeDelta>,
) {
    for (mut vel, mut force, mass, rapier_body) in &mut q {
        if rapier_body.is_some() {
            continue;
        }
        if mass.0 <= 0.0 {
            continue;
        }
        let acceleration = (force.0 / mass.0) + params.gravity;
        vel.0 += acceleration * dt.0;
        vel.0 *= 1.0 / (1.0 + params.linear_damping * dt.0);
        force.0 = Vec2::ZERO;
    }
}

pub fn sys_integrate_positions(
    mut q: Query<(&mut Transform, &Velocity, Option<&RapierBody>)>,
    dt: Res<TimeDelta>,
) {
    for (mut t, v, rapier_body) in &mut q {
        if rapier_body.is_some() {
            continue;
        }
        t.translation += v.0 * dt.0;
    }
}

pub fn sys_step_rapier(mut rapier: ResMut<RapierState>, mut events: ResMut<EventBus>, dt: Res<TimeDelta>) {
    if dt.0 > 0.0 {
        rapier.step(dt.0);
    }
    for (phase, a, b) in rapier.drain_collision_events() {
        match phase {
            CollisionEventKind::Started => events.push(GameEvent::collision_started(a, b)),
            CollisionEventKind::Stopped => events.push(GameEvent::collision_ended(a, b)),
            CollisionEventKind::Force(force) => events.push(GameEvent::collision_force(a, b, force)),
        }
    }
}

pub fn sys_sync_from_rapier(
    rapier: Res<RapierState>,
    mut query: Query<(&RapierBody, &mut Transform, Option<&mut Velocity>)>,
) {
    for (body_handle, mut transform, velocity) in &mut query {
        if let Some(body) = rapier.body(body_handle.handle) {
            let translation = body.translation();
            transform.translation = Vec2::new(translation.x, translation.y);
            transform.rotation = body.rotation().angle();
            if let Some(mut vel) = velocity {
                let linvel = body.linvel();
                vel.0 = Vec2::new(linvel.x, linvel.y);
            }
        }
    }
}

pub fn sys_drive_orbits(
    mut rapier: ResMut<RapierState>,
    query: Query<(&RapierBody, &Transform, &OrbitController)>,
) {
    for (body, transform, orbit) in &query {
        if let Some(rb) = rapier.body_mut(body.handle) {
            let offset = transform.translation - orbit.center;
            let radius_sq = offset.length_squared();
            if radius_sq <= f32::EPSILON {
                continue;
            }
            let tangent = Vec2::new(-offset.y, offset.x) * orbit.angular_speed;
            rb.set_linvel(Vector::new(tangent.x, tangent.y), true);
            rb.wake_up(true);
        }
    }
}

pub fn sys_update_emitters(
    mut commands: Commands,
    mut emitters: Query<(&mut ParticleEmitter, &Transform)>,
    dt: Res<TimeDelta>,
) {
    let mut rng = rand::thread_rng();
    for (mut emitter, transform) in &mut emitters {
        let spawn_rate = emitter.rate.max(0.0);
        emitter.accumulator += spawn_rate * dt.0;
        let count = emitter.accumulator.floor() as i32;
        if count <= 0 {
            continue;
        }
        emitter.accumulator -= count as f32;
        for _ in 0..count {
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
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("green") },
                Tint(emitter.start_color),
                Aabb { half: Vec2::splat((start_size * 0.5).max(0.01)) },
                Particle {
                    lifetime,
                    max_lifetime: lifetime,
                },
                ParticleVisual {
                    start_color: emitter.start_color,
                    end_color: emitter.end_color,
                    start_size: emitter.start_size,
                    end_size: emitter.end_size,
                },
            ));
        }
    }
}

pub fn sys_update_particles(
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

pub fn sys_world_bounds_bounce(
    bounds: Res<WorldBounds>,
    mut q: Query<(&mut Transform, &mut Velocity, Option<&Aabb>, Option<&RapierBody>)>,
) {
    let min = bounds.min;
    let max = bounds.max;
    for (mut t, mut v, aabb, rapier_body) in &mut q {
        if rapier_body.is_some() {
            continue;
        }
        let half = aabb.map_or(Vec2::splat(0.25), |a| a.half);
        if t.translation.x - half.x < min.x {
            t.translation.x = min.x + half.x;
            v.0.x = v.0.x.abs();
        }
        if t.translation.x + half.x > max.x {
            t.translation.x = max.x - half.x;
            v.0.x = -v.0.x.abs();
        }
        if t.translation.y - half.y < min.y {
            t.translation.y = min.y + half.y;
            v.0.y = v.0.y.abs();
        }
        if t.translation.y + half.y > max.y {
            t.translation.y = max.y - half.y;
            v.0.y = -v.0.y.abs();
        }
    }
}

pub fn sys_build_spatial_hash(
    mut grid: ResMut<SpatialHash>,
    q: Query<(Entity, &Transform, &Aabb), Without<RapierBody>>,
) {
    grid.clear();
    for (e, t, a) in &q {
        grid.insert(e, t.translation, a.half);
    }
}

pub fn sys_collide_spatial(
    grid: Res<SpatialHash>,
    mut movers: Query<(Entity, &Transform, &Aabb, &mut Velocity), Without<RapierBody>>,
    positions: Query<(&Transform, &Aabb), Without<RapierBody>>,
    mut events: ResMut<EventBus>,
    mut contacts: ResMut<ParticleContacts>,
) {
    let neighbors = [(-1, -1), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
    let mut checked: SmallVec<[Entity; 16]> = SmallVec::new();
    let mut previous_pairs = std::mem::take(&mut contacts.pairs);
    contacts.pairs.clear();
    for (e, t, a, mut v) in &mut movers {
        let key = grid.key(t.translation);
        let mut impulse = Vec2::ZERO;
        checked.clear();
        for (dx, dy) in neighbors {
            if let Some(list) = grid.grid.get(&(key.0 + dx, key.1 + dy)) {
                for &other in list {
                    if other == e || checked.iter().any(|&c| c == other) {
                        continue;
                    }
                    checked.push(other);
                    if let Ok((ot, oa)) = positions.get(other) {
                        if overlap(t.translation, a.half, ot.translation, oa.half) {
                            let delta = t.translation - ot.translation;
                            let dir = delta.signum();
                            impulse += dir * 0.04;
                            let pair = if e.index() <= other.index() { (e, other) } else { (other, e) };
                            if contacts.pairs.insert(pair) && !previous_pairs.remove(&pair) {
                                events.push(GameEvent::collision_started(pair.0, pair.1));
                            }
                        }
                    }
                }
            }
        }
        v.0 += impulse;
    }
    for pair in previous_pairs {
        events.push(GameEvent::collision_ended(pair.0, pair.1));
    }
}

pub fn ray_sphere_intersection(origin: Vec3, dir: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - center;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 {
        return None;
    }
    let sqrt_d = discriminant.sqrt();
    let mut t = -b - sqrt_d;
    if t < 0.0 {
        t = -b + sqrt_d;
    }
    if t < 0.0 {
        return None;
    }
    Some(t)
}

pub fn ray_hit_obb(origin: Vec3, dir: Vec3, transform: &Transform3D, bounds: &MeshBounds) -> Option<f32> {
    if !transform.scale.is_finite() {
        return None;
    }
    let min_scale = 0.0001;
    let scale = Vec3::new(
        transform.scale.x.abs().max(min_scale),
        transform.scale.y.abs().max(min_scale),
        transform.scale.z.abs().max(min_scale),
    );
    let world = Mat4::from_scale_rotation_translation(scale, transform.rotation, transform.translation);
    let inv = world.inverse();
    if !matrix_is_finite(&inv) {
        return None;
    }
    let origin_local = inv.transform_point3(origin);
    let dir_local = inv.transform_vector3(dir);
    if dir_local.length_squared() <= f32::EPSILON {
        return None;
    }
    let dir_local = dir_local.normalize();
    let (t_local, hit_local) = ray_aabb_intersection(origin_local, dir_local, bounds.min, bounds.max)?;
    if t_local < 0.0 {
        return None;
    }
    let hit_world = world.transform_point3(hit_local);
    let distance = (hit_world - origin).length();
    Some(distance)
}

pub fn matrix_is_finite(mat: &Mat4) -> bool {
    mat.to_cols_array().iter().all(|v| v.is_finite())
}

pub fn ray_aabb_intersection(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let mut t_min: f32 = 0.0;
    let mut t_max: f32 = f32::INFINITY;
    let origin_arr = origin.to_array();
    let dir_arr = dir.to_array();
    let min_arr = min.to_array();
    let max_arr = max.to_array();
    for i in 0..3 {
        let o = origin_arr[i];
        let d = dir_arr[i];
        let min_axis = min_arr[i];
        let max_axis = max_arr[i];
        if d.abs() < 1e-6 {
            if o < min_axis || o > max_axis {
                return None;
            }
        } else {
            let inv_d = 1.0 / d;
            let mut t1 = (min_axis - o) * inv_d;
            let mut t2 = (max_axis - o) * inv_d;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        }
    }
    if t_max < 0.0 {
        return None;
    }
    let t_hit = if t_min >= 0.0 { t_min } else { t_max };
    let hit = origin + dir * t_hit;
    Some((t_hit, hit))
}

pub fn overlap(a_pos: Vec2, a_half: Vec2, b_pos: Vec2, b_half: Vec2) -> bool {
    (a_pos.x - b_pos.x).abs() < (a_half.x + b_half.x) && (a_pos.y - b_pos.y).abs() < (a_half.y + b_half.y)
}
