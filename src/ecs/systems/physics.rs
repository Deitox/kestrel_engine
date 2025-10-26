use super::TimeDelta;
use crate::ecs::physics::{
    CollisionEventKind, ParticleContacts, PhysicsParams, RapierState, SpatialHash, SpatialIndexConfig,
    SpatialMetrics, SpatialMode, SpatialQuadtree, WorldBounds,
};
use crate::ecs::profiler::SystemProfiler;
use crate::ecs::types::*;
use crate::events::{EventBus, GameEvent};
use bevy_ecs::prelude::*;
use bevy_ecs::query::Without;
use bevy_ecs::system::{Res, ResMut};
use glam::Vec2;
use rapier2d::prelude::Vector;
use smallvec::SmallVec;
use std::collections::HashSet;

pub fn sys_apply_spin(
    mut profiler: ResMut<SystemProfiler>,
    mut q: Query<(&mut Transform, &Spin)>,
    dt: Res<TimeDelta>,
) {
    let _span = profiler.scope("sys_apply_spin");
    for (mut t, s) in &mut q {
        t.rotation += s.speed * dt.0;
    }
}

pub fn sys_solve_forces(
    mut profiler: ResMut<SystemProfiler>,
    mut q: Query<(&mut Velocity, &mut Force, &Mass, Option<&RapierBody>)>,
    params: Res<PhysicsParams>,
    dt: Res<TimeDelta>,
) {
    let _span = profiler.scope("sys_solve_forces");
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
    mut profiler: ResMut<SystemProfiler>,
    mut q: Query<(&mut Transform, &Velocity, Option<&RapierBody>)>,
    dt: Res<TimeDelta>,
) {
    let _span = profiler.scope("sys_integrate_positions");
    for (mut t, v, rapier_body) in &mut q {
        if rapier_body.is_some() {
            continue;
        }
        t.translation += v.0 * dt.0;
    }
}

pub fn sys_step_rapier(
    mut profiler: ResMut<SystemProfiler>,
    mut rapier: ResMut<RapierState>,
    mut events: ResMut<EventBus>,
    dt: Res<TimeDelta>,
) {
    let _span = profiler.scope("sys_step_rapier");
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
    mut profiler: ResMut<SystemProfiler>,
    rapier: Res<RapierState>,
    mut query: Query<(&RapierBody, &mut Transform, Option<&mut Velocity>)>,
) {
    let _span = profiler.scope("sys_sync_from_rapier");
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
    mut profiler: ResMut<SystemProfiler>,
    mut rapier: ResMut<RapierState>,
    query: Query<(&RapierBody, &Transform, &OrbitController)>,
) {
    let _span = profiler.scope("sys_drive_orbits");
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
    mut profiler: ResMut<SystemProfiler>,
    bounds: Res<WorldBounds>,
    mut q: Query<(&mut Transform, &mut Velocity, Option<&Aabb>, Option<&RapierBody>)>,
) {
    let _span = profiler.scope("sys_world_bounds_bounce");
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
    mut profiler: ResMut<SystemProfiler>,
    mut grid: ResMut<SpatialHash>,
    mut quadtree: ResMut<SpatialQuadtree>,
    bounds: Res<WorldBounds>,
    settings: Res<SpatialIndexConfig>,
    mut metrics: ResMut<SpatialMetrics>,
    q: Query<(Entity, &Transform, &Aabb), Without<RapierBody>>,
) {
    let _span = profiler.scope("sys_build_spatial_hash");
    grid.clear();
    let mut collider_data: Vec<(Entity, Vec2, Vec2)> = Vec::new();
    for (e, t, a) in &q {
        grid.insert(e, t.translation, a.half);
        collider_data.push((e, t.translation, a.half));
    }
    let occupied_cells = grid.grid.len();
    let mut total_entries = 0usize;
    let mut max_cell_occupancy = 0usize;
    for list in grid.grid.values() {
        total_entries += list.len();
        max_cell_occupancy = max_cell_occupancy.max(list.len());
    }
    let average = if occupied_cells > 0 {
        total_entries as f32 / occupied_cells as f32
    } else {
        0.0
    };
    let use_quadtree = settings.fallback_enabled && average >= settings.density_threshold.max(1.0);
    let mut node_count = 0usize;
    let mode = if use_quadtree {
        quadtree.rebuild(&bounds, &collider_data);
        node_count = quadtree.node_count();
        SpatialMode::Quadtree
    } else {
        quadtree.clear();
        SpatialMode::Grid
    };
    *metrics = SpatialMetrics {
        occupied_cells,
        max_cell_occupancy,
        average_occupancy: average,
        entity_count: collider_data.len(),
        mode,
        quadtree_nodes: node_count,
    };
}

pub fn sys_collide_spatial(
    mut profiler: ResMut<SystemProfiler>,
    grid: Res<SpatialHash>,
    quadtree: Res<SpatialQuadtree>,
    metrics: Res<SpatialMetrics>,
    mut movers: Query<(Entity, &Transform, &Aabb, &mut Velocity), Without<RapierBody>>,
    positions: Query<(&Transform, &Aabb), Without<RapierBody>>,
    mut events: ResMut<EventBus>,
    mut contacts: ResMut<ParticleContacts>,
) {
    let _span = profiler.scope("sys_collide_spatial");
    let mut previous_pairs = std::mem::take(&mut contacts.pairs);
    contacts.pairs.clear();
    let mut checked: SmallVec<[Entity; 16]> = SmallVec::new();
    let mut candidates: SmallVec<[Entity; 16]> = SmallVec::new();
    let neighbors = [(-1, -1), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
    for (e, t, a, mut v) in &mut movers {
        let mut impulse = Vec2::ZERO;
        checked.clear();
        match metrics.mode {
            SpatialMode::Grid => {
                let key = grid.key(t.translation);
                for (dx, dy) in neighbors {
                    if let Some(list) = grid.grid.get(&(key.0 + dx, key.1 + dy)) {
                        process_neighbors(
                            e,
                            t.translation,
                            a.half,
                            list.iter().copied(),
                            &positions,
                            &mut checked,
                            &mut impulse,
                            contacts.as_mut(),
                            &mut previous_pairs,
                            events.as_mut(),
                        );
                    }
                }
            }
            SpatialMode::Quadtree => {
                if quadtree.node_count() == 0 {
                    continue;
                }
                quadtree.query(t.translation, a.half * 1.05, &mut candidates);
                process_neighbors(
                    e,
                    t.translation,
                    a.half,
                    candidates.iter().copied(),
                    &positions,
                    &mut checked,
                    &mut impulse,
                    contacts.as_mut(),
                    &mut previous_pairs,
                    events.as_mut(),
                );
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

fn process_neighbors<'a, I>(
    entity: Entity,
    translation: Vec2,
    half: Vec2,
    neighbors: I,
    positions: &Query<(&Transform, &Aabb), Without<RapierBody>>,
    checked: &mut SmallVec<[Entity; 16]>,
    impulse: &mut Vec2,
    contacts: &mut ParticleContacts,
    previous_pairs: &mut HashSet<(Entity, Entity)>,
    events: &mut EventBus,
) where
    I: IntoIterator<Item = Entity>,
{
    for other in neighbors {
        if other == entity || checked.iter().any(|&c| c == other) {
            continue;
        }
        checked.push(other);
        if let Ok((ot, oa)) = positions.get(other) {
            if overlap(translation, half, ot.translation, oa.half) {
                let delta = translation - ot.translation;
                let dir = delta.signum();
                *impulse += dir * 0.04;
                let pair = if entity.index() <= other.index() { (entity, other) } else { (other, entity) };
                if contacts.pairs.insert(pair) && !previous_pairs.remove(&pair) {
                    events.push(GameEvent::collision_started(pair.0, pair.1));
                }
            }
        }
    }
}
