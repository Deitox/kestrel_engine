use std::any::Any;
use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use crate::plugins::{EnginePlugin, PluginContext};
use anyhow::{anyhow, Context, Result};
use glam::{Vec2, Vec4};
use rand::Rng;
use rhai::{Dynamic, Engine, EvalAltResult, Scope, AST};

use bevy_ecs::prelude::Entity;

pub type ScriptHandle = rhai::INT;

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
        x: f32,
        y: f32,
        scale: f32,
        vx: f32,
        vy: f32,
    ) -> ScriptHandle {
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

    fn set_velocity(&mut self, handle: ScriptHandle, vx: f32, vy: f32) -> bool {
        if !self.ensure_finite("set_velocity", &[vx, vy]) {
            return false;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetVelocity { handle, velocity: Vec2::new(vx, vy) });
        true
    }

    fn set_position(&mut self, handle: ScriptHandle, x: f32, y: f32) -> bool {
        if !self.ensure_finite("set_position", &[x, y]) {
            return false;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetPosition { handle, position: Vec2::new(x, y) });
        true
    }

    fn set_rotation(&mut self, handle: ScriptHandle, radians: f32) -> bool {
        if !self.ensure_finite("set_rotation", &[radians]) {
            return false;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetRotation { handle, rotation: radians });
        true
    }

    fn set_scale(&mut self, handle: ScriptHandle, sx: f32, sy: f32) -> bool {
        if !self.ensure_finite("set_scale", &[sx, sy]) {
            return false;
        }
        let clamped = Vec2::new(sx.max(0.01), sy.max(0.01));
        self.state.borrow_mut().commands.push(ScriptCommand::SetScale { handle, scale: clamped });
        true
    }

    fn set_tint(&mut self, handle: ScriptHandle, r: f32, g: f32, b: f32, a: f32) -> bool {
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

    fn set_auto_spawn_rate(&mut self, rate: f32) {
        self.state.borrow_mut().commands.push(ScriptCommand::SetAutoSpawnRate { rate });
    }

    fn set_spawn_per_press(&mut self, count: i64) {
        let clamped = count.clamp(0, 10_000) as i32;
        self.state.borrow_mut().commands.push(ScriptCommand::SetSpawnPerPress { count: clamped });
    }

    fn set_emitter_rate(&mut self, rate: f32) {
        if !self.ensure_finite("set_emitter_rate", &[rate]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterRate { rate: rate.max(0.0) });
    }

    fn set_emitter_spread(&mut self, spread: f32) {
        if !self.ensure_finite("set_emitter_spread", &[spread]) {
            return;
        }
        let clamped = spread.clamp(0.0, std::f32::consts::PI);
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterSpread { spread: clamped });
    }

    fn set_emitter_speed(&mut self, speed: f32) {
        if !self.ensure_finite("set_emitter_speed", &[speed]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterSpeed { speed: speed.max(0.0) });
    }

    fn set_emitter_lifetime(&mut self, lifetime: f32) {
        if !self.ensure_finite("set_emitter_lifetime", &[lifetime]) {
            return;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterLifetime { lifetime: lifetime.max(0.05) });
    }

    fn set_emitter_start_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        if !self.ensure_finite("set_emitter_start_color", &[r, g, b, a]) {
            return;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterStartColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_end_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        if !self.ensure_finite("set_emitter_end_color", &[r, g, b, a]) {
            return;
        }
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterEndColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_start_size(&mut self, size: f32) {
        if !self.ensure_finite("set_emitter_start_size", &[size]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterStartSize { size: size.max(0.01) });
    }

    fn set_emitter_end_size(&mut self, size: f32) {
        if !self.ensure_finite("set_emitter_end_size", &[size]) {
            return;
        }
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterEndSize { size: size.max(0.01) });
    }

    fn random_range(&mut self, min: f32, max: f32) -> f32 {
        let mut lo = min;
        let mut hi = max;
        if !lo.is_finite() || !hi.is_finite() {
            self.log("random_range received non-finite bounds; returning 0.0");
            return 0.0;
        }
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        if (hi - lo).abs() <= f32::EPSILON {
            return lo;
        }
        rand::thread_rng().gen_range(lo..hi)
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
    error: Option<String>,
    enabled: bool,
    initialized: bool,
    shared: Rc<RefCell<SharedState>>,
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
            error: None,
            enabled: true,
            initialized: false,
            shared: Rc::new(RefCell::new(shared)),
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

    pub fn set_error_message(&mut self, msg: impl Into<String>) {
        self.error = Some(msg.into());
    }

    pub fn force_reload(&mut self) -> Result<()> {
        self.load_script().map(|_| ())
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
                    if matches!(err.as_ref(), EvalAltResult::ErrorFunctionNotFound(..)) {
                        self.initialized = true;
                    } else {
                        self.error = Some(err.to_string());
                        return;
                    }
                }
            }
        }

        match self.engine.call_fn::<()>(&mut self.scope, ast, "update", (world, dt)) {
            Ok(_) => {
                self.error = None;
            }
            Err(err) => {
                if matches!(err.as_ref(), EvalAltResult::ErrorFunctionNotFound(..)) {
                    self.error = None;
                } else {
                    self.error = Some(err.to_string());
                }
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
            let source = fs::read_to_string(&self.script_path)
                .with_context(|| format!("Reading {}", self.script_path.display()))?;
            let digest = hash_source(&source);
            if digest != previous_digest {
                self.load_script_from_source(source, modified, len)?;
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
        self.initialized = false;
        self.error = None;
        self.ast = Some(ast);
        Ok(self.ast.as_ref().expect("script AST set during load"))
    }
}

fn hash_source(source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
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

    fn update(&mut self, _ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
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
        if !self.paused {
            self.step_once = false;
        }
        self.commands.extend(self.host.drain_commands());
        self.logs.extend(self.host.drain_logs());
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        self.host.clear_handles();
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
    engine.register_fn("log", ScriptWorld::log);
    engine.register_fn("rand", ScriptWorld::random_range);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;
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
        let value = world.random_range(std::f32::consts::PI, std::f32::consts::PI);
        assert_eq!(value, std::f32::consts::PI);
    }
}
