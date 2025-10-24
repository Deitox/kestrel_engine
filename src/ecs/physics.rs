use bevy_ecs::prelude::*;
use glam::Vec2;
use rapier2d::geometry::{CollisionEvent, CollisionEventFlags};
use rapier2d::pipeline::{ActiveEvents, EventHandler};
use rapier2d::prelude::{
    CCDSolver, Collider, ColliderBuilder, ColliderHandle, ColliderSet, ContactPair, DefaultBroadPhase,
    ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline,
    QueryPipeline, Real, RigidBody, RigidBodyBuilder, RigidBodyHandle, RigidBodySet, SharedShape, Vector,
};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

#[derive(Resource, Clone, Copy)]
pub struct PhysicsParams {
    pub gravity: Vec2,
    pub linear_damping: f32,
}

#[derive(Resource, Clone, Copy)]
pub struct WorldBounds {
    pub min: Vec2,
    pub max: Vec2,
    pub thickness: f32,
}

pub enum CollisionEventKind {
    Started,
    Stopped,
    Force(f32),
}

struct CollisionEventCollector {
    collision_events: Mutex<Vec<CollisionEvent>>,
    force_events: Mutex<Vec<(ColliderHandle, ColliderHandle, f32)>>,
}

impl CollisionEventCollector {
    fn new() -> Self {
        Self { collision_events: Mutex::new(Vec::new()), force_events: Mutex::new(Vec::new()) }
    }

    fn drain(&self) -> (Vec<CollisionEvent>, Vec<(ColliderHandle, ColliderHandle, f32)>) {
        let collisions = if let Ok(mut events) = self.collision_events.lock() {
            std::mem::take(&mut *events)
        } else {
            Vec::new()
        };
        let forces = if let Ok(mut events) = self.force_events.lock() {
            std::mem::take(&mut *events)
        } else {
            Vec::new()
        };
        (collisions, forces)
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
        if let Ok(mut events) = self.collision_events.lock() {
            events.push(event);
        }
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        contact_pair: &ContactPair,
        total_force_magnitude: Real,
    ) {
        if let Ok(mut events) = self.force_events.lock() {
            events.push((contact_pair.collider1, contact_pair.collider2, total_force_magnitude));
        }
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
    bounds: WorldBounds,
}

impl RapierState {
    pub fn new(params: &PhysicsParams, bounds: &WorldBounds, boundary_entity: Entity) -> Self {
        let mut state = Self {
            pipeline: PhysicsPipeline::new(),
            gravity: vec_to_rapier(params.gravity),
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
            bounds: *bounds,
        };
        state.init_bounds();
        state
    }

    fn init_bounds(&mut self) {
        let thickness = self.bounds.thickness;
        let min = Vector::new(self.bounds.min.x, self.bounds.min.y);
        let max = Vector::new(self.bounds.max.x, self.bounds.max.y);
        let horizontal_half = Vector::new((max.x - min.x) * 0.5 + thickness, thickness);
        let vertical_half = Vector::new(thickness, (max.y - min.y) * 0.5 + thickness);

        let centers = [
            Vector::new(0.0, min.y - thickness),
            Vector::new(0.0, max.y + thickness),
            Vector::new(min.x - thickness, 0.0),
            Vector::new(max.x + thickness, 0.0),
        ];
        let half_extents = [horizontal_half, horizontal_half, vertical_half, vertical_half];

        for (center, half) in centers.into_iter().zip(half_extents) {
            self.insert_static_collider(center, half);
        }
    }

    fn insert_static_collider(&mut self, center: Vector<Real>, half: Vector<Real>) {
        let body = RigidBodyBuilder::fixed().translation(center).build();
        let body_handle = self.bodies.insert(body);
        let collider = ColliderBuilder::cuboid(half.x, half.y)
            .restitution(0.4)
            .friction(0.8)
            .active_events(ActiveEvents::COLLISION_EVENTS | ActiveEvents::CONTACT_FORCE_EVENTS)
            .contact_force_event_threshold(0.0)
            .build();
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
        let body = RigidBodyBuilder::dynamic().translation(Vector::new(position.x, position.y)).build();
        let body_handle = self.bodies.insert(body);
        if let Some(body) = self.bodies.get_mut(body_handle) {
            if mass > 0.0 {
                body.set_additional_mass(mass, true);
            }
            body.set_linvel(Vector::new(velocity.x, velocity.y), true);
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

    pub fn drain_collision_events(&mut self) -> Vec<(CollisionEventKind, Entity, Entity)> {
        let (collision_events, force_events) = self.event_collector.drain();
        let mut out = Vec::new();
        for event in collision_events {
            match event {
                CollisionEvent::Started(a, b, flags) => {
                    if flags.contains(CollisionEventFlags::SENSOR) {
                        continue;
                    }
                    if let (Some(entity_a), Some(entity_b)) =
                        (self.collider_entities.get(&a), self.collider_entities.get(&b))
                    {
                        out.push((CollisionEventKind::Started, *entity_a, *entity_b));
                    }
                }
                CollisionEvent::Stopped(a, b, flags) => {
                    if flags.contains(CollisionEventFlags::SENSOR) {
                        continue;
                    }
                    if let (Some(entity_a), Some(entity_b)) =
                        (self.collider_entities.get(&a), self.collider_entities.get(&b))
                    {
                        out.push((CollisionEventKind::Stopped, *entity_a, *entity_b));
                    }
                }
            }
        }
        for (a, b, magnitude) in force_events {
            if let (Some(entity_a), Some(entity_b)) = (self.collider_entities.get(&a), self.collider_entities.get(&b))
            {
                out.push((CollisionEventKind::Force(magnitude), *entity_a, *entity_b));
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

    pub fn bounds(&self) -> WorldBounds {
        self.bounds
    }
}

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
    pub fn key(&self, p: Vec2) -> (i32, i32) {
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

#[derive(Resource, Default)]
pub struct ParticleContacts {
    pub pairs: HashSet<(Entity, Entity)>,
}

fn vec_to_rapier(v: Vec2) -> Vector<Real> {
    Vector::new(v.x, v.y)
}
