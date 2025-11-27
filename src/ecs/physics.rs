use bevy_ecs::prelude::*;
use glam::Vec2;
use rapier2d::geometry::{CollisionEvent, CollisionEventFlags};
use rapier2d::pipeline::{ActiveEvents, EventHandler};
use rapier2d::prelude::{
    CCDSolver, Collider, ColliderBuilder, ColliderHandle, ColliderSet, ContactPair, DefaultBroadPhase,
    ImpulseJointSet, IntegrationParameters, IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline,
    QueryPipeline, Real, RigidBody, RigidBodyBuilder, RigidBodyHandle, RigidBodySet, SharedShape, Vector,
};
use smallvec::SmallVec;
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

    fn push_collision(&self, event: CollisionEvent) {
        if let Ok(mut events) = self.collision_events.try_lock() {
            events.push(event);
            return;
        }
        if let Ok(mut events) = self.collision_events.lock() {
            events.push(event);
        }
    }

    fn push_force(&self, force: (ColliderHandle, ColliderHandle, f32)) {
        if let Ok(mut events) = self.force_events.try_lock() {
            events.push(force);
            return;
        }
        if let Ok(mut events) = self.force_events.lock() {
            events.push(force);
        }
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
        self.push_collision(event);
    }

    fn handle_contact_force_event(
        &self,
        _dt: Real,
        _bodies: &RigidBodySet,
        _colliders: &ColliderSet,
        contact_pair: &ContactPair,
        total_force_magnitude: Real,
    ) {
        self.push_force((contact_pair.collider1, contact_pair.collider2, total_force_magnitude));
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
        let collider_handles: Vec<ColliderHandle> =
            self.bodies.get(handle).map(|body| body.colliders().to_vec()).unwrap_or_default();
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
            if let (Some(entity_a), Some(entity_b)) =
                (self.collider_entities.get(&a), self.collider_entities.get(&b))
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

    pub fn query_view(&self) -> RapierQueryView<'_> {
        RapierQueryView {
            pipeline: &self.query_pipeline,
            colliders: &self.colliders,
            bodies: &self.bodies,
            collider_entities: &self.collider_entities,
        }
    }
}

pub struct RapierQueryView<'a> {
    pub pipeline: &'a QueryPipeline,
    pub bodies: &'a RigidBodySet,
    pub colliders: &'a ColliderSet,
    pub collider_entities: &'a HashMap<ColliderHandle, Entity>,
}

#[derive(Resource)]
pub struct SpatialHash {
    pub cell: f32,
    pub grid: HashMap<(i32, i32), Vec<Entity>>,
    pub active_cells: Vec<(i32, i32)>,
}

impl SpatialHash {
    pub fn new(cell: f32) -> Self {
        Self { cell, grid: HashMap::new(), active_cells: Vec::new() }
    }
    pub fn begin_frame(&mut self) {
        for key in self.active_cells.drain(..) {
            if let Some(entries) = self.grid.get_mut(&key) {
                entries.clear();
            }
        }
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
                let key = (kx, ky);
                let list = self.grid.entry(key).or_default();
                if list.is_empty() {
                    self.active_cells.push(key);
                }
                list.push(e);
            }
        }
    }

    pub fn occupied_cells(&self) -> usize {
        self.active_cells.len()
    }
}

#[derive(Resource, Clone, Copy)]
pub struct SpatialIndexConfig {
    pub fallback_enabled: bool,
    pub density_threshold: f32,
}

impl Default for SpatialIndexConfig {
    fn default() -> Self {
        Self { fallback_enabled: false, density_threshold: 6.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SpatialMode {
    #[default]
    Grid,
    Quadtree,
}

#[derive(Resource, Clone, Copy, Debug)]
pub struct SpatialMetrics {
    pub occupied_cells: usize,
    pub max_cell_occupancy: usize,
    pub average_occupancy: f32,
    pub entity_count: usize,
    pub mode: SpatialMode,
    pub quadtree_nodes: usize,
}

impl Default for SpatialMetrics {
    fn default() -> Self {
        Self {
            occupied_cells: 0,
            max_cell_occupancy: 0,
            average_occupancy: 0.0,
            entity_count: 0,
            mode: SpatialMode::Grid,
            quadtree_nodes: 0,
        }
    }
}

struct QuadtreeNode {
    min: Vec2,
    max: Vec2,
    children: Option<[usize; 4]>,
    entries: SmallVec<[usize; 8]>,
}

impl QuadtreeNode {
    fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max, children: None, entries: SmallVec::new() }
    }
}

struct QuadtreeEntry {
    entity: Entity,
    center: Vec2,
    half: Vec2,
}

#[derive(Resource)]
pub struct SpatialQuadtree {
    nodes: Vec<QuadtreeNode>,
    entries: Vec<QuadtreeEntry>,
    max_depth: u32,
    capacity: usize,
}

impl SpatialQuadtree {
    pub fn new(max_depth: u32, capacity: usize) -> Self {
        Self { nodes: Vec::new(), entries: Vec::new(), max_depth, capacity }
    }

    pub fn clear(&mut self) {
        self.nodes.clear();
        self.entries.clear();
    }

    pub fn rebuild(&mut self, bounds: &WorldBounds, colliders: &[(Entity, Vec2, Vec2)]) {
        self.clear();
        self.nodes.push(QuadtreeNode::new(bounds.min, bounds.max));
        for (entity, center, half) in colliders {
            let entry_index = self.entries.len();
            self.entries.push(QuadtreeEntry { entity: *entity, center: *center, half: *half });
            self.insert_entry(0, entry_index, 0);
        }
    }

    fn insert_entry(&mut self, node_index: usize, entry_index: usize, depth: u32) {
        if depth >= self.max_depth {
            self.nodes[node_index].entries.push(entry_index);
            return;
        }
        if self.nodes[node_index].entries.len() >= self.capacity {
            self.subdivide(node_index);
        }
        if let Some(children) = self.nodes[node_index].children {
            if let Some(child_index) = self.child_for_entry(node_index, entry_index) {
                self.insert_entry(children[child_index], entry_index, depth + 1);
                return;
            }
        }
        self.nodes[node_index].entries.push(entry_index);
    }

    fn child_for_entry(&self, node_index: usize, entry_index: usize) -> Option<usize> {
        let entry = &self.entries[entry_index];
        let node = &self.nodes[node_index];
        let mid = (node.min + node.max) * 0.5;
        let mut quadrant = 0usize;
        let mut child_min = node.min;
        let mut child_max = node.max;
        if entry.center.x >= mid.x {
            quadrant |= 1;
            child_min.x = mid.x;
        } else {
            child_max.x = mid.x;
        }
        if entry.center.y >= mid.y {
            quadrant |= 2;
            child_min.y = mid.y;
        } else {
            child_max.y = mid.y;
        }
        let entry_min = entry.center - entry.half;
        let entry_max = entry.center + entry.half;
        if entry_min.x >= child_min.x
            && entry_max.x <= child_max.x
            && entry_min.y >= child_min.y
            && entry_max.y <= child_max.y
        {
            Some(quadrant)
        } else {
            None
        }
    }

    fn subdivide(&mut self, node_index: usize) {
        if self.nodes[node_index].children.is_some() {
            return;
        }
        let node = &self.nodes[node_index];
        let mid = (node.min + node.max) * 0.5;
        let mins = [
            Vec2::new(node.min.x, node.min.y),
            Vec2::new(mid.x, node.min.y),
            Vec2::new(node.min.x, mid.y),
            Vec2::new(mid.x, mid.y),
        ];
        let maxs = [
            Vec2::new(mid.x, mid.y),
            Vec2::new(node.max.x, mid.y),
            Vec2::new(mid.x, node.max.y),
            Vec2::new(node.max.x, node.max.y),
        ];
        let mut children = [0usize; 4];
        for i in 0..4 {
            let child_index = self.nodes.len();
            self.nodes.push(QuadtreeNode::new(mins[i], maxs[i]));
            children[i] = child_index;
        }
        self.nodes[node_index].children = Some(children);
    }

    pub fn query(&self, center: Vec2, half: Vec2, out: &mut SmallVec<[Entity; 16]>) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        self.query_node(0, center, half, out);
    }

    fn query_node(&self, node_index: usize, center: Vec2, half: Vec2, out: &mut SmallVec<[Entity; 16]>) {
        let node = &self.nodes[node_index];
        if !aabb_overlap(center, half, node_center(node), node_half(node)) {
            return;
        }
        for &entry_index in &node.entries {
            out.push(self.entries[entry_index].entity);
        }
        if let Some(children) = node.children {
            for child in children {
                self.query_node(child, center, half, out);
            }
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

fn node_center(node: &QuadtreeNode) -> Vec2 {
    (node.min + node.max) * 0.5
}

fn node_half(node: &QuadtreeNode) -> Vec2 {
    (node.max - node.min) * 0.5
}

fn aabb_overlap(center_a: Vec2, half_a: Vec2, center_b: Vec2, half_b: Vec2) -> bool {
    (center_a.x - center_b.x).abs() < (half_a.x + half_b.x)
        && (center_a.y - center_b.y).abs() < (half_a.y + half_b.y)
}

#[derive(Resource, Default)]
pub struct SpatialScratch {
    pub colliders: Vec<(Entity, Vec2, Vec2)>,
}

#[derive(Resource, Default)]
pub struct ParticleContacts {
    pub pairs: HashSet<(Entity, Entity)>,
    pub previous_pairs: HashSet<(Entity, Entity)>,
}

fn vec_to_rapier(v: Vec2) -> Vector<Real> {
    Vector::new(v.x, v.y)
}
