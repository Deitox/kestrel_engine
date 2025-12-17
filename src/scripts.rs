use std::any::Any;
use std::cell::RefCell;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::env;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use crate::assets::AssetManager;
use crate::plugins::{EnginePlugin, PluginContext};
use anyhow::{anyhow, Context, Error, Result};
use glam::{Vec2, Vec4};
use rapier2d::prelude::{
    ColliderHandle, Isometry, Point, QueryFilter as RapierQueryFilter, QueryFilterFlags, Ray as RapierRay,
    RayIntersection, SharedShape, Vector,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::{Map as JsonMap, Value as JsonValue};
use serde::{Deserialize, Serialize};
use rhai::module_resolvers::ModuleResolver;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Module, Scope, Shared, AST, FLOAT};

use bevy_ecs::prelude::{Component, Entity};
use crate::ecs::{Aabb, SceneEntityTag, Tint, Transform, Velocity, WorldTransform};
use std::fmt::Write as FmtWrite;
use crate::input::Input;

pub type ScriptHandle = rhai::INT;
pub type ListenerHandle = rhai::INT;

const SCRIPT_DIGEST_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const SCRIPT_IMPORT_ROOT: &str = "assets/scripts";
const SCRIPT_EVENT_QUEUE_LIMIT: usize = 256;
const SCRIPT_OFFENDER_LIMIT: usize = 8;

fn derive_project_root_from_scripts_root(scripts_root: &Path) -> Option<PathBuf> {
    let scripts_dir = scripts_root.file_name()?.to_str()?;
    if !scripts_dir.eq_ignore_ascii_case("scripts") {
        return None;
    }
    let assets_dir = scripts_root.parent()?;
    let assets_dir_name = assets_dir.file_name()?.to_str()?;
    if !assets_dir_name.eq_ignore_ascii_case("assets") {
        return None;
    }
    assets_dir.parent().map(|p| p.to_path_buf())
}

#[derive(Clone)]
pub struct CompiledScript {
    pub ast: AST,
    pub has_ready: bool,
    pub has_process: bool,
    pub has_physics_process: bool,
    pub has_exit: bool,
    pub len: u64,
    pub digest: u64,
    pub import_digests: HashMap<PathBuf, u64>,
    pub last_checked: Option<Instant>,
    pub asset_revision: Option<u64>,
}

#[derive(Clone)]
pub struct ScriptInstance {
    pub script_path: String,
    pub entity: Entity,
    pub scope: Scope<'static>,
    pub has_ready_run: bool,
    pub errored: bool,
    pub persist_state: bool,
    pub mute_errors: bool,
    pub state: Rc<RefCell<InstanceRuntimeState>>,
}

#[derive(Clone)]
pub struct EntitySnapshot {
    pub translation: Vec2,
    pub rotation: f32,
    pub scale: Vec2,
    pub velocity: Option<Vec2>,
    pub tint: Option<Vec4>,
    pub half_extents: Option<Vec2>,
}

#[derive(Clone, Default)]
pub struct InputSnapshot {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub ascend: bool,
    pub descend: bool,
    pub boost: bool,
    pub ctrl: bool,
    pub left_mouse: bool,
    pub right_mouse: bool,
    pub cursor: Option<Vec2>,
    pub cursor_world: Option<Vec2>,
    pub mouse_delta: Vec2,
    pub wheel: f32,
}

#[derive(Component, Clone, Debug)]
pub struct ScriptBehaviour {
    pub script_path: String,
    pub instance_id: u64,
    pub persist_state: bool,
    pub mute_errors: bool,
}

impl ScriptBehaviour {
    pub fn new(path: impl Into<String>) -> Self {
        Self { script_path: path.into(), instance_id: 0, persist_state: false, mute_errors: false }
    }

    pub fn with_persistence(path: impl Into<String>, persist: bool) -> Self {
        Self { script_path: path.into(), instance_id: 0, persist_state: persist, mute_errors: false }
    }
}

#[derive(Debug, Clone)]
pub enum ScriptCommand {
    Spawn { handle: ScriptHandle, atlas: String, region: String, position: Vec2, scale: f32, velocity: Vec2 },
    SetVelocity { handle: ScriptHandle, velocity: Vec2 },
    SetPosition { handle: ScriptHandle, position: Vec2 },
    SetRotation { handle: ScriptHandle, rotation: f32 },
    SetScale { handle: ScriptHandle, scale: Vec2 },
    SetTint { handle: ScriptHandle, tint: Option<Vec4> },
    SetSpriteRegion { handle: ScriptHandle, region: String },
    Despawn { handle: ScriptHandle },
    SetAutoSpawnRate { rate: f32 },
    SetSpawnPerPress { count: i32 },
    SetEmitterRate { rate: f32 },
    SetEmitterSpread { spread: f32 },
    SetEmitterSpeed { speed: f32 },
    SetEmitterLifetime { lifetime: f32 },
    SetEmitterStartColor { color: Vec4 },
    SetEmitterEndColor { color: Vec4 },
    SetEmitterStartSize { size: f32 },
    SetEmitterEndSize { size: f32 },
    SpawnPrefab { handle: ScriptHandle, path: String, tag: Option<String> },
    SpawnTemplate { handle: ScriptHandle, template: String, tag: Option<String> },
    EntitySetPosition { entity: Entity, position: Vec2 },
    EntitySetRotation { entity: Entity, rotation: f32 },
    EntitySetScale { entity: Entity, scale: Vec2 },
    EntitySetTint { entity: Entity, tint: Option<Vec4> },
    EntitySetVelocity { entity: Entity, velocity: Vec2 },
    EntityDespawn { entity: Entity },
}

#[derive(Clone)]
struct ScriptEvent {
    name: Arc<str>,
    payload: Dynamic,
    target: Option<Entity>,
    source: Option<Entity>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum ListenerOwner {
    Host,
    Instance(u64),
}

#[derive(Clone)]
struct ScriptEventListener {
    id: u64,
    name: Arc<str>,
    handler: Arc<str>,
    owner: ListenerOwner,
    scope_entity: Option<Entity>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptTimingSummary {
    pub name: &'static str,
    pub last_ms: f32,
    pub average_ms: f32,
    pub max_ms: f32,
    pub samples: u64,
}

#[derive(Clone, Debug)]
pub struct ScriptTimingOffender {
    pub script_path: String,
    pub function: String,
    pub entity: Option<Entity>,
    pub last_ms: f32,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptSafetyMetrics {
    pub invalid_handle_uses: u64,
    pub despawn_dead_uses: u64,
    pub spawn_failures: HashMap<String, u64>,
}

#[derive(Component, Clone, Debug)]
pub struct ScriptPersistedState(pub JsonValue);

#[derive(Clone, Copy, Default)]
struct ScriptTiming {
    last_ms: f32,
    total_ms: f32,
    max_ms: f32,
    samples: u64,
}

#[derive(Clone, Default)]
struct ScriptSpatialIndex {
    cell_size: f32,
    cells: HashMap<(i32, i32), Vec<Entity>>,
}

impl ScriptSpatialIndex {
    fn enabled(&self) -> bool {
        self.cell_size.is_finite() && self.cell_size > 0.0
    }

    fn has_cells(&self) -> bool {
        self.enabled() && !self.cells.is_empty()
    }

    fn normalized_cell_size(cell_size: f32) -> f32 {
        if cell_size.is_finite() && cell_size > 0.0 {
            cell_size
        } else {
            0.0
        }
    }

    fn add_snapshot(&mut self, entity: Entity, snap: &EntitySnapshot) {
        if !self.enabled() {
            return;
        }
        let half = snap.half_extents.unwrap_or_else(|| snap.scale * 0.5);
        if !half.is_finite() || half.x <= 0.0 || half.y <= 0.0 {
            return;
        }
        if !snap.translation.is_finite() {
            return;
        }
        let min = snap.translation - half;
        let max = snap.translation + half;
        let (kx0, ky0) = self.key(min);
        let (kx1, ky1) = self.key(max);
        for ky in ky0..=ky1 {
            for kx in kx0..=kx1 {
                self.cells.entry((kx, ky)).or_default().push(entity);
            }
        }
    }

    fn backfill_missing(&mut self, snapshots: &HashMap<Entity, EntitySnapshot>) {
        if !self.enabled() {
            return;
        }
        let mut covered = HashSet::new();
        for list in self.cells.values() {
            for &entity in list {
                covered.insert(entity);
            }
        }
        for (entity, snap) in snapshots {
            if covered.contains(entity) {
                continue;
            }
            self.add_snapshot(*entity, snap);
        }
    }

    fn rebuild(&mut self, snapshots: &HashMap<Entity, EntitySnapshot>, cell_size: f32) {
        self.cells.clear();
        self.cell_size = Self::normalized_cell_size(cell_size);
        if !self.enabled() {
            return;
        }
        for (entity, snap) in snapshots {
            self.add_snapshot(*entity, snap);
        }
    }

    fn rebuild_with_spatial_hash(
        &mut self,
        snapshots: &HashMap<Entity, EntitySnapshot>,
        spatial_cells: Option<HashMap<(i32, i32), Vec<Entity>>>,
        cell_size: f32,
    ) {
        self.cells.clear();
        self.cell_size = Self::normalized_cell_size(cell_size);
        if !self.enabled() {
            return;
        }
        if let Some(cells) = spatial_cells {
            for (key, list) in cells {
                if !list.is_empty() {
                    self.cells.insert(key, list);
                }
            }
        }
        if self.cells.is_empty() {
            self.rebuild(snapshots, self.cell_size);
            return;
        }
        self.backfill_missing(snapshots);
    }

    fn ray_candidates(&self, origin: Vec2, dir: Vec2, max_dist: f32) -> Option<Vec<Entity>> {
        if !self.has_cells() || !max_dist.is_finite() || max_dist <= 0.0 || dir.length_squared() <= f32::EPSILON {
            return None;
        }
        let dir = dir.normalize();
        let end = origin + dir * max_dist;
        let min = Vec2::new(origin.x.min(end.x), origin.y.min(end.y));
        let max = Vec2::new(origin.x.max(end.x), origin.y.max(end.y));
        self.query_aabb(min, max)
    }

    fn circle_candidates(&self, center: Vec2, radius: f32) -> Option<Vec<Entity>> {
        if !self.has_cells() || !radius.is_finite() || radius <= 0.0 {
            return None;
        }
        let half = Vec2::splat(radius);
        self.query_aabb(center - half, center + half)
    }

    fn query_aabb(&self, min: Vec2, max: Vec2) -> Option<Vec<Entity>> {
        if !self.has_cells() || !min.is_finite() || !max.is_finite() {
            return None;
        }
        let (kx0, ky0) = self.key(min);
        let (kx1, ky1) = self.key(max);
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for ky in ky0..=ky1 {
            for kx in kx0..=kx1 {
                if let Some(list) = self.cells.get(&(kx, ky)) {
                    for &entity in list {
                        if seen.insert(entity) {
                            out.push(entity);
                        }
                    }
                }
            }
        }
        Some(out)
    }

    fn key(&self, p: Vec2) -> (i32, i32) {
        let cell = self.cell_size;
        ((p.x / cell).floor() as i32, (p.y / cell).floor() as i32)
    }
}

#[derive(Clone, Copy)]
struct PhysicsQueryContext {
    rapier: *const crate::ecs::RapierState,
}

impl PhysicsQueryContext {
    fn from_state(state: &crate::ecs::RapierState) -> Self {
        Self { rapier: state as *const _ }
    }

    unsafe fn rapier(&self) -> Option<&crate::ecs::RapierState> {
        self.rapier.as_ref()
    }
}

struct SharedState {
    next_handle: ScriptHandle,
    handle_nonce: u32,
    pending_handles: HashSet<ScriptHandle>,
    next_listener_id: u64,
    commands: Vec<ScriptCommand>,
    command_quota: Option<usize>,
    commands_per_owner: HashMap<ListenerOwner, usize>,
    logs: Vec<String>,
    rng: Option<StdRng>,
    global_stats: HashMap<String, f64>,
    entity_snapshots: HashMap<Entity, EntitySnapshot>,
    entity_scene_ids: HashMap<Entity, Arc<str>>,
    scene_id_entities: HashMap<Arc<str>, Entity>,
    input_snapshot: Option<InputSnapshot>,
    spatial_index: ScriptSpatialIndex,
    physics_ctx: Option<PhysicsQueryContext>,
    time_scale: f32,
    unscaled_time: f32,
    scaled_time: f32,
    last_unscaled_dt: f32,
    last_scaled_dt: f32,
    timers: HashMap<String, TimerState>,
    event_queue: VecDeque<ScriptEvent>,
    event_listeners: Vec<ScriptEventListener>,
    events_dispatched: usize,
    event_overflowed: bool,
    timings: HashMap<&'static str, ScriptTiming>,
    offenders: Vec<ScriptTimingOffender>,
    handle_lookup: HashMap<ScriptHandle, Entity>,
    entity_handles: HashMap<Entity, ScriptHandle>,
    handle_tags: HashMap<ScriptHandle, String>,
    entity_tags: HashMap<Entity, String>,
    invalid_handle_uses: u64,
    despawn_dead_uses: u64,
    spawn_failures: HashMap<String, u64>,
    invalid_handle_labels: HashSet<String>,
}

impl Default for SharedState {
    fn default() -> Self {
        let mut nonce = rand::random::<u32>() & 0x7FFF_FFFF;
        if nonce == 0 {
            nonce = 1;
        }
        Self {
            next_handle: 0,
            handle_nonce: nonce,
            pending_handles: HashSet::new(),
            next_listener_id: 1,
            commands: Vec::new(),
            command_quota: None,
            commands_per_owner: HashMap::new(),
            logs: Vec::new(),
            rng: None,
            global_stats: HashMap::new(),
            entity_snapshots: HashMap::new(),
            entity_scene_ids: HashMap::new(),
            scene_id_entities: HashMap::new(),
            input_snapshot: None,
            spatial_index: ScriptSpatialIndex::default(),
            physics_ctx: None,
            time_scale: 1.0,
            unscaled_time: 0.0,
            scaled_time: 0.0,
            last_unscaled_dt: 0.0,
            last_scaled_dt: 0.0,
            timers: HashMap::new(),
            event_queue: VecDeque::new(),
            event_listeners: Vec::new(),
            events_dispatched: 0,
            event_overflowed: false,
            timings: HashMap::new(),
            offenders: Vec::new(),
            handle_lookup: HashMap::new(),
            entity_handles: HashMap::new(),
            handle_tags: HashMap::new(),
            entity_tags: HashMap::new(),
            invalid_handle_uses: 0,
            despawn_dead_uses: 0,
            spawn_failures: HashMap::new(),
            invalid_handle_labels: HashSet::new(),
        }
    }
}

impl SharedState {
    fn record_timing(&mut self, name: &'static str, duration_ms: f32) {
        let entry = self.timings.entry(name).or_default();
        entry.last_ms = duration_ms;
        entry.total_ms += duration_ms;
        entry.max_ms = entry.max_ms.max(duration_ms);
        entry.samples = entry.samples.saturating_add(1);
    }

    fn timing_summaries(&self) -> Vec<ScriptTimingSummary> {
        let mut out = Vec::with_capacity(self.timings.len());
        for (&name, timing) in &self.timings {
            let avg = if timing.samples == 0 { 0.0 } else { timing.total_ms / timing.samples as f32 };
            out.push(ScriptTimingSummary {
                name,
                last_ms: timing.last_ms,
                average_ms: avg,
                max_ms: timing.max_ms,
                samples: timing.samples,
            });
        }
        out.sort_by(|a, b| b.last_ms.partial_cmp(&a.last_ms).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    fn record_offender(&mut self, entry: ScriptTimingOffender) {
        if !entry.last_ms.is_finite() {
            return;
        }
        self.offenders.push(entry);
        self.offenders
            .sort_by(|a, b| b.last_ms.partial_cmp(&a.last_ms).unwrap_or(std::cmp::Ordering::Equal));
        if self.offenders.len() > SCRIPT_OFFENDER_LIMIT {
            self.offenders.truncate(SCRIPT_OFFENDER_LIMIT);
        }
    }

    fn record_invalid_handle_use(&mut self, label: Option<&str>) {
        self.invalid_handle_uses = self.invalid_handle_uses.saturating_add(1);
        if let Some(label) = label {
            if self.invalid_handle_labels.insert(label.to_string()) {
                self.logs.push(format!("[script] {label}: invalid handle ignored (first occurrence)"));
            }
        }
        if self.invalid_handle_uses == 1 || self.invalid_handle_uses % 10 == 0 {
            self.logs.push(format!(
                "[script] invalid handle ignored (count={})",
                self.invalid_handle_uses
            ));
        }
    }

    fn record_despawn_dead(&mut self) {
        self.despawn_dead_uses = self.despawn_dead_uses.saturating_add(1);
        if self.despawn_dead_uses == 1 || self.despawn_dead_uses % 10 == 0 {
            self.logs.push(format!(
                "[script] despawn_safe/EntityDespawn ignored dead handle (count={})",
                self.despawn_dead_uses
            ));
        }
    }

    fn record_spawn_failure(&mut self, reason: &str) {
        if reason.is_empty() {
            return;
        }
        *self.spawn_failures.entry(reason.to_string()).or_insert(0) += 1;
    }

    fn safety_metrics(&self) -> ScriptSafetyMetrics {
        ScriptSafetyMetrics {
            invalid_handle_uses: self.invalid_handle_uses,
            despawn_dead_uses: self.despawn_dead_uses,
            spawn_failures: self.spawn_failures.clone(),
        }
    }
}

#[derive(Clone, Default)]
struct TimerState {
    duration: f32,
    elapsed: f32,
    repeat: bool,
    fired: bool,
}

impl TimerState {
    fn new(duration: f32, repeat: bool) -> Self {
        Self { duration, elapsed: 0.0, repeat, fired: false }
    }

    fn tick(&mut self, dt: f32) {
        if !dt.is_finite() || dt <= 0.0 {
            return;
        }
        if self.duration <= 0.0 {
            self.fired = true;
            return;
        }
        self.elapsed += dt;
        while self.elapsed >= self.duration {
            self.elapsed -= self.duration;
            self.fired = true;
            if !self.repeat {
                self.elapsed = self.duration;
                break;
            }
        }
    }

    fn consume_fired(&mut self) -> bool {
        let fired = self.fired;
        self.fired = false;
        fired
    }

    fn remaining(&self) -> f32 {
        if self.duration <= 0.0 {
            0.0
        } else {
            (self.duration - self.elapsed).max(0.0)
        }
    }
}

#[derive(Default)]
struct QueryFilters {
    include: Option<HashSet<Entity>>,
    exclude: HashSet<Entity>,
}

#[derive(Clone, Copy)]
struct RaycastHit {
    entity: Entity,
    distance: f32,
    point: Vec2,
    normal: Option<Vec2>,
    collider: Option<ColliderHandle>,
}

#[derive(Clone, Copy)]
struct OverlapHit {
    entity: Entity,
    collider: Option<ColliderHandle>,
}

impl QueryFilters {
    fn matches(&self, entity: Entity) -> bool {
        if let Some(include) = &self.include {
            if !include.contains(&entity) {
                return false;
            }
        }
        !self.exclude.contains(&entity)
    }
}

#[derive(Default)]
pub struct InstanceRuntimeState {
    persistent: Map,
    is_hot_reload: bool,
    timers: HashMap<String, TimerState>,
    instance_id: Option<u64>,
    entity: Option<Entity>,
}

impl InstanceRuntimeState {
    fn tick_timers(&mut self, dt: f32) {
        for timer in self.timers.values_mut() {
            timer.tick(dt);
        }
    }
}

#[derive(Clone)]
struct CachedModule {
    digest: u64,
    ast: AST,
    module: Shared<Module>,
}

#[derive(Clone)]
struct CachedModuleResolver {
    root: PathBuf,
    cache: Arc<RwLock<HashMap<PathBuf, CachedModule>>>,
}

impl CachedModuleResolver {
    fn new(root: PathBuf) -> Self {
        let root = root.canonicalize().unwrap_or(root);
        Self { root, cache: Arc::new(RwLock::new(HashMap::new())) }
    }

    fn resolve_import_path(&self, import: &str) -> Result<PathBuf> {
        let trimmed = import.trim();
        if trimmed.is_empty() {
            return Err(anyhow!("Empty import path"));
        }
        let mut rel = PathBuf::from(trimmed);
        if rel.extension().is_none() {
            rel.set_extension("rhai");
        }
        if rel.is_absolute() {
            return Err(anyhow!("Absolute import paths are not allowed: {trimmed}"));
        }
        let candidate = self.root.join(rel);
        let canonical = candidate.canonicalize().with_context(|| format!("Resolving import '{trimmed}'"))?;
        if !canonical.starts_with(&self.root) {
            anyhow::bail!("Import '{trimmed}' escapes scripts directory");
        }
        Ok(canonical)
    }

    fn compute_import_digests(&self, source: &str) -> Result<HashMap<PathBuf, u64>> {
        let mut imports = HashMap::new();
        for name in parse_literal_imports(source) {
            let path = self.resolve_import_path(&name)?;
            let contents = std::fs::read_to_string(&path)
                .with_context(|| format!("Reading imported module '{}'", path.display()))?;
            imports.insert(path, hash_source(&contents));
        }
        Ok(imports)
    }

    fn load_module_entry(
        &self,
        engine: &Engine,
        path: &str,
        pos: rhai::Position,
    ) -> Result<CachedModule, Box<EvalAltResult>> {
        let canonical = self
            .resolve_import_path(path)
            .map_err(|_| Box::new(EvalAltResult::ErrorModuleNotFound(path.to_string(), pos)))?;
        let source = std::fs::read_to_string(&canonical)
            .map_err(|_| Box::new(EvalAltResult::ErrorModuleNotFound(path.to_string(), pos)))?;
        let digest = hash_source(&source);
        if let Ok(cache) = self.cache.read() {
            if let Some(entry) = cache.get(&canonical) {
                if entry.digest == digest {
                    return Ok(entry.clone());
                }
            }
        }
        let mut ast = engine
            .compile(&source)
            .map_err(|err| Box::new(EvalAltResult::ErrorInModule(path.to_string(), err.into(), pos)))?;
        ast.set_source(path);
        let module =
            Module::eval_ast_as_new(Scope::new(), &ast, engine)
                .map_err(|err| Box::new(EvalAltResult::ErrorInModule(path.to_string(), err, pos)))?;
        let shared: Shared<_> = module.into();
        let entry = CachedModule { digest, ast, module: shared.clone() };
        if let Ok(mut cache) = self.cache.write() {
            cache.insert(canonical, entry.clone());
        }
        Ok(entry)
    }
}

impl ModuleResolver for CachedModuleResolver {
    fn resolve(
        &self,
        engine: &Engine,
        _source: Option<&str>,
        path: &str,
        pos: rhai::Position,
    ) -> Result<Shared<Module>, Box<EvalAltResult>> {
        self.load_module_entry(engine, path, pos).map(|entry| entry.module)
    }

    fn resolve_ast(
        &self,
        engine: &Engine,
        _source: Option<&str>,
        path: &str,
        pos: rhai::Position,
    ) -> Option<Result<AST, Box<EvalAltResult>>> {
        Some(self.load_module_entry(engine, path, pos).map(|entry| entry.ast))
    }
}

#[derive(Clone)]
pub struct ScriptWorld {
    state: Rc<RefCell<SharedState>>,
    instance_state: Option<Rc<RefCell<InstanceRuntimeState>>>,
    owner: ListenerOwner,
}

impl ScriptWorld {
    fn new(state: Rc<RefCell<SharedState>>) -> Self {
        Self { state, instance_state: None, owner: ListenerOwner::Host }
    }

    fn with_instance(
        state: Rc<RefCell<SharedState>>,
        instance_state: Rc<RefCell<InstanceRuntimeState>>,
        instance_id: u64,
    ) -> Self {
        Self { state, instance_state: Some(instance_state), owner: ListenerOwner::Instance(instance_id) }
    }

    fn entity_is_alive(&self, entity: Entity) -> bool {
        self.state.borrow().entity_snapshots.contains_key(&entity)
    }

    fn entity_snapshot(&mut self, entity_bits: ScriptHandle) -> Map {
        let entity = Entity::from_bits(entity_bits as u64);
        let mut map = Map::new();
        let state = self.state.borrow();
        let Some(snapshot) = state.entity_snapshots.get(&entity) else {
            return map;
        };
        map.insert("pos".into(), Dynamic::from(Self::vec2_to_array(snapshot.translation)));
        map.insert("rot".into(), Dynamic::from(snapshot.rotation as FLOAT));
        map.insert("scale".into(), Dynamic::from(Self::vec2_to_array(snapshot.scale)));
        if let Some(vel) = snapshot.velocity {
            map.insert("vel".into(), Dynamic::from(Self::vec2_to_array(vel)));
        }
        if let Some(tint) = snapshot.tint {
            map.insert("tint".into(), Dynamic::from(Self::vec4_to_array(tint)));
        }
        if let Some(half) = snapshot.half_extents {
            map.insert("half_extents".into(), Dynamic::from(Self::vec2_to_array(half)));
        }
        map
    }

    fn entity_position(&mut self, entity_bits: ScriptHandle) -> Array {
        let entity = Entity::from_bits(entity_bits as u64);
        let state = self.state.borrow();
        let Some(snapshot) = state.entity_snapshots.get(&entity) else {
            return Array::new();
        };
        Self::vec2_to_array(snapshot.translation)
    }

    fn entity_rotation(&mut self, entity_bits: ScriptHandle) -> FLOAT {
        let entity = Entity::from_bits(entity_bits as u64);
        let state = self.state.borrow();
        state.entity_snapshots.get(&entity).map(|s| s.rotation as FLOAT).unwrap_or(0.0)
    }

    fn entity_tag(&mut self, entity_bits: ScriptHandle) -> String {
        if entity_bits < 0 {
            return String::new();
        }
        let entity = Entity::from_bits(entity_bits as u64);
        self.state.borrow().entity_tags.get(&entity).cloned().unwrap_or_default()
    }

    fn entity_handle(&mut self, entity_bits: ScriptHandle) -> Dynamic {
        if entity_bits < 0 {
            return Dynamic::UNIT;
        }
        let entity = Entity::from_bits(entity_bits as u64);
        self.state
            .borrow()
            .entity_handles
            .get(&entity)
            .copied()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT)
    }

    fn entity_scene_id(&mut self, entity_bits: ScriptHandle) -> String {
        if entity_bits < 0 {
            return String::new();
        }
        let entity = Entity::from_bits(entity_bits as u64);
        self.state
            .borrow()
            .entity_scene_ids
            .get(&entity)
            .map(|id| id.as_ref().to_string())
            .unwrap_or_default()
    }

    fn entity_scale(&mut self, entity_bits: ScriptHandle) -> Array {
        let entity = Entity::from_bits(entity_bits as u64);
        let state = self.state.borrow();
        let Some(snapshot) = state.entity_snapshots.get(&entity) else {
            return Array::new();
        };
        Self::vec2_to_array(snapshot.scale)
    }

    fn entity_velocity(&mut self, entity_bits: ScriptHandle) -> Array {
        let entity = Entity::from_bits(entity_bits as u64);
        let state = self.state.borrow();
        let Some(snapshot) = state.entity_snapshots.get(&entity) else {
            return Array::new();
        };
        snapshot.velocity.map(Self::vec2_to_array).unwrap_or_else(Array::new)
    }

    fn entity_tint(&mut self, entity_bits: ScriptHandle) -> Array {
        let entity = Entity::from_bits(entity_bits as u64);
        let state = self.state.borrow();
        let Some(snapshot) = state.entity_snapshots.get(&entity) else {
            return Array::new();
        };
        snapshot.tint.map(Self::vec4_to_array).unwrap_or_else(Array::new)
    }

    fn resolve_handle_entity(&self, handle: ScriptHandle) -> Option<Entity> {
        let state = self.state.borrow();
        state
            .handle_lookup
            .get(&handle)
            .copied()
            .filter(|entity| state.entity_snapshots.contains_key(entity))
    }

    fn handle_is_usable(&self, handle: ScriptHandle) -> bool {
        if self.resolve_handle_entity(handle).is_some() {
            return true;
        }
        self.state.borrow().pending_handles.contains(&handle)
    }

    fn handle_is_alive(&mut self, handle: ScriptHandle) -> bool {
        self.handle_is_usable(handle)
    }

    fn handle_validate(&mut self, handle: ScriptHandle) -> Dynamic {
        if self.handle_is_alive(handle) {
            Dynamic::from(handle)
        } else {
            self.state.borrow_mut().record_invalid_handle_use(Some("handle_validate"));
            Dynamic::UNIT
        }
    }

    fn handles_with_tag(&mut self, tag: &str) -> Array {
        let trimmed = tag.trim();
        if trimmed.is_empty() {
            return Array::new();
        }
        let state = self.state.borrow();
        let mut seen = HashSet::new();
        let mut handles = Array::new();
        for (entity, entity_tag) in state.entity_tags.iter() {
            if entity_tag != trimmed {
                continue;
            }
            if !state.entity_snapshots.contains_key(entity) {
                continue;
            }
            if let Some(handle) = state.entity_handles.get(entity).copied() {
                if seen.insert(handle) {
                    handles.push(Dynamic::from(handle));
                }
            }
        }
        handles
    }

    fn find_scene_entity(&mut self, scene_id: &str) -> Dynamic {
        let trimmed = scene_id.trim();
        if trimmed.is_empty() {
            return Dynamic::UNIT;
        }
        let state = self.state.borrow();
        state
            .scene_id_entities
            .get(trimmed)
            .copied()
            .filter(|entity| state.entity_snapshots.contains_key(entity))
            .map(|entity| Dynamic::from(entity_to_rhai(entity)))
            .unwrap_or(Dynamic::UNIT)
    }

    fn raycast(&mut self, ox: FLOAT, oy: FLOAT, dx: FLOAT, dy: FLOAT, max_dist: FLOAT) -> Map {
        self.raycast_filtered(ox, oy, dx, dy, max_dist, QueryFilters::default())
    }

    fn raycast_with_filters(&mut self, ox: FLOAT, oy: FLOAT, dx: FLOAT, dy: FLOAT, max_dist: FLOAT, filters: Map) -> Map {
        let filters = Self::parse_query_filters(filters);
        self.raycast_filtered(ox, oy, dx, dy, max_dist, filters)
    }

    fn raycast_filtered(
        &mut self,
        ox: FLOAT,
        oy: FLOAT,
        dx: FLOAT,
        dy: FLOAT,
        max_dist: FLOAT,
        filters: QueryFilters,
    ) -> Map {
        let origin = Vec2::new(ox as f32, oy as f32);
        let dir = Vec2::new(dx as f32, dy as f32);
        let dir_len_sq = dir.length_squared();
        if dir_len_sq <= f32::EPSILON {
            return Map::new();
        }
        let dir_norm = dir / dir_len_sq.sqrt();
        let max_dist = if max_dist.is_finite() && max_dist > 0.0 { max_dist as f32 } else { f32::INFINITY };
        let mut best = self.rapier_raycast(origin, dir_norm, max_dist, &filters);
        let state = self.state.borrow();
        let candidates = state
            .spatial_index
            .ray_candidates(origin, dir_norm, max_dist)
            .unwrap_or_else(|| state.entity_snapshots.keys().copied().collect());
        let mut snapshots: Vec<_> = candidates
            .into_iter()
            .filter_map(|entity| state.entity_snapshots.get(&entity).map(|snap| (entity, snap)))
            .collect();
        snapshots.sort_by_key(|(entity, _)| entity.to_bits());
        for (entity, snap) in snapshots {
            if !filters.matches(entity) {
                continue;
            }
            let half = snap.half_extents.unwrap_or_else(|| snap.scale * 0.5);
            if half.x <= 0.0 || half.y <= 0.0 {
                continue;
            }
            if let Some((dist, hit)) = Self::ray_aabb_2d(origin, dir_norm, snap.translation, half) {
                Self::update_best_ray_hit(
                    &mut best,
                    RaycastHit { entity, distance: dist, point: hit, normal: None, collider: None },
                    max_dist,
                );
            }
        }
        best.map(Self::raycast_hit_to_map).unwrap_or_default()
    }

    fn overlap_circle(&mut self, cx: FLOAT, cy: FLOAT, radius: FLOAT) -> Array {
        self.overlap_circle_filtered(cx, cy, radius, QueryFilters::default())
    }

    fn overlap_circle_with_filters(&mut self, cx: FLOAT, cy: FLOAT, radius: FLOAT, filters: Map) -> Array {
        let filters = Self::parse_query_filters(filters);
        self.overlap_circle_filtered(cx, cy, radius, filters)
    }

    fn overlap_circle_hits(&mut self, cx: FLOAT, cy: FLOAT, radius: FLOAT) -> Array {
        self.overlap_circle_hits_filtered(cx, cy, radius, QueryFilters::default())
    }

    fn overlap_circle_hits_with_filters(&mut self, cx: FLOAT, cy: FLOAT, radius: FLOAT, filters: Map) -> Array {
        let filters = Self::parse_query_filters(filters);
        self.overlap_circle_hits_filtered(cx, cy, radius, filters)
    }

    fn overlap_circle_filtered(&mut self, cx: FLOAT, cy: FLOAT, radius: FLOAT, filters: QueryFilters) -> Array {
        let center = Vec2::new(cx as f32, cy as f32);
        let radius = radius.abs() as f32;
        if radius <= 0.0 || !radius.is_finite() {
            return Array::new();
        }
        let mut seen = HashSet::new();
        let mut hits = Vec::new();
        let r2 = radius * radius;
        if let Some(mut rapier_hits) = self.rapier_overlap_circle(center, radius, &filters) {
            rapier_hits.sort_by_key(|h| h.entity.to_bits());
            for hit in rapier_hits {
                if seen.insert(hit.entity) {
                    hits.push(hit.entity);
                }
            }
        }
        let state = self.state.borrow();
        let candidates = state
            .spatial_index
            .circle_candidates(center, radius)
            .unwrap_or_else(|| state.entity_snapshots.keys().copied().collect());
        let mut snapshots: Vec<_> = candidates
            .into_iter()
            .filter_map(|entity| state.entity_snapshots.get(&entity).map(|snap| (entity, snap)))
            .collect();
        snapshots.sort_by_key(|(entity, _)| entity.to_bits());
        for (entity, snap) in snapshots {
            if !filters.matches(entity) {
                continue;
            }
            let half = snap.half_extents.unwrap_or_else(|| snap.scale * 0.5);
            if half.x <= 0.0 || half.y <= 0.0 {
                continue;
            }
            let closest = snap.translation.clamp(center - half, center + half);
            if (closest - center).length_squared() <= r2 && seen.insert(entity) {
                hits.push(entity);
            }
        }
        hits.sort_by_key(|e| e.to_bits());
        hits.into_iter().map(|entity| Dynamic::from(entity_to_rhai(entity))).collect()
    }

    fn overlap_circle_hits_filtered(&mut self, cx: FLOAT, cy: FLOAT, radius: FLOAT, filters: QueryFilters) -> Array {
        let center = Vec2::new(cx as f32, cy as f32);
        let radius = radius.abs() as f32;
        if radius <= 0.0 || !radius.is_finite() {
            return Array::new();
        }
        let mut merged: HashMap<Entity, (OverlapHit, Option<Vec2>)> = HashMap::new();
        if let Some(rapier_hits) = self.rapier_overlap_circle(center, radius, &filters) {
            for hit in rapier_hits {
                let entry = merged.entry(hit.entity).or_insert((hit, None));
                if entry.0.collider.is_none() {
                    entry.0.collider = hit.collider;
                }
            }
        }
        let state = self.state.borrow();
        let r2 = radius * radius;
        let candidates = state
            .spatial_index
            .circle_candidates(center, radius)
            .unwrap_or_else(|| state.entity_snapshots.keys().copied().collect());
        let mut snapshots: Vec<_> = candidates
            .into_iter()
            .filter_map(|entity| state.entity_snapshots.get(&entity).map(|snap| (entity, snap)))
            .collect();
        snapshots.sort_by_key(|(entity, _)| entity.to_bits());
        for (entity, snap) in snapshots {
            if !filters.matches(entity) {
                continue;
            }
            let half = snap.half_extents.unwrap_or_else(|| snap.scale * 0.5);
            if half.x <= 0.0 || half.y <= 0.0 {
                continue;
            }
            let closest = snap.translation.clamp(center - half, center + half);
            if (closest - center).length_squared() <= r2 {
                let entry = merged.entry(entity).or_insert((OverlapHit { entity, collider: None }, None));
                if entry.1.is_none() {
                    entry.1 = Some(snap.translation);
                }
            }
        }
        let mut hits: Vec<_> = merged.into_iter().collect();
        hits.sort_by_key(|(entity, _)| entity.to_bits());
        hits.into_iter()
            .map(|(_, (hit, translation))| Dynamic::from(Self::overlap_hit_to_map(hit, center, translation)))
            .collect()
    }

    fn rapier_context(&self) -> Option<PhysicsQueryContext> {
        self.state.borrow().physics_ctx
    }

    fn overlap_hit_to_map(hit: OverlapHit, center: Vec2, translation: Option<Vec2>) -> Map {
        let mut out = Map::new();
        out.insert("entity".into(), Dynamic::from(entity_to_rhai(hit.entity)));
        if let Some(collider) = hit.collider {
            out.insert("collider".into(), Dynamic::from(Self::collider_to_int(collider)));
        }
        if let Some(pos) = translation {
            let delta = pos - center;
            let len_sq = delta.length_squared();
            if len_sq > 1e-8 {
                out.insert("approx_normal".into(), Dynamic::from(Self::vec2_to_array(delta.normalize())));
            }
        }
        out
    }

    fn raycast_hit_to_map(hit: RaycastHit) -> Map {
        let mut out = Map::new();
        out.insert("entity".into(), Dynamic::from(entity_to_rhai(hit.entity)));
        out.insert("distance".into(), Dynamic::from(hit.distance as FLOAT));
        out.insert("point".into(), Dynamic::from(Self::vec2_to_array(hit.point)));
        if let Some(normal) = hit.normal {
            out.insert("normal".into(), Dynamic::from(Self::vec2_to_array(normal)));
        }
        if let Some(collider) = hit.collider {
            out.insert("collider".into(), Dynamic::from(Self::collider_to_int(collider)));
        }
        out
    }

    fn collider_to_int(handle: ColliderHandle) -> ScriptHandle {
        let (idx, gen) = handle.into_raw_parts();
        let packed = ((gen as u64) << 32) | (idx as u64);
        packed as ScriptHandle
    }

    fn update_best_ray_hit(best: &mut Option<RaycastHit>, candidate: RaycastHit, max_dist: f32) {
        if !candidate.distance.is_finite() || candidate.distance < 0.0 || candidate.distance > max_dist {
            return;
        }
        let replace = match best {
            None => true,
            Some(existing) => {
                if candidate.distance < existing.distance - 1e-5 {
                    true
                } else {
                    (candidate.distance - existing.distance).abs() <= 1e-5
                        && existing.collider.is_none()
                        && candidate.collider.is_some()
                }
            }
        };
        if replace {
            *best = Some(candidate);
        }
    }

    fn rapier_raycast(
        &self,
        origin: Vec2,
        dir: Vec2,
        max_dist: f32,
        filters: &QueryFilters,
    ) -> Option<RaycastHit> {
        if dir.length_squared() <= f32::EPSILON || !max_dist.is_finite() || max_dist <= 0.0 {
            return None;
        }
        let ctx = self.rapier_context()?;
        let rapier = unsafe { ctx.rapier()? };
        let view = rapier.query_view();
        let ray = RapierRay::new(Point::new(origin.x, origin.y), Vector::new(dir.x, dir.y));
        let filter = RapierQueryFilter { flags: QueryFilterFlags::EXCLUDE_SENSORS, ..Default::default() };
        let mut best: Option<RaycastHit> = None;
        let mut callback = |handle: ColliderHandle, hit: RayIntersection| {
            if let Some(entity) = view.collider_entities.get(&handle).copied() {
                if filters.matches(entity) {
                    let distance = hit.time_of_impact as f32;
                    let candidate = RaycastHit {
                        entity,
                        distance,
                        point: origin + dir * distance,
                        normal: Some(Vec2::new(hit.normal.x, hit.normal.y)),
                        collider: Some(handle),
                    };
                    Self::update_best_ray_hit(&mut best, candidate, max_dist);
                }
            }
            true
        };
        view.pipeline.intersections_with_ray(&view.bodies, &view.colliders, &ray, max_dist, true, filter, &mut callback);
        best
    }

    fn rapier_overlap_circle(
        &self,
        center: Vec2,
        radius: f32,
        filters: &QueryFilters,
    ) -> Option<Vec<OverlapHit>> {
        let ctx = self.rapier_context()?;
        let rapier = unsafe { ctx.rapier()? };
        let view = rapier.query_view();
        let iso = Isometry::new(Vector::new(center.x, center.y), 0.0);
        let shape = SharedShape::ball(radius);
        let filter = RapierQueryFilter { flags: QueryFilterFlags::EXCLUDE_SENSORS, ..Default::default() };
        let mut hits = Vec::new();
        let mut callback = |handle: ColliderHandle| {
            if let Some(entity) = view.collider_entities.get(&handle).copied() {
                if filters.matches(entity) {
                    hits.push(OverlapHit { entity, collider: Some(handle) });
                }
            }
            true
        };
        view.pipeline.intersections_with_shape(
            &view.bodies,
            &view.colliders,
            &iso,
            &*shape,
            filter,
            &mut callback,
        );
        if hits.is_empty() { None } else { Some(hits) }
    }

    fn input_forward(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.forward)
    }

    fn input_backward(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.backward)
    }

    fn input_left(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.left)
    }

    fn input_right(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.right)
    }

    fn input_ascend(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.ascend)
    }

    fn input_descend(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.descend)
    }

    fn input_boost(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.boost)
    }

    fn input_ctrl(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.ctrl)
    }

    fn input_left_mouse(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.left_mouse)
    }

    fn input_right_mouse(&mut self) -> bool {
        self.state.borrow().input_snapshot.as_ref().map_or(false, |s| s.right_mouse)
    }

    fn input_cursor(&mut self) -> Array {
        if let Some(snap) = self.state.borrow().input_snapshot.as_ref() {
            if let Some(cursor) = snap.cursor {
                return Self::vec2_to_array(cursor);
            }
        }
        Array::new()
    }

    fn input_cursor_world(&mut self) -> Array {
        if let Some(snap) = self.state.borrow().input_snapshot.as_ref() {
            if let Some(cursor) = snap.cursor_world {
                return Self::vec2_to_array(cursor);
            }
        }
        Array::new()
    }

    fn input_mouse_delta(&mut self) -> Array {
        if let Some(snap) = self.state.borrow().input_snapshot.as_ref() {
            return Self::vec2_to_array(snap.mouse_delta);
        }
        Array::new()
    }

    fn input_wheel(&mut self) -> FLOAT {
        self.state
            .borrow()
            .input_snapshot
            .as_ref()
            .map(|s| s.wheel as FLOAT)
            .unwrap_or(0.0)
    }

    fn state_get(&mut self, key: &str) -> Dynamic {
        self.instance_state
            .as_ref()
            .and_then(|s| {
                let state = s.borrow();
                state.persistent.get(key).cloned()
            })
            .unwrap_or(Dynamic::UNIT)
    }

    fn state_set(&mut self, key: &str, value: Dynamic) {
        if let Some(state) = &self.instance_state {
            state.borrow_mut().persistent.insert(key.into(), value);
        }
    }

    fn state_clear(&mut self) {
        if let Some(state) = &self.instance_state {
            state.borrow_mut().persistent.clear();
        }
    }

    fn state_keys(&mut self) -> Array {
        if let Some(state) = &self.instance_state {
            return state.borrow().persistent.keys().map(|k| Dynamic::from(k.clone())).collect();
        }
        Array::new()
    }

    fn stat_get(&mut self, key: &str) -> FLOAT {
        let Some(key) = Self::stat_key(key) else { return 0.0; };
        self.state
            .borrow()
            .global_stats
            .get(&key)
            .copied()
            .unwrap_or(0.0) as FLOAT
    }

    fn stat_set(&mut self, key: &str, value: Dynamic) -> bool {
        let Some(key) = Self::stat_key(key) else { return false; };
        let number = value
            .clone()
            .try_cast::<FLOAT>()
            .or_else(|| value.try_cast::<rhai::INT>().map(|v| v as FLOAT));
        let Some(raw) = number else { return false; };
        let val = raw as f32;
        if !self.ensure_finite("stat_set", &[val]) {
            return false;
        }
        self.state.borrow_mut().global_stats.insert(key, val as f64);
        true
    }

    fn stat_add(&mut self, key: &str, delta: FLOAT) -> FLOAT {
        let Some(key) = Self::stat_key(key) else { return 0.0; };
        let delta = delta as f32;
        if !self.ensure_finite("stat_add", &[delta]) {
            return 0.0;
        }
        let mut state = self.state.borrow_mut();
        let entry = state.global_stats.entry(key).or_insert(0.0);
        *entry += delta as f64;
        *entry as FLOAT
    }

    fn stat_clear(&mut self, key: &str) -> bool {
        let Some(key) = Self::stat_key(key) else { return false; };
        self.state.borrow_mut().global_stats.remove(&key).is_some()
    }

    fn stat_keys(&mut self) -> Array {
        let mut keys: Vec<_> = self.state.borrow().global_stats.keys().cloned().collect();
        keys.sort();
        keys.into_iter().map(Dynamic::from).collect()
    }

    fn is_hot_reload(&mut self) -> bool {
        self.instance_state
            .as_ref()
            .map(|s| s.borrow().is_hot_reload)
            .unwrap_or(false)
    }

    fn vec2(&mut self, x: FLOAT, y: FLOAT) -> Array {
        Self::vec2_to_array(Vec2::new(x as f32, y as f32))
    }

    fn vec2_len(&mut self, v: Array) -> FLOAT {
        Self::array_to_vec2(&v).map(|vec| vec.length() as FLOAT).unwrap_or(0.0)
    }

    fn vec2_normalize(&mut self, v: Array) -> Array {
        let vec = Self::array_to_vec2(&v).unwrap_or(Vec2::ZERO);
        if vec.length_squared() <= f32::EPSILON {
            Self::vec2_to_array(Vec2::ZERO)
        } else {
            Self::vec2_to_array(vec.normalize())
        }
    }

    fn vec2_distance(&mut self, a: Array, b: Array) -> FLOAT {
        match (Self::array_to_vec2(&a), Self::array_to_vec2(&b)) {
            (Some(va), Some(vb)) => va.distance(vb) as FLOAT,
            _ => 0.0,
        }
    }

    fn vec2_lerp(&mut self, a: Array, b: Array, t: FLOAT) -> Array {
        let t = (t as f32).clamp(0.0, 1.0);
        match (Self::array_to_vec2(&a), Self::array_to_vec2(&b)) {
            (Some(va), Some(vb)) => Self::vec2_to_array(va + (vb - va) * t),
            _ => Array::new(),
        }
    }

    fn move_toward_vec2(&mut self, current: Array, target: Array, max_delta: FLOAT) -> Array {
        let Some(cur) = Self::array_to_vec2(&current) else { return Array::new(); };
        let Some(trg) = Self::array_to_vec2(&target) else { return Array::new(); };
        let max_delta = max_delta as f32;
        let delta = trg - cur;
        let dist = delta.length();
        if dist <= max_delta || dist <= 1e-4 {
            Self::vec2_to_array(trg)
        } else {
            let step = delta.normalize_or_zero() * max_delta;
            Self::vec2_to_array(cur + step)
        }
    }

    fn angle_to_vec(&mut self, angle: FLOAT) -> Array {
        let a = angle as f32;
        Self::vec2_to_array(Vec2::new(a.cos(), a.sin()))
    }

    fn vec_to_angle(&mut self, v: Array) -> FLOAT {
        Self::array_to_vec2(&v)
            .map(|vec| vec.y.atan2(vec.x) as FLOAT)
            .unwrap_or(0.0)
    }

    fn wrap_angle_pi(&mut self, rad: FLOAT) -> FLOAT {
        let mut a = rad as f32;
        let pi = std::f32::consts::PI;
        let two_pi = std::f32::consts::PI * 2.0;
        while a > pi {
            a -= two_pi;
        }
        while a < -pi {
            a += two_pi;
        }
        a as FLOAT
    }

    fn vec2_to_array(v: Vec2) -> Array {
        vec![Dynamic::from(v.x as FLOAT), Dynamic::from(v.y as FLOAT)]
    }

    fn vec4_to_array(v: Vec4) -> Array {
        vec![
            Dynamic::from(v.x as FLOAT),
            Dynamic::from(v.y as FLOAT),
            Dynamic::from(v.z as FLOAT),
            Dynamic::from(v.w as FLOAT),
        ]
    }

    fn array_to_vec2(arr: &Array) -> Option<Vec2> {
        if arr.len() < 2 {
            return None;
        }
        let x: Option<FLOAT> = arr[0].clone().try_cast();
        let y: Option<FLOAT> = arr[1].clone().try_cast();
        match (x, y) {
            (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Some(Vec2::new(x as f32, y as f32)),
            _ => None,
        }
    }

    fn stat_key(key: &str) -> Option<String> {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    fn entities_from_array(arr: &Array) -> HashSet<Entity> {
        arr.iter()
            .filter_map(|value| value.clone().try_cast::<ScriptHandle>())
            .filter(|handle| *handle >= 0)
            .map(|handle| Entity::from_bits(handle as u64))
            .collect()
    }

    fn parse_query_filters(filters: Map) -> QueryFilters {
        let mut parsed = QueryFilters::default();
        if let Some(include) = filters.get("include") {
            if let Some(arr) = include.clone().try_cast::<Array>() {
                let set = Self::entities_from_array(&arr);
                if !set.is_empty() {
                    parsed.include = Some(set);
                }
            }
        }
        if let Some(exclude) = filters.get("exclude") {
            if let Some(arr) = exclude.clone().try_cast::<Array>() {
                parsed.exclude = Self::entities_from_array(&arr);
            }
        }
        parsed
    }

    fn ray_aabb_2d(origin: Vec2, dir: Vec2, center: Vec2, half: Vec2) -> Option<(f32, Vec2)> {
        if dir.length_squared() <= f32::EPSILON {
            return None;
        }
        let dir = dir.normalize();
        let min = center - half;
        let max = center + half;
        let mut t_min = 0.0_f32;
        let mut t_max = f32::INFINITY;
        for i in 0..2 {
            let o = if i == 0 { origin.x } else { origin.y };
            let d = if i == 0 { dir.x } else { dir.y };
            let min_axis = if i == 0 { min.x } else { min.y };
            let max_axis = if i == 0 { max.x } else { max.y };
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
        if !t_hit.is_finite() {
            return None;
        }
        let hit = origin + dir * t_hit;
        Some((t_hit, hit))
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_sprite(
        &mut self,
        atlas: &str,
        region: &str,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        vx: FLOAT,
        vy: FLOAT,
    ) -> ScriptHandle {
        self.spawn_sprite_internal(atlas, region, x, y, scale, vx, vy)
    }

    fn spawn_sprite_internal(
        &mut self,
        atlas: &str,
        region: &str,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        vx: FLOAT,
        vy: FLOAT,
    ) -> ScriptHandle {
        let x = x as f32;
        let y = y as f32;
        let scale = scale as f32;
        let vx = vx as f32;
        let vy = vy as f32;
        if !self.ensure_finite("spawn_sprite", &[x, y, scale, vx, vy]) {
            return -1;
        }
        if scale <= 0.0 {
            return -1;
        }
        self.push_command_with_handle(|handle| ScriptCommand::Spawn {
            handle,
            atlas: atlas.to_string(),
            region: region.to_string(),
            position: Vec2::new(x, y),
            scale,
            velocity: Vec2::new(vx, vy),
        })
    }

    fn spawn_sprite_safe(
        &mut self,
        atlas: &str,
        region: &str,
        x: FLOAT,
        y: FLOAT,
        scale: FLOAT,
        vx: FLOAT,
        vy: FLOAT,
    ) -> Dynamic {
        let handle = self.spawn_sprite_internal(atlas, region, x, y, scale, vx, vy);
        if handle < 0 {
            Dynamic::UNIT
        } else {
            Dynamic::from(handle)
        }
    }

    fn set_velocity(&mut self, handle: ScriptHandle, vx: FLOAT, vy: FLOAT) -> bool {
        let vx = vx as f32;
        let vy = vy as f32;
        if !self.ensure_finite("set_velocity", &[vx, vy]) {
            return false;
        }
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("set_velocity"));
            return false;
        }
        self.push_command_plain(ScriptCommand::SetVelocity { handle, velocity: Vec2::new(vx, vy) })
    }

    fn set_position(&mut self, handle: ScriptHandle, x: FLOAT, y: FLOAT) -> bool {
        let x = x as f32;
        let y = y as f32;
        if !self.ensure_finite("set_position", &[x, y]) {
            return false;
        }
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("set_position"));
            return false;
        }
        self.push_command_plain(ScriptCommand::SetPosition { handle, position: Vec2::new(x, y) })
    }

    fn set_rotation(&mut self, handle: ScriptHandle, radians: FLOAT) -> bool {
        let radians = radians as f32;
        if !self.ensure_finite("set_rotation", &[radians]) {
            return false;
        }
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("set_rotation"));
            return false;
        }
        self.push_command_plain(ScriptCommand::SetRotation { handle, rotation: radians })
    }

    fn set_scale(&mut self, handle: ScriptHandle, sx: FLOAT, sy: FLOAT) -> bool {
        let sx = sx as f32;
        let sy = sy as f32;
        if !self.ensure_finite("set_scale", &[sx, sy]) {
            return false;
        }
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("set_scale"));
            return false;
        }
        let clamped = Vec2::new(sx.max(0.01), sy.max(0.01));
        self.push_command_plain(ScriptCommand::SetScale { handle, scale: clamped })
    }

    fn set_tint(&mut self, handle: ScriptHandle, r: FLOAT, g: FLOAT, b: FLOAT, a: FLOAT) -> bool {
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let a = a as f32;
        if !self.ensure_finite("set_tint", &[r, g, b, a]) {
            return false;
        }
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("set_tint"));
            return false;
        }
        self.push_command_plain(ScriptCommand::SetTint { handle, tint: Some(Vec4::new(r, g, b, a)) })
    }

    fn clear_tint(&mut self, handle: ScriptHandle) -> bool {
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("clear_tint"));
            return false;
        }
        self.push_command_plain(ScriptCommand::SetTint { handle, tint: None })
    }

    fn set_sprite_region(&mut self, handle: ScriptHandle, region: &str) -> bool {
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("set_sprite_region"));
            return false;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetSpriteRegion { handle, region: region.to_string() });
        true
    }

    fn despawn(&mut self, handle: ScriptHandle) -> bool {
        if !self.handle_is_usable(handle) {
            self.state.borrow_mut().record_invalid_handle_use(Some("despawn"));
            return false;
        }
        self.push_command_plain(ScriptCommand::Despawn { handle })
    }

    fn spawn_prefab(&mut self, path: &str) -> ScriptHandle {
        self.spawn_prefab_with_tag_internal(path, None)
    }

    #[allow(dead_code)]
    fn spawn_prefab_with_tag(&mut self, path: &str, tag: Option<String>) -> ScriptHandle {
        self.spawn_prefab_with_tag_internal(path, tag)
    }

    fn spawn_prefab_with_tag_internal(&mut self, path: &str, tag: Option<String>) -> ScriptHandle {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return -1;
        }
        let tag = tag.and_then(|t| {
            let trimmed = t.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        });
        let path_owned = trimmed.to_string();
        self.push_command_with_handle(move |handle| {
            ScriptCommand::SpawnPrefab { handle, path: path_owned.clone(), tag: tag.clone() }
        })
    }

    fn spawn_template(&mut self, name: &str) -> ScriptHandle {
        self.spawn_template_with_tag_internal(name, None)
    }

    #[allow(dead_code)]
    fn spawn_template_with_tag(&mut self, name: &str, tag: Option<String>) -> ScriptHandle {
        self.spawn_template_with_tag_internal(name, tag)
    }

    fn spawn_template_with_tag_internal(&mut self, name: &str, tag: Option<String>) -> ScriptHandle {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return -1;
        }
        let tag = tag.and_then(|t| {
            let trimmed = t.trim();
            if trimmed.is_empty() { None } else { Some(trimmed.to_string()) }
        });
        let template_owned = trimmed.to_string();
        self.push_command_with_handle(move |handle| {
            ScriptCommand::SpawnTemplate { handle, template: template_owned.clone(), tag: tag.clone() }
        })
    }

    fn spawn_player(&mut self, tag: &str) -> ScriptHandle {
        self.spawn_template_with_tag_internal("player", Some(tag.to_string()))
    }

    fn spawn_enemy(&mut self, template: &str, tag: &str) -> ScriptHandle {
        self.spawn_template_with_tag_internal(template, Some(tag.to_string()))
    }

    fn spawn_prefab_safe_with_tag(&mut self, path: &str, tag: &str) -> Dynamic {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            self.state.borrow_mut().record_spawn_failure("prefab_empty_path");
            return Dynamic::UNIT;
        }
        let handle = self.spawn_prefab_with_tag_internal(trimmed, Some(tag.to_string()));
        if handle < 0 {
            self.state.borrow_mut().record_spawn_failure("prefab_spawn_rejected");
            Dynamic::UNIT
        } else {
            Dynamic::from(handle)
        }
    }

    fn spawn_template_safe_with_tag(&mut self, name: &str, tag: &str) -> Dynamic {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            self.state.borrow_mut().record_spawn_failure("template_empty_name");
            return Dynamic::UNIT;
        }
        let handle = self.spawn_template_with_tag_internal(trimmed, Some(tag.to_string()));
        if handle < 0 {
            self.state.borrow_mut().record_spawn_failure("template_spawn_rejected");
            Dynamic::UNIT
        } else {
            Dynamic::from(handle)
        }
    }

    fn spawn_prefab_safe(&mut self, path: &str) -> Dynamic {
        self.spawn_prefab_safe_with_tag(path, "")
    }

    fn spawn_template_safe(&mut self, name: &str) -> Dynamic {
        self.spawn_template_safe_with_tag(name, "")
    }

    fn spawn_player_safe(&mut self, tag: &str) -> Dynamic {
        let handle = self.spawn_player(tag);
        if handle < 0 {
            Dynamic::UNIT
        } else {
            Dynamic::from(handle)
        }
    }

    fn spawn_enemy_safe(&mut self, template: &str, tag: &str) -> Dynamic {
        let handle = self.spawn_enemy(template, tag);
        if handle < 0 {
            Dynamic::UNIT
        } else {
            Dynamic::from(handle)
        }
    }

    fn entity_set_position(&mut self, entity_bits: ScriptHandle, x: FLOAT, y: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let pos = Vec2::new(x as f32, y as f32);
        if !self.ensure_finite("entity_set_position", &[pos.x, pos.y]) {
            return false;
        }
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_invalid_handle_use(Some("entity_set_position"));
            return false;
        }
        self.push_command_plain(ScriptCommand::EntitySetPosition { entity, position: pos })
    }

    fn entity_set_rotation(&mut self, entity_bits: ScriptHandle, radians: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let rot = radians as f32;
        if !self.ensure_finite("entity_set_rotation", &[rot]) {
            return false;
        }
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_invalid_handle_use(Some("entity_set_rotation"));
            return false;
        }
        self.push_command_plain(ScriptCommand::EntitySetRotation { entity, rotation: rot })
    }

    fn entity_set_scale(&mut self, entity_bits: ScriptHandle, sx: FLOAT, sy: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let sx = sx as f32;
        let sy = sy as f32;
        if !self.ensure_finite("entity_set_scale", &[sx, sy]) {
            return false;
        }
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_invalid_handle_use(Some("entity_set_scale"));
            return false;
        }
        let clamped = Vec2::new(sx.max(0.01), sy.max(0.01));
        self.push_command_plain(ScriptCommand::EntitySetScale { entity, scale: clamped })
    }

    fn entity_set_tint(&mut self, entity_bits: ScriptHandle, r: FLOAT, g: FLOAT, b: FLOAT, a: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let a = a as f32;
        if !self.ensure_finite("entity_set_tint", &[r, g, b, a]) {
            return false;
        }
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_invalid_handle_use(Some("entity_set_tint"));
            return false;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::EntitySetTint { entity, tint: Some(Vec4::new(r, g, b, a)) });
        true
    }

    fn entity_clear_tint(&mut self, entity_bits: ScriptHandle) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_invalid_handle_use(Some("entity_clear_tint"));
            return false;
        }
        self.push_command_plain(ScriptCommand::EntitySetTint { entity, tint: None })
    }

    fn entity_set_velocity(&mut self, entity_bits: ScriptHandle, vx: FLOAT, vy: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let vx = vx as f32;
        let vy = vy as f32;
        if !self.ensure_finite("entity_set_velocity", &[vx, vy]) {
            return false;
        }
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_invalid_handle_use(Some("entity_set_velocity"));
            return false;
        }
        self.push_command_plain(ScriptCommand::EntitySetVelocity { entity, velocity: Vec2::new(vx, vy) })
    }

    fn entity_despawn(&mut self, entity_bits: ScriptHandle) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_despawn_dead();
            return false;
        }
        self.push_command_plain(ScriptCommand::EntityDespawn { entity })
    }

    fn despawn_safe(&mut self, handle: ScriptHandle) -> bool {
        if self.handle_is_alive(handle) {
            self.despawn(handle)
        } else {
            self.state.borrow_mut().record_despawn_dead();
            true
        }
    }

    fn set_auto_spawn_rate(&mut self, rate: FLOAT) {
        let rate = rate as f32;
        if !self.ensure_finite("set_auto_spawn_rate", &[rate]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetAutoSpawnRate { rate });
    }

    fn set_spawn_per_press(&mut self, count: i64) {
        let clamped = count.clamp(0, 10_000) as i32;
        let _ = self.push_command_plain(ScriptCommand::SetSpawnPerPress { count: clamped });
    }

    fn set_emitter_rate(&mut self, rate: FLOAT) {
        let rate = rate as f32;
        if !self.ensure_finite("set_emitter_rate", &[rate]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetEmitterRate { rate: rate.max(0.0) });
    }

    fn set_emitter_spread(&mut self, spread: FLOAT) {
        let spread = spread as f32;
        if !self.ensure_finite("set_emitter_spread", &[spread]) {
            return;
        }
        let clamped = spread.clamp(0.0, std::f32::consts::PI);
        let _ = self.push_command_plain(ScriptCommand::SetEmitterSpread { spread: clamped });
    }

    fn set_emitter_speed(&mut self, speed: FLOAT) {
        let speed = speed as f32;
        if !self.ensure_finite("set_emitter_speed", &[speed]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetEmitterSpeed { speed: speed.max(0.0) });
    }

    fn set_emitter_lifetime(&mut self, lifetime: FLOAT) {
        let lifetime = lifetime as f32;
        if !self.ensure_finite("set_emitter_lifetime", &[lifetime]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetEmitterLifetime { lifetime: lifetime.max(0.05) });
    }

    fn set_emitter_start_color(&mut self, r: FLOAT, g: FLOAT, b: FLOAT, a: FLOAT) {
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let a = a as f32;
        if !self.ensure_finite("set_emitter_start_color", &[r, g, b, a]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetEmitterStartColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_end_color(&mut self, r: FLOAT, g: FLOAT, b: FLOAT, a: FLOAT) {
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let a = a as f32;
        if !self.ensure_finite("set_emitter_end_color", &[r, g, b, a]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetEmitterEndColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_start_size(&mut self, size: FLOAT) {
        let size = size as f32;
        if !self.ensure_finite("set_emitter_start_size", &[size]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetEmitterStartSize { size: size.max(0.01) });
    }

    fn set_emitter_end_size(&mut self, size: FLOAT) {
        let size = size as f32;
        if !self.ensure_finite("set_emitter_end_size", &[size]) {
            return;
        }
        let _ = self.push_command_plain(ScriptCommand::SetEmitterEndSize { size: size.max(0.01) });
    }

    fn random_range(&mut self, min: FLOAT, max: FLOAT) -> FLOAT {
        let mut lo = min as f32;
        let mut hi = max as f32;
        if !lo.is_finite() || !hi.is_finite() {
            self.log("random_range received non-finite bounds; returning 0.0");
            return 0.0;
        }
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        if (hi - lo).abs() <= f32::EPSILON {
            return lo as FLOAT;
        }
        let mut state = self.state.borrow_mut();
        let sample = if let Some(rng) = state.rng.as_mut() {
            rng.gen_range(lo..hi)
        } else {
            rand::thread_rng().gen_range(lo..hi)
        };
        sample as FLOAT
    }

    fn rand_seed(&mut self, seed: rhai::INT) {
        let seed = seed as u64;
        self.state.borrow_mut().rng = Some(StdRng::seed_from_u64(seed));
    }

    fn time_scale(&mut self) -> FLOAT {
        let scale = self.state.borrow().time_scale;
        if scale.is_finite() { scale as FLOAT } else { 1.0 }
    }

    fn set_time_scale(&mut self, scale: FLOAT) -> bool {
        let scale = scale as f32;
        if !scale.is_finite() || scale < 0.0 {
            return false;
        }
        self.state.borrow_mut().time_scale = scale;
        true
    }

    fn delta_seconds(&mut self) -> FLOAT {
        let dt = self.state.borrow().last_scaled_dt;
        if dt.is_finite() { dt as FLOAT } else { 0.0 }
    }

    fn unscaled_delta_seconds(&mut self) -> FLOAT {
        let dt = self.state.borrow().last_unscaled_dt;
        if dt.is_finite() { dt as FLOAT } else { 0.0 }
    }

    fn time_seconds(&mut self) -> FLOAT {
        let t = self.state.borrow().scaled_time;
        if t.is_finite() { t as FLOAT } else { 0.0 }
    }

    fn unscaled_time_seconds(&mut self) -> FLOAT {
        let t = self.state.borrow().unscaled_time;
        if t.is_finite() { t as FLOAT } else { 0.0 }
    }

    fn timer_start(&mut self, name: &str, seconds: FLOAT) -> bool {
        self.timer_start_internal(name, seconds, false)
    }

    fn timer_start_repeat(&mut self, name: &str, seconds: FLOAT) -> bool {
        self.timer_start_internal(name, seconds, true)
    }

    fn timer_fired(&mut self, name: &str) -> bool {
        let key = name.trim();
        if key.is_empty() {
            return false;
        }
        self.with_timer_store(|timers| timers.get_mut(key).map(|t| t.consume_fired()).unwrap_or(false))
    }

    fn timer_remaining(&mut self, name: &str) -> FLOAT {
        let key = name.trim();
        if key.is_empty() {
            return 0.0;
        }
        self.with_timer_store(|timers| timers.get(key).map(|t| t.remaining() as FLOAT).unwrap_or(0.0))
    }

    fn timer_clear(&mut self, name: &str) -> bool {
        let key = name.trim();
        if key.is_empty() {
            return false;
        }
        self.with_timer_store(|timers| timers.remove(key).is_some())
    }

    fn timer_start_internal(&mut self, name: &str, seconds: FLOAT, repeat: bool) -> bool {
        let duration = (seconds as f32).max(0.0);
        if !duration.is_finite() {
            return false;
        }
        let key = name.trim();
        if key.is_empty() {
            return false;
        }
        self.with_timer_store(|timers| {
            timers.insert(key.to_string(), TimerState::new(duration, repeat));
        });
        true
    }

    fn with_timer_store<R, F>(&mut self, mut f: F) -> R
    where
        F: FnMut(&mut HashMap<String, TimerState>) -> R,
    {
        if let Some(state) = &self.instance_state {
            let mut state = state.borrow_mut();
            f(&mut state.timers)
        } else {
            let mut state = self.state.borrow_mut();
            f(&mut state.timers)
        }
    }

    fn listen(&mut self, event: &str, handler: &str) -> ListenerHandle {
        self.register_listener(event, handler, None)
    }

    fn listen_for_entity(&mut self, event: &str, entity_bits: ScriptHandle, handler: &str) -> ListenerHandle {
        let entity = Entity::from_bits(entity_bits as u64);
        if !self.entity_is_alive(entity) {
            self.state.borrow_mut().record_invalid_handle_use(Some("listen_for_entity"));
            return -1;
        }
        self.register_listener(event, handler, Some(entity))
    }

    fn unlisten(&mut self, handle: ListenerHandle) -> bool {
        if handle <= 0 {
            return false;
        }
        let id = handle as u64;
        let mut state = self.state.borrow_mut();
        let before = state.event_listeners.len();
        state.event_listeners.retain(|listener| listener.id != id);
        before != state.event_listeners.len()
    }

    fn emit(&mut self, name: &str) -> bool {
        self.enqueue_event(name, Dynamic::UNIT, None)
    }

    fn emit_with_payload(&mut self, name: &str, payload: Dynamic) -> bool {
        self.enqueue_event(name, payload, None)
    }

    fn emit_to(&mut self, name: &str, entity_bits: ScriptHandle) -> bool {
        let target = Entity::from_bits(entity_bits as u64);
        if !self.entity_is_alive(target) {
            self.state.borrow_mut().record_invalid_handle_use(Some("emit_to"));
            return false;
        }
        self.enqueue_event(name, Dynamic::UNIT, Some(target))
    }

    fn emit_to_with_payload(&mut self, name: &str, entity_bits: ScriptHandle, payload: Dynamic) -> bool {
        let target = Entity::from_bits(entity_bits as u64);
        if !self.entity_is_alive(target) {
            self.state.borrow_mut().record_invalid_handle_use(Some("emit_to_with_payload"));
            return false;
        }
        self.enqueue_event(name, payload, Some(target))
    }

    fn register_listener(&mut self, event: &str, handler: &str, scope: Option<Entity>) -> ListenerHandle {
        let event = event.trim();
        let handler = handler.trim();
        if event.is_empty() || handler.is_empty() {
            return -1;
        }
        let (owner, _) = self.listener_owner();
        let mut state = self.state.borrow_mut();
        let id = state.next_listener_id;
        state.next_listener_id = state.next_listener_id.saturating_add(1);
        state.event_listeners.push(ScriptEventListener {
            id,
            name: Arc::from(event),
            handler: Arc::from(handler),
            owner,
            scope_entity: scope,
        });
        id as ListenerHandle
    }

    fn enqueue_event(&mut self, name: &str, payload: Dynamic, target: Option<Entity>) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        let (_, source) = self.listener_owner();
        let target = target.or(source);
        let mut state = self.state.borrow_mut();
        let pending_total = state.events_dispatched + state.event_queue.len();
        if pending_total >= SCRIPT_EVENT_QUEUE_LIMIT {
            if !state.event_overflowed {
                state.logs.push(format!(
                    "event queue limit ({}) reached; dropping '{}'",
                    SCRIPT_EVENT_QUEUE_LIMIT, name
                ));
                state.event_overflowed = true;
            }
            return false;
        }
        let event = ScriptEvent { name: Arc::from(name), payload, target, source };
        state.event_queue.push_back(event);
        true
    }

    fn listener_owner(&self) -> (ListenerOwner, Option<Entity>) {
        if let Some(state) = &self.instance_state {
            let state = state.borrow();
            let entity = state.entity;
            let owner = state.instance_id.map(ListenerOwner::Instance).unwrap_or(ListenerOwner::Host);
            (owner, entity)
        } else {
            (ListenerOwner::Host, None)
        }
    }

    fn event_to_map(event: &ScriptEvent, listener_entity: Option<Entity>) -> Map {
        let mut map = Map::new();
        map.insert("name".into(), Dynamic::from(event.name.to_string()));
        map.insert("payload".into(), event.payload.clone());
        if let Some(target) = event.target {
            map.insert("target".into(), Dynamic::from(entity_to_rhai(target)));
        }
        if let Some(source) = event.source {
            map.insert("source".into(), Dynamic::from(entity_to_rhai(source)));
        }
        if let Some(listener) = listener_entity {
            map.insert("listener".into(), Dynamic::from(entity_to_rhai(listener)));
        }
        map
    }

    fn move_toward(&mut self, current: FLOAT, target: FLOAT, max_delta: FLOAT) -> FLOAT {
        let current = current as f32;
        let target = target as f32;
        let max_delta = max_delta.abs() as f32;
        if !self.ensure_finite("move_toward", &[current, target, max_delta]) {
            return current as FLOAT;
        }
        let delta = target - current;
        if delta.abs() <= max_delta {
            target as FLOAT
        } else {
            (current + delta.signum() * max_delta) as FLOAT
        }
    }

    fn log(&mut self, message: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.logs.push(message.to_string());
        }
        println!("[script] {message}");
    }

    fn ensure_finite(&mut self, label: &str, values: &[f32]) -> bool {
        if values.iter().all(|v| v.is_finite()) {
            true
        } else {
            self.log(&format!("{label} received non-finite values; command ignored"));
            false
        }
    }

    fn owner_label(&self) -> String {
        match self.owner {
            ListenerOwner::Host => "host".to_string(),
            ListenerOwner::Instance(id) => format!("instance {id}"),
        }
    }

    fn push_command_plain(&mut self, command: ScriptCommand) -> bool {
        let owner_label = self.owner_label();
        let mut state = self.state.borrow_mut();
        if let Some(quota) = state.command_quota {
            let count = state.commands_per_owner.entry(self.owner).or_insert(0);
            if *count >= quota {
                state
                    .logs
                    .push(format!("Command quota exceeded for {owner_label} (quota {quota})"));
                return false;
            }
            *count += 1;
        }
        state.commands.push(command);
        true
    }

    fn push_command_with_handle<F>(&mut self, build: F) -> ScriptHandle
    where
        F: FnOnce(ScriptHandle) -> ScriptCommand,
    {
        let owner_label = self.owner_label();
        let mut state = self.state.borrow_mut();
        if let Some(quota) = state.command_quota {
            let count = state.commands_per_owner.entry(self.owner).or_insert(0);
            if *count >= quota {
                state
                    .logs
                    .push(format!("Command quota exceeded for {owner_label} (quota {quota})"));
                return -1;
            }
            *count += 1;
        }
        let raw = state.next_handle;
        state.next_handle = state.next_handle.saturating_add(1);
        let handle: ScriptHandle = (((state.handle_nonce as i64) << 32) | (raw as i64 & 0xFFFF_FFFF)) as ScriptHandle;
        state.pending_handles.insert(handle);
        state.commands.push(build(handle));
        handle
    }
}

pub struct ScriptHost {
    engine: Engine,
    ast: Option<AST>,
    scope: Scope<'static>,
    script_path: PathBuf,
    project_root: PathBuf,
    ast_cache_dir: Option<PathBuf>,
    import_resolver: CachedModuleResolver,
    last_modified: Option<SystemTime>,
    last_len: Option<u64>,
    last_digest: Option<u64>,
    last_import_digests: HashMap<PathBuf, u64>,
    last_digest_check: Option<Instant>,
    last_asset_revision: Option<u64>,
    callback_budget_ms: Option<f32>,
    error: Option<String>,
    enabled: bool,
    initialized: bool,
    shared: Rc<RefCell<SharedState>>,
    scripts: HashMap<String, CompiledScript>,
    instances: HashMap<u64, ScriptInstance>,
    next_instance_id: u64,
    handle_map: HashMap<ScriptHandle, Entity>,
    entity_errors: HashSet<Entity>,
}

impl ScriptHost {
    fn set_physics_context(&mut self, ctx: Option<PhysicsQueryContext>) {
        self.shared.borrow_mut().physics_ctx = ctx;
    }

    pub fn set_ast_cache_dir(&mut self, dir: Option<PathBuf>) {
        self.ast_cache_dir = dir.map(|d| d.canonicalize().unwrap_or(d));
    }

    fn drop_listeners_for_owner(&mut self, owner: ListenerOwner) {
        let mut state = self.shared.borrow_mut();
        state.event_listeners.retain(|listener| listener.owner != owner);
    }

    fn drop_listeners_by_id(&mut self, ids: &HashSet<u64>) {
        if ids.is_empty() {
            return;
        }
        let mut state = self.shared.borrow_mut();
        state.event_listeners.retain(|listener| !ids.contains(&listener.id));
    }

    fn prune_event_listeners(&mut self) {
        let mut state = self.shared.borrow_mut();
        state.event_listeners.retain(|listener| match listener.owner {
            ListenerOwner::Host => true,
            ListenerOwner::Instance(id) => self.instances.contains_key(&id),
        });
    }

    fn record_timing_elapsed(&self, name: &'static str, duration_ms: f32) {
        self.shared.borrow_mut().record_timing(name, duration_ms);
    }

    fn pop_next_event(&mut self) -> Option<ScriptEvent> {
        let mut state = self.shared.borrow_mut();
        if let Some(event) = state.event_queue.pop_front() {
            state.events_dispatched = state.events_dispatched.saturating_add(1);
            Some(event)
        } else {
            None
        }
    }

    fn dispatch_script_events(&mut self) {
        self.prune_event_listeners();
        while let Some(event) = self.pop_next_event() {
            self.dispatch_event(event);
        }
    }

    fn dispatch_event(&mut self, event: ScriptEvent) {
        let listeners = { self.shared.borrow().event_listeners.clone() };
        let mut stale_listeners = HashSet::new();
        for listener in listeners.iter() {
            if listener.name.as_ref() != event.name.as_ref() {
                continue;
            }
            if let Some(scope) = listener.scope_entity {
                if event.target != Some(scope) {
                    continue;
                }
            }
            match listener.owner {
                ListenerOwner::Host => {
                    let Some(ast) = &self.ast else { continue; };
                    let script_path = self.script_path.to_string_lossy().into_owned();
                    let world = ScriptWorld::new(self.shared.clone());
                    let map = ScriptWorld::event_to_map(&event, None);
                    let start = Instant::now();
                    if let Err(err) = self.engine.call_fn::<Dynamic>(
                        &mut self.scope,
                        ast,
                        listener.handler.as_ref(),
                        (world, map),
                    ) {
                        let message = Self::format_rhai_error(
                            err.as_ref(),
                            self.script_path.to_string_lossy().as_ref(),
                            listener.handler.as_ref(),
                        );
                        self.error = Some(message);
                        stale_listeners.insert(listener.id);
                    }
                    let elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
                    self.record_timing_elapsed("event", elapsed_ms);
                    self.record_offender_entry(
                        self.script_path.to_string_lossy().as_ref(),
                        listener.handler.as_ref(),
                        None,
                        elapsed_ms,
                    );
                    self.enforce_budget(elapsed_ms, script_path.as_ref(), listener.handler.as_ref(), None);
                }
                ListenerOwner::Instance(id) => {
                    let mut elapsed_ms = 0.0;
                    let mut mark_stale = false;
                    let mut error_message = None;
                    let mut script_path = String::new();
                    let mut listener_entity_opt = None;
                    {
                        let Some(instance) = self.instances.get_mut(&id) else {
                            stale_listeners.insert(listener.id);
                            continue;
                        };
                        if instance.errored {
                            mark_stale = true;
                        } else if let Some(compiled) = self.scripts.get(&instance.script_path) {
                            script_path = instance.script_path.clone();
                            let listener_entity = instance.state.borrow().entity;
                            listener_entity_opt = listener_entity;
                            let world = ScriptWorld::with_instance(self.shared.clone(), instance.state.clone(), id);
                            let map = ScriptWorld::event_to_map(&event, listener_entity);
                            let start = Instant::now();
                            let call_result = self.engine.call_fn::<Dynamic>(
                                &mut instance.scope,
                                &compiled.ast,
                                listener.handler.as_ref(),
                                (world, map),
                            );
                            elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
                            if let Err(err) = call_result {
                                instance.errored = true;
                                error_message = Some(Self::format_rhai_error(
                                    err.as_ref(),
                                    &script_path,
                                    listener.handler.as_ref(),
                                ));
                            }
                        } else {
                            mark_stale = true;
                        }
                    }
                    if mark_stale {
                        stale_listeners.insert(listener.id);
                        continue;
                    }
                    if let Some(message) = error_message {
                        self.set_instance_error_message(id, message);
                        stale_listeners.insert(listener.id);
                    }
                    self.record_timing_elapsed("event", elapsed_ms);
                    self.record_offender_entry(
                        &script_path,
                        listener.handler.as_ref(),
                        listener_entity_opt,
                        elapsed_ms,
                    );
                    self.enforce_budget(elapsed_ms, &script_path, listener.handler.as_ref(), Some(id));
                }
            }
        }
        self.drop_listeners_by_id(&stale_listeners);
    }

    fn reset_import_resolver(&mut self) {
        self.engine.set_module_resolver(self.import_resolver.clone());
    }

    fn format_rhai_error(err: &EvalAltResult, script_path: &str, fn_name: &str) -> String {
        let location = Self::format_location(script_path, err.position());
        let mut message = format!("{location} in {fn_name}: {err}");
        let mut frames = Vec::new();
        Self::collect_rhai_call_stack(err, &mut frames);
        if !frames.is_empty() {
            message.push_str("\nCall stack:");
            for frame in frames {
                message.push_str("\n - ");
                message.push_str(&frame);
            }
        }
        message
    }

    fn format_location(source: &str, pos: rhai::Position) -> String {
        let base = if source.is_empty() { "<unknown>".to_string() } else { source.to_string() };
        match (pos.line(), pos.position()) {
            (Some(line), Some(col)) => format!("{base}:{line}:{col}"),
            (Some(line), None) => format!("{base}:{line}"),
            _ => base,
        }
    }

    fn collect_rhai_call_stack(err: &EvalAltResult, frames: &mut Vec<String>) {
        match err {
            EvalAltResult::ErrorInFunctionCall(fn_name, src, inner, pos) => {
                let label = if src.is_empty() {
                    fn_name.clone()
                } else {
                    format!("{fn_name} @ {}", Self::format_location(src, *pos))
                };
                frames.push(label);
                Self::collect_rhai_call_stack(inner.as_ref(), frames);
            }
            EvalAltResult::ErrorInModule(module, inner, pos) => {
                let label = if module.is_empty() {
                    "module".to_string()
                } else {
                    format!("module {module}")
                };
                let loc = Self::format_location(module, *pos);
                frames.push(format!("{label} @ {loc}"));
                Self::collect_rhai_call_stack(inner.as_ref(), frames);
            }
            _ => {}
        }
    }

    fn dynamic_to_json(value: &Dynamic) -> Option<JsonValue> {
        if value.is_unit() {
            return Some(JsonValue::Null);
        }
        if let Some(b) = value.clone().try_cast::<bool>() {
            return Some(JsonValue::Bool(b));
        }
        if let Some(int_val) = value.clone().try_cast::<rhai::INT>() {
            return Some(JsonValue::from(int_val));
        }
        if let Some(float_val) = value.clone().try_cast::<FLOAT>() {
            return serde_json::Number::from_f64(float_val as f64).map(JsonValue::Number);
        }
        if let Some(text) = value.clone().try_cast::<rhai::ImmutableString>() {
            return Some(JsonValue::String(text.into_owned()));
        }
        if let Some(arr) = value.clone().try_cast::<Array>() {
            let mut out = Vec::with_capacity(arr.len());
            for v in arr {
                if let Some(json) = Self::dynamic_to_json(&v) {
                    out.push(json);
                }
            }
            return Some(JsonValue::Array(out));
        }
        if let Some(map) = value.clone().try_cast::<Map>() {
            return Some(Self::map_to_json(&map));
        }
        None
    }

    fn map_to_json(map: &Map) -> JsonValue {
        let mut obj = JsonMap::new();
        for (k, v) in map.iter() {
            if let Some(json) = Self::dynamic_to_json(v) {
                obj.insert(k.to_string(), json);
            }
        }
        JsonValue::Object(obj)
    }

    fn json_to_dynamic(val: &JsonValue) -> Option<Dynamic> {
        match val {
            JsonValue::Null => Some(Dynamic::UNIT),
            JsonValue::Bool(b) => Some(Dynamic::from_bool(*b)),
            JsonValue::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Some(Dynamic::from_int(i as rhai::INT))
                } else if let Some(f) = n.as_f64() {
                    Some(Dynamic::from_float(f as FLOAT))
                } else {
                    None
                }
            }
            JsonValue::String(s) => Some(Dynamic::from(s.clone())),
            JsonValue::Array(arr) => {
                let mut out = Array::with_capacity(arr.len());
                for v in arr {
                    if let Some(d) = Self::json_to_dynamic(v) {
                        out.push(d);
                    }
                }
                Some(Dynamic::from_array(out))
            }
            JsonValue::Object(obj) => {
                let mut map = Map::new();
                for (k, v) in obj {
                    if let Some(d) = Self::json_to_dynamic(v) {
                        map.insert(k.into(), d);
                    }
                }
                Some(Dynamic::from_map(map))
            }
        }
    }

    fn json_to_map(val: &JsonValue) -> Option<Map> {
        let mut out = Map::new();
        if let JsonValue::Object(obj) = val {
            for (k, v) in obj {
                if let Some(d) = Self::json_to_dynamic(v) {
                    out.insert(k.into(), d);
                }
            }
            Some(out)
        } else {
            None
        }
    }

    fn instance_muted(&self, instance_id: u64) -> bool {
        self.instances.get(&instance_id).map_or(false, |instance| instance.mute_errors)
    }

    fn set_instance_error_message(&mut self, instance_id: u64, message: String) {
        if !self.instance_muted(instance_id) {
            self.error = Some(message);
        }
    }

    fn enforce_budget(&mut self, duration_ms: f32, script_path: &str, fn_name: &str, instance_id: Option<u64>) {
        let Some(budget_ms) = self.callback_budget_ms else { return };
        if duration_ms <= budget_ms {
            return;
        }
        let mut message =
            format!("{script_path}:{fn_name} exceeded callback budget ({duration_ms:.3} ms > {budget_ms:.3} ms)");
        if let Some(id) = instance_id {
            if let Some(instance) = self.instances.get_mut(&id) {
                message.push_str(&format!(" [instance {id}]"));
                instance.errored = true;
            }
            self.set_instance_error_message(id, message);
        } else {
            self.set_error_message(message);
        }
    }

    pub fn new(path: impl AsRef<Path>) -> Self {
        let mut engine = Engine::new();
        engine.set_fast_operators(true);
        // Lift Rhai safety ceilings (0 = unlimited).
        engine.set_max_expr_depths(0, 0); // expr depth + function expr depth
        engine.set_max_operations(0);     // op budget
        let canonical_script_path = path.as_ref().canonicalize().unwrap_or_else(|_| path.as_ref().to_path_buf());
        let scripts_root_candidate = canonical_script_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from(SCRIPT_IMPORT_ROOT));
        let derived_project_root = derive_project_root_from_scripts_root(&scripts_root_candidate);
        let is_project_layout = derived_project_root.is_some();
        let project_root = derived_project_root
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let scripts_root = if is_project_layout {
            scripts_root_candidate.clone()
        } else {
            project_root.join(SCRIPT_IMPORT_ROOT)
        };
        let import_resolver = CachedModuleResolver::new(scripts_root);
        engine.set_module_resolver(import_resolver.clone());
        register_api(&mut engine);
        let shared = SharedState { next_handle: 1, ..Default::default() };
        let ast_cache_dir = env::var("KESTREL_SCRIPT_AST_CACHE").ok().map(PathBuf::from);
        Self {
            engine,
            ast: None,
            scope: Scope::new(),
            script_path: canonical_script_path,
            project_root,
            ast_cache_dir: ast_cache_dir.and_then(|p| p.canonicalize().ok()),
            import_resolver,
            last_modified: None,
            last_len: None,
            last_digest: None,
            last_import_digests: HashMap::new(),
            last_digest_check: None,
            last_asset_revision: None,
            callback_budget_ms: None,
            error: None,
            enabled: true,
            initialized: false,
            shared: Rc::new(RefCell::new(shared)),
            scripts: HashMap::new(),
            instances: HashMap::new(),
            next_instance_id: 1,
            handle_map: HashMap::new(),
            entity_errors: HashSet::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enable: bool) {
        self.enabled = enable;
    }

    pub fn set_callback_budget_ms(&mut self, budget_ms: Option<f32>) {
        self.callback_budget_ms = budget_ms.filter(|v| *v > 0.0);
    }

    pub fn set_command_quota(&mut self, quota: Option<usize>) {
        let quota = quota.filter(|q| *q > 0);
        let mut shared = self.shared.borrow_mut();
        shared.command_quota = quota;
        shared.commands_per_owner.clear();
    }

    pub fn last_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn set_entity_snapshots(
        &mut self,
        snapshots: HashMap<Entity, EntitySnapshot>,
        cell_size: f32,
        spatial_cells: Option<HashMap<(i32, i32), Vec<Entity>>>,
        scene_ids: HashMap<Entity, Arc<str>>,
    ) {
        let mut index = ScriptSpatialIndex::default();
        index.rebuild_with_spatial_hash(&snapshots, spatial_cells, cell_size);
        let mut scene_id_entities = HashMap::new();
        let mut scene_pairs: Vec<_> = scene_ids.iter().collect();
        scene_pairs.sort_by_key(|(entity, _)| entity.to_bits());
        for (entity, scene_id) in scene_pairs {
            scene_id_entities.entry(Arc::clone(scene_id)).or_insert(*entity);
        }
        let mut shared = self.shared.borrow_mut();
        shared.entity_snapshots = snapshots;
        shared.spatial_index = index;
        shared.entity_scene_ids = scene_ids;
        shared.scene_id_entities = scene_id_entities;
    }

    pub fn set_input_snapshot(&mut self, snapshot: InputSnapshot) {
        let mut shared = self.shared.borrow_mut();
        shared.input_snapshot = Some(snapshot);
    }

    pub fn entity_has_errored_instance(&self, entity: Entity) -> bool {
        self.instances.values().any(|instance| instance.entity == entity && instance.errored)
            || self.entity_errors.contains(&entity)
    }

    fn mark_entity_error(&mut self, entity: Entity) {
        self.entity_errors.insert(entity);
    }

    fn clear_entity_error(&mut self, entity: Entity) {
        self.entity_errors.remove(&entity);
    }

    fn call_exit_for_script_instances(&mut self, script_path: &str) {
        let ids: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, inst)| inst.script_path == script_path)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let _ = self.call_instance_exit(id);
        }
    }

    fn prune_entity_errors<F>(&mut self, mut keep: F)
    where
        F: FnMut(Entity) -> bool,
    {
        self.entity_errors.retain(|entity| keep(*entity));
    }

    pub fn script_path(&self) -> &Path {
        &self.script_path
    }

    pub fn ensure_script_loaded(&mut self, path: &str, assets: Option<&AssetManager>) -> Result<()> {
        let now = Instant::now();
        let was_loaded = self.scripts.contains_key(path);
        let (source, asset_rev) = self.load_script_source_with_revision(path, assets)?;
        let len = source.len() as u64;
        let digest = hash_source(&source);
        if let Some(compiled) = self.scripts.get_mut(path) {
            let imports_clean = imports_unchanged(&compiled.import_digests);
            let same_source = compiled.len == len && compiled.digest == digest;
            let same_asset_rev = asset_rev.map(|rev| compiled.asset_revision == Some(rev)).unwrap_or(true);
            if same_source && imports_clean && same_asset_rev {
                compiled.last_checked = Some(now);
                compiled.asset_revision = asset_rev.or(compiled.asset_revision);
                self.error = None;
                return Ok(());
            }
            if !same_source || !imports_clean {
                self.call_exit_for_script_instances(path);
            }
        }
        let mut compiled = self.compile_script_from_source(path, &source, len, digest)?;
        compiled.asset_revision = asset_rev;
        compiled.last_checked = Some(now);
        self.error = None;
        self.scripts.insert(path.to_string(), compiled);
        if was_loaded {
            self.reinitialize_instances_for_script(path)?;
        }
        Ok(())
    }

    pub fn compiled_script(&self, path: &str) -> Option<&CompiledScript> {
        self.scripts.get(path)
    }

    pub fn create_instance(
        &mut self,
        script_path: &str,
        entity: Entity,
        assets: Option<&AssetManager>,
    ) -> Result<u64> {
        self.ensure_script_loaded(script_path, assets)?;
        self.create_instance_preloaded(script_path, entity, false)
    }

    fn create_instance_preloaded(&mut self, script_path: &str, entity: Entity, persist_state: bool) -> Result<u64> {
        let compiled =
            self.scripts.get(script_path).ok_or_else(|| anyhow!("Script '{script_path}' not cached after load"))?;
        let id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.saturating_add(1);
        let mut scope = Scope::new();
        // Run global statements to initialize script-scoped state for this instance.
        if let Err(err) = self.engine.run_ast_with_scope(&mut scope, &compiled.ast) {
            return Err(anyhow!("Evaluating globals for '{script_path}': {err}"));
        }
        let state = Rc::new(RefCell::new(InstanceRuntimeState::default()));
        {
            let mut runtime = state.borrow_mut();
            runtime.instance_id = Some(id);
            runtime.entity = Some(entity);
        }
        let instance = ScriptInstance {
            script_path: script_path.to_string(),
            entity,
            scope,
            has_ready_run: false,
            errored: false,
            persist_state,
            mute_errors: false,
            state,
        };
        self.instances.insert(id, instance);
        Ok(id)
    }

    pub fn remove_instance(&mut self, id: u64) {
        self.drop_listeners_for_owner(ListenerOwner::Instance(id));
        self.instances.remove(&id);
    }

    pub fn set_error_message(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        eprintln!("[script] {msg}");
        self.error = Some(msg);
    }

    pub fn set_error_with_details(&mut self, err: &Error) {
        let mut buf = String::new();
        let _ = write!(&mut buf, "{err}");
        for cause in err.chain().skip(1) {
            let _ = write!(&mut buf, "\ncaused by: {cause}");
        }
        self.set_error_message(buf);
    }

    pub fn force_reload(&mut self, assets: Option<&AssetManager>) -> Result<()> {
        self.load_script(assets).map(|_| ())
    }

    fn resolve_script_path(&self, path: &str) -> PathBuf {
        let raw = PathBuf::from(path);
        if raw.is_absolute() {
            return raw;
        }
        self.project_root.join(raw)
    }

    fn load_script_source(&self, path: &str, assets: Option<&AssetManager>) -> Result<String> {
        let resolved = self.resolve_script_path(path);
        if let Some(assets) = assets {
            return assets.read_text(&resolved).with_context(|| format!("Reading script asset '{}'", resolved.display()));
        }
        std::fs::read_to_string(&resolved).with_context(|| format!("Reading script file '{}'", resolved.display()))
    }

    fn load_script_source_with_revision(
        &self,
        path: &str,
        assets: Option<&AssetManager>,
    ) -> Result<(String, Option<u64>)> {
        let resolved = self.resolve_script_path(path);
        if let Some(assets) = assets {
            let revision = Some(assets.revision());
            let source =
                assets.read_text(&resolved).with_context(|| format!("Reading script asset '{}'", resolved.display()))?;
            return Ok((source, revision));
        }
        let source =
            std::fs::read_to_string(&resolved).with_context(|| format!("Reading script file '{}'", resolved.display()))?;
        Ok((source, None))
    }

    fn compile_script_from_source(
        &mut self,
        path: &str,
        source: &str,
        len: u64,
        digest: u64,
    ) -> Result<CompiledScript> {
        self.reset_import_resolver();
        let ast = self
            .engine
            .compile(source)
            .with_context(|| format!("Compiling Rhai script '{}'", path))?;
        let import_digests = self.import_resolver.compute_import_digests(source)?;
        let (has_ready, has_process, has_physics_process, has_exit) = detect_callbacks(&ast);
        Ok(CompiledScript {
            ast,
            has_ready,
            has_process,
            has_physics_process,
            has_exit,
            len,
            digest,
            import_digests,
            last_checked: Some(Instant::now()),
            asset_revision: None,
        })
    }

    fn reinitialize_instances_for_script(&mut self, script_path: &str) -> Result<()> {
        let Some(compiled) = self.scripts.get(script_path).cloned() else {
            return Ok(());
        };
        let targets: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, instance)| instance.script_path == script_path)
            .map(|(id, _)| *id)
            .collect();
        for id in targets {
            self.drop_listeners_for_owner(ListenerOwner::Instance(id));
            let Some(instance) = self.instances.get_mut(&id) else { continue };
            if instance.persist_state {
                let current = instance.state.borrow().persistent.clone();
                let sanitized = {
                    let shared = self.shared.borrow();
                    sanitize_persisted_map(&current, PersistedHandlePolicy::DropStaleHandles, &shared)
                };
                instance.state.borrow_mut().persistent = sanitized;
            } else {
                instance.state.borrow_mut().persistent.clear();
            }
            instance.state.borrow_mut().is_hot_reload = true;
            instance.scope = Scope::new();
            instance.has_ready_run = false;
            instance.errored = false;
            if let Err(err) = self.engine.run_ast_with_scope(&mut instance.scope, &compiled.ast) {
                instance.errored = true;
                let message = Self::format_rhai_error(err.as_ref(), script_path, "globals");
                self.error = Some(message.clone());
                return Err(anyhow!(message));
            }
        }
        Ok(())
    }

    fn call_instance_ready(&mut self, instance_id: u64) -> Result<()> {
        let (script_path, elapsed_ms, error_message, entity) = {
            let Some(instance) = self.instances.get_mut(&instance_id) else {
                return Ok(());
            };
            let Some(compiled) = self.scripts.get(&instance.script_path) else {
                return Ok(());
            };
            if !compiled.has_ready || instance.has_ready_run || instance.errored {
                return Ok(());
            }
            let elapsed_ms;
            let mut error_message = None;
            let script_path = instance.script_path.clone();
            let entity = instance.entity;
            {
                let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
                let world = ScriptWorld::with_instance(self.shared.clone(), instance.state.clone(), instance_id);
                let start = Instant::now();
                let result =
                    self.engine.call_fn::<Dynamic>(&mut instance.scope, &compiled.ast, "ready", (world, entity_int));
                elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
                match result {
                    Ok(_) => {
                        instance.has_ready_run = true;
                        instance.state.borrow_mut().is_hot_reload = false;
                    }
                    Err(err) => {
                        instance.errored = true;
                        error_message = Some(Self::format_rhai_error(err.as_ref(), &script_path, "ready"));
                    }
                }
            }
            (script_path, elapsed_ms, error_message, entity)
        };
        self.record_timing_elapsed("ready", elapsed_ms);
        self.record_offender_entry(&script_path, "ready", Some(entity), elapsed_ms);
        self.enforce_budget(elapsed_ms, &script_path, "ready", Some(instance_id));
        if let Some(message) = error_message {
            self.set_instance_error_message(instance_id, message.clone());
            Err(anyhow!(message))
        } else {
            Ok(())
        }
    }

    fn call_instance_process(&mut self, instance_id: u64, dt: f32) -> Result<()> {
        let (script_path, elapsed_ms, error_message, entity) = {
            let Some(instance) = self.instances.get_mut(&instance_id) else {
                return Ok(());
            };
            let Some(compiled) = self.scripts.get(&instance.script_path) else {
                return Ok(());
            };
            if !compiled.has_process || instance.errored {
                return Ok(());
            }
            let elapsed_ms;
            let mut error_message = None;
            let script_path = instance.script_path.clone();
            let entity = instance.entity;
            {
                let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
                let world = ScriptWorld::with_instance(self.shared.clone(), instance.state.clone(), instance_id);
                instance.state.borrow_mut().tick_timers(dt);
                let dt_rhai: FLOAT = dt as FLOAT;
                let start = Instant::now();
                let result = self.engine.call_fn::<Dynamic>(
                    &mut instance.scope,
                    &compiled.ast,
                    "process",
                    (world, entity_int, dt_rhai),
                );
                elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
                if let Err(err) = result {
                    instance.errored = true;
                    error_message = Some(Self::format_rhai_error(err.as_ref(), &script_path, "process"));
                }
            }
            (script_path, elapsed_ms, error_message, entity)
        };
        self.record_timing_elapsed("process", elapsed_ms);
        self.record_offender_entry(&script_path, "process", Some(entity), elapsed_ms);
        self.enforce_budget(elapsed_ms, &script_path, "process", Some(instance_id));
        if let Some(message) = error_message {
            self.set_instance_error_message(instance_id, message.clone());
            Err(anyhow!(message))
        } else {
            Ok(())
        }
    }

    fn call_instance_physics_process(&mut self, instance_id: u64, dt: f32) -> Result<()> {
        let (script_path, elapsed_ms, error_message, entity) = {
            let Some(instance) = self.instances.get_mut(&instance_id) else {
                return Ok(());
            };
            let Some(compiled) = self.scripts.get(&instance.script_path) else {
                return Ok(());
            };
            if !compiled.has_physics_process || instance.errored {
                return Ok(());
            }
            let elapsed_ms;
            let mut error_message = None;
            let script_path = instance.script_path.clone();
            let entity = instance.entity;
            {
                let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
                let world = ScriptWorld::with_instance(self.shared.clone(), instance.state.clone(), instance_id);
                instance.state.borrow_mut().tick_timers(dt);
                let dt_rhai: FLOAT = dt as FLOAT;
                let start = Instant::now();
                let result = self.engine.call_fn::<Dynamic>(
                    &mut instance.scope,
                    &compiled.ast,
                    "physics_process",
                    (world, entity_int, dt_rhai),
                );
                elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
                if let Err(err) = result {
                    instance.errored = true;
                    error_message =
                        Some(Self::format_rhai_error(err.as_ref(), &script_path, "physics_process"));
                }
            }
            (script_path, elapsed_ms, error_message, entity)
        };
        self.record_timing_elapsed("physics_process", elapsed_ms);
        self.record_offender_entry(&script_path, "physics_process", Some(entity), elapsed_ms);
        self.enforce_budget(elapsed_ms, &script_path, "physics_process", Some(instance_id));
        if let Some(message) = error_message {
            self.set_instance_error_message(instance_id, message.clone());
            Err(anyhow!(message))
        } else {
            Ok(())
        }
    }

    fn call_instance_exit(&mut self, instance_id: u64) -> Result<()> {
        let (script_path, elapsed_ms, error_message, entity) = {
            let Some(instance) = self.instances.get_mut(&instance_id) else {
                return Ok(());
            };
            let Some(compiled) = self.scripts.get(&instance.script_path) else {
                return Ok(());
            };
            if !compiled.has_exit || instance.errored {
                return Ok(());
            }
            let elapsed_ms;
            let mut error_message = None;
            let script_path = instance.script_path.clone();
            let entity = instance.entity;
            {
                let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
                let world = ScriptWorld::with_instance(self.shared.clone(), instance.state.clone(), instance_id);
                let start = Instant::now();
                let result =
                    self.engine.call_fn::<Dynamic>(&mut instance.scope, &compiled.ast, "exit", (world, entity_int));
                elapsed_ms = start.elapsed().as_secs_f32() * 1000.0;
                if let Err(err) = result {
                    instance.errored = true;
                    error_message = Some(Self::format_rhai_error(err.as_ref(), &script_path, "exit"));
                }
            }
            (script_path, elapsed_ms, error_message, entity)
        };
        self.record_timing_elapsed("exit", elapsed_ms);
        self.record_offender_entry(&script_path, "exit", Some(entity), elapsed_ms);
        self.enforce_budget(elapsed_ms, &script_path, "exit", Some(instance_id));
        if let Some(message) = error_message {
            self.set_instance_error_message(instance_id, message.clone());
            Err(anyhow!(message))
        } else {
            Ok(())
        }
    }

    fn begin_frame(&mut self, dt: f32) -> f32 {
        let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };
        let mut shared = self.shared.borrow_mut();
        shared.events_dispatched = 0;
        shared.event_overflowed = false;
        shared.commands_per_owner.clear();
        shared.offenders.clear();
        let mut scale = shared.time_scale;
        if !scale.is_finite() {
            scale = 1.0;
            shared.time_scale = 1.0;
        }
        let scaled = dt * scale;
        let scaled_dt = if scaled.is_finite() { scaled } else { 0.0 };
        shared.last_unscaled_dt = dt;
        shared.last_scaled_dt = scaled_dt;
        shared.unscaled_time += dt;
        shared.scaled_time += scaled_dt;
        for timer in shared.timers.values_mut() {
            timer.tick(scaled_dt);
        }
        scaled_dt
    }

    pub fn update(&mut self, dt: f32, run_scripts: bool, assets: Option<&AssetManager>) -> f32 {
        if let Err(err) = self.reload_if_needed(assets) {
            self.error = Some(err.to_string());
            return 0.0;
        }

        if !self.enabled {
            return 0.0;
        }
        if !run_scripts {
            return 0.0;
        }
        let dt_scaled = self.begin_frame(dt);
        let dt_rhai: FLOAT = dt_scaled as FLOAT;

        {
            let mut shared = self.shared.borrow_mut();
            shared.commands.clear();
        }

        let script_path = self.script_path.to_string_lossy().into_owned();
        let world = ScriptWorld::new(self.shared.clone());
        if !self.initialized {
            let (init_result, init_elapsed, init_fn_exists) = {
                let ast = match &self.ast {
                    Some(ast) => ast,
                    None => return dt_scaled,
                };
                let init_fn_exists = self.function_exists_with_any_arity(ast, "init");
                let start = Instant::now();
                let result = self.engine.call_fn::<()>(&mut self.scope, ast, "init", (world.clone(),));
                (result, start.elapsed().as_secs_f32() * 1000.0, init_fn_exists)
            };
            self.record_timing_elapsed("init", init_elapsed);
            self.record_offender_entry(script_path.as_ref(), "init", None, init_elapsed);
            self.enforce_budget(init_elapsed, script_path.as_ref(), "init", None);
            match init_result {
                Ok(_) => {
                    self.initialized = true;
                    self.error = None;
                }
                Err(err) => {
                    if let EvalAltResult::ErrorFunctionNotFound(fn_sig, _) = err.as_ref() {
                        if fn_sig.starts_with("init") {
                            if init_fn_exists {
                                let msg = format!(
                                    "{}: Script function 'init' has wrong signature; expected init(world).",
                                    self.script_path.display()
                                );
                                self.error = Some(msg);
                                self.dispatch_script_events();
                                return dt_scaled;
                            }
                            self.initialized = true;
                            self.dispatch_script_events();
                            return dt_scaled;
                        }
                    }
                    let msg = Self::format_rhai_error(err.as_ref(), script_path.as_ref(), "init");
                    self.error = Some(msg);
                    return dt_scaled;
                }
            }
        }

        let (result, elapsed_ms, update_fn_exists) = {
            let ast = match &self.ast {
                Some(ast) => ast,
                None => return dt_scaled,
            };
            let update_fn_exists = self.function_exists_with_any_arity(ast, "update");
            let start = Instant::now();
            let result = self.engine.call_fn::<()>(&mut self.scope, ast, "update", (world, dt_rhai));
            (result, start.elapsed().as_secs_f32() * 1000.0, update_fn_exists)
        };
        self.record_timing_elapsed("update", elapsed_ms);
        self.record_offender_entry(script_path.as_ref(), "update", None, elapsed_ms);
        self.enforce_budget(elapsed_ms, script_path.as_ref(), "update", None);
        match result {
            Ok(_) => {
                self.error = None;
            }
            Err(err) => {
                if let EvalAltResult::ErrorFunctionNotFound(fn_sig, _) = err.as_ref() {
                    if fn_sig.starts_with("update") {
                        if update_fn_exists {
                            let msg = format!(
                                "{}: Script function 'update' has wrong signature; expected update(world, dt: number).",
                                self.script_path.display()
                            );
                            self.error = Some(msg);
                        } else {
                            self.error = None;
                        }
                        self.dispatch_script_events();
                        return dt_scaled;
                    }
                }
                let msg =
                    Self::format_rhai_error(err.as_ref(), self.script_path.to_string_lossy().as_ref(), "update");
                self.error = Some(msg);
            }
        }
        self.dispatch_script_events();
        dt_scaled
    }

    pub fn drain_commands(&mut self) -> Vec<ScriptCommand> {
        self.shared.borrow_mut().commands.drain(..).collect()
    }

    pub fn drain_logs(&mut self) -> Vec<String> {
        self.shared.borrow_mut().logs.drain(..).collect()
    }

    pub fn timing_summaries(&self) -> Vec<ScriptTimingSummary> {
        self.shared.borrow().timing_summaries()
    }

    pub fn timing_offenders(&self) -> Vec<ScriptTimingOffender> {
        self.shared.borrow().offenders.clone()
    }

    fn record_offender_entry(
        &self,
        script_path: &str,
        function: &str,
        entity: Option<Entity>,
        last_ms: f32,
    ) {
        let mut shared = self.shared.borrow_mut();
        shared.record_offender(ScriptTimingOffender {
            script_path: script_path.to_string(),
            function: function.to_string(),
            entity,
            last_ms,
        });
    }

    pub fn eval_repl(&mut self, source: &str) -> Result<Option<String>> {
        let trimmed = source.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        self.reload_if_needed(None)?;
        let marker = self.scope.len();
        self.scope.push_constant("world", ScriptWorld::new(self.shared.clone()));
        let eval = self.engine.eval_with_scope::<Dynamic>(&mut self.scope, trimmed);
        while self.scope.len() > marker {
            self.scope.pop();
        }
        let result = match eval {
            Ok(value) => {
                if value.is_unit() {
                    Ok(None)
                } else {
                    Ok(Some(value.to_string()))
                }
            }
            Err(err) => Err(anyhow!(err.to_string())),
        };
        self.dispatch_script_events();
        result
    }

    fn sync_handle_snapshot(&mut self) {
        let mut shared = self.shared.borrow_mut();
        shared.handle_lookup.clear();
        shared.entity_handles.clear();
        shared.entity_tags.clear();
        for (handle, entity) in &self.handle_map {
            let tag = shared.handle_tags.get(handle).cloned();
            shared.handle_lookup.insert(*handle, *entity);
            shared.entity_handles.insert(*entity, *handle);
            if let Some(tag) = tag {
                shared.entity_tags.insert(*entity, tag);
            }
        }
    }

    pub fn register_spawn_result(&mut self, handle: ScriptHandle, entity: Entity, tag: Option<String>) {
        self.handle_map.insert(handle, entity);
        {
            let mut shared = self.shared.borrow_mut();
            shared.pending_handles.remove(&handle);
            if let Some(tag) = tag.filter(|t| !t.is_empty()) {
                shared.handle_tags.insert(handle, tag);
            }
        }
        self.sync_handle_snapshot();
    }

    pub fn record_spawn_failure(&mut self, reason: &str) {
        self.shared.borrow_mut().record_spawn_failure(reason);
    }

    pub fn safety_metrics(&self) -> ScriptSafetyMetrics {
        self.shared.borrow().safety_metrics()
    }

    pub fn resolve_handle(&self, handle: ScriptHandle) -> Option<Entity> {
        self.handle_map.get(&handle).copied()
    }

    pub fn forget_handle(&mut self, handle: ScriptHandle) {
        let entity = self.handle_map.remove(&handle);
        {
            let mut shared = self.shared.borrow_mut();
            shared.pending_handles.remove(&handle);
            shared.handle_tags.remove(&handle);
            if let Some(entity) = entity {
                shared.entity_tags.remove(&entity);
            }
        }
        self.sync_handle_snapshot();
    }

    pub fn forget_entity(&mut self, entity: Entity) {
        {
            let mut shared = self.shared.borrow_mut();
            shared.entity_tags.remove(&entity);
            self.handle_map.retain(|handle, value| {
                if *value == entity {
                    shared.pending_handles.remove(handle);
                    shared.handle_tags.remove(handle);
                    false
                } else {
                    true
                }
            });
        }
        self.sync_handle_snapshot();
    }

    pub fn clear_handles(&mut self) {
        self.handle_map.clear();
        {
            let mut shared = self.shared.borrow_mut();
            shared.pending_handles.clear();
            shared.handle_tags.clear();
            shared.entity_tags.clear();
        }
        self.sync_handle_snapshot();
    }

    pub fn handles_snapshot(&self) -> Vec<(ScriptHandle, Entity)> {
        self.handle_map.iter().map(|(handle, entity)| (*handle, *entity)).collect()
    }

    pub fn clear_instances(&mut self) {
        let ids: Vec<u64> = self.instances.keys().copied().collect();
        for id in ids {
            let _ = self.call_instance_exit(id);
        }
        {
            let mut state = self.shared.borrow_mut();
            state.event_listeners.retain(|listener| matches!(listener.owner, ListenerOwner::Host));
        }
        self.instances.clear();
        self.entity_errors.clear();
    }

    fn reload_if_needed(&mut self, assets: Option<&AssetManager>) -> Result<()> {
        let now = Instant::now();
        if let Some(assets) = assets {
            let revision = assets.revision();
            let imports_clean = imports_unchanged(&self.last_import_digests);
            let (source, _) = self
                .load_script_source_with_revision(self.script_path.to_string_lossy().as_ref(), Some(assets))
                .with_context(|| format!("Reading script asset '{}'", self.script_path.display()))?;
            let len = source.len() as u64;
            let digest = hash_source(&source);
            let should_reload = self.ast.is_none()
                || self.last_digest.is_none_or(|prev| prev != digest)
                || self.last_len.is_none_or(|prev| prev != len)
                || self.last_asset_revision.is_none_or(|prev| prev != revision)
                || !imports_clean;
            if should_reload {
                // Without filesystem metadata, fall back to epoch/length for change tracking.
                self.load_script_from_source(source, SystemTime::UNIX_EPOCH, len, Some(revision))?;
                self.last_digest = Some(digest);
            } else {
                self.last_asset_revision = Some(revision);
            }
            self.last_digest_check = Some(now);
            return Ok(());
        }

        let source = self
            .load_script_source(self.script_path.to_string_lossy().as_ref(), None)
            .with_context(|| format!("Reading script asset '{}'", self.script_path.display()))?;
        let len = source.len() as u64;
        let digest = hash_source(&source);
        let imports_clean = imports_unchanged(&self.last_import_digests);
        let should_reload = self.ast.is_none()
            || self.last_digest.is_none_or(|prev| prev != digest)
            || self.last_len.is_none_or(|prev| prev != len)
            || !imports_clean;
        if should_reload {
            self.load_script_from_source(source, SystemTime::UNIX_EPOCH, len, None)?;
            self.last_digest = Some(digest);
            self.last_digest_check = Some(now);
        } else {
            let should_update_timestamp = self
                .last_digest_check
                .map(|last| now.duration_since(last) >= SCRIPT_DIGEST_CHECK_INTERVAL)
                .unwrap_or(true);
            if should_update_timestamp {
                self.last_digest_check = Some(now);
            }
        }
        Ok(())
    }

    fn load_script(&mut self, assets: Option<&AssetManager>) -> Result<&AST> {
        let (source, revision) = self
            .load_script_source_with_revision(self.script_path.to_string_lossy().as_ref(), assets)
            .with_context(|| format!("Reading script asset '{}'", self.script_path.display()))?;
        let len = source.len() as u64;
        self.load_script_from_source(source, SystemTime::UNIX_EPOCH, len, revision)
    }

    fn load_script_from_source(
        &mut self,
        source: String,
        modified: SystemTime,
        len: u64,
        revision: Option<u64>,
    ) -> Result<&AST> {
        self.reset_import_resolver();
        self.drop_listeners_for_owner(ListenerOwner::Host);
        let script_digest = hash_source(&source);
        let mut import_digests = None;
        if let Some(root) = &self.ast_cache_dir {
            let cache = ScriptAstCache::new(root.clone());
            if let Some(imports) = cache.load(&self.script_path, script_digest) {
                import_digests = Some(imports);
            }
        }
        let import_digests = match import_digests {
            Some(imports) => imports,
            None => self.import_resolver.compute_import_digests(&source)?,
        };
        if let Some(root) = &self.ast_cache_dir {
            let cache = ScriptAstCache::new(root.clone());
            cache.store(&self.script_path, script_digest, &import_digests);
        }
        let ast = self.engine.compile(&source).with_context(|| "Compiling Rhai script")?;
        self.scope = Scope::new();
        self.engine
            .run_ast_with_scope(&mut self.scope, &ast)
            .map_err(|err| anyhow!("Evaluating script global statements: {err}"))?;
        self.last_import_digests = import_digests.clone();
        self.last_modified = Some(modified);
        self.last_len = Some(len);
        self.last_digest = Some(script_digest);
        self.last_digest_check = Some(Instant::now());
        self.last_asset_revision = revision;
        self.initialized = false;
        self.error = None;
        self.ast = Some(ast);
        Ok(self.ast.as_ref().expect("script AST set during load"))
    }

    fn function_exists_with_any_arity(&self, ast: &AST, name: &str) -> bool {
        const MAX_ARITY: usize = 4;
        (0..=MAX_ARITY).any(|arity| self.function_exists_with_arity(ast, name, arity))
    }

    fn function_exists_with_arity(&self, ast: &AST, name: &str, arity: usize) -> bool {
        ast.iter_functions().any(|f| f.name == name && f.params.len() == arity)
    }
}

fn hash_source(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

fn parse_literal_imports(source: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("import ") {
            continue;
        }
        if let Some(start) = trimmed.find('"') {
            if let Some(end_rel) = trimmed[start + 1..].find('"') {
                let end = start + 1 + end_rel;
                imports.push(trimmed[start + 1..end].to_string());
            }
        }
    }
    imports
}

fn imports_unchanged(imports: &HashMap<PathBuf, u64>) -> bool {
    imports.iter().all(|(path, digest)| {
        std::fs::read_to_string(path)
            .map(|src| hash_source(&src) == *digest)
            .unwrap_or(false)
    })
}

#[derive(Serialize, Deserialize)]
struct AstCacheFile {
    script_digest: u64,
    import_digests: HashMap<PathBuf, u64>,
}

struct ScriptAstCache {
    root: PathBuf,
}

impl ScriptAstCache {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn file_path(&self, script_path: &Path) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        script_path.to_string_lossy().hash(&mut hasher);
        let name = format!("{:016x}.rhaiast", hasher.finish());
        self.root.join(name)
    }

    fn load(&self, script_path: &Path, script_digest: u64) -> Option<HashMap<PathBuf, u64>> {
        let path = self.file_path(script_path);
        let data = std::fs::read(path).ok()?;
        let cached: AstCacheFile = bincode::deserialize(&data).ok()?;
        if cached.script_digest != script_digest {
            return None;
        }
        if !imports_unchanged(&cached.import_digests) {
            return None;
        }
        Some(cached.import_digests)
    }

    fn store(&self, script_path: &Path, script_digest: u64, import_digests: &HashMap<PathBuf, u64>) {
        if std::fs::create_dir_all(&self.root).is_err() {
            return;
        }
        let cache = AstCacheFile { script_digest, import_digests: import_digests.clone() };
        if let Ok(bytes) = bincode::serialize(&cache) {
            let _ = std::fs::write(self.file_path(script_path), bytes);
        }
    }
}

fn detect_callbacks(ast: &AST) -> (bool, bool, bool, bool) {
    let mut has_ready = false;
    let mut has_process = false;
    let mut has_physics_process = false;
    let mut has_exit = false;
    for func in ast.iter_functions() {
        let arity = func.params.len();
        match func.name.as_ref() {
            "ready" if arity == 2 => has_ready = true,
            "process" if arity == 3 => has_process = true,
            "physics_process" if arity == 3 => has_physics_process = true,
            "exit" if arity == 2 => has_exit = true,
            _ => {}
        }
    }
    (has_ready, has_process, has_physics_process, has_exit)
}

fn entity_to_rhai(entity: Entity) -> ScriptHandle {
    entity.to_bits() as ScriptHandle
}

#[derive(Clone, Copy)]
enum PersistedHandlePolicy {
    DropAllHandles,
    DropStaleHandles,
}

fn should_strip_persisted_handle(handle: ScriptHandle, policy: PersistedHandlePolicy, shared: &SharedState) -> bool {
    if handle <= 0 {
        return false;
    }
    let upper = ((handle as u64) >> 32) as u32;
    if upper == 0 {
        return false;
    }
    if upper != shared.handle_nonce {
        return true;
    }
    let Some(entity) = shared.handle_lookup.get(&handle) else {
        return true;
    };
    match policy {
        PersistedHandlePolicy::DropAllHandles => true,
        PersistedHandlePolicy::DropStaleHandles => {
            if shared.entity_snapshots.is_empty() {
                return false;
            }
            !shared.entity_snapshots.contains_key(entity)
        }
    }
}

fn sanitize_persisted_value(
    value: &Dynamic,
    policy: PersistedHandlePolicy,
    shared: &SharedState,
) -> Option<Dynamic> {
    if let Some(int_val) = value.clone().try_cast::<ScriptHandle>() {
        if should_strip_persisted_handle(int_val, policy, shared) {
            return None;
        }
    }
    if let Some(arr) = value.clone().try_cast::<Array>() {
        let mut cleaned = Array::new();
        for v in arr {
            if let Some(cleaned_value) = sanitize_persisted_value(&v, policy, shared) {
                cleaned.push(cleaned_value);
            }
        }
        return Some(Dynamic::from_array(cleaned));
    }
    if let Some(map) = value.clone().try_cast::<Map>() {
        let mut cleaned = Map::new();
        for (k, v) in map {
            if let Some(cleaned_value) = sanitize_persisted_value(&v, policy, shared) {
                cleaned.insert(k, cleaned_value);
            }
        }
        if cleaned.is_empty() {
            return None;
        }
        return Some(Dynamic::from_map(cleaned));
    }
    Some(value.clone())
}

fn sanitize_persisted_map(map: &Map, policy: PersistedHandlePolicy, shared: &SharedState) -> Map {
    let mut cleaned = Map::new();
    for (k, v) in map {
        if let Some(cleaned_value) = sanitize_persisted_value(v, policy, shared) {
            cleaned.insert(k.clone(), cleaned_value);
        }
    }
    cleaned
}

pub struct ScriptPlugin {
    host: ScriptHost,
    commands: Vec<ScriptCommand>,
    logs: Vec<String>,
    paused: bool,
    step_once: bool,
    deterministic_ordering: bool,
    path_indices: HashMap<Arc<str>, usize>,
    path_list: Vec<Arc<str>>,
    failed_path_scratch: HashSet<usize>,
    id_updates: Vec<(Entity, u64)>,
    behaviour_worklist: Vec<(Entity, usize, u64, bool, bool)>,
    pending_persistent: HashMap<Entity, Map>,
}

impl ScriptPlugin {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            host: ScriptHost::new(path),
            commands: Vec::new(),
            logs: Vec::new(),
            paused: false,
            step_once: false,
            deterministic_ordering: false,
            path_indices: HashMap::new(),
            path_list: Vec::new(),
            failed_path_scratch: HashSet::new(),
            id_updates: Vec::new(),
            behaviour_worklist: Vec::new(),
            pending_persistent: HashMap::new(),
        }
    }

    pub fn set_ast_cache_dir(&mut self, dir: Option<PathBuf>) {
        self.host.set_ast_cache_dir(dir);
    }

    pub fn take_commands(&mut self) -> Vec<ScriptCommand> {
        self.commands.drain(..).collect()
    }

    pub fn take_logs(&mut self) -> Vec<String> {
        self.logs.drain(..).collect()
    }

    pub fn timing_offenders(&self) -> Vec<ScriptTimingOffender> {
        self.host.timing_offenders()
    }

    pub fn time_scale(&self) -> f32 {
        let scale = self.host.shared.borrow().time_scale;
        if scale.is_finite() && scale >= 0.0 { scale } else { 1.0 }
    }

    pub fn register_spawn_result(&mut self, handle: ScriptHandle, entity: Entity, tag: Option<String>) {
        self.host.register_spawn_result(handle, entity, tag);
    }

    pub fn record_spawn_failure(&mut self, reason: &str) {
        self.host.record_spawn_failure(reason);
    }

    pub fn safety_metrics(&self) -> ScriptSafetyMetrics {
        self.host.safety_metrics()
    }

    pub fn resolve_handle(&self, handle: ScriptHandle) -> Option<Entity> {
        self.host.resolve_handle(handle)
    }

    pub fn forget_handle(&mut self, handle: ScriptHandle) {
        self.host.forget_handle(handle);
    }

    pub fn forget_entity(&mut self, entity: Entity) {
        self.host.forget_entity(entity);
    }

    pub fn clear_handles(&mut self) {
        self.host.clear_handles();
    }

    pub fn handles_snapshot(&self) -> Vec<(ScriptHandle, Entity)> {
        self.host.handles_snapshot()
    }

    pub fn timing_summaries(&self) -> Vec<ScriptTimingSummary> {
        self.host.timing_summaries()
    }

    pub fn set_rng_seed(&mut self, seed: u64) {
        let mut shared = self.host.shared.borrow_mut();
        shared.rng = Some(StdRng::seed_from_u64(seed));
    }

    pub fn set_callback_budget_ms(&mut self, budget_ms: Option<f32>) {
        self.host.set_callback_budget_ms(budget_ms);
    }

    pub fn set_command_quota(&mut self, quota: Option<usize>) {
        self.host.set_command_quota(quota);
    }

    fn populate_entity_snapshots(&mut self, ecs: &mut crate::ecs::EcsWorld) {
        let (cell_size, spatial_cells) = {
            let spatial_hash = ecs.world.resource::<crate::ecs::SpatialHash>();
            let mut cells = HashMap::new();
            for key in &spatial_hash.active_cells {
                if let Some(list) = spatial_hash.grid.get(key) {
                    if !list.is_empty() {
                        cells.insert(*key, list.clone());
                    }
                }
            }
            let cells = if cells.is_empty() { None } else { Some(cells) };
            (spatial_hash.cell, cells)
        };
        let mut snapshots = HashMap::new();
        let mut scene_ids: HashMap<Entity, Arc<str>> = HashMap::new();
        let shared = self.host.shared.borrow();
        let prev_scene_ids = &shared.entity_scene_ids;
        let mut query = ecs.world.query::<(
            Entity,
            Option<&WorldTransform>,
            Option<&Transform>,
            Option<&Velocity>,
            Option<&Tint>,
            Option<&Aabb>,
            Option<&SceneEntityTag>,
        )>();
        for (entity, wt, transform, vel, tint, aabb, scene_tag) in query.iter(&ecs.world) {
            let (translation, rotation, scale) = if let Some(t) = transform {
                let world_pos = wt
                    .map(|w| Vec2::new(w.0.w_axis.x, w.0.w_axis.y))
                    .unwrap_or(t.translation);
                (world_pos, t.rotation, t.scale)
            } else if let Some(wt) = wt {
                (Vec2::new(wt.0.w_axis.x, wt.0.w_axis.y), 0.0, Vec2::ONE)
            } else {
                continue;
            };
            if let Some(tag) = scene_tag {
                let scene_id = tag.id.as_str();
                if !scene_id.is_empty() {
                    let arc = match prev_scene_ids.get(&entity) {
                        Some(existing) if existing.as_ref() == scene_id => Arc::clone(existing),
                        _ => Arc::from(scene_id),
                    };
                    scene_ids.insert(entity, arc);
                }
            }
            snapshots.insert(
                entity,
                EntitySnapshot {
                    translation,
                    rotation,
                    scale,
                    velocity: vel.map(|v| v.0),
                    tint: tint.map(|t| t.0),
                    half_extents: aabb.map(|a| a.half),
                },
            );
        }
        drop(shared);
        self.host.set_entity_snapshots(snapshots, cell_size, spatial_cells, scene_ids);
    }

    fn snapshot_from_input(input: &Input) -> InputSnapshot {
        InputSnapshot {
            forward: input.freefly_forward(),
            backward: input.freefly_backward(),
            left: input.freefly_left(),
            right: input.freefly_right(),
            ascend: input.freefly_ascend(),
            descend: input.freefly_descend(),
            boost: input.freefly_boost(),
            ctrl: input.ctrl_held(),
            left_mouse: input.left_mouse_held(),
            right_mouse: input.right_mouse_held(),
            cursor: input.cursor_position().map(|(x, y)| Vec2::new(x, y)),
            cursor_world: input.cursor_world_position().map(|(x, y)| Vec2::new(x, y)),
            mouse_delta: Vec2::new(input.mouse_delta.0, input.mouse_delta.1),
            wheel: input.wheel,
        }
    }

    pub fn clear_rng_seed(&mut self) {
        let mut shared = self.host.shared.borrow_mut();
        shared.rng = None;
    }

    fn command_rank(cmd: &ScriptCommand) -> u8 {
        match cmd {
            ScriptCommand::Spawn { .. } => 0,
            ScriptCommand::SetVelocity { .. } => 1,
            ScriptCommand::SetPosition { .. } => 2,
            ScriptCommand::SetRotation { .. } => 3,
            ScriptCommand::SetScale { .. } => 4,
            ScriptCommand::SetTint { .. } => 5,
            ScriptCommand::SetSpriteRegion { .. } => 6,
            ScriptCommand::Despawn { .. } => 7,
            ScriptCommand::SetAutoSpawnRate { .. } => 8,
            ScriptCommand::SetSpawnPerPress { .. } => 9,
            ScriptCommand::SetEmitterRate { .. } => 10,
            ScriptCommand::SetEmitterSpread { .. } => 11,
            ScriptCommand::SetEmitterSpeed { .. } => 12,
            ScriptCommand::SetEmitterLifetime { .. } => 13,
            ScriptCommand::SetEmitterStartColor { .. } => 14,
            ScriptCommand::SetEmitterEndColor { .. } => 15,
            ScriptCommand::SetEmitterStartSize { .. } => 16,
            ScriptCommand::SetEmitterEndSize { .. } => 17,
            ScriptCommand::SpawnPrefab { .. } => 18,
            ScriptCommand::SpawnTemplate { .. } => 19,
            ScriptCommand::EntitySetPosition { .. } => 20,
            ScriptCommand::EntitySetRotation { .. } => 21,
            ScriptCommand::EntitySetScale { .. } => 22,
            ScriptCommand::EntitySetTint { .. } => 23,
            ScriptCommand::EntitySetVelocity { .. } => 24,
            ScriptCommand::EntityDespawn { .. } => 25,
        }
    }

    fn cmp_float(a: f32, b: f32) -> std::cmp::Ordering {
        a.total_cmp(&b)
    }

    fn cmp_vec2(a: &Vec2, b: &Vec2) -> std::cmp::Ordering {
        Self::cmp_float(a.x, b.x).then_with(|| Self::cmp_float(a.y, b.y))
    }

    fn cmp_vec4(a: &Vec4, b: &Vec4) -> std::cmp::Ordering {
        Self::cmp_float(a.x, b.x)
            .then_with(|| Self::cmp_float(a.y, b.y))
            .then_with(|| Self::cmp_float(a.z, b.z))
            .then_with(|| Self::cmp_float(a.w, b.w))
    }

    fn cmp_commands(a: &ScriptCommand, b: &ScriptCommand) -> std::cmp::Ordering {
        use ScriptCommand::*;
        let rank_a = Self::command_rank(a);
        let rank_b = Self::command_rank(b);
        rank_a
            .cmp(&rank_b)
            .then_with(|| match (a, b) {
                (Spawn { handle: ha, atlas: aa, region: ra, position: pa, scale: sa, velocity: va },
                 Spawn { handle: hb, atlas: ab, region: rb, position: pb, scale: sb, velocity: vb }) => {
                    ha.cmp(hb)
                        .then_with(|| aa.cmp(ab))
                        .then_with(|| ra.cmp(rb))
                        .then_with(|| Self::cmp_vec2(pa, pb))
                        .then_with(|| Self::cmp_float(*sa, *sb))
                        .then_with(|| Self::cmp_vec2(va, vb))
                }
                (SetVelocity { handle: ha, velocity: va }, SetVelocity { handle: hb, velocity: vb }) => {
                    ha.cmp(hb).then_with(|| Self::cmp_vec2(va, vb))
                }
                (SetPosition { handle: ha, position: pa }, SetPosition { handle: hb, position: pb }) => {
                    ha.cmp(hb).then_with(|| Self::cmp_vec2(pa, pb))
                }
                (SetRotation { handle: ha, rotation: ra }, SetRotation { handle: hb, rotation: rb }) => {
                    ha.cmp(hb).then_with(|| Self::cmp_float(*ra, *rb))
                }
                (SetScale { handle: ha, scale: sa }, SetScale { handle: hb, scale: sb }) => {
                    ha.cmp(hb).then_with(|| Self::cmp_vec2(sa, sb))
                }
                (SetTint { handle: ha, tint: ta }, SetTint { handle: hb, tint: tb }) => {
                    ha.cmp(hb).then_with(|| match (ta, tb) {
                        (None, None) => std::cmp::Ordering::Equal,
                        (None, Some(_)) => std::cmp::Ordering::Less,
                        (Some(_), None) => std::cmp::Ordering::Greater,
                        (Some(a), Some(b)) => Self::cmp_vec4(a, b),
                    })
                }
                (SetSpriteRegion { handle: ha, region: ra }, SetSpriteRegion { handle: hb, region: rb }) => {
                    ha.cmp(hb).then_with(|| ra.cmp(rb))
                }
                (Despawn { handle: ha }, Despawn { handle: hb }) => ha.cmp(hb),
                (SetAutoSpawnRate { rate: ra }, SetAutoSpawnRate { rate: rb }) => Self::cmp_float(*ra, *rb),
                (SetSpawnPerPress { count: ca }, SetSpawnPerPress { count: cb }) => ca.cmp(cb),
                (SetEmitterRate { rate: ra }, SetEmitterRate { rate: rb }) => Self::cmp_float(*ra, *rb),
                (SetEmitterSpread { spread: sa }, SetEmitterSpread { spread: sb }) => Self::cmp_float(*sa, *sb),
                (SetEmitterSpeed { speed: sa }, SetEmitterSpeed { speed: sb }) => Self::cmp_float(*sa, *sb),
                (SetEmitterLifetime { lifetime: la }, SetEmitterLifetime { lifetime: lb }) => {
                    Self::cmp_float(*la, *lb)
                }
                (SetEmitterStartColor { color: ca }, SetEmitterStartColor { color: cb }) => Self::cmp_vec4(ca, cb),
                (SetEmitterEndColor { color: ca }, SetEmitterEndColor { color: cb }) => Self::cmp_vec4(ca, cb),
                (SetEmitterStartSize { size: sa }, SetEmitterStartSize { size: sb }) => Self::cmp_float(*sa, *sb),
                (SetEmitterEndSize { size: sa }, SetEmitterEndSize { size: sb }) => Self::cmp_float(*sa, *sb),
                (SpawnPrefab { handle: ha, path: pa, tag: taga }, SpawnPrefab { handle: hb, path: pb, tag: tagb }) => {
                    ha.cmp(hb).then_with(|| pa.cmp(pb)).then_with(|| taga.cmp(tagb))
                }
                (
                    SpawnTemplate { handle: ha, template: ta, tag: taga },
                    SpawnTemplate { handle: hb, template: tb, tag: tagb },
                ) => {
                    ha.cmp(hb).then_with(|| ta.cmp(tb)).then_with(|| taga.cmp(tagb))
                }
                (
                    EntitySetPosition { entity: ea, position: pa },
                    EntitySetPosition { entity: eb, position: pb },
                ) => ea.to_bits().cmp(&eb.to_bits()).then_with(|| Self::cmp_vec2(pa, pb)),
                (EntitySetRotation { entity: ea, rotation: ra }, EntitySetRotation { entity: eb, rotation: rb }) => {
                    ea.to_bits().cmp(&eb.to_bits()).then_with(|| Self::cmp_float(*ra, *rb))
                }
                (EntitySetScale { entity: ea, scale: sa }, EntitySetScale { entity: eb, scale: sb }) => {
                    ea.to_bits().cmp(&eb.to_bits()).then_with(|| Self::cmp_vec2(sa, sb))
                }
                (EntitySetTint { entity: ea, tint: ta }, EntitySetTint { entity: eb, tint: tb }) => {
                    ea.to_bits()
                        .cmp(&eb.to_bits())
                        .then_with(|| match (ta, tb) {
                            (None, None) => std::cmp::Ordering::Equal,
                            (None, Some(_)) => std::cmp::Ordering::Less,
                            (Some(_), None) => std::cmp::Ordering::Greater,
                            (Some(a), Some(b)) => Self::cmp_vec4(a, b),
                        })
                }
                (EntitySetVelocity { entity: ea, velocity: va }, EntitySetVelocity { entity: eb, velocity: vb }) => {
                    ea.to_bits().cmp(&eb.to_bits()).then_with(|| Self::cmp_vec2(va, vb))
                }
                (EntityDespawn { entity: ea }, EntityDespawn { entity: eb }) => ea.to_bits().cmp(&eb.to_bits()),
                _ => std::cmp::Ordering::Equal,
            })
    }

    fn drain_host_commands(&mut self) -> Vec<ScriptCommand> {
        let mut cmds = self.host.drain_commands();
        if self.deterministic_ordering {
            cmds.sort_by(Self::cmp_commands);
        }
        cmds
    }

    pub fn instance_count_for_test(&self) -> usize {
        self.host.instances.len()
    }

    fn sort_behaviour_worklist(&mut self) {
        if !self.deterministic_ordering {
            return;
        }
        self.behaviour_worklist.sort_by(
            |(entity_a, path_idx_a, instance_a, _, _), (entity_b, path_idx_b, instance_b, _, _)| {
                let path_a = self.path_list.get(*path_idx_a).map(|p| p.as_ref()).unwrap_or("");
                let path_b = self.path_list.get(*path_idx_b).map(|p| p.as_ref()).unwrap_or("");
                path_a
                    .cmp(path_b)
                    .then_with(|| entity_a.to_bits().cmp(&entity_b.to_bits()))
                    .then_with(|| instance_a.cmp(instance_b))
            },
        );
    }

    fn run_behaviours(
        &mut self,
        ecs: &mut crate::ecs::EcsWorld,
        assets: &AssetManager,
        dt: f32,
        fixed_step: bool,
    ) -> Result<()> {
        self.path_indices.clear();
        self.path_list.clear();
        self.failed_path_scratch.clear();
        self.id_updates.clear();
        self.behaviour_worklist.clear();
        self.populate_pending_persistent_from_components(ecs);
        let mut query = ecs.world.query::<(Entity, &mut ScriptBehaviour)>();
        for (entity, behaviour) in query.iter_mut(&mut ecs.world) {
            let path = behaviour.script_path.trim();
            if path.is_empty() {
                continue;
            }
            let idx = if let Some(idx) = self.path_indices.get(path).copied() {
                idx
            } else {
                let idx = self.path_list.len();
                let arc: Arc<str> = Arc::from(path);
                self.path_list.push(Arc::clone(&arc));
                self.path_indices.insert(arc, idx);
                idx
            };
            self.behaviour_worklist.push((entity, idx, behaviour.instance_id, behaviour.persist_state, behaviour.mute_errors));
        }
        for (idx, path) in self.path_list.iter().enumerate() {
            if let Err(err) = self.host.ensure_script_loaded(path.as_ref(), Some(assets)) {
                self.host.set_error_with_details(&err);
                self.failed_path_scratch.insert(idx);
            }
        }
        self.sort_behaviour_worklist();
        for (entity, path_idx, mut instance_id, persist_state, mute_errors) in self.behaviour_worklist.drain(..) {
            if self.failed_path_scratch.contains(&path_idx) {
                self.host.mark_entity_error(entity);
                continue;
            }
            let script_path = &self.path_list[path_idx];
            if instance_id == 0 {
                match self.host.create_instance_preloaded(script_path, entity, persist_state) {
                    Ok(id) => {
                        instance_id = id;
                        self.id_updates.push((entity, id));
                        if let Some(persistent) = self.pending_persistent.remove(&entity) {
                            let sanitized = {
                                let shared = self.host.shared.borrow();
                                sanitize_persisted_map(
                                    &persistent,
                                    PersistedHandlePolicy::DropStaleHandles,
                                    &shared,
                                )
                            };
                            if let Some(new_instance) = self.host.instances.get_mut(&id) {
                                new_instance.mute_errors = mute_errors;
                                let mut state = new_instance.state.borrow_mut();
                                state.persistent = sanitized;
                                state.is_hot_reload = true;
                            }
                        } else if let Some(new_instance) = self.host.instances.get_mut(&id) {
                            new_instance.mute_errors = mute_errors;
                        }
                    }
                    Err(err) => {
                        self.host.set_error_with_details(&err);
                        self.host.mark_entity_error(entity);
                        continue;
                    }
                }
            } else if let Some(instance) = self.host.instances.get_mut(&instance_id) {
                instance.persist_state = persist_state;
                instance.mute_errors = mute_errors;
            }
            if !persist_state {
                if let Some(instance) = self.host.instances.get_mut(&instance_id) {
                    instance.state.borrow_mut().persistent.clear();
                }
                self.pending_persistent.remove(&entity);
                if let Ok(mut entity_ref) = ecs.world.get_entity_mut(entity) {
                    entity_ref.remove::<ScriptPersistedState>();
                }
            }
            if let Err(err) = self.host.call_instance_ready(instance_id) {
                eprintln!("[script] ready error for {}: {}", script_path, err);
                self.host.mark_entity_error(entity);
            }
            let call_result = if fixed_step {
                self.host.call_instance_physics_process(instance_id, dt)
            } else {
                self.host.call_instance_process(instance_id, dt)
            };
            if let Err(err) = call_result {
                eprintln!(
                    "[script] {} error for {}: {}",
                    if fixed_step { "physics_process" } else { "process" },
                    &self.path_list[path_idx],
                    err
                );
                self.host.mark_entity_error(entity);
            }
            let instance_ok = self
                .host
                .instances
                .get(&instance_id)
                .map_or(false, |instance| !instance.errored);
            if instance_ok {
                self.host.clear_entity_error(entity);
            }
        }
        for (entity, id) in self.id_updates.drain(..) {
            if let Ok(mut entity_ref) = ecs.world.get_entity_mut(entity) {
                if let Some(mut behaviour) = entity_ref.get_mut::<ScriptBehaviour>() {
                    behaviour.instance_id = id;
                }
            }
        }
        self.sync_persisted_state_components(ecs);
        Ok(())
    }

    fn cleanup_orphaned_instances(&mut self, ecs: &mut crate::ecs::EcsWorld) {
        self.pending_persistent.retain(|entity, _| ecs.world.get_entity(*entity).is_ok());
        self.host.prune_entity_errors(|entity| ecs.world.get_entity(entity).is_ok());
        let mut stale_ids = Vec::new();
        for (&id, instance) in self.host.instances.iter() {
            let Ok(entity_ref) = ecs.world.get_entity(instance.entity) else {
                stale_ids.push(id);
                continue;
            };
            match entity_ref.get::<ScriptBehaviour>() {
                Some(behaviour)
                    if behaviour.instance_id == id && behaviour.script_path == instance.script_path => {}
                _ => stale_ids.push(id),
            }
        }
        for id in stale_ids {
            let _ = self.host.call_instance_exit(id);
            if let Some(instance) = self.host.instances.get(&id) {
                self.host.clear_entity_error(instance.entity);
            }
            self.host.remove_instance(id);
        }
        let mut behaviours = ecs.world.query::<(Entity, &mut ScriptBehaviour)>();
        for (entity, mut behaviour) in behaviours.iter_mut(&mut ecs.world) {
            if behaviour.instance_id != 0 && !self.host.instances.contains_key(&behaviour.instance_id) {
                self.host.drop_listeners_for_owner(ListenerOwner::Instance(behaviour.instance_id));
                behaviour.instance_id = 0;
                self.host.clear_entity_error(entity);
            }
        }
    }

    fn populate_pending_persistent_from_components(&mut self, ecs: &mut crate::ecs::EcsWorld) {
        let mut query = ecs.world.query::<(Entity, &ScriptPersistedState, Option<&ScriptBehaviour>)>();
        let mut stale: Vec<Entity> = Vec::new();
        for (entity, persisted, behaviour) in query.iter(&ecs.world) {
            let wants_persist = behaviour.map(|b| b.persist_state).unwrap_or(false);
            if !wants_persist {
                stale.push(entity);
                continue;
            }
            if let Some(map) = ScriptHost::json_to_map(&persisted.0) {
                let sanitized = {
                    let shared = self.host.shared.borrow();
                    sanitize_persisted_map(&map, PersistedHandlePolicy::DropAllHandles, &shared)
                };
                self.pending_persistent.insert(entity, sanitized);
            }
        }
        for entity in stale {
            if let Ok(mut entity_ref) = ecs.world.get_entity_mut(entity) {
                entity_ref.remove::<ScriptPersistedState>();
            }
        }
    }

    fn sync_persisted_state_components(&mut self, ecs: &mut crate::ecs::EcsWorld) {
        let mut to_update: HashMap<Entity, JsonValue> = HashMap::new();
        let mut to_remove: HashSet<Entity> = HashSet::new();
        for instance in self.host.instances.values() {
            if !instance.persist_state || instance.errored {
                continue;
            }
            let map = instance.state.borrow().persistent.clone();
            let sanitized = {
                let shared = self.host.shared.borrow();
                sanitize_persisted_map(&map, PersistedHandlePolicy::DropAllHandles, &shared)
            };
            if sanitized.is_empty() {
                to_remove.insert(instance.entity);
                continue;
            }
            let json = ScriptHost::map_to_json(&sanitized);
            to_update.insert(instance.entity, json);
        }
        for (entity, json) in &to_update {
            let entity = *entity;
            if let Ok(mut entity_ref) = ecs.world.get_entity_mut(entity) {
                if let Some(mut existing) = entity_ref.get_mut::<ScriptPersistedState>() {
                    existing.0 = json.clone();
                } else {
                    entity_ref.insert(ScriptPersistedState(json.clone()));
                }
            }
        }
        let mut stale: Vec<Entity> = Vec::new();
        {
            let mut query = ecs.world.query::<(Entity, &ScriptPersistedState, Option<&ScriptBehaviour>)>();
            for (entity, _, behaviour) in query.iter(&ecs.world) {
                let wants_persist = behaviour.map(|b| b.persist_state).unwrap_or(false);
                if !wants_persist || to_remove.contains(&entity) || !to_update.contains_key(&entity) {
                    stale.push(entity);
                }
            }
        }
        for entity in stale {
            if let Ok(mut entity_ref) = ecs.world.get_entity_mut(entity) {
                entity_ref.remove::<ScriptPersistedState>();
            }
        }
    }

    pub fn script_path(&self) -> &Path {
        self.host.script_path()
    }

    pub fn enabled(&self) -> bool {
        self.host.enabled()
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.host.set_enabled(enabled);
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if !paused {
            self.step_once = false;
        }
    }

    pub fn set_deterministic_ordering(&mut self, enabled: bool) {
        self.deterministic_ordering = enabled;
    }

    pub fn enable_deterministic_mode(&mut self, seed: u64) {
        self.deterministic_ordering = true;
        self.set_rng_seed(seed);
        {
            let mut shared = self.host.shared.borrow_mut();
            let mut nonce = (seed as u32) & 0x7FFF_FFFF;
            if nonce == 0 {
                nonce = 1;
            }
            shared.handle_nonce = nonce;
            shared.next_handle = 1;
            shared.pending_handles.clear();
        }
    }

    pub fn step_once(&mut self) {
        self.step_once = true;
    }

    pub fn reload_instance_for_entity(&mut self, entity: Entity, preserve_state: bool) {
        let Some((&id, instance)) = self.host.instances.iter().find(|(_, inst)| inst.entity == entity) else {
            if !preserve_state {
                self.pending_persistent.remove(&entity);
            }
            return;
        };
        let preserved = if preserve_state && instance.persist_state {
            let current = instance.state.borrow().persistent.clone();
            let sanitized = {
                let shared = self.host.shared.borrow();
                sanitize_persisted_map(&current, PersistedHandlePolicy::DropStaleHandles, &shared)
            };
            Some(sanitized)
        } else {
            None
        };
        let _ = self.host.call_instance_exit(id);
        self.host.remove_instance(id);
        self.host.clear_entity_error(entity);
        if let Some(map) = preserved {
            self.pending_persistent.insert(entity, map);
        } else {
            self.pending_persistent.remove(&entity);
        }
    }

    pub fn force_reload(&mut self, assets: Option<&AssetManager>) -> Result<()> {
        self.host.force_reload(assets)
    }

    pub fn set_error_message(&mut self, msg: impl Into<String>) {
        self.host.set_error_message(msg);
    }

    pub fn last_error(&self) -> Option<&str> {
        self.host.last_error()
    }

    pub fn entity_has_errored_instance(&self, entity: Entity) -> bool {
        self.host.entity_has_errored_instance(entity)
    }

    pub fn eval_repl(&mut self, source: &str) -> Result<Option<String>> {
        let result = self.host.eval_repl(source)?;
        let drained = self.drain_host_commands();
        self.commands.extend(drained);
        self.logs.extend(self.host.drain_logs());
        Ok(result)
    }
}

impl EnginePlugin for ScriptPlugin {
    fn name(&self) -> &'static str {
        "scripts"
    }

    fn version(&self) -> &'static str {
        "1.0.0"
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        let run_scripts = if self.paused {
            if self.step_once {
                self.step_once = false;
                true
            } else {
                false
            }
        } else {
            true
        };
        let input_snapshot = ctx.input().ok().map(Self::snapshot_from_input);
        let (assets, ecs) = ctx.assets_and_ecs_mut()?;
        if let Some(snap) = input_snapshot {
            self.host.set_input_snapshot(snap);
        }
        self.populate_entity_snapshots(ecs);
        self.cleanup_orphaned_instances(ecs);
        let dt_scaled = self.host.update(dt, run_scripts, Some(assets));
        if run_scripts && self.host.enabled() {
            let rapier_ctx = {
                let rapier = ecs.world.resource::<crate::ecs::RapierState>();
                PhysicsQueryContext::from_state(rapier)
            };
            self.host.set_physics_context(Some(rapier_ctx));
            let result = self.run_behaviours(ecs, assets, dt_scaled, false);
            self.host.set_physics_context(None);
            result?;
        }
        self.host.dispatch_script_events();
        if !self.paused {
            self.step_once = false;
        }
        let drained = self.drain_host_commands();
        self.commands.extend(drained);
        self.logs.extend(self.host.drain_logs());
        Ok(())
    }

    fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        let input_snapshot = ctx.input().ok().map(Self::snapshot_from_input);
        let (assets, ecs) = ctx.assets_and_ecs_mut()?;
        if let Some(snap) = input_snapshot {
            self.host.set_input_snapshot(snap);
        }
        self.populate_entity_snapshots(ecs);
        self.cleanup_orphaned_instances(ecs);
        if self.paused {
            self.host.dispatch_script_events();
            let drained = self.drain_host_commands();
            self.commands.extend(drained);
            self.logs.extend(self.host.drain_logs());
            return Ok(());
        }
        if self.host.enabled() {
            let dt_scaled = self.host.begin_frame(dt);
            let rapier_ctx = {
                let rapier = ecs.world.resource::<crate::ecs::RapierState>();
                PhysicsQueryContext::from_state(rapier)
            };
            self.host.set_physics_context(Some(rapier_ctx));
            let result = self.run_behaviours(ecs, assets, dt_scaled, true);
            self.host.set_physics_context(None);
            result?;
        }
        self.host.dispatch_script_events();
        let drained = self.drain_host_commands();
        self.commands.extend(drained);
        self.logs.extend(self.host.drain_logs());
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        self.host.clear_handles();
        self.host.clear_instances();
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<ScriptWorld>("World");
    engine.register_fn("handle_is_alive", ScriptWorld::handle_is_alive);
    engine.register_fn("handle_validate", ScriptWorld::handle_validate);
    engine.register_fn("handles_with_tag", ScriptWorld::handles_with_tag);
    engine.register_fn("find_scene_entity", ScriptWorld::find_scene_entity);
    engine.register_fn("spawn_sprite", ScriptWorld::spawn_sprite);
    engine.register_fn("spawn_sprite_safe", ScriptWorld::spawn_sprite_safe);
    engine.register_fn("spawn_prefab_safe", ScriptWorld::spawn_prefab_safe);
    engine.register_fn("spawn_prefab_safe", ScriptWorld::spawn_prefab_safe_with_tag);
    engine.register_fn("spawn_template_safe", ScriptWorld::spawn_template_safe);
    engine.register_fn("spawn_template_safe", ScriptWorld::spawn_template_safe_with_tag);
    engine.register_fn("spawn_player", ScriptWorld::spawn_player);
    engine.register_fn("spawn_player_safe", ScriptWorld::spawn_player_safe);
    engine.register_fn("spawn_enemy", ScriptWorld::spawn_enemy);
    engine.register_fn("spawn_enemy_safe", ScriptWorld::spawn_enemy_safe);
    engine.register_fn("set_velocity", ScriptWorld::set_velocity);
    engine.register_fn("set_position", ScriptWorld::set_position);
    engine.register_fn("set_rotation", ScriptWorld::set_rotation);
    engine.register_fn("set_scale", ScriptWorld::set_scale);
    engine.register_fn("set_tint", ScriptWorld::set_tint);
    engine.register_fn("clear_tint", ScriptWorld::clear_tint);
    engine.register_fn("set_sprite_region", ScriptWorld::set_sprite_region);
    engine.register_fn("despawn", ScriptWorld::despawn);
    engine.register_fn("spawn_prefab", ScriptWorld::spawn_prefab);
    engine.register_fn("spawn_template", ScriptWorld::spawn_template);
    engine.register_fn("set_auto_spawn_rate", ScriptWorld::set_auto_spawn_rate);
    engine.register_fn("set_spawn_per_press", ScriptWorld::set_spawn_per_press);
    engine.register_fn("set_emitter_rate", ScriptWorld::set_emitter_rate);
    engine.register_fn("set_emitter_spread", ScriptWorld::set_emitter_spread);
    engine.register_fn("set_emitter_speed", ScriptWorld::set_emitter_speed);
    engine.register_fn("set_emitter_lifetime", ScriptWorld::set_emitter_lifetime);
    engine.register_fn("set_emitter_start_color", ScriptWorld::set_emitter_start_color);
    engine.register_fn("set_emitter_end_color", ScriptWorld::set_emitter_end_color);
    engine.register_fn("set_emitter_start_size", ScriptWorld::set_emitter_start_size);
    engine.register_fn("set_emitter_end_size", ScriptWorld::set_emitter_end_size);
    engine.register_fn("entity_set_position", ScriptWorld::entity_set_position);
    engine.register_fn("entity_set_rotation", ScriptWorld::entity_set_rotation);
    engine.register_fn("entity_set_scale", ScriptWorld::entity_set_scale);
    engine.register_fn("entity_set_tint", ScriptWorld::entity_set_tint);
    engine.register_fn("entity_clear_tint", ScriptWorld::entity_clear_tint);
    engine.register_fn("entity_set_velocity", ScriptWorld::entity_set_velocity);
    engine.register_fn("entity_despawn", ScriptWorld::entity_despawn);
    engine.register_fn("despawn_safe", ScriptWorld::despawn_safe);
    engine.register_fn("entity_snapshot", ScriptWorld::entity_snapshot);
    engine.register_fn("entity_position", ScriptWorld::entity_position);
    engine.register_fn("entity_rotation", ScriptWorld::entity_rotation);
    engine.register_fn("entity_tag", ScriptWorld::entity_tag);
    engine.register_fn("entity_handle", ScriptWorld::entity_handle);
    engine.register_fn("entity_scene_id", ScriptWorld::entity_scene_id);
    engine.register_fn("entity_scale", ScriptWorld::entity_scale);
    engine.register_fn("entity_velocity", ScriptWorld::entity_velocity);
    engine.register_fn("entity_tint", ScriptWorld::entity_tint);
    engine.register_fn("raycast", ScriptWorld::raycast);
    engine.register_fn("raycast", ScriptWorld::raycast_with_filters);
    engine.register_fn("overlap_circle", ScriptWorld::overlap_circle);
    engine.register_fn("overlap_circle", ScriptWorld::overlap_circle_with_filters);
    engine.register_fn("overlap_circle_hits", ScriptWorld::overlap_circle_hits);
    engine.register_fn("overlap_circle_hits", ScriptWorld::overlap_circle_hits_with_filters);
    engine.register_fn("input_forward", ScriptWorld::input_forward);
    engine.register_fn("input_backward", ScriptWorld::input_backward);
    engine.register_fn("input_left", ScriptWorld::input_left);
    engine.register_fn("input_right", ScriptWorld::input_right);
    engine.register_fn("input_ascend", ScriptWorld::input_ascend);
    engine.register_fn("input_descend", ScriptWorld::input_descend);
    engine.register_fn("input_boost", ScriptWorld::input_boost);
    engine.register_fn("input_ctrl", ScriptWorld::input_ctrl);
    engine.register_fn("input_left_mouse", ScriptWorld::input_left_mouse);
    engine.register_fn("input_right_mouse", ScriptWorld::input_right_mouse);
    engine.register_fn("input_cursor", ScriptWorld::input_cursor);
    engine.register_fn("input_cursor_world", ScriptWorld::input_cursor_world);
    engine.register_fn("input_mouse_delta", ScriptWorld::input_mouse_delta);
    engine.register_fn("input_wheel", ScriptWorld::input_wheel);
    engine.register_fn("listen", ScriptWorld::listen);
    engine.register_fn("listen_for_entity", ScriptWorld::listen_for_entity);
    engine.register_fn("unlisten", ScriptWorld::unlisten);
    engine.register_fn("emit", ScriptWorld::emit);
    engine.register_fn("emit", ScriptWorld::emit_with_payload);
    engine.register_fn("emit_to", ScriptWorld::emit_to);
    engine.register_fn("emit_to", ScriptWorld::emit_to_with_payload);
    engine.register_fn("log", ScriptWorld::log);
    engine.register_fn("rand_seed", ScriptWorld::rand_seed);
    engine.register_fn("rand", ScriptWorld::random_range);
    engine.register_fn("time_scale", ScriptWorld::time_scale);
    engine.register_fn("set_time_scale", ScriptWorld::set_time_scale);
    engine.register_fn("delta_seconds", ScriptWorld::delta_seconds);
    engine.register_fn("unscaled_delta_seconds", ScriptWorld::unscaled_delta_seconds);
    engine.register_fn("time_seconds", ScriptWorld::time_seconds);
    engine.register_fn("unscaled_time_seconds", ScriptWorld::unscaled_time_seconds);
    engine.register_fn("timer_start", ScriptWorld::timer_start);
    engine.register_fn("timer_start_repeat", ScriptWorld::timer_start_repeat);
    engine.register_fn("timer_fired", ScriptWorld::timer_fired);
    engine.register_fn("timer_remaining", ScriptWorld::timer_remaining);
    engine.register_fn("timer_clear", ScriptWorld::timer_clear);
    engine.register_fn("move_toward", ScriptWorld::move_toward);
    engine.register_fn("state_get", ScriptWorld::state_get);
    engine.register_fn("state_set", ScriptWorld::state_set);
    engine.register_fn("state_clear", ScriptWorld::state_clear);
    engine.register_fn("state_keys", ScriptWorld::state_keys);
    engine.register_fn("stat_get", ScriptWorld::stat_get);
    engine.register_fn("stat_set", ScriptWorld::stat_set);
    engine.register_fn("stat_add", ScriptWorld::stat_add);
    engine.register_fn("stat_clear", ScriptWorld::stat_clear);
    engine.register_fn("stat_keys", ScriptWorld::stat_keys);
    engine.register_fn("is_hot_reload", ScriptWorld::is_hot_reload);
    engine.register_fn("vec2", ScriptWorld::vec2);
    engine.register_fn("vec2_len", ScriptWorld::vec2_len);
    engine.register_fn("vec2_normalize", ScriptWorld::vec2_normalize);
    engine.register_fn("vec2_distance", ScriptWorld::vec2_distance);
    engine.register_fn("vec2_lerp", ScriptWorld::vec2_lerp);
    engine.register_fn("move_toward_vec2", ScriptWorld::move_toward_vec2);
    engine.register_fn("angle_to_vec", ScriptWorld::angle_to_vec);
    engine.register_fn("vec_to_angle", ScriptWorld::vec_to_angle);
    engine.register_fn("wrap_angle_pi", ScriptWorld::wrap_angle_pi);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use std::cell::RefCell;
    use std::fs;
    use std::io::Write;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use crate::ecs::{EcsWorld, PhysicsParams, RapierState, SpatialHash, Transform, WorldBounds};
    use tempfile::{tempdir, Builder, NamedTempFile};

    fn write_script(contents: &str) -> NamedTempFile {
        let mut temp = NamedTempFile::new().expect("temp script");
        write!(temp, "{contents}").expect("write script");
        temp
    }

    #[test]
    fn stat_helpers_store_and_clear_values() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state);
        assert_eq!(world.stat_get("score"), 0.0);
        assert!(!world.stat_set("", Dynamic::from(1.0)));
        assert!(world.stat_set("score", Dynamic::from(5.0)));
        assert!((world.stat_get("score") - 5.0).abs() < 1e-4);
        let updated = world.stat_add("score", 3.5);
        assert!((updated - 8.5).abs() < 1e-4);
        let mut keys = world.stat_keys();
        assert_eq!(keys.len(), 1);
        keys.sort_by(|a, b| a.to_string().cmp(&b.to_string()));
        let key: String = keys[0].clone().try_cast().expect("string key");
        assert_eq!(key, "score");
        assert!(world.stat_clear("score"));
        assert_eq!(world.stat_keys().len(), 0);
    }

    #[test]
    fn repl_mutates_scope_and_enqueues_commands() {
        let script = write_script(
            r#"
                let counter = 0;
                fn init(world) {}
                fn update(world, dt) {}
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("load script");

        let out = host.eval_repl("counter += 5; counter").expect("repl result");
        assert_eq!(out.as_deref(), Some("5"));

        let confirm = host.eval_repl("counter").expect("counter read");
        assert_eq!(confirm.as_deref(), Some("5"));

        host.eval_repl("world.set_spawn_per_press(7);").expect("repl command");
        let commands = host.drain_commands();
        assert!(matches!(&commands[..], [ScriptCommand::SetSpawnPerPress { count }] if *count == 7));
    }

    #[test]
    fn reload_detects_changes_when_metadata_is_stable() {
        let script = write_script(
            r#"
                let value = 1;
                fn init(world) {}
                fn update(world, dt) {}
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("initial load");
        assert_eq!(host.eval_repl("value").expect("read value").as_deref(), Some("1"));

        let replacement = r#"
                let value = 2;
                fn init(world) {}
                fn update(world, dt) {}
            "#;
        std::fs::write(script.path(), replacement).expect("rewrite script");
        let metadata = std::fs::metadata(script.path()).expect("metadata");
        host.last_modified = metadata.modified().ok();
        host.last_len = Some(metadata.len());

        host.reload_if_needed(None).expect("reload check");
        assert_eq!(host.eval_repl("value").expect("read value").as_deref(), Some("2"));
    }

    #[test]
    fn init_with_wrong_signature_reports_error() {
        let script = write_script(
            r#"
                fn init() { }
                fn update(world, dt) { }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("initial load");
        let _ = host.update(0.016, true, None);
        let err = host.last_error().expect("error recorded");
        assert!(err.contains("init") && err.contains("signature"), "unexpected error: {err}");
    }

    #[test]
    fn update_with_wrong_signature_reports_error() {
        let script = write_script(
            r#"
                fn init(world) { }
                fn update(world) { }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("initial load");
        let _ = host.update(0.016, true, None);
        let err = host.last_error().expect("error recorded");
        assert!(err.contains("update") && err.contains("signature"), "unexpected error: {err}");
    }

    #[test]
    fn digest_check_is_throttled() {
        let script = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("initial load");
        let first_check = host.last_digest_check;
        host.reload_if_needed(None).expect("no reload needed");
        assert_eq!(host.last_digest_check, first_check, "digest check should not update without interval");
        std::thread::sleep(Duration::from_millis(260));
        host.last_digest_check = Some(Instant::now() - Duration::from_millis(251));
        host.reload_if_needed(None).expect("reload after interval");
        assert!(host
            .last_digest_check
            .expect("digest check set")
            .duration_since(first_check.expect("initial digest check"))
            >= Duration::from_millis(250));
    }

    #[test]
    fn random_range_handles_inverted_bounds() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state);
        for _ in 0..8 {
            let value = world.random_range(5.0, -2.0);
            assert!((-2.0..=5.0).contains(&value), "value {value} should stay within swapped bounds");
        }
    }

    #[test]
    fn random_range_returns_value_for_equal_bounds() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state);
        let value = world.random_range(std::f32::consts::PI as FLOAT, std::f32::consts::PI as FLOAT);
        assert_eq!(value as f32, std::f32::consts::PI);
    }

    #[test]
    fn world_vec_helpers_are_available_without_import() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state);
        let v = world.vec2(3.0, 4.0);
        let len = world.vec2_len(v.clone());
        assert!((len - 5.0).abs() < 1e-5);
        let norm = world.vec2_normalize(v.clone());
        let nx: FLOAT = norm[0].clone().try_cast().unwrap();
        let ny: FLOAT = norm[1].clone().try_cast().unwrap();
        assert!((nx - 0.6).abs() < 0.05 && (ny - 0.8).abs() < 0.05);
        let zero = world.vec2(0.0, 0.0);
        let dist = world.vec2_distance(v.clone(), zero.clone());
        assert!((dist - 5.0).abs() < 0.05);
        let b = world.vec2(2.0, 2.0);
        let lerp = world.vec2_lerp(zero.clone(), b.clone(), 0.25);
        let lx: FLOAT = lerp[0].clone().try_cast().unwrap();
        let ly: FLOAT = lerp[1].clone().try_cast().unwrap();
        assert!((lx - 0.5).abs() < 0.05 && (ly - 0.5).abs() < 0.05);
        let target = world.vec2(2.0, 0.0);
        let step = world.move_toward_vec2(zero, target, 0.5);
        let sx: FLOAT = step[0].clone().try_cast().unwrap();
        assert!((sx - 0.5).abs() < 0.05);
        let dir = world.angle_to_vec(std::f32::consts::FRAC_PI_2 as FLOAT);
        let dx: FLOAT = dir[0].clone().try_cast().unwrap();
        let dy: FLOAT = dir[1].clone().try_cast().unwrap();
        assert!(dx.abs() < 0.05 && (dy - 1.0).abs() < 0.05);
        let up = world.vec2(0.0, 1.0);
        let ang = world.vec_to_angle(up);
        let target_ang: FLOAT = std::f64::consts::FRAC_PI_2 as FLOAT;
        assert!((ang - target_ang).abs() < 0.05);
        let wrapped = world.wrap_angle_pi(7.0);
        assert!((wrapped - 0.7168).abs() < 0.05);
    }

    #[test]
    fn random_range_is_deterministic_with_seed() {
        let state_a = SharedState { rng: Some(rand::rngs::StdRng::seed_from_u64(1234)), ..Default::default() };
        let state_b = SharedState { rng: Some(rand::rngs::StdRng::seed_from_u64(1234)), ..Default::default() };
        let mut world_a = ScriptWorld::new(Rc::new(RefCell::new(state_a)));
        let mut world_b = ScriptWorld::new(Rc::new(RefCell::new(state_b)));
        let samples_a = [world_a.random_range(-1.0, 1.0), world_a.random_range(0.0, 10.0)];
        let samples_b = [world_b.random_range(-1.0, 1.0), world_b.random_range(0.0, 10.0)];
        assert_eq!(samples_a, samples_b, "seeded RNG should be deterministic across worlds");
    }

    #[test]
    fn set_auto_spawn_rate_accepts_float_literal() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state.clone());
        world.set_auto_spawn_rate(1.25 as FLOAT);
        let cmds = &state.borrow().commands;
        assert!(matches!(
            cmds.as_slice(),
            [ScriptCommand::SetAutoSpawnRate { rate }] if (*rate - 1.25).abs() < 1e-4
        ));
    }

    #[test]
    fn init_and_update_with_expected_signatures_run() {
        let script = write_script(
            r#"
                let state = #{ count: 0 };
                fn init(world) {
                    world.log("hello");
                }
                fn update(world, dt) {
                    if dt > 0.0 {
                        state.count += 1;
                    }
                }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("load script");
        let funcs: Vec<(String, usize)> = host
            .ast
            .as_ref()
            .expect("ast loaded")
            .iter_functions()
            .map(|f| (f.name.to_string(), f.params.len()))
            .collect();
        assert!(funcs.iter().any(|(n, _)| n == "init"), "init missing: {:?}", funcs);
        assert!(
            funcs.iter().any(|(n, p)| n == "update" && *p == 2),
            "update missing: {:?}",
            funcs
        );
        let _ = host.update(0.016, true, None);
        assert!(
            host.last_error().is_none(),
            "init should succeed, got {:?}",
            host.last_error()
        );
        let _ = host.update(0.016, true, None);
        assert!(
            host.last_error().is_none(),
            "update should succeed, got {:?}",
            host.last_error()
        );
        let logs = host.drain_logs();
        assert!(logs.iter().any(|l| l.contains("hello")), "init log missing: {:?}", logs);
        let commands = host.drain_commands();
        assert!(commands.is_empty(), "unexpected commands from fixture script");
    }

    #[test]
    fn ensure_script_loaded_caches_and_detects_callbacks() {
        let script = write_script(
            r#"
                fn ready(world, entity) { }
                fn process(world, entity, dt) { }
            "#,
        );
        let path_str = script.path().to_string_lossy().into_owned();
        let mut host = ScriptHost::new(script.path());
        host.ensure_script_loaded(&path_str, None).expect("load behaviour script");
        let compiled = host.compiled_script(&path_str).expect("script cached");
        assert!(compiled.has_ready);
        assert!(compiled.has_process);
        assert!(!compiled.has_physics_process);
        // second call should be a no-op
        host.ensure_script_loaded(&path_str, None).expect("cached load should succeed");
    }

    #[test]
    fn module_import_resolves_from_assets_scripts() {
        let mut module = Builder::new()
            .prefix("rhai_module_test_")
            .suffix(".rhai")
            .tempfile_in("assets/scripts")
            .expect("module file");
        write!(module.as_file_mut(), "fn value() {{ 123 }}").expect("write module");
        let module_name = module
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("module name");

        let script = write_script(&format!(
            r#"
                import "{module_name}" as m;
                fn init(world) {{ }}
                fn update(world, dt) {{
                    let v = m::value();
                    world.log(v.to_string());
                }}
            "#
        ));
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("load script with import");
        let _ = host.update(0.016, true, None);
        let logs = host.drain_logs();
        assert!(
            logs.iter().any(|l| l.contains("123")),
            "expected imported module log, got {:?}",
            logs
        );
    }

    #[test]
    fn module_import_reloads_when_dependency_changes() {
        let mut module = Builder::new()
            .prefix("rhai_module_reload_")
            .suffix(".rhai")
            .tempfile_in("assets/scripts")
            .expect("module file");
        write!(module.as_file_mut(), "fn value() {{ 1 }}").expect("write module v1");
        let module_name = module.path().file_stem().and_then(|s| s.to_str()).expect("module name");

        let behaviour = write_script(&format!(
            r#"
                import "{module_name}" as m;
                fn ready(world, entity) {{ world.log(m::value().to_string()); }}
                fn process(world, entity, dt) {{ }}
            "#
        ));
        let behaviour_path = behaviour.path().to_string_lossy().into_owned();
        let mut host = ScriptHost::new(behaviour.path());
        let mut world = bevy_ecs::world::World::new();
        let entity = world.spawn_empty().id();

        host.ensure_script_loaded(&behaviour_path, None).expect("initial load");
        let instance_id = host.create_instance(&behaviour_path, entity, None).expect("instance create");
        host.call_instance_ready(instance_id).expect("ready call");
        let first_logs = host.drain_logs();
        assert!(first_logs.iter().any(|l| l.contains("1")), "expected module v1 log, got {first_logs:?}");

        fs::write(module.path(), "fn value() { 2 }").expect("write module v2");
        if let Some(compiled) = host.scripts.get_mut(&behaviour_path) {
            compiled.last_checked = Some(Instant::now() - Duration::from_millis(300));
        }
        host.ensure_script_loaded(&behaviour_path, None).expect("reload after module change");
        host.call_instance_ready(instance_id).expect("ready rerun");
        let second_logs = host.drain_logs();
        assert!(second_logs.iter().any(|l| l.contains("2")), "expected module v2 log, got {second_logs:?}");
    }

    #[test]
    fn module_import_cache_reuses_until_source_changes() {
        let mut module = Builder::new()
            .prefix("rhai_module_cache_")
            .suffix(".rhai")
            .tempfile_in("assets/scripts")
            .expect("module file");
        write!(module.as_file_mut(), "fn value() {{ 1 }}").expect("write module v1");
        let module_name = module.path().file_stem().and_then(|s| s.to_str()).expect("module name");

        let script = write_script(&format!(
            r#"
                import "{module_name}" as m;
                fn init(world) {{}}
                fn update(world, dt) {{
                    world.log(m::value().to_string());
                }}
            "#
        ));
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("load script with import");
        let canonical = module.path().canonicalize().expect("canonical module path");
        let first_ptr = {
            let cache = host.import_resolver.cache.read().expect("cache read");
            let entry = cache.get(&canonical).expect("module cached");
            Rc::as_ptr(&entry.module)
        };

        host.force_reload(None).expect("reload without change");
        let second_ptr = {
            let cache = host.import_resolver.cache.read().expect("cache read");
            let entry = cache.get(&canonical).expect("module cached");
            Rc::as_ptr(&entry.module)
        };
        assert_eq!(first_ptr, second_ptr, "cache should be reused when import is unchanged");

        fs::write(module.path(), "fn value() { 2 }").expect("rewrite module v2");
        host.force_reload(None).expect("reload after module change");
        let third_ptr = {
            let cache = host.import_resolver.cache.read().expect("cache read");
            let entry = cache.get(&canonical).expect("module cached");
            Rc::as_ptr(&entry.module)
        };
        assert_ne!(first_ptr, third_ptr, "cache should refresh after import source changes");
    }

    #[test]
    fn module_import_rejects_parent_directory_escape() {
        let script = write_script(
            r#"
                import "../outside" as bad;
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        assert!(
            host.force_reload(None).is_err(),
            "importing with '..' should be rejected"
        );
    }

    #[test]
    fn common_rhai_helpers_cover_basic_paths() {
        let script = write_script(
            r#"
                import "common" as c;
                fn init(world) { }
                fn update(world, dt) {
                    let mv = c::move_toward_vec2([0.0, 0.0], [2.0, 0.0], 0.5);
                    world.log("mv:" + mv[0].to_string() + "," + mv[1].to_string());
                    let clamped = c::clamp_length([3.0, 4.0], 2.0);
                    let len = (clamped[0] * clamped[0] + clamped[1] * clamped[1]).sqrt();
                    world.log("len:" + len.to_string());
                    let cd = c::cooldown(0.5);
                    cd = c::cooldown_trigger(cd);
                    cd = c::cooldown_tick(cd, 0.3);
                    world.log("ready1:" + c::cooldown_ready(cd).to_string());
                    cd = c::cooldown_tick(cd, 0.25);
                    world.log("ready2:" + c::cooldown_ready(cd).to_string());
                    let dir = c::angle_to_vec(c::deg_to_rad(90.0));
                    world.log("dir:" + dir[0].to_string() + "," + dir[1].to_string());
                    let ang = c::vec_to_angle([0.0, 1.0]);
                    world.log("ang:" + ang.to_string());
                    let norm = c::vec2_normalize([3.0, 4.0]);
                    world.log("norm:" + norm[0].to_string() + "," + norm[1].to_string());
                    let dist = c::vec2_distance([1.0, 1.0], [4.0, 5.0]);
                    world.log("dist:" + dist.to_string());
                    let lerp = c::vec2_lerp([0.0, 0.0], [2.0, 2.0], 0.5);
                    world.log("lerp:" + lerp[0].to_string() + "," + lerp[1].to_string());
                    let wrapped = c::wrap_angle_pi(7.0);
                    world.log("wrap:" + wrapped.to_string());
                }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("load script with common import");
        let _ = host.update(0.016, true, None);
        assert!(host.last_error().is_none(), "script error: {:?}", host.last_error());
        let logs = host.drain_logs();
        let len_line = logs.iter().find(|l| l.starts_with("len:")).expect("len log");
        let len_val: f32 = len_line["len:".len()..].parse().expect("len parse");
        assert!(logs.iter().any(|l| l.contains("mv:0.5")), "move_toward_vec2 should advance toward target");
        assert!((len_val - 2.0).abs() < 0.05, "clamp_length should cap magnitude near 2, got {len_val}");
        assert!(logs.iter().any(|l| l.contains("ready1:false")), "cooldown should not be ready mid-way");
        assert!(logs.iter().any(|l| l.contains("ready2:true")), "cooldown should become ready after duration");
        let dir_vals: Vec<f32> = logs
            .iter()
            .find(|l| l.starts_with("dir:"))
            .expect("dir log")
            ["dir:".len()..]
                .split(',')
                .map(|v| v.parse::<f32>().expect("dir parse"))
                .collect();
        assert!((dir_vals[0]).abs() < 0.05 && (dir_vals[1] - 1.0).abs() < 0.05, "angle_to_vec should face up");
        let ang: f32 = logs
            .iter()
            .find(|l| l.starts_with("ang:"))
            .expect("ang log")
            ["ang:".len()..]
                .parse()
                .expect("ang parse");
        assert!((ang - std::f32::consts::FRAC_PI_2).abs() < 0.05, "vec_to_angle should read 90deg");
        let norm_vals: Vec<f32> = logs
            .iter()
            .find(|l| l.starts_with("norm:"))
            .expect("norm log")
            ["norm:".len()..]
                .split(',')
                .map(|v| v.parse::<f32>().expect("norm parse"))
                .collect();
        assert!(
            (norm_vals[0] - 0.6).abs() < 0.05 && (norm_vals[1] - 0.8).abs() < 0.05,
            "normalize should yield 0.6/0.8"
        );
        let dist: f32 = logs
            .iter()
            .find(|l| l.starts_with("dist:"))
            .expect("dist log")
            ["dist:".len()..]
                .parse()
                .expect("dist parse");
        assert!((dist - 5.0).abs() < 0.05, "vec2_distance should be 5");
        let lerp_vals: Vec<f32> = logs
            .iter()
            .find(|l| l.starts_with("lerp:"))
            .expect("lerp log")
            ["lerp:".len()..]
                .split(',')
                .map(|v| v.parse::<f32>().expect("lerp parse"))
                .collect();
        assert!(
            (lerp_vals[0] - 1.0).abs() < 0.05 && (lerp_vals[1] - 1.0).abs() < 0.05,
            "vec2_lerp should hit midpoint"
        );
        let wrapped: f32 = logs
            .iter()
            .find(|l| l.starts_with("wrap:"))
            .expect("wrap log")
            ["wrap:".len()..]
                .parse()
                .expect("wrap parse");
        assert!((wrapped - 0.7168).abs() < 0.05, "wrap_angle_pi should wrap into [-pi,pi]");
    }

    #[test]
    fn deterministic_ordering_sorts_worklist_by_path_then_entity() {
        let main = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let mut plugin = ScriptPlugin::new(main.path());
        plugin.set_deterministic_ordering(true);
        plugin.path_list = vec![Arc::from("b.rhai"), Arc::from("a.rhai")];
        plugin.behaviour_worklist = vec![
            (Entity::from_raw(2), 0, 10, false, false),
            (Entity::from_raw(1), 1, 5, false, false),
            (Entity::from_raw(3), 0, 2, false, false),
        ];
        plugin.sort_behaviour_worklist();
        let ordered: Vec<u64> = plugin.behaviour_worklist.iter().map(|(e, _, _, _, _)| e.to_bits()).collect();
        assert_eq!(
            ordered,
            vec![Entity::from_raw(1).to_bits(), Entity::from_raw(2).to_bits(), Entity::from_raw(3).to_bits()],
            "worklist should be sorted by path then entity id"
        );
    }

    #[test]
    fn timing_offenders_track_slowest_callbacks() {
        let script = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let host = ScriptHost::new(script.path());
        host.record_offender_entry("a.rhai", "process", Some(Entity::from_raw(2)), 1.0);
        host.record_offender_entry("b.rhai", "process", Some(Entity::from_raw(3)), 5.0);
        host.record_offender_entry("c.rhai", "ready", None, 3.0);
        host.record_offender_entry("d.rhai", "process", Some(Entity::from_raw(4)), 9.0);
        for i in 0..16 {
            host.record_offender_entry("spam.rhai", "process", None, i as f32);
        }
        let offenders = host.timing_offenders();
        assert!(!offenders.is_empty(), "offenders should capture samples");
        assert!(
            offenders.len() <= SCRIPT_OFFENDER_LIMIT,
            "offenders list should be truncated to the configured cap"
        );
        assert!(
            offenders.first().map(|o| o.last_ms).unwrap_or_default()
                >= offenders.last().map(|o| o.last_ms).unwrap_or_default(),
            "offenders should be sorted by descending runtime"
        );
        assert!(
            offenders.iter().any(|o| o.script_path == "d.rhai"),
            "largest offender should be retained after truncation"
        );
    }

    #[test]
    fn deterministic_ordering_sorts_command_queue() {
        let main = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let mut plugin = ScriptPlugin::new(main.path());
        plugin.set_deterministic_ordering(true);
        {
            let mut shared = plugin.host.shared.borrow_mut();
            shared.commands.push(ScriptCommand::SpawnPrefab { handle: 2, path: "b".into(), tag: None });
            shared.commands.push(ScriptCommand::SetPosition { handle: 1, position: Vec2::new(1.0, 0.0) });
            shared.commands.push(ScriptCommand::SpawnPrefab { handle: 0, path: "a".into(), tag: None });
        }
        let cmds = plugin.drain_host_commands();
        assert!(
            matches!(&cmds[..],
                [ScriptCommand::SetPosition { handle: 1, .. },
                 ScriptCommand::SpawnPrefab { handle: 0, path: p0, .. },
                 ScriptCommand::SpawnPrefab { handle: 2, path: p2, .. }] if p0 == "a" && p2 == "b"),
            "expected deterministic sort by kind then handle/path, got {:?}", cmds
        );
    }

    #[test]
    fn deterministic_mode_stabilizes_multi_entity_commands() {
        fn run_once(main_script: &NamedTempFile, behaviour_path: &str) -> Vec<ScriptCommand> {
            let mut plugin = ScriptPlugin::new(main_script.path());
            plugin.enable_deterministic_mode(999);
            let mut ecs = EcsWorld::new();
            let assets = AssetManager::new();
            ecs.world
                .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.to_string())));
            ecs.world
                .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.to_string())));
            plugin
                .run_behaviours(&mut ecs, &assets, 0.016, false)
                .expect("behaviours run");
            plugin.drain_host_commands()
        }

        let main = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let behaviour = write_script(
            r#"
                fn ready(world, entity) {
                    world.spawn_prefab("b_prefab");
                    world.spawn_prefab("a_prefab");
                    world.entity_set_position(entity, entity.to_float(), 0.0);
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let behaviour_path = behaviour.path().to_string_lossy().into_owned();
        let first = run_once(&main, &behaviour_path);
        let second = run_once(&main, &behaviour_path);
        let first_fmt: Vec<String> = first.iter().map(|c| format!("{c:?}")).collect();
        let second_fmt: Vec<String> = second.iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(first_fmt, second_fmt, "deterministic mode should produce stable command ordering across runs");
    }

    #[test]
    fn deterministic_mode_seeds_shared_rng() {
        let main = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let mut plugin = ScriptPlugin::new(main.path());
        plugin.enable_deterministic_mode(77);
        let mut world_a = ScriptWorld::new(plugin.host.shared.clone());
        let samples_a: Vec<_> = (0..3).map(|_| world_a.random_range(-1.0, 1.0)).collect();
        plugin.set_rng_seed(77);
        let mut world_b = ScriptWorld::new(plugin.host.shared.clone());
        let samples_b: Vec<_> = (0..3).map(|_| world_b.random_range(-1.0, 1.0)).collect();
        assert_eq!(samples_a, samples_b, "deterministic seed should stabilize RNG output");
    }

    #[test]
    fn command_quota_limits_per_instance_commands() {
        let main = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let behaviour = write_script(
            r#"
                fn ready(world, entity) {
                    world.spawn_prefab("first");
                    world.spawn_prefab("second");
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let behaviour_path = behaviour.path().to_string_lossy().into_owned();
        let mut plugin = ScriptPlugin::new(main.path());
        plugin.set_command_quota(Some(1));
        let mut ecs = EcsWorld::new();
        let assets = AssetManager::new();
        ecs.world.spawn((Transform::default(), ScriptBehaviour::new(behaviour_path)));
        plugin.populate_entity_snapshots(&mut ecs);
        plugin.cleanup_orphaned_instances(&mut ecs);
        plugin.host.begin_frame(0.016);
        plugin
            .run_behaviours(&mut ecs, &assets, 0.016, false)
            .expect("behaviours should run with quota");
        let cmds = plugin.drain_host_commands();
        assert_eq!(cmds.len(), 1, "quota should drop extra commands");
        let logs = plugin.host.drain_logs();
        assert!(
            logs.iter().any(|l| l.contains("quota")),
            "quota breach should log an informational message"
        );
    }

    #[test]
    fn callback_budget_marks_instances_errored() {
        let main = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let behaviour = write_script(
            r#"
                fn ready(world, entity) {
                    let i = 0;
                    while i < 2_000_000 {
                        i += 1;
                    }
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let behaviour_path = behaviour.path().to_string_lossy().into_owned();
        let mut plugin = ScriptPlugin::new(main.path());
        plugin.set_callback_budget_ms(Some(0.01));
        let mut ecs = EcsWorld::new();
        let assets = AssetManager::new();
        let entity = ecs
            .world
            .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())))
            .id();
        plugin.populate_entity_snapshots(&mut ecs);
        plugin.cleanup_orphaned_instances(&mut ecs);
        plugin.host.begin_frame(0.016);
        plugin
            .run_behaviours(&mut ecs, &assets, 0.016, false)
            .expect("behaviours should run with budget enforcement");
        let instance_id = ecs
            .world
            .get::<ScriptBehaviour>(entity)
            .expect("behaviour attached")
            .instance_id;
        assert!(instance_id != 0, "instance id should be assigned");
        let instance = plugin.host.instances.get(&instance_id).expect("instance cached");
        assert!(
            instance.errored,
            "budget overrun should mark instance errored; error was {:?}",
            plugin.host.last_error()
        );
        let err = plugin.last_error().unwrap_or("");
        assert!(err.contains("budget"), "expected budget error message, got {err:?}");
    }

    #[test]
    fn main_script_reloads_when_import_changes() {
        let mut module = Builder::new()
            .prefix("rhai_main_import_")
            .suffix(".rhai")
            .tempfile_in("assets/scripts")
            .expect("module file");
        write!(module.as_file_mut(), "fn value() {{ 1 }}").expect("write module v1");
        let module_name = module.path().file_stem().and_then(|s| s.to_str()).expect("module name");

        let script = write_script(&format!(
            r#"
                import "{module_name}" as m;
                fn init(world) {{}}
                fn update(world, dt) {{
                    world.log("v:" + m::value().to_string());
                }}
            "#
        ));
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("load main script");
        let _ = host.update(0.016, true, None);
        let first_logs = host.drain_logs();
        assert!(first_logs.iter().any(|l| l.contains("v:1")), "expected module v1 log, got {first_logs:?}");

        fs::write(module.path(), "fn value() { 2 }").expect("rewrite module v2");
        host.last_digest_check = Some(Instant::now() - Duration::from_millis(300));
        let _ = host.update(0.016, true, None);
        let second_logs = host.drain_logs();
        assert!(second_logs.iter().any(|l| l.contains("v:2")), "expected module v2 log, got {second_logs:?}");
    }

    #[test]
    fn rand_seed_is_deterministic_from_scriptworld() {
        let state_a = Rc::new(RefCell::new(SharedState::default()));
        let state_b = Rc::new(RefCell::new(SharedState::default()));
        let mut world_a = ScriptWorld::new(state_a);
        let mut world_b = ScriptWorld::new(state_b);
        world_a.rand_seed(999);
        world_b.rand_seed(999);
        let samples_a: Vec<_> = (0..4).map(|_| world_a.random_range(-5.0, 5.0)).collect();
        let samples_b: Vec<_> = (0..4).map(|_| world_b.random_range(-5.0, 5.0)).collect();
        assert_eq!(samples_a, samples_b, "seeded rand should be deterministic");
    }

    #[test]
    fn time_scale_applies_to_delta_tracking() {
        let mut host = ScriptHost::new("assets/scripts/main.rhai");
        let mut world = ScriptWorld::new(host.shared.clone());
        assert!((world.time_scale() - 1.0).abs() < 1e-6);
        host.begin_frame(0.25);
        assert!((world.delta_seconds() as f32 - 0.25).abs() < 1e-6);
        assert!((world.unscaled_time_seconds() as f32 - 0.25).abs() < 1e-6);
        assert!((world.time_seconds() as f32 - 0.25).abs() < 1e-6);
        world.set_time_scale(0.5);
        host.begin_frame(0.2);
        assert!((world.delta_seconds() as f32 - 0.1).abs() < 1e-6);
        assert!((world.unscaled_time_seconds() as f32 - 0.45).abs() < 1e-6);
        assert!((world.time_seconds() as f32 - 0.35).abs() < 1e-6);
    }

    #[test]
    fn timers_tick_and_repeat() {
        let shared = Rc::new(RefCell::new(SharedState::default()));
        let instance = Rc::new(RefCell::new(InstanceRuntimeState::default()));
        let mut world = ScriptWorld::with_instance(shared, instance.clone(), 1);
        assert!(world.timer_start("once", 0.1));
        instance.borrow_mut().tick_timers(0.05);
        let remaining = world.timer_remaining("once");
        assert!((remaining as f32 - 0.05).abs() < 1e-4);
        assert!(!world.timer_fired("once"));
        instance.borrow_mut().tick_timers(0.06);
        assert!(world.timer_fired("once"));
        assert!(!world.timer_fired("once"), "timer_fired should consume the fired flag");
        assert!(world.timer_start_repeat("loop", 0.1));
        instance.borrow_mut().tick_timers(0.12);
        assert!(world.timer_fired("loop"));
        instance.borrow_mut().tick_timers(0.11);
        assert!(world.timer_fired("loop"));
        assert!(world.timer_clear("loop"));
    }

    #[test]
    fn move_toward_clamps_step() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state);
        let step = world.move_toward(0.0, 10.0, 2.5);
        assert!((step as f32 - 2.5).abs() < 1e-4, "should move by max_delta");
        let final_step = world.move_toward(9.0, 10.0, 2.5);
        assert!((final_step as f32 - 10.0).abs() < 1e-4, "should clamp to target when within delta");
    }

    #[test]
    fn spawn_prefab_enqueues_command_with_handle() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state.clone());
        let handle = world.spawn_prefab("assets/scenes/example.json");
        assert!(handle >= 0);
        let cmds = state.borrow().commands.clone();
        assert!(matches!(
            cmds.as_slice(),
            [ScriptCommand::SpawnPrefab { handle: h, path, .. }] if *h == handle && path.contains("example.json")
        ));
    }

    #[test]
    fn spawn_template_enqueues_command_with_handle() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state.clone());
        let handle = world.spawn_template("enemy");
        assert!(handle >= 0);
        let cmds = state.borrow().commands.clone();
        assert!(matches!(
            cmds.as_slice(),
            [ScriptCommand::SpawnTemplate { handle: h, template, .. }] if *h == handle && template == "enemy"
        ));
    }

    #[test]
    fn set_position_rejects_invalid_handle() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state.clone());
        let ok = world.set_position(42, 1.0, 2.0);
        assert!(!ok, "invalid handle should return false");
        assert!(state.borrow().commands.is_empty(), "no commands should be queued");
        assert_eq!(state.borrow().invalid_handle_uses, 1);
    }

    #[test]
    fn despawn_safe_counts_dead_handle() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state.clone());
        let ok = world.despawn_safe(99);
        assert!(ok, "despawn_safe should no-op on dead handles");
        assert!(state.borrow().commands.is_empty(), "no commands should be queued");
        assert_eq!(state.borrow().despawn_dead_uses, 1);
    }

    #[test]
    fn handles_with_tag_filters_live_entities() {
        let mut host = ScriptHost::new("assets/scripts/main.rhai");
        let entity = Entity::from_raw(42);
        let handle: ScriptHandle = 12345;
        let mut snaps = HashMap::new();
        snaps.insert(
            entity,
            EntitySnapshot {
                translation: Vec2::ZERO,
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: None,
            },
        );
        host.set_entity_snapshots(snaps, 1.0, None, HashMap::new());
        host.register_spawn_result(handle, entity, Some("enemy".into()));
        let mut world = ScriptWorld::new(host.shared.clone());
        let handles = world.handles_with_tag("enemy");
        assert_eq!(handles.len(), 1, "expected a single live handle");
        assert_eq!(handles[0].clone_cast::<ScriptHandle>(), handle);
    }

    #[test]
    fn entity_tag_and_handle_return_spawn_metadata() {
        let mut host = ScriptHost::new("assets/scripts/main.rhai");
        let entity = Entity::from_raw(42);
        let handle: ScriptHandle = 12345;
        let mut snaps = HashMap::new();
        snaps.insert(
            entity,
            EntitySnapshot {
                translation: Vec2::ZERO,
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: None,
            },
        );
        host.set_entity_snapshots(snaps, 1.0, None, HashMap::new());
        host.register_spawn_result(handle, entity, Some("target".into()));

        let mut world = ScriptWorld::new(host.shared.clone());
        assert_eq!(world.entity_tag(entity.to_bits() as ScriptHandle), "target");

        let resolved = world.entity_handle(entity.to_bits() as ScriptHandle);
        assert!(!resolved.is_unit(), "expected handle metadata for script-spawned entity");
        assert_eq!(resolved.cast::<ScriptHandle>(), handle);
    }

    #[test]
    fn find_scene_entity_and_scene_id_resolve_from_snapshots() {
        let mut host = ScriptHost::new("assets/scripts/main.rhai");
        let entity = Entity::from_raw(7);
        let mut snaps = HashMap::new();
        snaps.insert(
            entity,
            EntitySnapshot {
                translation: Vec2::ZERO,
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: None,
            },
        );
        let mut scene_ids = HashMap::new();
        scene_ids.insert(entity, Arc::<str>::from("player"));
        host.set_entity_snapshots(snaps, 1.0, None, scene_ids);

        let mut world = ScriptWorld::new(host.shared.clone());
        let found = world.find_scene_entity("player");
        assert!(!found.is_unit(), "expected scene entity to resolve");
        let found_bits: ScriptHandle = found.try_cast().unwrap();
        assert_eq!(found_bits as u64, entity.to_bits());
        assert_eq!(world.entity_scene_id(found_bits), "player");

        assert!(world.find_scene_entity("").is_unit(), "empty lookup should return unit");
        assert!(world.find_scene_entity("missing").is_unit(), "unknown id should return unit");
    }

    #[test]
    fn spawn_prefab_safe_returns_unit_on_empty_path() {
        let mut world = ScriptWorld::new(Rc::new(RefCell::new(SharedState::default())));
        let result = world.spawn_prefab_safe("");
        assert!(result.is_unit(), "empty path should return unit");
        let state = world.state.borrow();
        assert_eq!(state.spawn_failures.get("prefab_empty_path").copied().unwrap_or(0), 1);
    }

    #[test]
    fn spawn_template_safe_returns_unit_on_empty_name() {
        let mut world = ScriptWorld::new(Rc::new(RefCell::new(SharedState::default())));
        let result = world.spawn_template_safe("");
        assert!(result.is_unit(), "empty name should return unit");
        let state = world.state.borrow();
        assert_eq!(state.spawn_failures.get("template_empty_name").copied().unwrap_or(0), 1);
    }

    #[test]
    fn spawn_sprite_safe_returns_unit_on_invalid_params() {
        let mut world = ScriptWorld::new(Rc::new(RefCell::new(SharedState::default())));
        let result = world.spawn_sprite_safe("atlas", "region", 0.0, 0.0, -1.0, 0.0, 0.0);
        assert!(result.is_unit(), "invalid params should return unit");
        assert!(world.state.borrow().commands.is_empty(), "no command should be queued on invalid spawn");
    }

    #[test]
    fn pending_spawn_handle_is_usable_before_materialize() {
        let mut host = ScriptHost::new("assets/scripts/main.rhai");
        let mut world = ScriptWorld::new(host.shared.clone());
        let handle_dyn = world.spawn_prefab_safe("assets/prefabs/enemy.json");
        let handle: ScriptHandle = handle_dyn.clone().try_cast::<ScriptHandle>().expect("handle");
        assert!(world.handle_is_alive(handle), "pending handle should be usable immediately");

        let after_spawn_commands = world.state.borrow().commands.len();
        let before_invalid = world.state.borrow().invalid_handle_uses;
        assert!(world.set_position(handle, 1.0, 2.0), "set_position should queue for pending handle");
        {
            let state = world.state.borrow();
            assert_eq!(state.invalid_handle_uses, before_invalid);
            assert_eq!(
                state.commands.len(),
                after_spawn_commands + 1,
                "set_position should enqueue for pending handle"
            );
        }

        let mut ecs_world = bevy_ecs::world::World::new();
        let entity = ecs_world.spawn_empty().id();
        let mut snaps = HashMap::new();
        snaps.insert(
            entity,
            EntitySnapshot {
                translation: Vec2::ZERO,
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: None,
            },
        );
        host.register_spawn_result(handle, entity, Some("enemy".into()));
        host.set_entity_snapshots(snaps, 1.0, None, HashMap::new());

        assert!(world.handle_is_alive(handle), "handle should become alive after spawn materializes");
        assert!(world.set_position(handle, 3.0, 4.0), "set_position should succeed on live handle");
        assert_eq!(
            world.state.borrow().commands.len(),
            after_spawn_commands + 2,
            "expected a queued command for pending + live handle"
        );
    }

    #[test]
    fn entity_snapshot_functions_read_cached_data() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut ecs_world = bevy_ecs::world::World::new();
        let entity = ecs_world.spawn_empty().id();
        {
            let mut shared = state.borrow_mut();
            let mut snaps = HashMap::new();
            snaps.insert(
                entity,
                EntitySnapshot {
                    translation: Vec2::new(1.0, 2.0),
                    rotation: 0.5,
                    scale: Vec2::new(3.0, 4.0),
                    velocity: Some(Vec2::new(5.0, 6.0)),
                    tint: Some(Vec4::new(0.1, 0.2, 0.3, 0.4)),
                    half_extents: None,
                },
            );
            shared.entity_snapshots = snaps;
        }
        let mut world = ScriptWorld::new(state);
        let bits = entity.to_bits() as ScriptHandle;
        let pos = world.entity_position(bits);
        assert_eq!(pos.len(), 2);
        let pos_x: FLOAT = pos[0].clone().try_cast().unwrap();
        assert!((pos_x - 1.0).abs() < 1e-5);
        assert_eq!(world.entity_rotation(bits), 0.5);
        let scale = world.entity_scale(bits);
        assert_eq!(scale.len(), 2);
        let vel = world.entity_velocity(bits);
        assert_eq!(vel.len(), 2);
        let tint = world.entity_tint(bits);
        assert_eq!(tint.len(), 4);
        let map = world.entity_snapshot(bits);
        assert!(map.contains_key("pos"));
        assert!(map.contains_key("rot"));
    }

    #[test]
    fn spatial_index_culls_to_aabb_bounds() {
        let mut index = ScriptSpatialIndex::default();
        let mut snaps = HashMap::new();
        let inside = Entity::from_raw(21);
        let outside = Entity::from_raw(22);
        snaps.insert(
            inside,
            EntitySnapshot {
                translation: Vec2::new(0.5, 0.5),
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: Some(Vec2::splat(0.25)),
            },
        );
        snaps.insert(
            outside,
            EntitySnapshot {
                translation: Vec2::new(5.0, 5.0),
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: Some(Vec2::splat(0.5)),
            },
        );
        index.rebuild(&snaps, 1.0);
        let hits = index.query_aabb(Vec2::new(-1.0, -1.0), Vec2::new(1.0, 1.0)).unwrap();
        assert!(hits.contains(&inside), "nearby entity should be returned");
        assert!(!hits.contains(&outside), "far entity should be culled");
    }

    #[test]
    fn spatial_index_reuses_physics_grid_and_backfills_missing() {
        let mut spatial = SpatialHash::new(1.0);
        spatial.begin_frame();
        let in_grid = Entity::from_raw(31);
        spatial.insert(in_grid, Vec2::ZERO, Vec2::splat(0.25));
        let missing = Entity::from_raw(32);
        let mut snaps = HashMap::new();
        snaps.insert(
            in_grid,
            EntitySnapshot {
                translation: Vec2::ZERO,
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: Some(Vec2::splat(0.25)),
            },
        );
        snaps.insert(
            missing,
            EntitySnapshot {
                translation: Vec2::new(1.2, 0.0),
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: Some(Vec2::splat(0.25)),
            },
        );
        let mut index = ScriptSpatialIndex::default();
        let mut cells = HashMap::new();
        for key in &spatial.active_cells {
            if let Some(list) = spatial.grid.get(key) {
                if !list.is_empty() {
                    cells.insert(*key, list.clone());
                }
            }
        }
        index.rebuild_with_spatial_hash(&snaps, Some(cells), 1.0);
        let hits = index.query_aabb(Vec2::new(-0.5, -0.5), Vec2::new(1.5, 0.5)).expect("index should be available");
        assert!(hits.contains(&in_grid), "entity from physics grid should be present");
        assert!(hits.contains(&missing), "entities missing from physics grid should be backfilled");
    }

    #[test]
    fn raycast_returns_closest_hit() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            let mut snaps = HashMap::new();
            let e1 = Entity::from_raw(1);
            let e2 = Entity::from_raw(2);
            snaps.insert(
                e1,
                EntitySnapshot {
                    translation: Vec2::new(5.0, 0.0),
                    rotation: 0.0,
                    scale: Vec2::ONE,
                    velocity: None,
                    tint: None,
                    half_extents: Some(Vec2::splat(1.0)),
                },
            );
            snaps.insert(
                e2,
                EntitySnapshot {
                    translation: Vec2::new(8.0, 0.0),
                    rotation: 0.0,
                    scale: Vec2::ONE,
                    velocity: None,
                    tint: None,
                    half_extents: Some(Vec2::splat(1.0)),
                },
            );
            let mut index = ScriptSpatialIndex::default();
            index.rebuild(&snaps, 0.5);
            shared.entity_snapshots = snaps;
            shared.spatial_index = index;
        }
        let mut world = ScriptWorld::new(state);
        let hit = world.raycast(0.0, 0.0, 1.0, 0.0, 20.0);
        let entity_val = hit.get("entity").unwrap().clone().try_cast::<ScriptHandle>().unwrap();
        let distance: FLOAT = hit.get("distance").unwrap().clone().try_cast().unwrap();
        assert_eq!(entity_val as u64, Entity::from_raw(1).to_bits());
        assert!((distance - 4.0).abs() < 1e-4, "expected hit at leading face");
    }

    #[test]
    fn raycast_respects_exclude_filter() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            let mut snaps = HashMap::new();
            let e1 = Entity::from_raw(1);
            let e2 = Entity::from_raw(2);
            snaps.insert(
                e1,
                EntitySnapshot {
                    translation: Vec2::new(5.0, 0.0),
                    rotation: 0.0,
                    scale: Vec2::ONE,
                    velocity: None,
                    tint: None,
                    half_extents: Some(Vec2::splat(1.0)),
                },
            );
            snaps.insert(
                e2,
                EntitySnapshot {
                    translation: Vec2::new(8.0, 0.0),
                    rotation: 0.0,
                    scale: Vec2::ONE,
                    velocity: None,
                    tint: None,
                    half_extents: Some(Vec2::splat(1.0)),
                },
            );
            let mut index = ScriptSpatialIndex::default();
            index.rebuild(&snaps, 1.0);
            shared.entity_snapshots = snaps;
            shared.spatial_index = index;
        }
        let mut world = ScriptWorld::new(state);
        let mut filters = Map::new();
        let mut exclude = Array::new();
        exclude.push(Dynamic::from(entity_to_rhai(Entity::from_raw(1))));
        filters.insert("exclude".into(), Dynamic::from(exclude));
        let hit = world.raycast_with_filters(0.0, 0.0, 1.0, 0.0, 20.0, filters);
        let entity_val = hit.get("entity").unwrap().clone().try_cast::<ScriptHandle>().unwrap();
        assert_eq!(entity_val as u64, Entity::from_raw(2).to_bits(), "excluded entity should be skipped");
    }

    #[test]
    fn raycast_prefers_closest_snapshot_over_rapier_hit() {
        let params = PhysicsParams { gravity: Vec2::ZERO, linear_damping: 0.0 };
        let bounds = WorldBounds { min: Vec2::splat(-10.0), max: Vec2::splat(10.0), thickness: 0.1 };
        let mut rapier = RapierState::new(&params, &bounds, Entity::from_raw(9000));
        let rapier_entity = Entity::from_raw(101);
        let (_body, collider) =
            rapier.spawn_dynamic_body(Vec2::new(6.0, 0.0), Vec2::splat(1.0), 0.0, Vec2::ZERO);
        rapier.register_collider_entity(collider, rapier_entity);
        rapier.step(0.0);

        let near_entity = Entity::from_raw(102);
        let mut snaps = HashMap::new();
        snaps.insert(
            near_entity,
            EntitySnapshot {
                translation: Vec2::new(3.0, 0.0),
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: Some(Vec2::splat(0.5)),
            },
        );
        let mut index = ScriptSpatialIndex::default();
        index.rebuild(&snaps, 0.5);

        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            shared.entity_snapshots = snaps;
            shared.spatial_index = index;
            shared.physics_ctx = Some(PhysicsQueryContext::from_state(&rapier));
        }
        let mut world = ScriptWorld::new(state);
        let hit = world.raycast(0.0, 0.0, 1.0, 0.0, 20.0);
        let entity_val: ScriptHandle = hit.get("entity").unwrap().clone().try_cast().unwrap();
        assert_eq!(entity_val as u64, near_entity.to_bits(), "closest snapshot hit should win");
        let distance: FLOAT = hit.get("distance").unwrap().clone().try_cast().unwrap();
        assert!((distance - 2.5).abs() < 1e-4, "expected entry face of snapshot AABB");
        assert!(hit.get("collider").is_none(), "snapshot-derived hit should not report a collider handle");
    }

    #[test]
    fn overlap_circle_collects_intersecting_entities() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            let mut snaps = HashMap::new();
            let inside = Entity::from_raw(3);
            snaps.insert(
                inside,
                EntitySnapshot {
                    translation: Vec2::new(1.0, 0.0),
                    rotation: 0.0,
                    scale: Vec2::ONE,
                    velocity: None,
                    tint: None,
                    half_extents: Some(Vec2::splat(0.5)),
                },
            );
            let mut index = ScriptSpatialIndex::default();
            index.rebuild(&snaps, 0.5);
            shared.entity_snapshots = snaps;
            shared.spatial_index = index;
        }
        let mut world = ScriptWorld::new(state);
        let hits = world.overlap_circle(0.0, 0.0, 2.0);
        assert_eq!(hits.len(), 1);
        let handle: ScriptHandle = hits[0].clone().try_cast().unwrap();
        assert_eq!(handle as u64, Entity::from_raw(3).to_bits());
    }

    #[test]
    fn overlap_circle_respects_include_filter() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let target = Entity::from_raw(7);
        let other = Entity::from_raw(8);
        {
            let mut shared = state.borrow_mut();
            let mut snaps = HashMap::new();
            snaps.insert(
                target,
                EntitySnapshot {
                    translation: Vec2::new(1.0, 0.0),
                    rotation: 0.0,
                    scale: Vec2::ONE,
                    velocity: None,
                    tint: None,
                    half_extents: Some(Vec2::splat(0.5)),
                },
            );
            snaps.insert(
                other,
                EntitySnapshot {
                    translation: Vec2::new(1.5, 0.0),
                    rotation: 0.0,
                    scale: Vec2::ONE,
                    velocity: None,
                    tint: None,
                    half_extents: Some(Vec2::splat(0.5)),
                },
            );
            let mut index = ScriptSpatialIndex::default();
            index.rebuild(&snaps, 0.5);
            shared.entity_snapshots = snaps;
            shared.spatial_index = index;
        }
        let mut filters = Map::new();
        let include = vec![Dynamic::from(entity_to_rhai(target))];
        filters.insert("include".into(), Dynamic::from(include));
        let mut world = ScriptWorld::new(state);
        let hits = world.overlap_circle_with_filters(0.0, 0.0, 2.0, filters);
        assert_eq!(hits.len(), 1, "include filter should drop extra hits");
        let handle: ScriptHandle = hits[0].clone().try_cast().unwrap();
        assert_eq!(handle as u64, target.to_bits());
    }

    #[test]
    fn overlap_circle_merges_snapshot_and_rapier_hits() {
        let params = PhysicsParams { gravity: Vec2::ZERO, linear_damping: 0.0 };
        let bounds = WorldBounds { min: Vec2::splat(-5.0), max: Vec2::splat(5.0), thickness: 0.1 };
        let mut rapier = RapierState::new(&params, &bounds, Entity::from_raw(9001));
        let rapier_entity = Entity::from_raw(111);
        let (_body, collider) =
            rapier.spawn_dynamic_body(Vec2::new(0.6, 0.0), Vec2::splat(0.25), 0.0, Vec2::ZERO);
        rapier.register_collider_entity(collider, rapier_entity);
        rapier.step(0.0);

        let snap_entity = Entity::from_raw(112);
        let mut snaps = HashMap::new();
        snaps.insert(
            rapier_entity,
            EntitySnapshot {
                translation: Vec2::new(0.6, 0.0),
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: Some(Vec2::splat(0.25)),
            },
        );
        snaps.insert(
            snap_entity,
            EntitySnapshot {
                translation: Vec2::new(-0.4, 0.0),
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: Some(Vec2::splat(0.25)),
            },
        );
        let mut index = ScriptSpatialIndex::default();
        index.rebuild(&snaps, 0.25);

        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            shared.entity_snapshots = snaps;
            shared.spatial_index = index;
            shared.physics_ctx = Some(PhysicsQueryContext::from_state(&rapier));
        }
        let mut world = ScriptWorld::new(state);
        let hits = world.overlap_circle_hits(0.0, 0.0, 1.0);
        assert_eq!(hits.len(), 2, "should combine rapier and snapshot overlaps");
        let mut info: HashMap<u64, (bool, bool)> = HashMap::new();
        for (entity_bits, has_collider, has_normal) in hits.into_iter().map(|d| {
            let map = d.try_cast::<Map>().unwrap();
            let entity_val: ScriptHandle = map.get("entity").unwrap().clone().try_cast().unwrap();
            let has_collider = map.get("collider").is_some();
            let has_normal = map.get("approx_normal").is_some();
            (entity_val as u64, has_collider, has_normal)
        }) {
            info.insert(entity_bits, (has_collider, has_normal));
        }
        let (snap_collider, snap_normal) =
            info.get(&snap_entity.to_bits()).expect("snapshot entity should be included");
        assert!(!*snap_collider, "snapshot hit should not report collider handle");
        assert!(*snap_normal, "snapshot hit should include an approximate normal");
        let (rapier_collider, rapier_normal) =
            info.get(&rapier_entity.to_bits()).expect("rapier entity should be included");
        assert!(*rapier_collider, "rapier hit should keep collider handle");
        assert!(*rapier_normal, "rapier hit should pick up snapshot translation for normals");
    }

    #[test]
    fn ast_cache_round_trips_and_refreshes_on_change() {
        let cache_dir = tempdir().expect("temp cache dir");
        let script = write_script(
            r#"
                fn init(world) { world.log("v1"); }
                fn update(world, dt) { }
            "#,
        );
        let script_path = script.path().to_path_buf();
        let mut host = ScriptHost::new(&script_path);
        host.set_ast_cache_dir(Some(cache_dir.path().to_path_buf()));
        host.force_reload(None).expect("initial load");

        let cache = ScriptAstCache::new(cache_dir.path().to_path_buf());
        let cache_path = cache.file_path(&host.script_path);
        assert!(cache_path.exists(), "cache file should be written after load");
        let bytes = std::fs::read(&cache_path).expect("read cache file");
        let cached: AstCacheFile = bincode::deserialize(&bytes).expect("deserialize cache");
        let first_digest = cached.script_digest;
        assert!(
            cached.script_digest != 0,
            "cache should capture the script digest even when there are no imports"
        );

        std::fs::write(
            &script_path,
            r#"
                fn init(world) { world.log("v2"); }
                fn update(world, dt) { }
            "#,
        )
        .expect("rewrite script");
        host.force_reload(None).expect("reload updated script");
        let bytes = std::fs::read(&cache_path).expect("read cache file after update");
        let cached_after: AstCacheFile = bincode::deserialize(&bytes).expect("deserialize updated cache");
        assert_ne!(cached_after.script_digest, first_digest, "cache should refresh when source changes");
    }

    #[test]
    fn raycast_uses_rapier_context_and_returns_normal_and_collider() {
        let params = PhysicsParams { gravity: Vec2::ZERO, linear_damping: 0.0 };
        let bounds = WorldBounds { min: Vec2::splat(-10.0), max: Vec2::splat(10.0), thickness: 0.1 };
        let mut rapier = RapierState::new(&params, &bounds, Entity::from_raw(9999));
        let target = Entity::from_raw(77);
        let (_body, collider) =
            rapier.spawn_dynamic_body(Vec2::new(5.0, 0.0), Vec2::splat(1.0), 0.0, Vec2::ZERO);
        rapier.register_collider_entity(collider, target);
        rapier.step(0.0);

        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            shared.physics_ctx = Some(PhysicsQueryContext::from_state(&rapier));
        }
        let mut world = ScriptWorld::new(state);
        let mut filters = Map::new();
        let include = vec![Dynamic::from(entity_to_rhai(target))];
        filters.insert("include".into(), Dynamic::from(include));
        let hit = world.raycast_with_filters(0.0, 0.0, 1.0, 0.0, 20.0, filters);
        let entity_val = hit.get("entity").unwrap().clone().try_cast::<ScriptHandle>().unwrap();
        assert_eq!(entity_val as u64, target.to_bits());
        let distance: FLOAT = hit.get("distance").unwrap().clone().try_cast().unwrap();
        assert!((distance - 4.0).abs() < 1e-4, "expected hit at leading face");
        let normal = hit.get("normal").expect("normal should be present").clone().try_cast::<Array>().unwrap();
        assert_eq!(normal.len(), 2);
        let nx: FLOAT = normal[0].clone().try_cast().unwrap();
        let ny: FLOAT = normal[1].clone().try_cast().unwrap();
        assert!((nx + 1.0).abs() < 1e-4 && ny.abs() < 1e-4, "expected outward normal on hit face");
        let collider_raw: ScriptHandle = hit.get("collider").unwrap().clone().try_cast().unwrap();
        let (idx, gen) = collider.into_raw_parts();
        let expected = ((gen as u64) << 32) | (idx as u64);
        assert_eq!(collider_raw as u64, expected, "should expose collider handle");
    }

    #[test]
    fn overlap_circle_uses_rapier_context_when_snapshots_empty() {
        let params = PhysicsParams { gravity: Vec2::ZERO, linear_damping: 0.0 };
        let bounds = WorldBounds { min: Vec2::splat(-5.0), max: Vec2::splat(5.0), thickness: 0.1 };
        let mut rapier = RapierState::new(&params, &bounds, Entity::from_raw(9998));
        let target = Entity::from_raw(88);
        let (_body, collider) =
            rapier.spawn_dynamic_body(Vec2::new(0.5, 0.0), Vec2::splat(0.25), 0.0, Vec2::ZERO);
        rapier.register_collider_entity(collider, target);
        rapier.step(0.0);

        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            shared.physics_ctx = Some(PhysicsQueryContext::from_state(&rapier));
        }
        let mut world = ScriptWorld::new(state);
        let hits = world.overlap_circle(0.0, 0.0, 1.0);
        assert_eq!(hits.len(), 1, "rapier overlap should return collider hit");
        let handle: ScriptHandle = hits[0].clone().try_cast().unwrap();
        assert_eq!(handle as u64, target.to_bits());
    }

    #[test]
    fn input_snapshot_reads_flags() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        {
            let mut shared = state.borrow_mut();
            shared.input_snapshot = Some(InputSnapshot {
                forward: true,
                right: true,
                boost: true,
                ctrl: false,
                left_mouse: true,
                right_mouse: false,
                cursor: Some(Vec2::new(10.0, 20.0)),
                cursor_world: Some(Vec2::new(-0.5, 0.75)),
                mouse_delta: Vec2::new(1.0, -2.0),
                wheel: 0.5,
                ..Default::default()
            });
        }
        let mut world = ScriptWorld::new(state);
        assert!(world.input_forward());
        assert!(world.input_right());
        assert!(world.input_boost());
        assert!(!world.input_ctrl());
        let cursor = world.input_cursor();
        assert_eq!(cursor.len(), 2);
        let cursor_world = world.input_cursor_world();
        assert_eq!(cursor_world.len(), 2);
        let wheel: FLOAT = world.input_wheel();
        assert!((wheel - 0.5).abs() < 1e-6);
    }

    #[test]
    fn behaviour_reload_occurs_when_asset_revision_is_stable() {
        let assets = AssetManager::new();
        let script = write_script(
            r#"
                fn ready(world, entity) { world.log("first"); }
                fn process(world, entity, dt) { }
            "#,
        );
        let path_str = script.path().to_string_lossy().into_owned();
        let mut host = ScriptHost::new(script.path());
        host.ensure_script_loaded(&path_str, Some(&assets)).expect("initial load");
        let first_digest = host.compiled_script(&path_str).expect("cached script").digest;

        let replacement = r#"
            fn ready(world, entity) { world.log("second"); }
            fn process(world, entity, dt) { }
        "#;
        fs::write(script.path(), replacement).expect("rewrite behaviour script");
        if let Some(compiled) = host.scripts.get_mut(&path_str) {
            compiled.last_checked = Some(Instant::now() - Duration::from_millis(300));
        }

        host.ensure_script_loaded(&path_str, Some(&assets)).expect("reload after change");
        let second_digest = host.compiled_script(&path_str).expect("reloaded script").digest;
        assert_ne!(
            first_digest, second_digest,
            "script digest should change even if asset manager revision stays constant"
        );
    }

    #[test]
    fn legacy_script_reload_uses_digest_when_asset_revision_is_stable() {
        let assets = AssetManager::new();
        let script = write_script(
            r#"
                let value = 1;
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(Some(&assets)).expect("initial load");
        assert_eq!(host.eval_repl("value").expect("read value").as_deref(), Some("1"));

        let replacement = r#"
            let value = 2;
            fn init(world) { }
            fn update(world, dt) { }
        "#;
        fs::write(script.path(), replacement).expect("rewrite legacy script");
        host.last_digest_check = Some(Instant::now() - Duration::from_millis(300));

        host.reload_if_needed(Some(&assets)).expect("reload after change");
        assert_eq!(host.eval_repl("value").expect("read value").as_deref(), Some("2"));
    }

    #[test]
    fn behaviour_reload_resets_instances_and_reruns_ready() {
        let main_script = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let behaviour_script = write_script(
            r#"
                let counter = 0;
                fn ready(world, entity) {
                    counter += 1;
                    world.log("ready v1 " + counter.to_string());
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();
        let mut host = ScriptHost::new(main_script.path());
        let mut world = bevy_ecs::world::World::new();
        let entity = world.spawn_empty().id();

        host.ensure_script_loaded(&behaviour_path, None).expect("initial load");
        let instance_id = host
            .create_instance(&behaviour_path, entity, None)
            .expect("create instance");

        host.call_instance_ready(instance_id).expect("initial ready call");
        let initial_logs = host.drain_logs();
        assert!(
            initial_logs.iter().any(|l| l.contains("ready v1 1")),
            "expected ready log before reload, got {initial_logs:?}"
        );

        let replacement = r#"
            let counter = 5;
            fn ready(world, entity) {
                counter += 1;
                world.log("ready v2 " + counter.to_string());
            }
            fn process(world, entity, dt) { }
        "#;
        fs::write(&behaviour_path, replacement).expect("rewrite behaviour script");

        host.ensure_script_loaded(&behaviour_path, None).expect("reload after change");
        host.call_instance_ready(instance_id).expect("ready should rerun after reload");
        let reloaded_logs = host.drain_logs();
        assert!(
            reloaded_logs.iter().any(|l| l.contains("ready v2 6")),
            "expected ready to rerun new script with reset state, got {reloaded_logs:?}"
        );
    }

    #[test]
    fn behaviour_reload_preserves_state_when_persistent_and_sets_hot_flag() {
        let main_script = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let behaviour_script = write_script(
            r#"
                fn ready(world, entity) {
                    let c = world.state_get("count");
                    if type_of(c) == "()" { c = 0; }
                    world.log("count:" + c.to_string());
                    world.state_set("count", c + 1);
                    if world.is_hot_reload() { world.log("hot"); }
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();
        let mut host = ScriptHost::new(main_script.path());
        let mut world = bevy_ecs::world::World::new();
        let entity = world.spawn_empty().id();

        host.ensure_script_loaded(&behaviour_path, None).expect("initial load");
        let instance_id = host
            .create_instance_preloaded(&behaviour_path, entity, true)
            .expect("create instance");

        host.call_instance_ready(instance_id).expect("initial ready call");
        let initial_logs = host.drain_logs();
        assert!(
            initial_logs.iter().any(|l| l.contains("count:0")) && !initial_logs.iter().any(|l| l.contains("hot")),
            "expected initial count and no hot reload flag, got {initial_logs:?}"
        );

        let replacement = r#"
            fn ready(world, entity) {
                let c = world.state_get("count");
                if type_of(c) == "()" { c = -1; }
                world.log("count:" + c.to_string());
                world.state_set("count", c + 1);
                if world.is_hot_reload() { world.log("hot"); }
            }
            fn process(world, entity, dt) { }
        "#;
        fs::write(&behaviour_path, replacement).expect("rewrite behaviour script");

        host.ensure_script_loaded(&behaviour_path, None).expect("reload after change");
        host.call_instance_ready(instance_id).expect("ready should rerun after reload");
        let reload_logs = host.drain_logs();
        assert!(
            reload_logs.iter().any(|l| l.contains("count:1")) && reload_logs.iter().any(|l| l.contains("hot")),
            "expected persisted state and hot flag on reload, got {reload_logs:?}"
        );
    }

    #[test]
    fn persisted_handles_are_stripped_before_serialization() {
        let mut shared = SharedState::default();
        shared.handle_nonce = 7;
        let handle: ScriptHandle = (((shared.handle_nonce as i64) << 32) | 3) as ScriptHandle;
        let mut inner = Map::new();
        inner.insert("h".into(), Dynamic::from(handle));
        let mut map = Map::new();
        map.insert("keep".into(), Dynamic::from(9 as ScriptHandle));
        map.insert("handle".into(), Dynamic::from(handle));
        map.insert(
            "nested".into(),
            Dynamic::from_array(vec![Dynamic::from(handle), Dynamic::from_map(inner)]),
        );

        let cleaned = sanitize_persisted_map(&map, PersistedHandlePolicy::DropAllHandles, &shared);
        assert!(cleaned.contains_key("keep"));
        assert!(!cleaned.contains_key("handle"), "handles should be removed from persisted state");
        let nested = cleaned
            .get("nested")
            .and_then(|v| v.clone().try_cast::<Array>())
            .unwrap_or_default();
        assert!(nested.is_empty(), "nested handle references should be stripped, got {nested:?}");
    }

    #[test]
    fn hot_reload_prunes_stale_handles_from_persistent_state() {
        let mut shared = SharedState::default();
        shared.handle_nonce = 11;
        let live_handle: ScriptHandle = (((shared.handle_nonce as i64) << 32) | 1) as ScriptHandle;
        let stale_handle: ScriptHandle = (((shared.handle_nonce as i64) << 32) | 2) as ScriptHandle;
        let mut world = bevy_ecs::world::World::new();
        let entity = world.spawn_empty().id();
        shared.handle_lookup.insert(live_handle, entity);
        shared.entity_snapshots.insert(
            entity,
            EntitySnapshot {
                translation: Vec2::ZERO,
                rotation: 0.0,
                scale: Vec2::ONE,
                velocity: None,
                tint: None,
                half_extents: None,
            },
        );

        let mut map = Map::new();
        map.insert("live".into(), Dynamic::from(live_handle));
        map.insert("stale".into(), Dynamic::from(stale_handle));

        let cleaned = sanitize_persisted_map(&map, PersistedHandlePolicy::DropStaleHandles, &shared);
        assert!(cleaned.contains_key("live"), "live handles should be preserved across hot reload");
        assert!(!cleaned.contains_key("stale"), "stale handles should be removed before reuse");
    }

    #[test]
    fn behaviour_reload_invokes_exit_before_ready() {
        let main_script = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let behaviour_script = write_script(
            r#"
                fn ready(world, entity) {
                    world.log("ready v1");
                }
                fn exit(world, entity) {
                    world.log("exit v1");
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();
        let mut host = ScriptHost::new(main_script.path());
        let mut world = bevy_ecs::world::World::new();
        let entity = world.spawn_empty().id();

        host.ensure_script_loaded(&behaviour_path, None).expect("initial load");
        let instance_id = host
            .create_instance_preloaded(&behaviour_path, entity, false)
            .expect("create instance");
        host.call_instance_ready(instance_id).expect("initial ready call");
        host.drain_logs();

        let replacement = r#"
            fn ready(world, entity) { world.log("ready v2"); }
            fn exit(world, entity) { world.log("exit v2"); }
            fn process(world, entity, dt) { }
        "#;
        fs::write(&behaviour_path, replacement).expect("rewrite behaviour script");

        host.ensure_script_loaded(&behaviour_path, None).expect("reload after change");
        host.call_instance_ready(instance_id).expect("ready rerun");
        let logs = host.drain_logs();
        assert!(
            logs.iter().position(|l| l.contains("exit v1")).is_some(),
            "expected exit from old script on reload, got {logs:?}"
        );
        assert!(
            logs.iter().position(|l| l.contains("ready v2")).is_some(),
            "expected ready from new script after reload, got {logs:?}"
        );
    }

    #[test]
    fn events_emit_and_listen_from_host() {
        let script = write_script(
            r#"
                fn init(world) {
                    world.listen("ping", "on_ping");
                    let _ok = world.emit("ping", #{ value: 7 });
                }
                fn on_ping(world, event) {
                    let payload = event["payload"]["value"];
                    world.log("ping:" + payload.to_string());
                }
                fn update(world, dt) { }
            "#,
        );
        let mut host = ScriptHost::new(script.path());
        host.force_reload(None).expect("load script");
        let _ = host.update(0.016, true, None);
        assert!(host.last_error().is_none(), "unexpected error: {:?}", host.last_error());
        let logs = host.drain_logs();
        assert!(
            logs.iter().any(|l| l.contains("ping:7")),
            "expected event handler to run once, got {logs:?}"
        );
    }

    #[test]
    fn event_queue_enforces_limit() {
        let state = Rc::new(RefCell::new(SharedState::default()));
        let mut world = ScriptWorld::new(state);
        for _ in 0..SCRIPT_EVENT_QUEUE_LIMIT {
            assert!(world.emit("overflow"), "queue should accept events until the cap");
        }
        assert!(!world.emit("overflow"), "queue cap should prevent additional events in the same frame");
    }

    #[test]
    fn entity_scoped_listeners_cleanup_on_remove() {
        let main = write_script(
            r#"
                fn init(world) { }
                fn update(world, dt) { }
            "#,
        );
        let emitter = write_script(
            r#"
                fn ready(world, entity) {
                    world.listen_for_entity("poke", entity, "on_poke");
                    world.emit("poke");
                }
                fn on_poke(world, event) {
                    world.log("emitter:" + event["listener"].to_string());
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let listener = write_script(
            r#"
                fn ready(world, entity) {
                    world.listen_for_entity("poke", entity, "on_poke");
                }
                fn on_poke(world, event) {
                    world.log("listener:" + event["listener"].to_string());
                }
                fn process(world, entity, dt) { }
            "#,
        );
        let emitter_path = emitter.path().to_string_lossy().into_owned();
        let listener_path = listener.path().to_string_lossy().into_owned();
        let mut plugin = ScriptPlugin::new(main.path());
        let mut ecs = EcsWorld::new();
        let assets = AssetManager::new();
        let emitter_entity = ecs
            .world
            .spawn((Transform::default(), ScriptBehaviour::new(emitter_path.clone())))
            .id();
        ecs.world
            .spawn((Transform::default(), ScriptBehaviour::new(listener_path.clone())));

        plugin.populate_entity_snapshots(&mut ecs);
        plugin.cleanup_orphaned_instances(&mut ecs);
        plugin.host.begin_frame(0.016);
        plugin
            .run_behaviours(&mut ecs, &assets, 0.016, false)
            .expect("behaviours run");
        plugin.host.dispatch_script_events();
        let logs = plugin.host.drain_logs();
        assert!(
            logs.iter().filter(|l| l.contains("emitter:")).count() == 1
                && logs.iter().all(|l| !l.contains("listener:")),
            "scoped listeners should receive only targeted events, got {logs:?}"
        );

        ecs.world.entity_mut(emitter_entity).remove::<ScriptBehaviour>();
        plugin.populate_entity_snapshots(&mut ecs);
        plugin.cleanup_orphaned_instances(&mut ecs);
        plugin.host.begin_frame(0.016);
        plugin
            .run_behaviours(&mut ecs, &assets, 0.016, false)
            .expect("behaviours run after removal");
        let mut world = ScriptWorld::new(plugin.host.shared.clone());
        assert!(world.emit_to("poke", emitter_entity.to_bits() as ScriptHandle));
        plugin.host.dispatch_script_events();
        let logs_after = plugin.host.drain_logs();
        assert!(
            logs_after.iter().all(|l| !l.contains("emitter:") && !l.contains("listener:")),
            "removed listener should not receive targeted events, got {logs_after:?}"
        );
    }

    #[test]
    fn script_timings_record_average_and_samples() {
        let mut state = SharedState::default();
        state.record_timing("ready", 1.0);
        state.record_timing("ready", 3.0);
        state.record_timing("process", 0.5);
        let mut summaries = state.timing_summaries();
        summaries.sort_by(|a, b| a.name.cmp(b.name));
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "process");
        assert!((summaries[0].average_ms - 0.5).abs() < 1e-4);
        assert_eq!(summaries[1].name, "ready");
        assert_eq!(summaries[1].samples, 2);
        assert!((summaries[1].average_ms - 2.0).abs() < 1e-4);
        assert!((summaries[1].max_ms - 3.0).abs() < 1e-4);
    }
}
