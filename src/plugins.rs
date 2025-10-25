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
use std::collections::BTreeSet;
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
}

pub trait EnginePlugin: Any {
    fn name(&self) -> &'static str;

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

pub struct PluginManager {
    plugins: Vec<PluginSlot>,
    features: Rc<RefCell<FeatureRegistry>>,
}

pub struct PluginSummary<'a> {
    pub name: &'a str,
    pub provides_features: &'a [String],
    pub dynamic: bool,
}

struct PluginSlot {
    name: String,
    plugin: Box<dyn EnginePlugin>,
    provides_features: Vec<String>,
    origin: PluginOrigin,
}

enum PluginOrigin {
    BuiltIn,
    Dynamic(Library),
}

impl PluginOrigin {
    fn library(&self) -> Option<&Library> {
        match self {
            Self::Dynamic(lib) => Some(lib),
            Self::BuiltIn => None,
        }
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self { plugins: Vec::new(), features: Rc::new(RefCell::new(FeatureRegistry::with_engine_defaults())) }
    }
}

impl PluginManager {
    pub fn feature_handle(&self) -> FeatureRegistryHandle {
        FeatureRegistryHandle::new(self.features.clone())
    }

    pub fn register(&mut self, plugin: Box<dyn EnginePlugin>, ctx: &mut PluginContext<'_>) -> Result<()> {
        self.insert_plugin(plugin, PluginOrigin::BuiltIn, Vec::new(), ctx)
    }

    pub fn register_with_features(
        &mut self,
        plugin: Box<dyn EnginePlugin>,
        provides: Vec<String>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<()> {
        self.insert_plugin(plugin, PluginOrigin::BuiltIn, provides, ctx)
    }

    pub fn load_from_manifest<P: AsRef<Path>>(
        &mut self,
        path: P,
        ctx: &mut PluginContext<'_>,
    ) -> Result<Vec<String>> {
        let manifest_path = path.as_ref();
        let Some(manifest) = PluginManifest::from_path(manifest_path)? else {
            return Ok(Vec::new());
        };
        let manifest_dir =
            manifest_path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let mut loaded_names = Vec::new();
        for entry in manifest.plugins {
            if !entry.enabled {
                continue;
            }
            match self.load_entry(&entry, &manifest_dir, ctx) {
                Ok(name) => loaded_names.push(name),
                Err(err) => eprintln!("[plugin:{}] failed to load: {err:?}", entry.name),
            }
        }
        Ok(loaded_names)
    }

    pub fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for slot in &mut self.plugins {
            let name = slot.name.as_str();
            if let Err(err) = slot.plugin.update(ctx, dt) {
                eprintln!("[plugin:{name}] update failed: {err:?}");
            }
        }
    }

    pub fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for slot in &mut self.plugins {
            let name = slot.name.as_str();
            if let Err(err) = slot.plugin.fixed_update(ctx, dt) {
                eprintln!("[plugin:{name}] fixed_update failed: {err:?}");
            }
        }
    }

    pub fn handle_events(&mut self, ctx: &mut PluginContext<'_>, events: &[GameEvent]) {
        if events.is_empty() {
            return;
        }
        for slot in &mut self.plugins {
            let name = slot.name.as_str();
            if let Err(err) = slot.plugin.on_events(ctx, events) {
                eprintln!("[plugin:{name}] event hook failed: {err:?}");
            }
        }
    }

    pub fn shutdown(&mut self, ctx: &mut PluginContext<'_>) {
        for slot in &mut self.plugins {
            let name = slot.name.as_str();
            if let Err(err) = slot.plugin.shutdown(ctx) {
                eprintln!("[plugin:{name}] shutdown failed: {err:?}");
            }
        }
    }

    pub fn get<T: EnginePlugin + 'static>(&self) -> Option<&T> {
        self.plugins.iter().find_map(|slot| slot.plugin.as_any().downcast_ref::<T>())
    }

    pub fn get_mut<T: EnginePlugin + 'static>(&mut self) -> Option<&mut T> {
        self.plugins.iter_mut().find_map(|slot| slot.plugin.as_any_mut().downcast_mut::<T>())
    }

    pub fn plugin_summaries(&self) -> Vec<PluginSummary<'_>> {
        self.plugins
            .iter()
            .map(|slot| PluginSummary {
                name: slot.name.as_str(),
                provides_features: slot.provides_features.as_slice(),
                dynamic: slot.origin.library().is_some(),
            })
            .collect()
    }

    fn insert_plugin(
        &mut self,
        mut plugin: Box<dyn EnginePlugin>,
        origin: PluginOrigin,
        provides: Vec<String>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<()> {
        plugin.build(ctx)?;
        let name = plugin.name().to_string();
        {
            let mut registry = self.features.borrow_mut();
            registry.register_all(&provides);
        }
        self.plugins.push(PluginSlot { name, plugin, provides_features: provides, origin });
        Ok(())
    }

    fn load_entry(
        &mut self,
        entry: &PluginManifestEntry,
        manifest_dir: &Path,
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

        let plugin_path = if Path::new(&entry.path).is_absolute() {
            PathBuf::from(&entry.path)
        } else {
            manifest_dir.join(&entry.path)
        };

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

        self.insert_plugin(plugin, PluginOrigin::Dynamic(library), entry.provides_features.clone(), ctx)?;
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

#[derive(Debug, Deserialize)]
struct PluginManifest {
    #[serde(default)]
    plugins: Vec<PluginManifestEntry>,
}

#[derive(Debug, Deserialize)]
struct PluginManifestEntry {
    name: String,
    path: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    min_engine_api: Option<u32>,
    #[serde(default)]
    requires_features: Vec<String>,
    #[serde(default)]
    provides_features: Vec<String>,
}

impl PluginManifest {
    fn from_path(path: &Path) -> Result<Option<Self>> {
        match fs::read_to_string(path) {
            Ok(contents) => {
                let manifest = serde_json::from_str(&contents)
                    .with_context(|| format!("parsing plugin manifest '{}'", path.display()))?;
                Ok(Some(manifest))
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(anyhow!(err).context(format!("reading plugin manifest '{}'", path.display()))),
        }
    }
}

fn default_enabled() -> bool {
    true
}
