use std::any::Any;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use crate::plugins::{EnginePlugin, PluginContext};
use anyhow::{anyhow, Context, Result};
use glam::{Vec2, Vec4};
use rand::Rng;
use rhai::{Dynamic, Engine, EvalAltResult, Scope, AST, FLOAT};

use bevy_ecs::prelude::{Component, Entity};

pub type ScriptHandle = rhai::INT;

const DIGEST_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct CompiledScript {
    pub ast: AST,
    pub has_ready: bool,
    pub has_process: bool,
    pub has_physics_process: bool,
}

#[derive(Clone)]
pub struct ScriptInstance {
    pub script_path: String,
    pub entity: Entity,
    pub scope: Scope<'static>,
    pub has_ready_run: bool,
    pub errored: bool,
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
    EntitySetPosition { entity: Entity, position: Vec2 },
    EntitySetRotation { entity: Entity, rotation: f32 },
    EntitySetScale { entity: Entity, scale: Vec2 },
    EntitySetVelocity { entity: Entity, velocity: Vec2 },
    EntityDespawn { entity: Entity },
}

#[derive(Default)]
struct SharedState {
    next_handle: ScriptHandle,
    commands: Vec<ScriptCommand>,
    logs: Vec<String>,
}

#[derive(Clone)]
pub struct ScriptWorld {
    state: Rc<RefCell<SharedState>>,
}

impl ScriptWorld {
    fn new(state: Rc<RefCell<SharedState>>) -> Self {
        Self { state }
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
        rand::thread_rng().gen_range(lo..hi) as FLOAT
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
    last_modified: Option<SystemTime>,
    last_len: Option<u64>,
    last_digest: Option<u64>,
    last_digest_check: Option<Instant>,
    error: Option<String>,
    enabled: bool,
    initialized: bool,
    shared: Rc<RefCell<SharedState>>,
    scripts: HashMap<String, CompiledScript>,
    instances: HashMap<u64, ScriptInstance>,
    next_instance_id: u64,
    handle_map: HashMap<ScriptHandle, Entity>,
}

impl ScriptHost {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let mut engine = Engine::new();
        engine.set_fast_operators(true);
        register_api(&mut engine);
        let shared = SharedState { next_handle: 1, ..Default::default() };
        Self {
            engine,
            ast: None,
            scope: Scope::new(),
            script_path: path.as_ref().to_path_buf(),
            last_modified: None,
            last_len: None,
            last_digest: None,
            last_digest_check: None,
            error: None,
            enabled: true,
            initialized: false,
            shared: Rc::new(RefCell::new(shared)),
            scripts: HashMap::new(),
            instances: HashMap::new(),
            next_instance_id: 1,
            handle_map: HashMap::new(),
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

    pub fn script_path(&self) -> &Path {
        &self.script_path
    }

    pub fn ensure_script_loaded(&mut self, path: &str) -> Result<()> {
        if self.scripts.contains_key(path) {
            return Ok(());
        }
        let compiled = self.load_external_script(path)?;
        self.scripts.insert(path.to_string(), compiled);
        Ok(())
    }

    pub fn compiled_script(&self, path: &str) -> Option<&CompiledScript> {
        self.scripts.get(path)
    }

    pub fn create_instance(&mut self, script_path: &str, entity: Entity) -> Result<u64> {
        self.ensure_script_loaded(script_path)?;
        let id = self.next_instance_id;
        self.next_instance_id = self.next_instance_id.saturating_add(1);
        let instance = ScriptInstance {
            script_path: script_path.to_string(),
            entity,
            scope: Scope::new(),
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
        self.error = Some(msg.into());
    }

    pub fn force_reload(&mut self) -> Result<()> {
        self.load_script().map(|_| ())
    }

    fn load_external_script(&self, path: &str) -> Result<CompiledScript> {
        let source =
            fs::read_to_string(path).with_context(|| format!("Reading script at path '{}'", path))?;
        let ast = self
            .engine
            .compile(&source)
            .with_context(|| format!("Compiling Rhai script '{}'", path))?;
        let (has_ready, has_process, has_physics_process) = detect_callbacks(&ast);
        Ok(CompiledScript { ast, has_ready, has_process, has_physics_process })
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
        let entity_int: FLOAT = entity_to_rhai(instance.entity) as FLOAT;
        let world = ScriptWorld::new(self.shared.clone());
        match self
            .engine
            .call_fn::<()>(&mut instance.scope, &compiled.ast, "ready", (world, entity_int))
        {
            Ok(_) => {
                instance.has_ready_run = true;
                Ok(())
            }
            Err(err) => {
                instance.errored = true;
                self.error = Some(err.to_string());
                Err(anyhow!(err.to_string()))
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
        let entity_int: FLOAT = entity_to_rhai(instance.entity) as FLOAT;
        let world = ScriptWorld::new(self.shared.clone());
        let dt_rhai: FLOAT = dt as FLOAT;
        match self
            .engine
            .call_fn::<()>(&mut instance.scope, &compiled.ast, "process", (world, entity_int, dt_rhai))
        {
            Ok(_) => Ok(()),
            Err(err) => {
                instance.errored = true;
                self.error = Some(err.to_string());
                Err(anyhow!(err.to_string()))
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
        let entity_int: FLOAT = entity_to_rhai(instance.entity) as FLOAT;
        let world = ScriptWorld::new(self.shared.clone());
        let dt_rhai: FLOAT = dt as FLOAT;
        match self.engine.call_fn::<()>(
            &mut instance.scope,
            &compiled.ast,
            "physics_process",
            (world, entity_int, dt_rhai),
        ) {
            Ok(_) => Ok(()),
            Err(err) => {
                instance.errored = true;
                self.error = Some(err.to_string());
                Err(anyhow!(err.to_string()))
            }
        }
    }

    pub fn update(&mut self, dt: f32, run_scripts: bool) {
        if let Err(err) = self.reload_if_needed() {
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
                                self.error = Some(
                                    "Script function 'init' has wrong signature; expected init(world).".to_string(),
                                );
                                return;
                            }
                            self.initialized = true;
                            return;
                        }
                    }
                    self.error = Some(err.to_string());
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
                            self.error = Some(
                                "Script function 'update' has wrong signature; expected update(world, dt: number)."
                                    .to_string(),
                            );
                        } else {
                            self.error = None;
                        }
                        return;
                    }
                }
                self.error = Some(err.to_string());
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
        self.reload_if_needed()?;
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
        self.instances.clear();
    }

    fn reload_if_needed(&mut self) -> Result<()> {
        let metadata = match fs::metadata(&self.script_path) {
            Ok(meta) => meta,
            Err(err) => {
                return Err(anyhow!("Script file not accessible: {err}"));
            }
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let len = metadata.len();

        let metadata_changed = self.ast.is_none()
            || self.last_modified.is_none_or(|prev| prev != modified)
            || self.last_len.is_none_or(|prev| prev != len);

        if metadata_changed {
            let source = fs::read_to_string(&self.script_path)
                .with_context(|| format!("Reading {}", self.script_path.display()))?;
            self.load_script_from_source(source, modified, len)?;
            return Ok(());
        }

        if let Some(previous_digest) = self.last_digest {
            let now = Instant::now();
            let should_check_digest = self
                .last_digest_check
                .map(|last| now.duration_since(last) >= DIGEST_POLL_INTERVAL)
                .unwrap_or(true);
            if should_check_digest {
                let source = fs::read_to_string(&self.script_path)
                    .with_context(|| format!("Reading {}", self.script_path.display()))?;
                let digest = hash_source(&source);
                self.last_digest_check = Some(now);
                if digest != previous_digest {
                    self.load_script_from_source(source, modified, len)?;
                }
            }
        }
        Ok(())
    }

    fn load_script(&mut self) -> Result<&AST> {
        let source = fs::read_to_string(&self.script_path)
            .with_context(|| format!("Reading {}", self.script_path.display()))?;
        let metadata = fs::metadata(&self.script_path)
            .with_context(|| format!("Reading {}", self.script_path.display()))?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let len = metadata.len();
        self.load_script_from_source(source, modified, len)
    }

    fn load_script_from_source(&mut self, source: String, modified: SystemTime, len: u64) -> Result<&AST> {
        let ast = self.engine.compile(&source).with_context(|| "Compiling Rhai script")?;
        self.scope = Scope::new();
        self.engine
            .run_ast_with_scope(&mut self.scope, &ast)
            .map_err(|err| anyhow!("Evaluating script global statements: {err}"))?;
        self.last_modified = Some(modified);
        self.last_len = Some(len);
        self.last_digest = Some(hash_source(&source));
        self.last_digest_check = Some(Instant::now());
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

fn detect_callbacks(ast: &AST) -> (bool, bool, bool) {
    let mut has_ready = false;
    let mut has_process = false;
    let mut has_physics_process = false;
    for func in ast.iter_functions() {
        let arity = func.params.len();
        match func.name.as_ref() {
            "ready" if arity == 2 => has_ready = true,
            "process" if arity == 3 => has_process = true,
            "physics_process" if arity == 3 => has_physics_process = true,
            _ => {}
        }
    }
    (has_ready, has_process, has_physics_process)
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
}

impl ScriptPlugin {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            host: ScriptHost::new(path),
            commands: Vec::new(),
            logs: Vec::new(),
            paused: false,
            step_once: false,
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

    fn run_behaviours(&mut self, ctx: &mut PluginContext<'_>, dt: f32, fixed_step: bool) -> Result<()> {
        let ecs = ctx.ecs_mut()?;
        let mut query = ecs.world.query::<(Entity, &mut ScriptBehaviour)>();
        for (entity, mut behaviour) in query.iter_mut(&mut ecs.world) {
            if behaviour.script_path.is_empty() {
                continue;
            }
            if let Err(err) = self.host.ensure_script_loaded(&behaviour.script_path) {
                self.host.set_error_message(err.to_string());
                continue;
            }
            if behaviour.instance_id == 0 {
                match self.host.create_instance(&behaviour.script_path, entity) {
                    Ok(id) => behaviour.instance_id = id,
                    Err(err) => {
                        self.host.set_error_message(err.to_string());
                        continue;
                    }
                }
            }
            let instance_id = behaviour.instance_id;
            if let Err(err) = self.host.call_instance_ready(instance_id) {
                eprintln!("[script] ready error for {}: {}", behaviour.script_path, err);
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
                    behaviour.script_path,
                    err
                );
            }
        }
        Ok(())
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

    pub fn step_once(&mut self) {
        self.step_once = true;
    }

    pub fn force_reload(&mut self) -> Result<()> {
        self.host.force_reload()
    }

    pub fn set_error_message(&mut self, msg: impl Into<String>) {
        self.host.set_error_message(msg);
    }

    pub fn last_error(&self) -> Option<&str> {
        self.host.last_error()
    }

    pub fn eval_repl(&mut self, source: &str) -> Result<Option<String>> {
        let result = self.host.eval_repl(source)?;
        self.commands.extend(self.host.drain_commands());
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
        self.host.update(dt, run_scripts);
        if run_scripts && self.host.enabled() {
            self.run_behaviours(ctx, dt, false)?;
        }
        if !self.paused {
            self.step_once = false;
        }
        self.commands.extend(self.host.drain_commands());
        self.logs.extend(self.host.drain_logs());
        Ok(())
    }

    fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        if self.paused {
            return Ok(());
        }
        if self.host.enabled() {
            self.run_behaviours(ctx, dt, true)?;
        }
        self.commands.extend(self.host.drain_commands());
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
    engine.register_fn("entity_set_velocity", ScriptWorld::entity_set_velocity);
    engine.register_fn("entity_despawn", ScriptWorld::entity_despawn);
    engine.register_fn("log", ScriptWorld::log);
    engine.register_fn("rand", ScriptWorld::random_range);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;
    use std::time::{Duration, Instant};
    use tempfile::NamedTempFile;

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
        host.force_reload().expect("load script");

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
        host.force_reload().expect("initial load");
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

        host.reload_if_needed().expect("reload check");
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
        host.force_reload().expect("initial load");
        host.update(0.016, true);
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
        host.force_reload().expect("initial load");
        host.update(0.016, true);
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
        host.force_reload().expect("initial load");
        let first_check = host.last_digest_check;
        host.reload_if_needed().expect("no reload needed");
        assert_eq!(host.last_digest_check, first_check, "digest check should not update without interval");
        host.last_digest_check = Some(Instant::now() - DIGEST_POLL_INTERVAL - Duration::from_millis(1));
        host.reload_if_needed().expect("reload after interval");
        assert!(host
            .last_digest_check
            .expect("digest check set")
            .duration_since(first_check.expect("initial digest check"))
            >= DIGEST_POLL_INTERVAL);
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
        host.force_reload().expect("load script");
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
        host.update(0.016, true);
        assert!(
            host.last_error().is_none(),
            "init should succeed, got {:?}",
            host.last_error()
        );
        host.update(0.016, true);
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
        host.ensure_script_loaded(&path_str).expect("load behaviour script");
        let compiled = host.compiled_script(&path_str).expect("script cached");
        assert!(compiled.has_ready);
        assert!(compiled.has_process);
        assert!(!compiled.has_physics_process);
        // second call should be a no-op
        host.ensure_script_loaded(&path_str).expect("cached load should succeed");
    }
}
