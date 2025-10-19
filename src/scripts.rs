use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use glam::{Vec2, Vec4};
use rand::Rng;
use rhai::{Engine, EvalAltResult, Scope, AST};

use bevy_ecs::prelude::Entity;

pub type ScriptHandle = rhai::INT;

#[derive(Debug, Clone)]
pub enum ScriptCommand {
    Spawn { handle: ScriptHandle, atlas: String, region: String, position: Vec2, scale: f32, velocity: Vec2 },
    SetVelocity { handle: ScriptHandle, velocity: Vec2 },
    SetPosition { handle: ScriptHandle, position: Vec2 },
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
}

#[derive(Clone)]
pub struct ScriptWorld {
    state: Rc<RefCell<SharedState>>,
}

impl ScriptWorld {
    fn new(state: Rc<RefCell<SharedState>>) -> Self {
        Self { state }
    }

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
        if scale <= 0.0 {
            return -1;
        }
        let mut state = self.state.borrow_mut();
        let handle = state.next_handle;
        state.next_handle -= 1;
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
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetVelocity { handle, velocity: Vec2::new(vx, vy) });
        true
    }

    fn set_position(&mut self, handle: ScriptHandle, x: f32, y: f32) -> bool {
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetPosition { handle, position: Vec2::new(x, y) });
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
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterRate { rate: rate.max(0.0) });
    }

    fn set_emitter_spread(&mut self, spread: f32) {
        let clamped = spread.clamp(0.0, std::f32::consts::PI);
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterSpread { spread: clamped });
    }

    fn set_emitter_speed(&mut self, speed: f32) {
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterSpeed { speed: speed.max(0.0) });
    }

    fn set_emitter_lifetime(&mut self, lifetime: f32) {
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterLifetime { lifetime: lifetime.max(0.05) });
    }

    fn set_emitter_start_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterStartColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_end_color(&mut self, r: f32, g: f32, b: f32, a: f32) {
        self.state
            .borrow_mut()
            .commands
            .push(ScriptCommand::SetEmitterEndColor { color: Vec4::new(r, g, b, a) });
    }

    fn set_emitter_start_size(&mut self, size: f32) {
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterStartSize { size: size.max(0.01) });
    }

    fn set_emitter_end_size(&mut self, size: f32) {
        self.state.borrow_mut().commands.push(ScriptCommand::SetEmitterEndSize { size: size.max(0.01) });
    }

    fn random_range(&mut self, min: f32, max: f32) -> f32 {
        rand::thread_rng().gen_range(min..max)
    }

    fn log(&mut self, message: &str) {
        println!("[script] {message}");
    }
}

pub struct ScriptHost {
    engine: Engine,
    ast: Option<AST>,
    scope: Scope<'static>,
    script_path: PathBuf,
    last_modified: Option<SystemTime>,
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
        let mut shared = SharedState::default();
        shared.next_handle = -1;
        Self {
            engine,
            ast: None,
            scope: Scope::new(),
            script_path: path.as_ref().to_path_buf(),
            last_modified: None,
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

    pub fn update(&mut self, dt: f32) {
        if let Err(err) = self.reload_if_needed() {
            self.error = Some(err.to_string());
            return;
        }

        if !self.enabled {
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

    pub fn register_spawn_result(&mut self, handle: ScriptHandle, entity: Entity) {
        self.handle_map.insert(handle, entity);
    }

    pub fn resolve_handle(&self, handle: ScriptHandle) -> Option<Entity> {
        if handle >= 0 {
            Some(Entity::from_bits(handle as u64))
        } else {
            self.handle_map.get(&handle).copied()
        }
    }

    pub fn forget_handle(&mut self, handle: ScriptHandle) {
        self.handle_map.remove(&handle);
    }

    fn reload_if_needed(&mut self) -> Result<()> {
        let metadata = match fs::metadata(&self.script_path) {
            Ok(meta) => meta,
            Err(err) => {
                return Err(anyhow!("Script file not accessible: {err}"));
            }
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if self.ast.is_none() || self.last_modified.map_or(true, |prev| modified > prev) {
            self.load_script()?;
        }
        Ok(())
    }

    fn load_script(&mut self) -> Result<&AST> {
        let source = fs::read_to_string(&self.script_path)
            .with_context(|| format!("Reading {}", self.script_path.display()))?;
        let ast = self.engine.compile(source).with_context(|| "Compiling Rhai script")?;
        self.scope = Scope::new();
        self.engine
            .run_ast_with_scope(&mut self.scope, &ast)
            .map_err(|err| anyhow!("Evaluating script global statements: {err}"))?;
        self.last_modified = fs::metadata(&self.script_path).ok().and_then(|meta| meta.modified().ok());
        self.initialized = false;
        self.error = None;
        self.ast = Some(ast);
        Ok(self.ast.as_ref().unwrap())
    }
}

fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<ScriptWorld>("World");
    engine.register_fn("spawn_sprite", ScriptWorld::spawn_sprite);
    engine.register_fn("set_velocity", ScriptWorld::set_velocity);
    engine.register_fn("set_position", ScriptWorld::set_position);
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
