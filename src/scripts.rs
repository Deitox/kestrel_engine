use std::any::Any;
use std::cell::RefCell;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use crate::assets::AssetManager;
use crate::plugins::{EnginePlugin, PluginContext};
use anyhow::{anyhow, Context, Error, Result};
use glam::{Vec2, Vec4};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use rhai::module_resolvers::ModuleResolver;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map, Module, Scope, Shared, AST, FLOAT};

use bevy_ecs::prelude::{Component, Entity};
use crate::ecs::{Aabb, Tint, Transform, Velocity, WorldTransform};
use std::fmt::Write as FmtWrite;
use crate::input::Input;

pub type ScriptHandle = rhai::INT;

const SCRIPT_DIGEST_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const SCRIPT_IMPORT_ROOT: &str = "assets/scripts";

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
    pub mouse_delta: Vec2,
    pub wheel: f32,
}

#[derive(Component, Clone, Debug)]
pub struct ScriptBehaviour {
    pub script_path: String,
    pub instance_id: u64,
}

impl ScriptBehaviour {
    pub fn new(path: impl Into<String>) -> Self {
        Self { script_path: path.into(), instance_id: 0 }
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
    SpawnPrefab { handle: ScriptHandle, path: String },
    EntitySetPosition { entity: Entity, position: Vec2 },
    EntitySetRotation { entity: Entity, rotation: f32 },
    EntitySetScale { entity: Entity, scale: Vec2 },
    EntitySetTint { entity: Entity, tint: Option<Vec4> },
    EntitySetVelocity { entity: Entity, velocity: Vec2 },
    EntityDespawn { entity: Entity },
}

#[derive(Default)]
struct SharedState {
    next_handle: ScriptHandle,
    commands: Vec<ScriptCommand>,
    logs: Vec<String>,
    rng: Option<StdRng>,
    entity_snapshots: HashMap<Entity, EntitySnapshot>,
    input_snapshot: Option<InputSnapshot>,
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
}

impl ScriptWorld {
    fn new(state: Rc<RefCell<SharedState>>) -> Self {
        Self { state }
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

    fn raycast(&mut self, ox: FLOAT, oy: FLOAT, dx: FLOAT, dy: FLOAT, max_dist: FLOAT) -> Map {
        let origin = Vec2::new(ox as f32, oy as f32);
        let dir = Vec2::new(dx as f32, dy as f32);
        let max_dist = if max_dist.is_finite() && max_dist > 0.0 { max_dist as f32 } else { f32::INFINITY };
        let mut best: Option<(Entity, f32, Vec2)> = None;
        let state = self.state.borrow();
        for (entity, snap) in state.entity_snapshots.iter() {
            let half = snap.half_extents.unwrap_or_else(|| snap.scale * 0.5);
            if half.x <= 0.0 || half.y <= 0.0 {
                continue;
            }
            if let Some((dist, hit)) = Self::ray_aabb_2d(origin, dir, snap.translation, half) {
                if dist <= max_dist && dist.is_finite() && dist >= 0.0 {
                    match best {
                        Some((_, best_dist, _)) if dist >= best_dist => {}
                        _ => best = Some((*entity, dist, hit)),
                    }
                }
            }
        }
        let mut out = Map::new();
        if let Some((entity, dist, hit)) = best {
            out.insert("entity".into(), Dynamic::from(entity_to_rhai(entity)));
            out.insert("distance".into(), Dynamic::from(dist as FLOAT));
            out.insert("point".into(), Dynamic::from(Self::vec2_to_array(hit)));
        }
        out
    }

    fn overlap_circle(&mut self, cx: FLOAT, cy: FLOAT, radius: FLOAT) -> Array {
        let center = Vec2::new(cx as f32, cy as f32);
        let radius = radius.abs() as f32;
        if radius <= 0.0 || !radius.is_finite() {
            return Array::new();
        }
        let mut hits = Array::new();
        let state = self.state.borrow();
        let r2 = radius * radius;
        for (entity, snap) in state.entity_snapshots.iter() {
            let half = snap.half_extents.unwrap_or_else(|| snap.scale * 0.5);
            if half.x <= 0.0 || half.y <= 0.0 {
                continue;
            }
            let closest = snap.translation.clamp(center - half, center + half);
            if (closest - center).length_squared() <= r2 {
                hits.push(Dynamic::from(entity_to_rhai(*entity)));
            }
        }
        hits
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
        let mut state = self.state.borrow_mut();
        let handle = state.next_handle;
        state.next_handle = state.next_handle.saturating_add(1);
        state.commands.push(ScriptCommand::Spawn {
            handle,
            atlas: atlas.to_string(),
            region: region.to_string(),
            position: Vec2::new(x, y),
            scale,
            velocity: Vec2::new(vx, vy),
        });
        handle
    }

    fn set_velocity(&mut self, handle: ScriptHandle, vx: FLOAT, vy: FLOAT) -> bool {
        let vx = vx as f32;
        let vy = vy as f32;
        if !self.ensure_finite("set_velocity", &[vx, vy]) {
            return false;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetVelocity { handle, velocity: Vec2::new(vx, vy) });
        true
    }

    fn set_position(&mut self, handle: ScriptHandle, x: FLOAT, y: FLOAT) -> bool {
        let x = x as f32;
        let y = y as f32;
        if !self.ensure_finite("set_position", &[x, y]) {
            return false;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetPosition { handle, position: Vec2::new(x, y) });
        true
    }

    fn set_rotation(&mut self, handle: ScriptHandle, radians: FLOAT) -> bool {
        let radians = radians as f32;
        if !self.ensure_finite("set_rotation", &[radians]) {
            return false;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetRotation { handle, rotation: radians });
        true
    }

    fn set_scale(&mut self, handle: ScriptHandle, sx: FLOAT, sy: FLOAT) -> bool {
        let sx = sx as f32;
        let sy = sy as f32;
        if !self.ensure_finite("set_scale", &[sx, sy]) {
            return false;
        }
        let clamped = Vec2::new(sx.max(0.01), sy.max(0.01));
        self.state.borrow_mut().commands.push(ScriptCommand::SetScale { handle, scale: clamped });
        true
    }

    fn set_tint(&mut self, handle: ScriptHandle, r: FLOAT, g: FLOAT, b: FLOAT, a: FLOAT) -> bool {
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let a = a as f32;
        if !self.ensure_finite("set_tint", &[r, g, b, a]) {
            return false;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetTint { handle, tint: Some(Vec4::new(r, g, b, a)) });
        true
    }

    fn clear_tint(&mut self, handle: ScriptHandle) -> bool {
        self.state.borrow_mut().commands.push(ScriptCommand::SetTint { handle, tint: None });
        true
    }

    fn set_sprite_region(&mut self, handle: ScriptHandle, region: &str) -> bool {
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetSpriteRegion { handle, region: region.to_string() });
        true
    }

    fn despawn(&mut self, handle: ScriptHandle) -> bool {
        self.state.borrow_mut().commands.push(ScriptCommand::Despawn { handle });
        true
    }

    fn spawn_prefab(&mut self, path: &str) -> ScriptHandle {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return -1;
        }
        let mut state = self.state.borrow_mut();
        let handle = state.next_handle;
        state.next_handle = state.next_handle.saturating_add(1);
        state.commands.push(ScriptCommand::SpawnPrefab { handle, path: trimmed.to_string() });
        handle
    }

    fn entity_set_position(&mut self, entity_bits: ScriptHandle, x: FLOAT, y: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let pos = Vec2::new(x as f32, y as f32);
        if !self.ensure_finite("entity_set_position", &[pos.x, pos.y]) {
            return false;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::EntitySetPosition { entity, position: pos });
        true
    }

    fn entity_set_rotation(&mut self, entity_bits: ScriptHandle, radians: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let rot = radians as f32;
        if !self.ensure_finite("entity_set_rotation", &[rot]) {
            return false;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::EntitySetRotation { entity, rotation: rot });
        true
    }

    fn entity_set_scale(&mut self, entity_bits: ScriptHandle, sx: FLOAT, sy: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let sx = sx as f32;
        let sy = sy as f32;
        if !self.ensure_finite("entity_set_scale", &[sx, sy]) {
            return false;
        }
        let clamped = Vec2::new(sx.max(0.01), sy.max(0.01));
        self.state.borrow_mut().commands.push(ScriptCommand::EntitySetScale { entity, scale: clamped });
        true
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
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::EntitySetTint { entity, tint: Some(Vec4::new(r, g, b, a)) });
        true
    }

    fn entity_clear_tint(&mut self, entity_bits: ScriptHandle) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        self.state.borrow_mut().commands.push(ScriptCommand::EntitySetTint { entity, tint: None });
        true
    }

    fn entity_set_velocity(&mut self, entity_bits: ScriptHandle, vx: FLOAT, vy: FLOAT) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        let vx = vx as f32;
        let vy = vy as f32;
        if !self.ensure_finite("entity_set_velocity", &[vx, vy]) {
            return false;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::EntitySetVelocity {
            entity,
            velocity: Vec2::new(vx, vy),
        });
        true
    }

    fn entity_despawn(&mut self, entity_bits: ScriptHandle) -> bool {
        let entity = Entity::from_bits(entity_bits as u64);
        self.state.borrow_mut().commands.push(ScriptCommand::EntityDespawn { entity });
        true
    }

    fn set_auto_spawn_rate(&mut self, rate: FLOAT) {
        let rate = rate as f32;
        if !self.ensure_finite("set_auto_spawn_rate", &[rate]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetAutoSpawnRate { rate });
    }

    fn set_spawn_per_press(&mut self, count: i64) {
        let clamped = count.clamp(0, 10_000) as i32;
        self.state.borrow_mut().commands.push(ScriptCommand::SetSpawnPerPress { count: clamped });
    }

    fn set_emitter_rate(&mut self, rate: FLOAT) {
        let rate = rate as f32;
        if !self.ensure_finite("set_emitter_rate", &[rate]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterRate { rate: rate.max(0.0) });
    }

    fn set_emitter_spread(&mut self, spread: FLOAT) {
        let spread = spread as f32;
        if !self.ensure_finite("set_emitter_spread", &[spread]) {
            return;
        }
        let clamped = spread.clamp(0.0, std::f32::consts::PI);
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterSpread { spread: clamped });
    }

    fn set_emitter_speed(&mut self, speed: FLOAT) {
        let speed = speed as f32;
        if !self.ensure_finite("set_emitter_speed", &[speed]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterSpeed { speed: speed.max(0.0) });
    }

    fn set_emitter_lifetime(&mut self, lifetime: FLOAT) {
        let lifetime = lifetime as f32;
        if !self.ensure_finite("set_emitter_lifetime", &[lifetime]) {
            return;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterLifetime { lifetime: lifetime.max(0.05) });
    }

    fn set_emitter_start_color(&mut self, r: FLOAT, g: FLOAT, b: FLOAT, a: FLOAT) {
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let a = a as f32;
        if !self.ensure_finite("set_emitter_start_color", &[r, g, b, a]) {
            return;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterStartColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_end_color(&mut self, r: FLOAT, g: FLOAT, b: FLOAT, a: FLOAT) {
        let r = r as f32;
        let g = g as f32;
        let b = b as f32;
        let a = a as f32;
        if !self.ensure_finite("set_emitter_end_color", &[r, g, b, a]) {
            return;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterEndColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_start_size(&mut self, size: FLOAT) {
        let size = size as f32;
        if !self.ensure_finite("set_emitter_start_size", &[size]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterStartSize { size: size.max(0.01) });
    }

    fn set_emitter_end_size(&mut self, size: FLOAT) {
        let size = size as f32;
        if !self.ensure_finite("set_emitter_end_size", &[size]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterEndSize { size: size.max(0.01) });
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
}

pub struct ScriptHost {
    engine: Engine,
    ast: Option<AST>,
    scope: Scope<'static>,
    script_path: PathBuf,
    import_resolver: CachedModuleResolver,
    last_modified: Option<SystemTime>,
    last_len: Option<u64>,
    last_digest: Option<u64>,
    last_import_digests: HashMap<PathBuf, u64>,
    last_digest_check: Option<Instant>,
    last_asset_revision: Option<u64>,
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
    fn reset_import_resolver(&mut self) {
        self.engine.set_module_resolver(self.import_resolver.clone());
    }

    fn format_rhai_error(err: &EvalAltResult, script_path: &str, fn_name: &str) -> String {
        let pos = err.position();
        let location = match (pos.line(), pos.position()) {
            (Some(line), Some(col)) => format!("{script_path}:{line}:{col}"),
            (Some(line), None) => format!("{script_path}:{line}"),
            _ => script_path.to_string(),
        };
        format!("{location} in {fn_name}: {err}")
    }

    pub fn new(path: impl AsRef<Path>) -> Self {
        let mut engine = Engine::new();
        engine.set_fast_operators(true);
        let import_resolver = CachedModuleResolver::new(canonical_import_root());
        engine.set_module_resolver(import_resolver.clone());
        register_api(&mut engine);
        let shared = SharedState { next_handle: 1, ..Default::default() };
        Self {
            engine,
            ast: None,
            scope: Scope::new(),
            script_path: path.as_ref().to_path_buf(),
            import_resolver,
            last_modified: None,
            last_len: None,
            last_digest: None,
            last_import_digests: HashMap::new(),
            last_digest_check: None,
            last_asset_revision: None,
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

    pub fn last_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn set_entity_snapshots(&mut self, snapshots: HashMap<Entity, EntitySnapshot>) {
        let mut shared = self.shared.borrow_mut();
        shared.entity_snapshots = snapshots;
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
            let same_source = compiled.len == len && compiled.digest == digest;
            let imports_clean = imports_unchanged(&compiled.import_digests);
            if same_source && imports_clean {
                compiled.last_checked = Some(now);
                compiled.asset_revision = asset_rev.or(compiled.asset_revision);
                return Ok(());
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
        self.create_instance_preloaded(script_path, entity)
    }

    fn create_instance_preloaded(&mut self, script_path: &str, entity: Entity) -> Result<u64> {
        let compiled =
            self.scripts.get(script_path).ok_or_else(|| anyhow!("Script '{script_path}' not cached after load"))?;
        let id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.saturating_add(1);
        let mut scope = Scope::new();
        // Run global statements to initialize script-scoped state for this instance.
        if let Err(err) = self.engine.run_ast_with_scope(&mut scope, &compiled.ast) {
            return Err(anyhow!("Evaluating globals for '{script_path}': {err}"));
        }
        let instance = ScriptInstance {
            script_path: script_path.to_string(),
            entity,
            scope,
            has_ready_run: false,
            errored: false,
        };
        self.instances.insert(id, instance);
        Ok(id)
    }

    pub fn remove_instance(&mut self, id: u64) {
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

    fn load_script_source(&self, path: &str, assets: Option<&AssetManager>) -> Result<String> {
        if let Some(assets) = assets {
            return assets.read_text(path).with_context(|| format!("Reading script asset '{path}'"));
        }
        std::fs::read_to_string(path).with_context(|| format!("Reading script file '{path}'"))
    }

    fn load_script_source_with_revision(
        &self,
        path: &str,
        assets: Option<&AssetManager>,
    ) -> Result<(String, Option<u64>)> {
        if let Some(assets) = assets {
            let revision = Some(assets.revision());
            let source = assets.read_text(path).with_context(|| format!("Reading script asset '{path}'"))?;
            return Ok((source, revision));
        }
        let source = std::fs::read_to_string(path).with_context(|| format!("Reading script file '{path}'"))?;
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
        for instance in self.instances.values_mut().filter(|instance| instance.script_path == script_path) {
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
        let Some(instance) = self.instances.get_mut(&instance_id) else {
            return Ok(());
        };
        let Some(compiled) = self.scripts.get(&instance.script_path) else {
            return Ok(());
        };
        if !compiled.has_ready || instance.has_ready_run || instance.errored {
            return Ok(());
        }
        let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
        let world = ScriptWorld::new(self.shared.clone());
        match self
            .engine
            .call_fn::<Dynamic>(&mut instance.scope, &compiled.ast, "ready", (world, entity_int))
        {
            Ok(_) => {
                instance.has_ready_run = true;
                Ok(())
            }
            Err(err) => {
                instance.errored = true;
                let message = Self::format_rhai_error(err.as_ref(), &instance.script_path, "ready");
                self.error = Some(message.clone());
                Err(anyhow!(message))
            }
        }
    }

    fn call_instance_process(&mut self, instance_id: u64, dt: f32) -> Result<()> {
        let Some(instance) = self.instances.get_mut(&instance_id) else {
            return Ok(());
        };
        let Some(compiled) = self.scripts.get(&instance.script_path) else {
            return Ok(());
        };
        if !compiled.has_process || instance.errored {
            return Ok(());
        }
        let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
        let world = ScriptWorld::new(self.shared.clone());
        let dt_rhai: FLOAT = dt as FLOAT;
        match self.engine.call_fn::<Dynamic>(
            &mut instance.scope,
            &compiled.ast,
            "process",
            (world, entity_int, dt_rhai),
        ) {
            Ok(_) => Ok(()),
            Err(err) => {
                instance.errored = true;
                let message = Self::format_rhai_error(err.as_ref(), &instance.script_path, "process");
                self.error = Some(message.clone());
                Err(anyhow!(message))
            }
        }
    }

    fn call_instance_physics_process(&mut self, instance_id: u64, dt: f32) -> Result<()> {
        let Some(instance) = self.instances.get_mut(&instance_id) else {
            return Ok(());
        };
        let Some(compiled) = self.scripts.get(&instance.script_path) else {
            return Ok(());
        };
        if !compiled.has_physics_process || instance.errored {
            return Ok(());
        }
        let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
        let world = ScriptWorld::new(self.shared.clone());
        let dt_rhai: FLOAT = dt as FLOAT;
        match self.engine.call_fn::<Dynamic>(
            &mut instance.scope,
            &compiled.ast,
            "physics_process",
            (world, entity_int, dt_rhai),
        ) {
            Ok(_) => Ok(()),
            Err(err) => {
                instance.errored = true;
                let message = Self::format_rhai_error(err.as_ref(), &instance.script_path, "physics_process");
                self.error = Some(message.clone());
                Err(anyhow!(message))
            }
        }
    }

    fn call_instance_exit(&mut self, instance_id: u64) -> Result<()> {
        let Some(instance) = self.instances.get_mut(&instance_id) else {
            return Ok(());
        };
        let Some(compiled) = self.scripts.get(&instance.script_path) else {
            return Ok(());
        };
        if !compiled.has_exit || instance.errored {
            return Ok(());
        }
        let entity_int: ScriptHandle = entity_to_rhai(instance.entity);
        let world = ScriptWorld::new(self.shared.clone());
        match self.engine.call_fn::<Dynamic>(&mut instance.scope, &compiled.ast, "exit", (world, entity_int)) {
            Ok(_) => Ok(()),
            Err(err) => {
                let message = Self::format_rhai_error(err.as_ref(), &instance.script_path, "exit");
                self.error = Some(message.clone());
                Err(anyhow!(message))
            }
        }
    }

    pub fn update(&mut self, dt: f32, run_scripts: bool, assets: Option<&AssetManager>) {
        if let Err(err) = self.reload_if_needed(assets) {
            self.error = Some(err.to_string());
            return;
        }

        if !self.enabled {
            return;
        }
        if !run_scripts {
            return;
        }
        let dt_rhai: FLOAT = dt as FLOAT;
        let ast = match &self.ast {
            Some(ast) => ast,
            None => return,
        };

        {
            let mut shared = self.shared.borrow_mut();
            shared.commands.clear();
        }

        let world = ScriptWorld::new(self.shared.clone());
        if !self.initialized {
            match self.engine.call_fn::<()>(&mut self.scope, ast, "init", (world.clone(),)) {
                Ok(_) => {
                    self.initialized = true;
                    self.error = None;
                }
                Err(err) => {
                    if let EvalAltResult::ErrorFunctionNotFound(fn_sig, _) = err.as_ref() {
                        if fn_sig.starts_with("init") {
                            if self.function_exists_with_any_arity(ast, "init") {
                                let msg = format!(
                                    "{}: Script function 'init' has wrong signature; expected init(world).",
                                    self.script_path.display()
                                );
                                self.error = Some(msg);
                                return;
                            }
                            self.initialized = true;
                            return;
                        }
                    }
                    let msg =
                        Self::format_rhai_error(err.as_ref(), self.script_path.to_string_lossy().as_ref(), "init");
                    self.error = Some(msg);
                    return;
                }
            }
        }

        match self.engine.call_fn::<()>(&mut self.scope, ast, "update", (world, dt_rhai)) {
            Ok(_) => {
                self.error = None;
            }
            Err(err) => {
                if let EvalAltResult::ErrorFunctionNotFound(fn_sig, _) = err.as_ref() {
                    if fn_sig.starts_with("update") {
                        if self.function_exists_with_any_arity(ast, "update") {
                            let msg = format!(
                                "{}: Script function 'update' has wrong signature; expected update(world, dt: number).",
                                self.script_path.display()
                            );
                            self.error = Some(msg);
                        } else {
                            self.error = None;
                        }
                        return;
                    }
                }
                let msg = Self::format_rhai_error(
                    err.as_ref(),
                    self.script_path.to_string_lossy().as_ref(),
                    "update",
                );
                self.error = Some(msg);
            }
        }
    }

    pub fn drain_commands(&mut self) -> Vec<ScriptCommand> {
        self.shared.borrow_mut().commands.drain(..).collect()
    }

    pub fn drain_logs(&mut self) -> Vec<String> {
        self.shared.borrow_mut().logs.drain(..).collect()
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
        match eval {
            Ok(value) => {
                if value.is_unit() {
                    Ok(None)
                } else {
                    Ok(Some(value.to_string()))
                }
            }
            Err(err) => Err(anyhow!(err.to_string())),
        }
    }

    pub fn register_spawn_result(&mut self, handle: ScriptHandle, entity: Entity) {
        self.handle_map.insert(handle, entity);
    }

    pub fn resolve_handle(&self, handle: ScriptHandle) -> Option<Entity> {
        self.handle_map.get(&handle).copied()
    }

    pub fn forget_handle(&mut self, handle: ScriptHandle) {
        self.handle_map.remove(&handle);
    }

    pub fn forget_entity(&mut self, entity: Entity) {
        self.handle_map.retain(|_, value| *value != entity);
    }

    pub fn clear_handles(&mut self) {
        self.handle_map.clear();
    }

    pub fn handles_snapshot(&self) -> Vec<(ScriptHandle, Entity)> {
        self.handle_map.iter().map(|(handle, entity)| (*handle, *entity)).collect()
    }

    pub fn clear_instances(&mut self) {
        let ids: Vec<u64> = self.instances.keys().copied().collect();
        for id in ids {
            let _ = self.call_instance_exit(id);
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
        let ast = self.engine.compile(&source).with_context(|| "Compiling Rhai script")?;
        self.scope = Scope::new();
        self.engine
            .run_ast_with_scope(&mut self.scope, &ast)
            .map_err(|err| anyhow!("Evaluating script global statements: {err}"))?;
        self.last_import_digests = self.import_resolver.compute_import_digests(&source)?;
        self.last_modified = Some(modified);
        self.last_len = Some(len);
        self.last_digest = Some(hash_source(&source));
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

fn canonical_import_root() -> PathBuf {
    PathBuf::from(SCRIPT_IMPORT_ROOT).canonicalize().unwrap_or_else(|_| PathBuf::from(SCRIPT_IMPORT_ROOT))
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
    behaviour_worklist: Vec<(Entity, usize, u64)>,
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
        }
    }

    pub fn take_commands(&mut self) -> Vec<ScriptCommand> {
        self.commands.drain(..).collect()
    }

    pub fn take_logs(&mut self) -> Vec<String> {
        self.logs.drain(..).collect()
    }

    pub fn register_spawn_result(&mut self, handle: ScriptHandle, entity: Entity) {
        self.host.register_spawn_result(handle, entity);
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

    pub fn set_rng_seed(&mut self, seed: u64) {
        let mut shared = self.host.shared.borrow_mut();
        shared.rng = Some(StdRng::seed_from_u64(seed));
    }

    fn populate_entity_snapshots(&mut self, ecs: &mut crate::ecs::EcsWorld) {
        let mut snapshots = HashMap::new();
        let mut query = ecs.world.query::<(
            Entity,
            Option<&WorldTransform>,
            Option<&Transform>,
            Option<&Velocity>,
            Option<&Tint>,
            Option<&Aabb>,
        )>();
        for (entity, wt, transform, vel, tint, aabb) in query.iter(&ecs.world) {
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
        self.host.set_entity_snapshots(snapshots);
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
            ScriptCommand::EntitySetPosition { .. } => 19,
            ScriptCommand::EntitySetRotation { .. } => 20,
            ScriptCommand::EntitySetScale { .. } => 21,
            ScriptCommand::EntitySetTint { .. } => 22,
            ScriptCommand::EntitySetVelocity { .. } => 23,
            ScriptCommand::EntityDespawn { .. } => 24,
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
                (SpawnPrefab { handle: ha, path: pa }, SpawnPrefab { handle: hb, path: pb }) => {
                    ha.cmp(hb).then_with(|| pa.cmp(pb))
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
            |(entity_a, path_idx_a, instance_a), (entity_b, path_idx_b, instance_b)| {
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
            self.behaviour_worklist.push((entity, idx, behaviour.instance_id));
        }
        for (idx, path) in self.path_list.iter().enumerate() {
            if let Err(err) = self.host.ensure_script_loaded(path.as_ref(), Some(assets)) {
                self.host.set_error_with_details(&err);
                self.failed_path_scratch.insert(idx);
            }
        }
        self.sort_behaviour_worklist();
        for (entity, path_idx, mut instance_id) in self.behaviour_worklist.drain(..) {
            if self.failed_path_scratch.contains(&path_idx) {
                self.host.mark_entity_error(entity);
                continue;
            }
            let script_path = &self.path_list[path_idx];
            if instance_id == 0 {
                match self.host.create_instance_preloaded(script_path, entity) {
                    Ok(id) => {
                        instance_id = id;
                        self.id_updates.push((entity, id));
                    }
                    Err(err) => {
                        self.host.set_error_with_details(&err);
                        self.host.mark_entity_error(entity);
                        continue;
                    }
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
        Ok(())
    }

    fn cleanup_orphaned_instances(&mut self, ecs: &mut crate::ecs::EcsWorld) {
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
                behaviour.instance_id = 0;
                self.host.clear_entity_error(entity);
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
    }

    pub fn step_once(&mut self) {
        self.step_once = true;
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
        self.host.update(dt, run_scripts, Some(assets));
        if run_scripts && self.host.enabled() {
            self.run_behaviours(ecs, assets, dt, false)?;
        }
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
            let drained = self.drain_host_commands();
            self.commands.extend(drained);
            self.logs.extend(self.host.drain_logs());
            return Ok(());
        }
        if self.host.enabled() {
            self.run_behaviours(ecs, assets, dt, true)?;
        }
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
    engine.register_fn("spawn_sprite", ScriptWorld::spawn_sprite);
    engine.register_fn("set_velocity", ScriptWorld::set_velocity);
    engine.register_fn("set_position", ScriptWorld::set_position);
    engine.register_fn("set_rotation", ScriptWorld::set_rotation);
    engine.register_fn("set_scale", ScriptWorld::set_scale);
    engine.register_fn("set_tint", ScriptWorld::set_tint);
    engine.register_fn("clear_tint", ScriptWorld::clear_tint);
    engine.register_fn("set_sprite_region", ScriptWorld::set_sprite_region);
    engine.register_fn("despawn", ScriptWorld::despawn);
    engine.register_fn("spawn_prefab", ScriptWorld::spawn_prefab);
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
    engine.register_fn("entity_snapshot", ScriptWorld::entity_snapshot);
    engine.register_fn("entity_position", ScriptWorld::entity_position);
    engine.register_fn("entity_rotation", ScriptWorld::entity_rotation);
    engine.register_fn("entity_scale", ScriptWorld::entity_scale);
    engine.register_fn("entity_velocity", ScriptWorld::entity_velocity);
    engine.register_fn("entity_tint", ScriptWorld::entity_tint);
    engine.register_fn("raycast", ScriptWorld::raycast);
    engine.register_fn("overlap_circle", ScriptWorld::overlap_circle);
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
    engine.register_fn("input_mouse_delta", ScriptWorld::input_mouse_delta);
    engine.register_fn("input_wheel", ScriptWorld::input_wheel);
    engine.register_fn("log", ScriptWorld::log);
    engine.register_fn("rand_seed", ScriptWorld::rand_seed);
    engine.register_fn("rand", ScriptWorld::random_range);
    engine.register_fn("move_toward", ScriptWorld::move_toward);
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
    use crate::ecs::{EcsWorld, Transform};
    use tempfile::{Builder, NamedTempFile};

    fn write_script(contents: &str) -> NamedTempFile {
        let mut temp = NamedTempFile::new().expect("temp script");
        write!(temp, "{contents}").expect("write script");
        temp
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
        host.update(0.016, true, None);
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
        host.update(0.016, true, None);
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
        host.update(0.016, true, None);
        assert!(
            host.last_error().is_none(),
            "init should succeed, got {:?}",
            host.last_error()
        );
        host.update(0.016, true, None);
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
        host.update(0.016, true, None);
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
        host.update(0.016, true, None);
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
            (Entity::from_raw(2), 0, 10),
            (Entity::from_raw(1), 1, 5),
            (Entity::from_raw(3), 0, 2),
        ];
        plugin.sort_behaviour_worklist();
        let ordered: Vec<u64> = plugin.behaviour_worklist.iter().map(|(e, _, _)| e.to_bits()).collect();
        assert_eq!(
            ordered,
            vec![Entity::from_raw(1).to_bits(), Entity::from_raw(2).to_bits(), Entity::from_raw(3).to_bits()],
            "worklist should be sorted by path then entity id"
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
            shared.commands.push(ScriptCommand::SpawnPrefab { handle: 2, path: "b".into() });
            shared.commands.push(ScriptCommand::SetPosition { handle: 1, position: Vec2::new(1.0, 0.0) });
            shared.commands.push(ScriptCommand::SpawnPrefab { handle: 0, path: "a".into() });
        }
        let cmds = plugin.drain_host_commands();
        assert!(
            matches!(&cmds[..],
                [ScriptCommand::SetPosition { handle: 1, .. },
                 ScriptCommand::SpawnPrefab { handle: 0, path: p0 },
                 ScriptCommand::SpawnPrefab { handle: 2, path: p2 }] if p0 == "a" && p2 == "b"),
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
        host.update(0.016, true, None);
        let first_logs = host.drain_logs();
        assert!(first_logs.iter().any(|l| l.contains("v:1")), "expected module v1 log, got {first_logs:?}");

        fs::write(module.path(), "fn value() { 2 }").expect("rewrite module v2");
        host.last_digest_check = Some(Instant::now() - Duration::from_millis(300));
        host.update(0.016, true, None);
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
            [ScriptCommand::SpawnPrefab { handle: h, path }] if *h == handle && path.contains("example.json")
        ));
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
            shared.entity_snapshots = snaps;
        }
        let mut world = ScriptWorld::new(state);
        let hit = world.raycast(0.0, 0.0, 1.0, 0.0, 20.0);
        let entity_val = hit.get("entity").unwrap().clone().try_cast::<ScriptHandle>().unwrap();
        let distance: FLOAT = hit.get("distance").unwrap().clone().try_cast().unwrap();
        assert_eq!(entity_val as u64, Entity::from_raw(1).to_bits());
        assert!((distance - 4.0).abs() < 1e-4, "expected hit at leading face");
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
            shared.entity_snapshots = snaps;
        }
        let mut world = ScriptWorld::new(state);
        let hits = world.overlap_circle(0.0, 0.0, 2.0);
        assert_eq!(hits.len(), 1);
        let handle: ScriptHandle = hits[0].clone().try_cast().unwrap();
        assert_eq!(handle as u64, Entity::from_raw(3).to_bits());
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
}
