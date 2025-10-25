use crate::assets::AssetManager;
use crate::ecs::EcsWorld;
use crate::environment::EnvironmentRegistry;
use crate::events::GameEvent;
use crate::input::Input;
use crate::material_registry::MaterialRegistry;
use crate::mesh_registry::MeshRegistry;
use crate::renderer::Renderer;
use crate::time::Time;
use anyhow::{anyhow, bail, Context, Result};
use bevy_ecs::prelude::Entity;
use libloading::Library;
use serde::Deserialize;
use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io;
use std::mem;
use std::path::{Path, PathBuf};
use std::ptr;
use std::rc::Rc;

const DEFAULT_ENGINE_FEATURES: &[&str] = &[
    "core.app",
    "core.renderer",
    "core.ecs",
    "core.assets",
    "core.input",
    "core.time",
    "ui.egui",
    "scripts.rhai",
    "audio.rodio",
    "render.2d",
    "render.3d",
];

pub const ENGINE_PLUGIN_API_VERSION: u32 = 1;
pub const PLUGIN_ENTRY_SYMBOL: &[u8] = b"kestrel_plugin_entry\0";

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PluginHandle {
    data: *mut (),
    vtable: *mut (),
}

impl PluginHandle {
    pub const fn null() -> Self {
        Self { data: ptr::null_mut(), vtable: ptr::null_mut() }
    }

    pub fn is_null(&self) -> bool {
        self.data.is_null() || self.vtable.is_null()
    }

    pub unsafe fn from_box(plugin: Box<dyn EnginePlugin>) -> Self {
        Self::from_raw(Box::into_raw(plugin))
    }

    pub unsafe fn from_raw(raw: *mut dyn EnginePlugin) -> Self {
        let erased: (*mut (), *mut ()) = mem::transmute(raw);
        Self { data: erased.0, vtable: erased.1 }
    }

    pub unsafe fn into_raw(self) -> *mut dyn EnginePlugin {
        mem::transmute((self.data, self.vtable))
    }

    pub unsafe fn into_box(self) -> Box<dyn EnginePlugin> {
        Box::from_raw(self.into_raw())
    }
}

pub type PluginEntryFn = unsafe extern "C" fn() -> PluginExport;
pub type PluginCreateFn = unsafe extern "C" fn() -> PluginHandle;

#[repr(C)]
pub struct PluginExport {
    pub api_version: u32,
    pub create: PluginCreateFn,
}

#[derive(Debug, Default)]
pub struct FeatureRegistry {
    features: BTreeSet<String>,
}

impl FeatureRegistry {
    pub fn new() -> Self {
        Self { features: BTreeSet::new() }
    }

    pub fn with_engine_defaults() -> Self {
        let mut registry = Self::new();
        for feature in DEFAULT_ENGINE_FEATURES {
            registry.features.insert((*feature).to_string());
        }
        registry
    }

    pub fn register(&mut self, feature: impl Into<String>) {
        self.features.insert(feature.into());
    }

    pub fn register_all(&mut self, features: &[String]) {
        for feature in features {
            self.features.insert(feature.clone());
        }
    }

    pub fn contains(&self, feature: &str) -> bool {
        self.features.contains(feature)
    }

    pub fn missing<'a>(&self, required: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        required
            .into_iter()
            .filter(|feature| !self.features.contains(*feature))
            .map(|feature| feature.to_string())
            .collect()
    }

    pub fn all(&self) -> impl Iterator<Item = &String> {
        self.features.iter()
    }
}

#[derive(Clone)]
pub struct FeatureRegistryHandle(Rc<RefCell<FeatureRegistry>>);

impl FeatureRegistryHandle {
    fn new(inner: Rc<RefCell<FeatureRegistry>>) -> Self {
        Self(inner)
    }

    pub fn borrow(&self) -> Ref<'_, FeatureRegistry> {
        self.0.borrow()
    }

    pub fn borrow_mut(&self) -> RefMut<'_, FeatureRegistry> {
        self.0.borrow_mut()
    }
}

pub struct PluginContext<'a> {
    pub renderer: &'a mut Renderer,
    pub ecs: &'a mut EcsWorld,
    pub assets: &'a mut AssetManager,
    pub input: &'a mut Input,
    pub material_registry: &'a mut MaterialRegistry,
    pub mesh_registry: &'a mut MeshRegistry,
    pub environment_registry: &'a mut EnvironmentRegistry,
    pub time: &'a Time,
    pub selected_entity: Option<Entity>,
    feature_registry: FeatureRegistryHandle,
    emit_event: fn(&mut EcsWorld, GameEvent),
}

impl<'a> PluginContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        renderer: &'a mut Renderer,
        ecs: &'a mut EcsWorld,
        assets: &'a mut AssetManager,
        input: &'a mut Input,
        material_registry: &'a mut MaterialRegistry,
        mesh_registry: &'a mut MeshRegistry,
        environment_registry: &'a mut EnvironmentRegistry,
        time: &'a Time,
        emit_event: fn(&mut EcsWorld, GameEvent),
        feature_registry: FeatureRegistryHandle,
        selected_entity: Option<Entity>,
    ) -> Self {
        Self {
            renderer,
            ecs,
            assets,
            input,
            material_registry,
            mesh_registry,
            environment_registry,
            time,
            selected_entity,
            feature_registry,
            emit_event,
        }
    }

    pub fn features(&self) -> Ref<'_, FeatureRegistry> {
        self.feature_registry.borrow()
    }

    pub fn features_mut(&self) -> RefMut<'_, FeatureRegistry> {
        self.feature_registry.borrow_mut()
    }

    pub fn emit_event(&mut self, event: GameEvent) {
        (self.emit_event)(self.ecs, event);
    }

    pub fn emit_script_message(&mut self, message: impl Into<String>) {
        self.emit_event(GameEvent::ScriptMessage { message: message.into() });
    }

    pub fn renderer_api(&mut self) -> RendererApi<'_> {
        RendererApi { renderer: self.renderer }
    }

    pub fn assets_api(&mut self) -> AssetApi<'_> {
        AssetApi { assets: self.assets }
    }
}

pub struct RendererApi<'a> {
    renderer: &'a mut Renderer,
}

impl<'a> RendererApi<'a> {
    pub fn mark_shadow_settings_dirty(&mut self) {
        self.renderer.mark_shadow_settings_dirty();
    }
}

pub struct AssetApi<'a> {
    assets: &'a mut AssetManager,
}

impl<'a> AssetApi<'a> {
    pub fn retain_atlas(&mut self, key: &str, path: Option<&str>) -> Result<()> {
        self.assets.retain_atlas(key, path)
    }

    pub fn release_atlas(&mut self, key: &str) {
        self.assets.release_atlas(key);
    }
}

pub trait EnginePlugin: Any + 'static {
    fn name(&self) -> &'static str;

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn depends_on(&self) -> &'static [&'static str] {
        &[]
    }

    fn build(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        Ok(())
    }

    fn update(&mut self, _ctx: &mut PluginContext<'_>, _dt: f32) -> Result<()> {
        Ok(())
    }

    fn fixed_update(&mut self, _ctx: &mut PluginContext<'_>, _dt: f32) -> Result<()> {
        Ok(())
    }

    fn on_events(&mut self, _ctx: &mut PluginContext<'_>, _events: &[GameEvent]) -> Result<()> {
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        Ok(())
    }

    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

#[derive(Clone, Debug)]
pub enum PluginState {
    Loaded,
    Disabled(String),
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct PluginStatus {
    pub name: String,
    pub version: Option<String>,
    pub dynamic: bool,
    pub provides: Vec<String>,
    pub depends_on: Vec<String>,
    pub state: PluginState,
}

pub struct PluginManager {
    plugins: Vec<PluginSlot>,
    features: Rc<RefCell<FeatureRegistry>>,
    statuses: Vec<PluginStatus>,
    loaded_names: HashSet<String>,
}

struct PluginSlot {
    name: String,
    plugin: Box<dyn EnginePlugin>,
    provides: Vec<String>,
    depends_on: Vec<String>,
    _library: Option<Library>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            features: Rc::new(RefCell::new(FeatureRegistry::with_engine_defaults())),
            statuses: Vec::new(),
            loaded_names: HashSet::new(),
        }
    }
}

impl PluginManager {
    pub fn feature_handle(&self) -> FeatureRegistryHandle {
        FeatureRegistryHandle::new(self.features.clone())
    }

    pub fn register(&mut self, plugin: Box<dyn EnginePlugin>, ctx: &mut PluginContext<'_>) -> Result<()> {
        self.insert_plugin(plugin, None, false, Vec::new(), ctx)
    }

    pub fn register_with_features(
        &mut self,
        plugin: Box<dyn EnginePlugin>,
        provides: Vec<String>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<()> {
        self.insert_plugin(plugin, None, false, provides, ctx)
    }

    pub fn load_manifest(path: impl AsRef<Path>) -> Result<Option<PluginManifest>> {
        PluginManifest::from_path(path.as_ref())
    }

    pub fn load_dynamic_from_manifest(
        &mut self,
        manifest: &PluginManifest,
        ctx: &mut PluginContext<'_>,
    ) -> Result<Vec<String>> {
        let manifest_dir = manifest.path_parent().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        let mut loaded = Vec::new();
        for entry in manifest.entries() {
            if self.loaded_names.contains(&entry.name) {
                if let Some(slot) = self.plugins.iter().find(|slot| slot.name == entry.name) {
                    self.statuses.push(PluginStatus {
                        name: slot.name.clone(),
                        version: Some(slot.plugin.version().to_string()),
                        dynamic: true,
                        provides: slot.provides.clone(),
                        depends_on: slot.depends_on.clone(),
                        state: PluginState::Loaded,
                    });
                }
                continue;
            }
            if !entry.enabled {
                self.statuses.push(PluginStatus {
                    name: entry.name.clone(),
                    version: entry.version.clone(),
                    dynamic: true,
                    provides: entry.provides_features.clone(),
                    depends_on: Vec::new(),
                    state: PluginState::Disabled("disabled in manifest".to_string()),
                });
                continue;
            }
            if entry.path.trim().is_empty() {
                self.statuses.push(PluginStatus {
                    name: entry.name.clone(),
                    version: entry.version.clone(),
                    dynamic: true,
                    provides: entry.provides_features.clone(),
                    depends_on: Vec::new(),
                    state: PluginState::Failed("missing plugin path".to_string()),
                });
                continue;
            }
            let plugin_path = if Path::new(&entry.path).is_absolute() {
                PathBuf::from(&entry.path)
            } else {
                manifest_dir.join(&entry.path)
            };
            if !plugin_path.exists() {
                let msg = format!("artifact missing: {}", plugin_path.display());
                self.statuses.push(PluginStatus {
                    name: entry.name.clone(),
                    version: entry.version.clone(),
                    dynamic: true,
                    provides: entry.provides_features.clone(),
                    depends_on: Vec::new(),
                    state: PluginState::Disabled(msg.clone()),
                });
                eprintln!("[plugin:{}] {msg}", entry.name);
                continue;
            }
            match self.load_entry(entry, plugin_path, ctx) {
                Ok(name) => loaded.push(name),
                Err(err) => {
                    self.statuses.push(PluginStatus {
                        name: entry.name.clone(),
                        version: entry.version.clone(),
                        dynamic: true,
                        provides: entry.provides_features.clone(),
                        depends_on: Vec::new(),
                        state: PluginState::Failed(err.to_string()),
                    });
                }
            }
        }
        Ok(loaded)
    }

    pub fn record_builtin_disabled(&mut self, name: &str, reason: &str) {
        self.statuses.push(PluginStatus {
            name: name.to_string(),
            version: None,
            dynamic: false,
            provides: Vec::new(),
            depends_on: Vec::new(),
            state: PluginState::Disabled(reason.to_string()),
        });
    }

    pub fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for slot in &mut self.plugins {
            if let Err(err) = slot.plugin.update(ctx, dt) {
                eprintln!("[plugin:{}] update failed: {err:?}", slot.name);
            }
        }
    }

    pub fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for slot in &mut self.plugins {
            if let Err(err) = slot.plugin.fixed_update(ctx, dt) {
                eprintln!("[plugin:{}] fixed_update failed: {err:?}", slot.name);
            }
        }
    }

    pub fn handle_events(&mut self, ctx: &mut PluginContext<'_>, events: &[GameEvent]) {
        if events.is_empty() {
            return;
        }
        for slot in &mut self.plugins {
            if let Err(err) = slot.plugin.on_events(ctx, events) {
                eprintln!("[plugin:{}] event hook failed: {err:?}", slot.name);
            }
        }
    }

    pub fn shutdown(&mut self, ctx: &mut PluginContext<'_>) {
        for slot in &mut self.plugins {
            if let Err(err) = slot.plugin.shutdown(ctx) {
                eprintln!("[plugin:{}] shutdown failed: {err:?}", slot.name);
            }
        }
    }

    pub fn get<T: EnginePlugin + 'static>(&self) -> Option<&T> {
        self.plugins.iter().find_map(|slot| slot.plugin.as_any().downcast_ref::<T>())
    }

    pub fn get_mut<T: EnginePlugin + 'static>(&mut self) -> Option<&mut T> {
        self.plugins.iter_mut().find_map(|slot| slot.plugin.as_any_mut().downcast_mut::<T>())
    }

    pub fn statuses(&self) -> &[PluginStatus] {
        &self.statuses
    }

    pub fn clear_dynamic_statuses(&mut self) {
        self.statuses.retain(|status| !status.dynamic);
    }

    fn insert_plugin(
        &mut self,
        mut plugin: Box<dyn EnginePlugin>,
        library: Option<Library>,
        is_dynamic: bool,
        provides: Vec<String>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<()> {
        let name = plugin.name().to_string();
        if self.loaded_names.contains(&name) {
            bail!("plugin '{name}' already registered");
        }
        self.ensure_dependencies(plugin.depends_on(), &name)?;
        plugin.build(ctx)?;
        let version = plugin.version().to_string();
        let depends = plugin.depends_on().iter().map(|s| s.to_string()).collect::<Vec<_>>();
        {
            let mut registry = self.features.borrow_mut();
            registry.register_all(&provides);
        }
        self.loaded_names.insert(name.clone());
        self.statuses.push(PluginStatus {
            name: name.clone(),
            version: Some(version.clone()),
            dynamic: is_dynamic,
            provides: provides.clone(),
            depends_on: depends.clone(),
            state: PluginState::Loaded,
        });
        self.plugins.push(PluginSlot { name, plugin, provides, depends_on: depends, _library: library });
        Ok(())
    }

    fn ensure_dependencies(&self, deps: &[&str], plugin_name: &str) -> Result<()> {
        let missing: Vec<_> = deps.iter().copied().filter(|dep| !self.loaded_names.contains(*dep)).collect();
        if missing.is_empty() {
            Ok(())
        } else {
            bail!("plugin '{}' requires {:?}, but they are not loaded yet", plugin_name, missing)
        }
    }

    fn load_entry(
        &mut self,
        entry: &PluginManifestEntry,
        plugin_path: PathBuf,
        ctx: &mut PluginContext<'_>,
    ) -> Result<String> {
        if let Some(min_engine_api) = entry.min_engine_api {
            if ENGINE_PLUGIN_API_VERSION < min_engine_api {
                bail!(
                    "requires engine plugin API {min_engine_api}, current version is {ENGINE_PLUGIN_API_VERSION}"
                );
            }
        }

        if let Some(missing) = self.try_consume_requirements(&entry.requires_features) {
            bail!("missing required features: {}", missing.join(", "));
        }
        let library = unsafe {
            Library::new(&plugin_path)
                .with_context(|| format!("loading plugin library '{}'", plugin_path.display()))?
        };

        let entry_fn = unsafe {
            library.get::<PluginEntryFn>(PLUGIN_ENTRY_SYMBOL).with_context(|| {
                format!(
                    "resolving '{symbol}' in plugin '{path}'",
                    symbol = "kestrel_plugin_entry",
                    path = plugin_path.display()
                )
            })?
        };

        let export = unsafe { entry_fn() };
        drop(entry_fn);

        if export.api_version != ENGINE_PLUGIN_API_VERSION {
            bail!(
                "api mismatch: plugin targets v{}, engine exports v{}",
                export.api_version,
                ENGINE_PLUGIN_API_VERSION
            );
        }

        let handle = unsafe { (export.create)() };
        if handle.is_null() {
            bail!("plugin '{}' returned a null pointer", entry.name);
        }
        let plugin = unsafe { handle.into_box() };

        self.insert_plugin(plugin, Some(library), true, entry.provides_features.clone(), ctx)?;
        Ok(entry.name.clone())
    }

    fn try_consume_requirements(&self, requirements: &[String]) -> Option<Vec<String>> {
        let registry = self.features.borrow();
        let missing = registry.missing(requirements.iter().map(|s| s.as_str()));
        if missing.is_empty() {
            None
        } else {
            Some(missing)
        }
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        self.plugins.clear();
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PluginManifest {
    #[serde(default)]
    disable_builtins: Vec<String>,
    #[serde(default)]
    plugins: Vec<PluginManifestEntry>,
    #[serde(skip)]
    source_path: Option<PathBuf>,
}

impl PluginManifest {
    fn from_path(path: &Path) -> Result<Option<Self>> {
        match fs::read_to_string(path) {
            Ok(contents) => {
                let mut manifest: PluginManifest = serde_json::from_str(&contents)
                    .with_context(|| format!("parsing plugin manifest '{}'", path.display()))?;
                manifest.source_path = Some(path.to_path_buf());
                Ok(Some(manifest))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(anyhow!(err).context(format!("reading plugin manifest '{}'", path.display()))),
        }
    }

    pub fn disabled_builtins(&self) -> impl Iterator<Item = &str> {
        self.disable_builtins.iter().map(String::as_str)
    }

    pub fn entries(&self) -> &[PluginManifestEntry] {
        &self.plugins
    }

    fn path_parent(&self) -> Option<&Path> {
        self.source_path.as_deref().and_then(|p| p.parent())
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PluginManifestEntry {
    pub name: String,
    pub path: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub min_engine_api: Option<u32>,
    #[serde(default)]
    pub requires_features: Vec<String>,
    #[serde(default)]
    pub provides_features: Vec<String>,
}

fn default_enabled() -> bool {
    true
}
