use crate::assets::AssetManager;
use crate::events::{EventBus, GameEvent};
use crate::scene::{
    ColliderData, ColorData, OrbitControllerData, ParticleEmitterData, Scene, SceneDependencies, SceneEntity,
    SpriteData, TransformData,
};
use anyhow::{anyhow, Context, Result};
use bevy_ecs::prelude::*;
use bevy_ecs::query::{With, Without};
use bevy_ecs::system::{Commands, Res, ResMut};
use glam::{Mat4, Vec2, Vec4};
use rand::Rng;
use rapier2d::geometry::{CollisionEvent, CollisionEventFlags};
use rapier2d::pipeline::{ActiveEvents, EventHandler};
use rapier2d::prelude::*;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

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
#[derive(Component, Clone, Copy)]
pub struct Tint(pub Vec4);
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
    pub start_color: Vec4,
    pub end_color: Vec4,
    pub start_size: f32,
    pub end_size: f32,
}
#[derive(Component)]
pub struct Particle {
    pub lifetime: f32,
    pub max_lifetime: f32,
}
#[derive(Component)]
pub struct ParticleVisual {
    pub start_color: Vec4,
    pub end_color: Vec4,
    pub start_size: f32,
    pub end_size: f32,
}

#[derive(Component, Clone, Copy)]
pub struct RapierBody {
    pub handle: RigidBodyHandle,
}

#[derive(Component, Clone, Copy)]
pub struct RapierCollider {
    pub handle: ColliderHandle,
}

#[derive(Component, Clone, Copy)]
pub struct OrbitController {
    pub center: Vec2,
    pub angular_speed: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceData {
    pub model: [[f32; 4]; 4],
    pub uv_rect: [f32; 4],
    pub tint: [f32; 4],
}

#[derive(Clone)]
pub struct EntityInfo {
    pub translation: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
    pub velocity: Option<Vec2>,
    pub sprite: Option<SpriteInfo>,
    pub tint: Option<Vec4>,
}

#[derive(Clone)]
pub struct SpriteInfo {
    pub atlas: String,
    pub region: String,
}

pub struct EmitterSnapshot {
    pub rate: f32,
    pub spread: f32,
    pub speed: f32,
    pub lifetime: f32,
    pub start_color: Vec4,
    pub end_color: Vec4,
    pub start_size: f32,
    pub end_size: f32,
}

#[derive(Resource)]
pub struct PhysicsParams {
    pub gravity: Vec2,
    pub linear_damping: f32,
}

pub enum CollisionLifecycle {
    Started,
    Stopped,
}

struct CollisionEventCollector {
    events: Mutex<Vec<CollisionEvent>>,
}

impl CollisionEventCollector {
    fn new() -> Self {
        Self { events: Mutex::new(Vec::new()) }
    }

    fn drain(&self) -> Vec<CollisionEvent> {
        if let Ok(mut events) = self.events.lock() {
            std::mem::take(&mut *events)
        } else {
            Vec::new()
        }
    }
}

impl EventHandler for CollisionEventCollector {
    fn handle_collision_event(
        &self,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        event: CollisionEvent,
        _contact_pair: Option<&ContactPair>,
    ) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        _contact_pair: &ContactPair,
        _total_force_magnitude: Real,
    ) {
        // Contact force events are not used for the current milestone.
    }
}

#[derive(Resource)]
pub struct RapierState {
    pipeline: PhysicsPipeline,
    gravity: Vector<Real>,
    integration_parameters: IntegrationParameters,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    query_pipeline: QueryPipeline,
    collider_entities: HashMap<ColliderHandle, Entity>,
    event_collector: CollisionEventCollector,
    boundary_entity: Entity,
}

impl RapierState {
    pub fn new(gravity: Vec2, boundary_entity: Entity) -> Self {
        let mut state = Self {
            pipeline: PhysicsPipeline::new(),
            gravity: vec_to_rapier(gravity),
            integration_parameters: IntegrationParameters::default(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            query_pipeline: QueryPipeline::new(),
            collider_entities: HashMap::new(),
            event_collector: CollisionEventCollector::new(),
            boundary_entity,
        };
        state.init_bounds();
        state
    }

    fn init_bounds(&mut self) {
        let thickness = 0.05;
        let min = vector![-1.4, -1.0];
        let max = vector![1.4, 1.0];
        let horizontal_half = vector![(max.x - min.x) * 0.5 + thickness, thickness];
        let vertical_half = vector![thickness, (max.y - min.y) * 0.5 + thickness];

        let centers = [
            vector![0.0, min.y - thickness],
            vector![0.0, max.y + thickness],
            vector![min.x - thickness, 0.0],
            vector![max.x + thickness, 0.0],
        ];
        let half_extents = [horizontal_half, horizontal_half, vertical_half, vertical_half];

        for (center, half) in centers.into_iter().zip(half_extents) {
            self.insert_static_collider(center, half);
        }
    }

    fn insert_static_collider(&mut self, center: Vector<Real>, half: Vector<Real>) {
        let body = RigidBodyBuilder::fixed().translation(center).build();
        let body_handle = self.bodies.insert(body);
        let collider = ColliderBuilder::cuboid(half.x, half.y).restitution(0.4).friction(0.8).build();
        let handle = self.colliders.insert_with_parent(collider, body_handle, &mut self.bodies);
        self.collider_entities.insert(handle, self.boundary_entity);
    }

    pub fn spawn_dynamic_body(
        &mut self,
        position: Vec2,
        half: Vec2,
        mass: f32,
        velocity: Vec2,
    ) -> (RigidBodyHandle, ColliderHandle) {
        let body = RigidBodyBuilder::dynamic().translation(vector![position.x, position.y]).build();
        let body_handle = self.bodies.insert(body);
        if let Some(body) = self.bodies.get_mut(body_handle) {
            if mass > 0.0 {
                body.set_additional_mass(mass, true);
            }
            body.set_linvel(vector![velocity.x, velocity.y], true);
            body.wake_up(true);
        }
        let collider = ColliderBuilder::cuboid(half.x, half.y)
            .restitution(0.3)
            .friction(0.6)
            .active_events(ActiveEvents::COLLISION_EVENTS)
            .build();
        let collider_handle = self.colliders.insert_with_parent(collider, body_handle, &mut self.bodies);
        (body_handle, collider_handle)
    }

    pub fn resize_collider(&mut self, handle: ColliderHandle, half: Vec2) {
        if let Some(collider) = self.colliders.get_mut(handle) {
            collider.set_shape(SharedShape::cuboid(half.x, half.y));
        }
    }

    pub fn set_body_mass(&mut self, handle: RigidBodyHandle, mass: f32) {
        if let Some(body) = self.bodies.get_mut(handle) {
            body.set_additional_mass(mass, true);
        }
    }

    pub fn remove_body(&mut self, handle: RigidBodyHandle) {
        let collider_handles: Vec<ColliderHandle> = self
            .bodies
            .get(handle)
            .map(|body| body.colliders().iter().copied().collect())
            .unwrap_or_default();
        for collider in collider_handles {
            self.collider_entities.remove(&collider);
        }
        let _ = self.bodies.remove(
            handle,
            &mut self.island_manager,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
    }

    pub fn clear_dynamic(&mut self) {
        let mut to_remove = Vec::new();
        for (handle, body) in self.bodies.iter() {
            if body.is_dynamic() {
                to_remove.push(handle);
            }
        }
        for handle in to_remove {
            self.remove_body(handle);
        }
    }

    pub fn step(&mut self, dt: f32) {
        self.integration_parameters.dt = dt;
        let hooks = ();
        self.pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &hooks,
            &self.event_collector,
        );
        self.query_pipeline.update(&self.colliders);
    }

    pub fn register_collider_entity(&mut self, collider: ColliderHandle, entity: Entity) {
        self.collider_entities.insert(collider, entity);
    }

    pub fn unregister_collider(&mut self, collider: ColliderHandle) {
        self.collider_entities.remove(&collider);
    }

    pub fn drain_collision_events(&mut self) -> Vec<(CollisionLifecycle, Entity, Entity)> {
        let mut out = Vec::new();
        for event in self.event_collector.drain() {
            match event {
                CollisionEvent::Started(a, b, flags) => {
                    if flags.contains(CollisionEventFlags::SENSOR) {
                        continue;
                    }
                    if let (Some(entity_a), Some(entity_b)) =
                        (self.collider_entities.get(&a), self.collider_entities.get(&b))
                    {
                        out.push((CollisionLifecycle::Started, *entity_a, *entity_b));
                    }
                }
                CollisionEvent::Stopped(a, b, flags) => {
                    if flags.contains(CollisionEventFlags::SENSOR) {
                        continue;
                    }
                    if let (Some(entity_a), Some(entity_b)) =
                        (self.collider_entities.get(&a), self.collider_entities.get(&b))
                    {
                        out.push((CollisionLifecycle::Stopped, *entity_a, *entity_b));
                    }
                }
            }
        }
        out
    }

    pub fn boundary_entity(&self) -> Entity {
        self.boundary_entity
    }

    pub fn body(&self, handle: RigidBodyHandle) -> Option<&RigidBody> {
        self.bodies.get(handle)
    }

    pub fn body_mut(&mut self, handle: RigidBodyHandle) -> Option<&mut RigidBody> {
        self.bodies.get_mut(handle)
    }

    pub fn collider(&self, handle: ColliderHandle) -> Option<&Collider> {
        self.colliders.get(handle)
    }
}

fn vec_to_rapier(v: Vec2) -> Vector<Real> {
    vector![v.x, v.y]
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
        world.insert_resource(ParticleContacts::default());
        world.insert_resource(PhysicsParams { gravity: Vec2::new(0.0, -0.6), linear_damping: 0.3 });
        let boundary_entity = world.spawn_empty().id();
        world.insert_resource(RapierState::new(Vec2::new(0.0, -0.6), boundary_entity));
        world.insert_resource(EventBus::default());

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
            sys_drive_orbits,
            sys_step_rapier,
            sys_sync_from_rapier,
            sys_world_bounds_bounce,
            sys_build_spatial_hash,
            sys_collide_spatial,
        ));

        Self { world, schedule_var, schedule_fixed }
    }

    fn emit(&mut self, event: GameEvent) {
        self.world.resource_mut::<EventBus>().push(event);
    }

    pub fn drain_events(&mut self) -> Vec<GameEvent> {
        self.world.resource_mut::<EventBus>().drain()
    }

    pub fn push_event(&mut self, event: GameEvent) {
        self.emit(event);
    }

    pub fn spawn_demo_scene(&mut self) -> Entity {
        let root = self
            .world
            .spawn((
                Transform { translation: Vec2::ZERO, rotation: 0.0, scale: Vec2::splat(0.8) },
                WorldTransform::default(),
                Spin { speed: 1.2 },
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("checker") },
                Tint(Vec4::new(1.0, 1.0, 1.0, 0.2)),
            ))
            .id();
        self.emit(GameEvent::SpriteSpawned {
            entity: root,
            atlas: "main".to_string(),
            region: "checker".to_string(),
        });

        let orbit_center = Vec2::ZERO;
        let orbit_speed_a = 0.9;
        let orbit_speed_b = 0.95;
        let orbit_speed_c = 1.05;

        let translation_a = Vec2::new(-0.9, 0.0);
        let half_a = Vec2::splat(0.35);
        let velocity_a = Vec2::new(-translation_a.y, translation_a.x) * orbit_speed_a;
        let (body_a, collider_a) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(translation_a, half_a, 1.0, velocity_a)
        };
        let a = self
            .world
            .spawn((
                Transform { translation: translation_a, rotation: 0.0, scale: Vec2::splat(0.7) },
                WorldTransform::default(),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("checker") },
                Aabb { half: half_a },
                Velocity(velocity_a),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_a },
                RapierCollider { handle: collider_a },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_a },
            ))
            .id();
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_a, a);
        }
        self.emit(GameEvent::SpriteSpawned {
            entity: a,
            atlas: "main".to_string(),
            region: "checker".to_string(),
        });

        let translation_b = Vec2::new(0.9, 0.0);
        let half_b = Vec2::splat(0.30);
        let velocity_b = Vec2::new(-translation_b.y, translation_b.x) * orbit_speed_b;
        let (body_b, collider_b) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(translation_b, half_b, 1.0, velocity_b)
        };
        let b = self
            .world
            .spawn((
                Transform { translation: translation_b, rotation: 0.0, scale: Vec2::splat(0.6) },
                WorldTransform::default(),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("redorb") },
                Aabb { half: half_b },
                Velocity(velocity_b),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_b },
                RapierCollider { handle: collider_b },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_b },
            ))
            .id();
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_b, b);
        }
        self.emit(GameEvent::SpriteSpawned {
            entity: b,
            atlas: "main".to_string(),
            region: "redorb".to_string(),
        });

        let translation_c = Vec2::new(0.0, 0.9);
        let half_c = Vec2::splat(0.25);
        let velocity_c = Vec2::new(-translation_c.y, translation_c.x) * orbit_speed_c;
        let (body_c, collider_c) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(translation_c, half_c, 1.0, velocity_c)
        };
        let c = self
            .world
            .spawn((
                Transform { translation: translation_c, rotation: 0.0, scale: Vec2::splat(0.5) },
                WorldTransform::default(),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("bluebox") },
                Aabb { half: half_c },
                Velocity(velocity_c),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_c },
                RapierCollider { handle: collider_c },
                OrbitController { center: orbit_center, angular_speed: orbit_speed_c },
            ))
            .id();
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_c, c);
        }
        self.emit(GameEvent::SpriteSpawned {
            entity: c,
            atlas: "main".to_string(),
            region: "bluebox".to_string(),
        });

        let emitter = self.spawn_particle_emitter(
            Vec2::new(0.0, 0.0),
            35.0,
            std::f32::consts::PI / 3.0,
            0.8,
            1.2,
            Vec4::new(1.0, 0.8, 0.2, 0.8),
            Vec4::new(1.0, 0.2, 0.2, 0.0),
            0.18,
            0.05,
        );
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
            let (body_handle, collider_handle) = {
                let mut rapier = self.world.resource_mut::<RapierState>();
                rapier.spawn_dynamic_body(pos, half, 0.8, vel)
            };
            let entity = self
                .world
                .spawn((
                    Transform { translation: pos, rotation: 0.0, scale: Vec2::splat(scale) },
                    WorldTransform::default(),
                    Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed(rname) },
                    Aabb { half },
                    Velocity(vel),
                    Force::default(),
                    Mass(0.8),
                    RapierBody { handle: body_handle },
                    RapierCollider { handle: collider_handle },
                ))
                .id();
            {
                let mut rapier = self.world.resource_mut::<RapierState>();
                rapier.register_collider_entity(collider_handle, entity);
            }
            self.emit(GameEvent::SpriteSpawned {
                entity,
                atlas: "main".to_string(),
                region: rname.to_string(),
            });
        }
    }

    pub fn spawn_particle_emitter(
        &mut self,
        position: Vec2,
        rate: f32,
        spread: f32,
        speed: f32,
        lifetime: f32,
        start_color: Vec4,
        end_color: Vec4,
        start_size: f32,
        end_size: f32,
    ) -> Entity {
        self.world
            .spawn((
                Transform { translation: position, rotation: 0.0, scale: Vec2::splat(start_size) },
                WorldTransform::default(),
                ParticleEmitter {
                    rate,
                    spread,
                    speed,
                    lifetime,
                    accumulator: 0.0,
                    start_color,
                    end_color,
                    start_size,
                    end_size,
                },
            ))
            .id()
    }

    pub fn set_emitter_rate(&mut self, entity: Entity, rate: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.rate = rate.max(0.0);
        }
    }

    pub fn set_emitter_colors(&mut self, entity: Entity, start: Vec4, end: Vec4) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.start_color = start;
            emitter.end_color = end;
        }
    }

    pub fn set_emitter_sizes(&mut self, entity: Entity, start: f32, end: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.start_size = start.max(0.01);
            emitter.end_size = end.max(0.01);
        }
    }

    pub fn clear_particles(&mut self) {
        let mut particles = Vec::new();
        {
            let mut query = self.world.query_filtered::<Entity, With<Particle>>();
            for entity in query.iter(&self.world) {
                particles.push(entity);
            }
        }
        for entity in particles {
            let _ = self.world.despawn(entity);
        }
        let mut emitters = self.world.query::<&mut ParticleEmitter>();
        for mut emitter in emitters.iter_mut(&mut self.world) {
            emitter.accumulator = 0.0;
        }
        self.world.resource_mut::<ParticleContacts>().pairs.clear();
    }

    pub fn clear_world(&mut self) {
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.clear_dynamic();
        }
        let boundary = {
            let rapier = self.world.resource::<RapierState>();
            rapier.boundary_entity()
        };
        let entities: Vec<Entity> =
            self.world.iter_entities().map(|e| e.id()).filter(|entity| *entity != boundary).collect();
        for entity in entities {
            let _ = self.world.despawn(entity);
        }
        self.world.resource_mut::<ParticleContacts>().pairs.clear();
    }

    pub fn set_emitter_spread(&mut self, entity: Entity, spread: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.spread = spread.clamp(0.0, std::f32::consts::PI);
        }
    }

    pub fn set_emitter_speed(&mut self, entity: Entity, speed: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.speed = speed.max(0.0);
        }
    }

    pub fn set_emitter_lifetime(&mut self, entity: Entity, lifetime: f32) {
        if let Some(mut emitter) = self.world.get_mut::<ParticleEmitter>(entity) {
            emitter.lifetime = lifetime.max(0.05);
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
        let (body_handle, collider_handle) = {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.spawn_dynamic_body(position, half, 1.0, velocity)
        };
        let entity = self
            .world
            .spawn((
                Transform { translation: position, rotation: 0.0, scale: Vec2::splat(scale) },
                WorldTransform::default(),
                Sprite { atlas_key: Cow::Owned(atlas.to_string()), region: Cow::Owned(region.to_string()) },
                Aabb { half },
                Velocity(velocity),
                Force::default(),
                Mass(1.0),
                RapierBody { handle: body_handle },
                RapierCollider { handle: collider_handle },
            ))
            .id();
        {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider_handle, entity);
        }
        self.emit(GameEvent::SpriteSpawned { entity, atlas: atlas.to_string(), region: region.to_string() });
        Ok(entity)
    }
    pub fn set_velocity(&mut self, entity: Entity, velocity: Vec2) -> bool {
        let mut updated = false;
        {
            if let Some(mut vel) = self.world.get_mut::<Velocity>(entity) {
                vel.0 = velocity;
                updated = true;
            }
        }
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(body) = rapier.body_mut(handle) {
                body.set_linvel(vector![velocity.x, velocity.y], true);
            }
            updated = true;
        }
        updated
    }
    pub fn set_translation(&mut self, entity: Entity, translation: Vec2) -> bool {
        let mut changed = false;
        {
            if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
                transform.translation = translation;
                changed = true;
            }
        }
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(body) = rapier.body_mut(handle) {
                body.set_translation(vector![translation.x, translation.y], true);
            }
            changed = true;
        }
        changed
    }
    pub fn set_rotation(&mut self, entity: Entity, rotation: f32) -> bool {
        let mut changed = false;
        {
            if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
                transform.rotation = rotation;
                changed = true;
            }
        }
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(body) = rapier.body_mut(handle) {
                body.set_rotation(Rotation::new(rotation), true);
            }
            changed = true;
        }
        changed
    }
    pub fn set_scale(&mut self, entity: Entity, scale: Vec2) -> bool {
        if scale.x.abs() < f32::EPSILON || scale.y.abs() < f32::EPSILON {
            return false;
        }
        let mut changed = false;
        if let Some(mut transform) = self.world.get_mut::<Transform>(entity) {
            transform.scale = scale;
            changed = true;
        }
        let half = Vec2::new(scale.x.abs(), scale.y.abs()) * 0.5;
        let previous_half = self.world.get::<Aabb>(entity).map(|a| a.half);
        if let Some(mut aabb) = self.world.get_mut::<Aabb>(entity) {
            aabb.half = half;
            changed = true;
        }
        let mut new_mass_value = None;
        if let Some(mut mass) = self.world.get_mut::<Mass>(entity) {
            let prev = previous_half.unwrap_or(half);
            let old_area = (prev.x.max(0.01) * 2.0) * (prev.y.max(0.01) * 2.0);
            let new_area = (half.x.max(0.01) * 2.0) * (half.y.max(0.01) * 2.0);
            if old_area > 0.0 {
                mass.0 = (mass.0 * (new_area / old_area)).max(0.01);
                new_mass_value = Some(mass.0);
                changed = true;
            }
        }
        let collider_handle = self.world.get::<RapierCollider>(entity).map(|c| c.handle);
        let body_handle = self.world.get::<RapierBody>(entity).map(|b| b.handle);
        if collider_handle.is_some() || (body_handle.is_some() && new_mass_value.is_some()) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            if let Some(handle) = collider_handle {
                rapier.resize_collider(handle, half);
            }
            if let (Some(handle), Some(mass_value)) = (body_handle, new_mass_value) {
                rapier.set_body_mass(handle, mass_value);
            }
            changed = true;
        }
        changed
    }
    pub fn set_tint(&mut self, entity: Entity, color: Option<Vec4>) -> bool {
        match color {
            Some(color) => {
                if let Some(mut tint) = self.world.get_mut::<Tint>(entity) {
                    tint.0 = color;
                    true
                } else {
                    self.world.entity_mut(entity).insert(Tint(color));
                    true
                }
            }
            None => {
                if self.world.get::<Tint>(entity).is_some() {
                    self.world.entity_mut(entity).remove::<Tint>();
                    true
                } else {
                    false
                }
            }
        }
    }
    pub fn set_sprite_region(&mut self, entity: Entity, assets: &AssetManager, region: &str) -> bool {
        if let Some(mut sprite) = self.world.get_mut::<Sprite>(entity) {
            let atlas = sprite.atlas_key.to_string();
            if !assets.atlas_region_exists(&atlas, region) {
                return false;
            }
            sprite.region = Cow::Owned(region.to_string());
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
        let mut q = self.world.query::<(&WorldTransform, &Sprite, Option<&Tint>)>();
        for (wt, s, tint) in q.iter(&self.world) {
            let uv_rect = assets
                .atlas_region_uv(atlas_key, s.region.as_ref())
                .with_context(|| format!("Collecting sprite instance for region '{}'", s.region))?;
            let color = tint.map(|t| t.0.to_array()).unwrap_or([1.0, 1.0, 1.0, 1.0]);
            out.push(InstanceData { model: wt.0.to_cols_array_2d(), uv_rect, tint: color });
        }
        Ok((out, atlas_key))
    }
    pub fn entity_count(&self) -> usize {
        let boundary = self.world.resource::<RapierState>().boundary_entity();
        self.world.iter_entities().filter(|entity_ref| entity_ref.id() != boundary).count()
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
        let transform = self.world.get::<Transform>(entity)?;
        let world_transform = self.world.get::<WorldTransform>(entity)?;
        let translation = Vec2::new(world_transform.0.w_axis.x, world_transform.0.w_axis.y);
        let velocity = self.world.get::<Velocity>(entity).map(|v| v.0);
        let sprite = self.world.get::<Sprite>(entity).map(|sprite| SpriteInfo {
            atlas: sprite.atlas_key.to_string(),
            region: sprite.region.to_string(),
        });
        let tint = self.world.get::<Tint>(entity).map(|t| t.0);
        Some(EntityInfo {
            translation,
            rotation: transform.rotation,
            scale: transform.scale,
            velocity,
            sprite,
            tint,
        })
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
        if let Some(handle) = self.world.get::<RapierBody>(entity).map(|b| b.handle) {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.remove_body(handle);
        }
        let entity_removed = self.world.despawn(entity);
        if entity_removed {
            removed = true;
            self.emit(GameEvent::EntityDespawned { entity });
        }
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

impl EcsWorld {
    pub fn save_scene_to_path(&mut self, path: impl AsRef<Path>, assets: &AssetManager) -> Result<()> {
        let scene = self.export_scene(assets);
        scene.save_to_path(path)
    }

    pub fn load_scene_from_path(&mut self, path: impl AsRef<Path>, assets: &mut AssetManager) -> Result<()> {
        let scene = Scene::load_from_path(path)?;
        self.ensure_scene_dependencies(&scene, assets)?;
        self.load_scene(&scene, assets)
    }

    fn ensure_scene_dependencies(&self, scene: &Scene, assets: &mut AssetManager) -> Result<()> {
        let mut missing = Vec::new();
        for dep in scene.dependencies.atlas_dependencies() {
            if assets.has_atlas(dep.key()) {
                continue;
            }
            if let Some(path) = dep.path() {
                if let Err(err) = assets.load_atlas(dep.key(), path) {
                    missing.push(format!("{} ({}): {err}", dep.key(), path));
                }
            } else {
                missing.push(format!("{} (no path provided)", dep.key()));
            }
        }
        if missing.is_empty() {
            Ok(())
        } else {
            Err(anyhow!("Scene requires atlases that could not be loaded: {}", missing.join(", ")))
        }
    }

    pub fn load_scene(&mut self, scene: &Scene, assets: &AssetManager) -> Result<()> {
        for dep in scene.dependencies.atlas_dependencies() {
            if !assets.has_atlas(dep.key()) {
                return Err(anyhow!(
                    "Scene requires atlas '{}' which is not loaded. Call AssetManager::load_atlas before loading the scene.",
                    dep.key()
                ));
            }
        }
        self.clear_scene_entities();
        let mut entity_map = Vec::with_capacity(scene.entities.len());
        for entity_data in &scene.entities {
            let entity = self.spawn_scene_entity(entity_data, assets)?;
            entity_map.push(entity);
        }
        for (index, entity_data) in scene.entities.iter().enumerate() {
            if let Some(parent_index) = entity_data.parent {
                let parent_entity = *entity_map
                    .get(parent_index)
                    .ok_or_else(|| anyhow!("Scene entity parent index {parent_index} out of bounds"))?;
                let child_entity = entity_map[index];
                self.world.entity_mut(child_entity).insert(Parent(parent_entity));
                if let Some(mut children) = self.world.get_mut::<Children>(parent_entity) {
                    if !children.0.contains(&child_entity) {
                        children.0.push(child_entity);
                    }
                } else {
                    self.world.entity_mut(parent_entity).insert(Children(vec![child_entity]));
                }
            }
        }
        Ok(())
    }

    pub fn export_scene(&mut self, assets: &AssetManager) -> Scene {
        let mut scene = Scene::default();
        let mut query = self.world.query::<(Entity, Option<&Parent>, Option<&Transform>)>();
        let mut roots = Vec::new();
        for (entity, parent, transform) in query.iter(&self.world) {
            if parent.is_none() && transform.is_some() {
                roots.push(entity);
            }
        }
        for root in roots {
            self.collect_scene_entity(root, None, &mut scene.entities);
        }
        scene.dependencies = SceneDependencies::from_entities(&scene.entities, assets);
        scene
    }

    pub fn first_emitter(&mut self) -> Option<Entity> {
        let mut query = self.world.query::<(Entity, &ParticleEmitter)>();
        query.iter(&self.world).map(|(entity, _)| entity).next()
    }

    pub fn emitter_snapshot(&self, entity: Entity) -> Option<EmitterSnapshot> {
        let emitter = self.world.get::<ParticleEmitter>(entity)?;
        Some(EmitterSnapshot {
            rate: emitter.rate,
            spread: emitter.spread,
            speed: emitter.speed,
            lifetime: emitter.lifetime,
            start_color: emitter.start_color,
            end_color: emitter.end_color,
            start_size: emitter.start_size,
            end_size: emitter.end_size,
        })
    }

    fn spawn_scene_entity(&mut self, data: &SceneEntity, assets: &AssetManager) -> Result<Entity> {
        let translation: Vec2 = data.transform.translation.clone().into();
        let scale: Vec2 = data.transform.scale.clone().into();
        let rotation = data.transform.rotation;
        let velocity_vec: Vec2 = data.velocity.as_ref().map(|v| Vec2::from(v.clone())).unwrap_or(Vec2::ZERO);
        let collider_half = data.collider.as_ref().map(|c| Vec2::from(c.half_extents.clone()));

        let mut body_handle = None;
        let mut collider_handle = None;
        if let Some(half) = collider_half.as_ref() {
            let mass_value = data.mass.unwrap_or(1.0);
            let mut rapier = self.world.resource_mut::<RapierState>();
            let (body, collider) = rapier.spawn_dynamic_body(translation, *half, mass_value, velocity_vec);
            body_handle = Some(body);
            collider_handle = Some(collider);
        }

        let mut entity =
            self.world.spawn((Transform { translation, rotation, scale }, WorldTransform::default()));

        if let Some(spin) = data.spin {
            entity.insert(Spin { speed: spin });
        }
        if let Some(tint) = data.tint.clone() {
            entity.insert(Tint(tint.into()));
        }
        if let Some(velocity) = data.velocity.as_ref() {
            entity.insert(Velocity(velocity.clone().into()));
        }
        if let Some(mass) = data.mass {
            entity.insert(Mass(mass));
        }
        if let Some(half) = collider_half.as_ref() {
            entity.insert(Aabb { half: *half });
            entity.insert(Force::default());
        }
        if let Some(emitter) = data.particle_emitter.clone() {
            entity.insert(ParticleEmitter {
                rate: emitter.rate,
                spread: emitter.spread,
                speed: emitter.speed,
                lifetime: emitter.lifetime,
                accumulator: 0.0,
                start_color: emitter.start_color.into(),
                end_color: emitter.end_color.into(),
                start_size: emitter.start_size,
                end_size: emitter.end_size,
            });
        }
        if let Some(orbit) = data.orbit.clone() {
            entity
                .insert(OrbitController { center: orbit.center.into(), angular_speed: orbit.angular_speed });
        }

        let mut sprite_event = None;
        if let Some(sprite) = data.sprite.as_ref() {
            if !assets.atlas_region_exists(&sprite.atlas, &sprite.region) {
                return Err(anyhow!(
                    "Scene references unknown atlas region '{}:{}'",
                    sprite.atlas,
                    sprite.region
                ));
            }
            entity.insert(Sprite {
                atlas_key: Cow::Owned(sprite.atlas.clone()),
                region: Cow::Owned(sprite.region.clone()),
            });
            sprite_event = Some((sprite.atlas.clone(), sprite.region.clone()));
        }

        if let Some(body) = body_handle {
            entity.insert(RapierBody { handle: body });
        }
        if let Some(collider) = collider_handle {
            entity.insert(RapierCollider { handle: collider });
        }

        let entity_id = entity.id();
        drop(entity);

        if let Some(collider) = collider_handle {
            let mut rapier = self.world.resource_mut::<RapierState>();
            rapier.register_collider_entity(collider, entity_id);
        }

        if let Some((atlas, region)) = sprite_event {
            self.emit(GameEvent::SpriteSpawned { entity: entity_id, atlas, region });
        }

        Ok(entity_id)
    }

    fn collect_scene_entity(&self, entity: Entity, parent_index: Option<usize>, out: &mut Vec<SceneEntity>) {
        if self.world.get::<Transform>(entity).is_none() {
            return;
        }
        if self.world.get::<Particle>(entity).is_some() {
            return;
        }

        let transform = *self.world.get::<Transform>(entity).unwrap();
        let scene_entity = SceneEntity {
            name: None,
            transform: TransformData::from_components(
                transform.translation,
                transform.rotation,
                transform.scale,
            ),
            sprite: self.world.get::<Sprite>(entity).map(|sprite| SpriteData {
                atlas: sprite.atlas_key.to_string(),
                region: sprite.region.to_string(),
            }),
            tint: self.world.get::<Tint>(entity).map(|t| ColorData::from(t.0)),
            velocity: self.world.get::<Velocity>(entity).map(|v| v.0.into()),
            mass: self.world.get::<Mass>(entity).map(|m| m.0),
            collider: self.world.get::<Aabb>(entity).map(|a| ColliderData { half_extents: a.half.into() }),
            particle_emitter: self.world.get::<ParticleEmitter>(entity).map(|emitter| ParticleEmitterData {
                rate: emitter.rate,
                spread: emitter.spread,
                speed: emitter.speed,
                lifetime: emitter.lifetime,
                start_color: emitter.start_color.into(),
                end_color: emitter.end_color.into(),
                start_size: emitter.start_size,
                end_size: emitter.end_size,
            }),
            orbit: self.world.get::<OrbitController>(entity).map(|orbit| OrbitControllerData {
                center: orbit.center.into(),
                angular_speed: orbit.angular_speed,
            }),
            spin: self.world.get::<Spin>(entity).map(|s| s.speed),
            parent: parent_index,
        };

        let current_index = out.len();
        out.push(scene_entity);

        if let Some(children) = self.world.get::<Children>(entity) {
            for &child in &children.0 {
                self.collect_scene_entity(child, Some(current_index), out);
            }
        }
    }

    fn clear_scene_entities(&mut self) {
        let boundary = {
            let rapier = self.world.resource::<RapierState>();
            rapier.boundary_entity()
        };
        let mut roots = Vec::new();
        {
            let mut query = self.world.query::<(Entity, Option<&Parent>)>();
            for (entity, parent) in query.iter(&self.world) {
                if parent.is_none() {
                    roots.push(entity);
                }
            }
        }
        for entity in roots {
            if entity == boundary {
                continue;
            }
            self.despawn_entity(entity);
        }
        self.world.resource_mut::<ParticleContacts>().pairs.clear();
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

fn sys_integrate_positions(
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

fn sys_step_rapier(mut rapier: ResMut<RapierState>, mut events: ResMut<EventBus>, dt: Res<TimeDelta>) {
    if dt.0 > 0.0 {
        rapier.step(dt.0);
    }
    for (phase, a, b) in rapier.drain_collision_events() {
        match phase {
            CollisionLifecycle::Started => events.push(GameEvent::collision_started(a, b)),
            CollisionLifecycle::Stopped => events.push(GameEvent::collision_ended(a, b)),
        }
    }
}

#[derive(Resource, Default)]
pub struct ParticleContacts {
    pairs: HashSet<(Entity, Entity)>,
}

fn sys_sync_from_rapier(
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

fn sys_drive_orbits(
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
            rb.set_linvel(vector![tangent.x, tangent.y], true);
            rb.wake_up(true);
        }
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
                    scale: Vec2::splat(emitter.start_size),
                },
                WorldTransform::default(),
                Velocity(velocity),
                Force::default(),
                Mass(0.2),
                Sprite { atlas_key: Cow::Borrowed("main"), region: Cow::Borrowed("green") },
                Tint(emitter.start_color),
                Aabb { half: Vec2::splat((emitter.start_size * 0.5).max(0.01)) },
                Particle { lifetime, max_lifetime: lifetime },
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

fn sys_update_particles(
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

fn sys_world_bounds_bounce(
    mut q: Query<(&mut Transform, &mut Velocity, Option<&Aabb>, Option<&RapierBody>)>,
) {
    let min = Vec2::new(-1.4, -1.0);
    let max = Vec2::new(1.4, 1.0);
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

fn sys_build_spatial_hash(
    mut grid: ResMut<SpatialHash>,
    q: Query<(Entity, &Transform, &Aabb), Without<RapierBody>>,
) {
    grid.clear();
    for (e, t, a) in &q {
        grid.insert(e, t.translation, a.half);
    }
}

fn sys_collide_spatial(
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
