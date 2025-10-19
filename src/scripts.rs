use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Context, Result};
use glam::Vec2;
use rand::Rng;
use rhai::{Engine, EvalAltResult, Scope, AST};

use crate::assets::AssetManager;
use crate::ecs::EcsWorld;

#[derive(Clone, Copy)]
pub struct ScriptApi {
    ecs: *mut EcsWorld,
    assets: *const AssetManager,
}

unsafe impl Send for ScriptApi {}
unsafe impl Sync for ScriptApi {}

impl ScriptApi {
    pub fn new(ecs: &mut EcsWorld, assets: &AssetManager) -> Self {
        Self { ecs, assets }
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
    ) -> rhai::INT {
        let ecs = unsafe { &mut *self.ecs };
        let assets = unsafe { &*self.assets };
        match ecs.spawn_scripted_sprite(assets, atlas, region, Vec2::new(x, y), scale, Vec2::new(vx, vy)) {
            Ok(entity) => entity.to_bits() as rhai::INT,
            Err(err) => {
                eprintln!("[script] spawn_sprite error: {err}");
                -1
            }
        }
    }

    fn set_velocity(&mut self, entity_bits: rhai::INT, vx: f32, vy: f32) -> bool {
        let ecs = unsafe { &mut *self.ecs };
        match entity_from_bits(entity_bits) {
            Some(entity) => ecs.set_velocity(entity, Vec2::new(vx, vy)),
            None => false,
        }
    }

    fn set_position(&mut self, entity_bits: rhai::INT, x: f32, y: f32) -> bool {
        let ecs = unsafe { &mut *self.ecs };
        match entity_from_bits(entity_bits) {
            Some(entity) => ecs.set_translation(entity, Vec2::new(x, y)),
            None => false,
        }
    }

    fn despawn(&mut self, entity_bits: rhai::INT) -> bool {
        let ecs = unsafe { &mut *self.ecs };
        match entity_from_bits(entity_bits) {
            Some(entity) => ecs.despawn_entity(entity),
            None => false,
        }
    }

    fn random_range(&mut self, min: f32, max: f32) -> f32 {
        let mut rng = rand::thread_rng();
        rng.gen_range(min..max)
    }

    fn log(&mut self, message: &str) {
        println!("[script] {message}");
    }
}

fn entity_from_bits(bits: rhai::INT) -> Option<bevy_ecs::prelude::Entity> {
    if bits == -1 {
        None
    } else {
        Some(bevy_ecs::prelude::Entity::from_bits(bits as u64))
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
}

impl ScriptHost {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let mut engine = Engine::new();
        engine.set_fast_operators(true);
        register_api(&mut engine);
        Self {
            engine,
            ast: None,
            scope: Scope::new(),
            script_path: path.as_ref().to_path_buf(),
            last_modified: None,
            error: None,
            enabled: true,
            initialized: false,
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

    pub fn update(&mut self, ecs: &mut EcsWorld, assets: &AssetManager, dt: f32) {
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

        let api_init = ScriptApi::new(ecs, assets);
        if !self.initialized {
            match self.engine.call_fn::<()>(&mut self.scope, ast, "init", (api_init,)) {
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

        let api = ScriptApi::new(ecs, assets);
        match self.engine.call_fn::<()>(&mut self.scope, ast, "update", (api, dt)) {
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
        self.last_modified = fs::metadata(&self.script_path).ok().and_then(|meta| meta.modified().ok());
        self.initialized = false;
        self.error = None;
        self.ast = Some(ast);
        Ok(self.ast.as_ref().unwrap())
    }
}

fn register_api(engine: &mut Engine) {
    engine.register_type_with_name::<ScriptApi>("World");
    engine.register_fn("spawn_sprite", ScriptApi::spawn_sprite);
    engine.register_fn("set_velocity", ScriptApi::set_velocity);
    engine.register_fn("set_position", ScriptApi::set_position);
    engine.register_fn("despawn", ScriptApi::despawn);
    engine.register_fn("log", ScriptApi::log);
    engine.register_fn("rand", ScriptApi::random_range);
}
