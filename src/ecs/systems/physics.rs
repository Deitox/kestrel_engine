use super::TimeDelta;
use crate::ecs::physics::{
    CollisionEventKind, ParticleContacts, PhysicsParams, RapierState, SpatialHash, WorldBounds,
};
use crate::ecs::types::*;
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::*;
use bevy_ecs::query::Without;
use bevy_ecs::system::{Res, ResMut};
use glam::Vec2;
use rapier2d::prelude::Vector;
use smallvec::SmallVec;

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

fn overlap(a_pos: Vec2, a_half: Vec2, b_pos: Vec2, b_half: Vec2) -> bool {
    (a_pos.x - b_pos.x).abs() < (a_half.x + b_half.x) && (a_pos.y - b_pos.y).abs() < (a_half.y + b_half.y)
}
