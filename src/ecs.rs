use crate::assets::AssetManager;
use anyhow::{anyhow, Context, Result};
use bevy_ecs::prelude::*;
use bevy_ecs::system::{Commands, Res, ResMut};
use glam::{Mat4, Vec2};
use rand::Rng;
use std::borrow::Cow;
use std::collections::HashMap;

// ---------- Components ----------
#[derive(Component, Clone, Copy)]
pub struct Transform {
    pub translation: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
}
impl Default for Transform {
    fn default() -> Self {
        Self { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::splat(1.0) }
    }
}
#[derive(Component, Clone, Copy, Default)]
pub struct WorldTransform(pub Mat4);
#[derive(Component, Clone, Copy)]
pub struct Parent(pub Entity);
#[derive(Component, Default)]
pub struct Children(pub Vec<Entity>);
#[derive(Component)]
pub struct Spin {
    pub speed: f32,
}
#[derive(Component, Clone)]
pub struct Sprite {
    pub atlas_key: Cow<'static, str>,
    pub region: Cow<'static, str>,
}
#[derive(Component, Clone, Copy)]
pub struct Velocity(pub Vec2);
#[derive(Component, Clone, Copy)]
pub struct Aabb {
    pub half: Vec2,
}
#[derive(Component, Clone, Copy, Default)]
pub struct Mass(pub f32);
#[derive(Component, Clone, Copy, Default)]
pub struct Force(pub Vec2);
#[derive(Component)]
pub struct ParticleEmitter {
    pub rate: f32,
    pub spread: f32,
    pub speed: f32,
    pub lifetime: f32,
    pub accumulator: f32,
}
#[derive(Component)]
pub struct Particle {
    pub lifetime: f32,
    pub max_lifetime: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceData {
    pub model: [[f32; 4]; 4],
    pub uv_rect: [f32; 4],
}

pub struct EntityInfo {
    pub translation: Vec2,
    pub velocity: Option<Vec2>,
    pub sprite_region: Option<String>,
}

#[derive(Resource)]
pub struct PhysicsParams {
    pub gravity: Vec2,
    pub linear_damping: f32,
}

// ---------- Spatial hash ----------
#[derive(Resource)]
pub struct SpatialHash {
    pub cell: f32,
    pub grid: HashMap<(i32, i32), Vec<Entity>>,
}
impl SpatialHash {
    pub fn new(cell: f32) -> Self {
        Self { cell, grid: HashMap::new() }
    }
    pub fn clear(&mut self) {
        self.grid.clear();
    }
    fn key(&self, p: Vec2) -> (i32, i32) {
        ((p.x / self.cell).floor() as i32, (p.y / self.cell).floor() as i32)
    }
    pub fn insert(&mut self, e: Entity, pos: Vec2, half: Vec2) {
        let min = pos - half;
        let max = pos + half;
        let (kx0, ky0) = self.key(min);
        let (kx1, ky1) = self.key(max);
        for ky in ky0..=ky1 {
            for kx in kx0..=kx1 {
                self.grid.entry((kx, ky)).or_default().push(e);
            }
        }
    }
}

// ---------- World container ----------
pub struct EcsWorld {
    pub world: World,
    schedule_var: Schedule,
    schedule_fixed: Schedule,
}

impl EcsWorld {
    pub fn new() -> Self {
        let mut world = World::new();
        world.insert_resource(TimeDelta(0.0));
        world.insert_resource(SpatialHash::new(0.25));
        world.insert_resource(PhysicsParams { gravity: Vec2::new(0.0, -0.6), linear_damping: 0.3 });

        let mut schedule_var = Schedule::default();
        schedule_var.add_systems((
            sys_apply_spin,
            sys_propagate_transforms,
            sys_update_emitters,
            sys_update_particles,
        ));

        let mut schedule_fixed = Schedule::default();
        schedule_fixed.add_systems((
            sys_solve_forces,
            sys_integrate_positions,
            sys_world_bounds_bounce,
            sys_build_spatial_hash,
            sys_collide_spatial,
        ));

        Self { world, schedule_var, schedule_fixed }
    }

    pub fn spawn_demo_scene(&mut self) -> Entity {
        let root = self
            .world
            .spawn((
                Transform { translation: Vec2::new(0.0, 0.0), rotation: 0.0, scale: Vec2::splat(1.2) },
                WorldTransform::default(),
                Spin { speed: 1.2 },
            ))
            .id();
        let a = self
            .world
            .spawn((
                Transform { translation: Vec2::new(-0.9, 0.0), rotation: 0.0, scale: Vec2::splat(0.7) },
                WorldTransform::default(),
                Parent(root),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("checker") },
                Aabb { half: Vec2::splat(0.35) },
                Velocity(Vec2::new(0.2, 0.0)),
                Force::default(),
                Mass(1.0),
            ))
            .id();
        let b = self
            .world
            .spawn((
                Transform { translation: Vec2::new(0.9, 0.0), rotation: 0.0, scale: Vec2::splat(0.6) },
                WorldTransform::default(),
                Parent(root),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
                Aabb { half: Vec2::splat(0.30) },
                Velocity(Vec2::new(-0.25, 0.0)),
                Force::default(),
                Mass(1.0),
            ))
            .id();
        let c = self
            .world
            .spawn((
                Transform { translation: Vec2::new(0.0, 0.9), rotation: 0.0, scale: Vec2::splat(0.5) },
                WorldTransform::default(),
                Parent(root),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("bluebox") },
                Aabb { half: Vec2::splat(0.25) },
                Velocity(Vec2::new(0.0, -0.2)),
                Force::default(),
                Mass(1.0),
            ))
            .id();
        self.world.entity_mut(root).insert(Children(vec![a, b, c]));
        let emitter =
            self.spawn_particle_emitter(Vec2::new(0.0, 0.0), 35.0, std::f32::consts::PI / 3.0, 0.8, 1.2);
        emitter
    }

    pub fn spawn_burst(&mut self, _assets: &AssetManager, count: usize) {
        let regions = ["checker", "redorb", "bluebox", "green"];
        let mut rng = rand::thread_rng();
        for _ in 0..count {
            let rname = regions[rng.gen_range(0..regions.len())];
            let pos = Vec2::new(rng.gen_range(-1.2..1.2), rng.gen_range(-0.9..0.9));
            let vel = Vec2::new(rng.gen_range(-1.0..1.0), rng.gen_range(-1.0..1.0)) * 0.6;
            let scale = rng.gen_range(0.15..0.35);
            let half = Vec2::splat(scale * 0.5);
            self.world.spawn((
                Transform { translation: pos, rotation: 0.0, scale: Vec2::splat(scale) },
                WorldTransform::default(),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed(rname) },
                Aabb { half },
                Velocity(vel),
                Force::default(),
                Mass(0.8),
            ));
        }
    }

    pub fn spawn_particle_emitter(
        &mut self,
        position: Vec2,
        rate: f32,
        spread: f32,
        speed: f32,
        lifetime: f32,
    ) -> Entity {
        self.world
            .spawn((
                Transform { translation: position, rotation: 0.0, scale: Vec2::splat(0.2) },
                WorldTransform::default(),
                ParticleEmitter { rate, spread, speed, lifetime, accumulator: 0.0 },
            ))
            .id()
    }

    pub fn set_emitter_rate(&mut self, entity: Entity, rate: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.rate = rate.max(0.0);
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.world.resource_mut::<TimeDelta>().0 = dt;
        self.schedule_var.run(&mut self.world);
    }
    pub fn fixed_step(&mut self, dt: f32) {
        self.world.resource_mut::<TimeDelta>().0 = dt;
        self.schedule_fixed.run(&mut self.world);
    }
    pub fn adjust_root_spin(&mut self, delta: f32) {
        let mut q = self.world.query::<&mut Spin>();
        for mut s in q.iter_mut(&mut self.world) {
            s.speed += delta;
            break;
        }
    }
    pub fn spawn_scripted_sprite(
        &mut self,
        assets: &AssetManager,
        atlas: &str,
        region: &str,
        position: Vec2,
        scale: f32,
        velocity: Vec2,
    ) -> Result<Entity> {
        if scale <= 0.0 {
            return Err(anyhow!("Scale must be positive"));
        }
        if atlas != "main" {
            return Err(anyhow!("Only atlas 'main' is supported by the current renderer"));
        }
        if !assets.atlas_region_exists(atlas, region) {
            return Err(anyhow!("Region '{region}' not found in atlas '{atlas}'"));
        }
        let half = Vec2::splat(scale * 0.5);
        let entity = self
            .world
            .spawn((
                Transform { translation: position, rotation: 0.0, scale: Vec2::splat(scale) },
                WorldTransform::default(),
                Sprite { atlas_key: Cow::Owned(atlas.to_string()), region: Cow::Owned(region.to_string()) },
                Aabb { half },
                Velocity(velocity),
            ))
            .id();
        Ok(entity)
    }
    pub fn set_velocity(&mut self, entity: Entity, velocity: Vec2) -> bool {
        if let Some(mut vel) = self.world.get_mut::<Velocity>(entity) {
            vel.0 = velocity;
            true
        } else {
            false
        }
    }
    pub fn set_translation(&mut self, entity: Entity, translation: Vec2) -> bool {
        if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
            transform.translation = translation;
            true
        } else {
            false
        }
    }
    pub fn collect_sprite_instances(
        &mut self,
        assets: &AssetManager,
    ) -> Result<(Vec<InstanceData>, &'static str)> {
        let mut out = Vec::new();
        let atlas_key = "main";
        let mut q = self.world.query::<(&WorldTransform, &Sprite)>();
        for (wt, s) in q.iter(&self.world) {
            let uv_rect = assets
                .atlas_region_uv(atlas_key, s.region.as_ref())
                .with_context(|| format!("Collecting sprite instance for region '{}'", s.region))?;
            out.push(InstanceData { model: wt.0.to_cols_array_2d(), uv_rect });
        }
        Ok((out, atlas_key))
    }
    pub fn entity_count(&self) -> usize {
        self.world.entities().len() as usize
    }
    pub fn set_spatial_cell(&mut self, cell: f32) {
        let mut grid = self.world.resource_mut::<SpatialHash>();
        grid.cell = cell;
    }
    pub fn pick_entity(&mut self, world_pos: Vec2) -> Option<Entity> {
        let mut query = self.world.query::<(Entity, &WorldTransform, Option<&Aabb>)>();
        query.iter(&self.world).find_map(|(entity, wt, aabb)| {
            let center = Vec2::new(wt.0.w_axis.x, wt.0.w_axis.y);
            let half = aabb.map_or(Vec2::splat(0.25), |a| a.half);
            if (world_pos.x - center.x).abs() <= half.x && (world_pos.y - center.y).abs() <= half.y {
                Some(entity)
            } else {
                None
            }
        })
    }
    pub fn entity_bounds(&self, entity: Entity) -> Option<(Vec2, Vec2)> {
        let wt = self.world.get::<WorldTransform>(entity)?;
        let center = Vec2::new(wt.0.w_axis.x, wt.0.w_axis.y);
        let half = self.world.get::<Aabb>(entity).map(|a| a.half).unwrap_or(Vec2::splat(0.25));
        Some((center - half, center + half))
    }
    pub fn entity_info(&self, entity: Entity) -> Option<EntityInfo> {
        let wt = self.world.get::<WorldTransform>(entity)?;
        let translation = Vec2::new(wt.0.w_axis.x, wt.0.w_axis.y);
        let velocity = self.world.get::<Velocity>(entity).map(|v| v.0);
        let sprite_region = self.world.get::<Sprite>(entity).map(|s| s.region.to_string());
        Some(EntityInfo { translation, velocity, sprite_region })
    }
    pub fn entity_exists(&self, entity: Entity) -> bool {
        self.world.get_entity(entity).is_ok()
    }
    pub fn despawn_entity(&mut self, entity: Entity) -> bool {
        if let Some(parent) = self.world.get::<Parent>(entity).copied() {
            if let Some(mut siblings) = self.world.get_mut::<Children>(parent.0) {
                siblings.0.retain(|&child| child != entity);
            }
        }
        let child_ids = self.world.get::<Children>(entity).map(|c| c.0.clone()).unwrap_or_default();
        let mut removed = false;
        for child in child_ids {
            removed |= self.despawn_entity(child);
        }
        removed |= self.world.despawn(entity);
        removed
    }
    pub fn set_root_spin(&mut self, speed: f32) {
        let mut query = self.world.query::<&mut Spin>();
        for mut spin in query.iter_mut(&mut self.world) {
            spin.speed = speed;
            break;
        }
    }
}

// ---------- Systems ----------
#[derive(Resource, Clone, Copy)]
pub struct TimeDelta(pub f32);

fn sys_apply_spin(mut q: Query<(&mut Transform, &Spin)>, dt: Res<TimeDelta>) {
    for (mut t, s) in &mut q {
        t.rotation += s.speed * dt.0;
    }
}

fn sys_propagate_transforms(
    mut sets: ParamSet<(
        Query<(Entity, &Transform, Option<&Parent>, &WorldTransform)>,
        Query<&mut WorldTransform>,
    )>,
) {
    for _ in 0..2 {
        let mut updates = Vec::new();
        {
            let world_query = sets.p0();
            for (entity, transform, parent, _current) in world_query.iter() {
                let local = mat_from_transform(*transform);
                let world_mat = if let Some(parent) = parent {
                    world_query.get(parent.0).map(|(_, _, _, parent_wt)| parent_wt.0 * local).unwrap_or(local)
                } else {
                    local
                };
                updates.push((entity, world_mat));
            }
        }
        {
            let mut world_mut = sets.p1();
            for (entity, mat) in updates {
                if let Ok(mut wt) = world_mut.get_mut(entity) {
                    wt.0 = mat;
                }
            }
        }
    }
}

fn sys_solve_forces(
    mut q: Query<(&mut Velocity, &mut Force, &Mass)>,
    params: Res<PhysicsParams>,
    dt: Res<TimeDelta>,
) {
    for (mut vel, mut force, mass) in &mut q {
        if mass.0 <= 0.0 {
            continue;
        }
        let acceleration = (force.0 / mass.0) + params.gravity;
        vel.0 += acceleration * dt.0;
        vel.0 *= 1.0 / (1.0 + params.linear_damping * dt.0);
        force.0 = Vec2::ZERO;
    }
}

fn sys_integrate_positions(mut q: Query<(&mut Transform, &Velocity)>, dt: Res<TimeDelta>) {
    for (mut t, v) in &mut q {
        t.translation += v.0 * dt.0;
    }
}

fn sys_update_emitters(
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
            commands.spawn((
                Transform {
                    translation: transform.translation + dir * 0.05,
                    rotation: 0.0,
                    scale: Vec2::splat(0.12),
                },
                WorldTransform::default(),
                Velocity(velocity),
                Force::default(),
                Mass(0.2),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("green") },
                Particle { lifetime, max_lifetime: lifetime },
            ));
        }
    }
}

fn sys_update_particles(
    mut commands: Commands,
    mut particles: Query<(Entity, &mut Particle, &mut Transform, Option<&mut Velocity>)>,
    dt: Res<TimeDelta>,
) {
    for (entity, mut particle, mut transform, velocity) in &mut particles {
        particle.lifetime -= dt.0;
        if particle.lifetime <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        let t = (particle.lifetime / particle.max_lifetime).clamp(0.0, 1.0);
        transform.scale = Vec2::splat(0.12 * t.max(0.2));
        if let Some(mut vel) = velocity {
            vel.0 *= 0.98;
        }
    }
}

fn sys_world_bounds_bounce(mut q: Query<(&mut Transform, &mut Velocity, Option<&Aabb>)>) {
    let min = Vec2::new(-1.4, -1.0);
    let max = Vec2::new(1.4, 1.0);
    for (mut t, mut v, aabb) in &mut q {
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

fn sys_build_spatial_hash(mut grid: ResMut<SpatialHash>, q: Query<(Entity, &Transform, &Aabb)>) {
    grid.clear();
    for (e, t, a) in &q {
        grid.insert(e, t.translation, a.half);
    }
}

fn sys_collide_spatial(
    grid: Res<SpatialHash>,
    mut movers: Query<(Entity, &Transform, &Aabb, &mut Velocity)>,
    positions: Query<(&Transform, &Aabb)>,
) {
    let neighbors = [(-1, -1), (0, -1), (1, -1), (-1, 0), (0, 0), (1, 0), (-1, 1), (0, 1), (1, 1)];
    for (e, t, a, mut v) in &mut movers {
        let key = grid.key(t.translation);
        let mut impulse = Vec2::ZERO;
        for (dx, dy) in neighbors {
            if let Some(list) = grid.grid.get(&(key.0 + dx, key.1 + dy)) {
                for &other in list {
                    if other == e {
                        continue;
                    }
                    if let Ok((ot, oa)) = positions.get(other) {
                        if overlap(t.translation, a.half, ot.translation, oa.half) {
                            let delta = t.translation - ot.translation;
                            let dir = delta.signum();
                            impulse += dir * 0.04;
                        }
                    }
                }
            }
        }
        v.0 += impulse;
    }
}

fn mat_from_transform(t: Transform) -> Mat4 {
    let (sx, sy) = (t.scale.x, t.scale.y);
    let (s, c) = t.rotation.sin_cos();
    Mat4::from_cols_array(&[
        c * sx,
        s * sx,
        0.0,
        0.0,
        -s * sy,
        c * sy,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        t.translation.x,
        t.translation.y,
        0.0,
        1.0,
    ])
}

fn overlap(a_pos: Vec2, a_half: Vec2, b_pos: Vec2, b_half: Vec2) -> bool {
    (a_pos.x - b_pos.x).abs() < (a_half.x + b_half.x) && (a_pos.y - b_pos.y).abs() < (a_half.y + b_half.y)
}
