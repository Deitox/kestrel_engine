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
use bitflags::bitflags;
use libloading::Library;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct CapabilityFlags: u32 {
        const RENDERER = 1 << 0;
        const ECS = 1 << 1;
        const ASSETS = 1 << 2;
        const INPUT = 1 << 3;
        const SCRIPTS = 1 << 4;
        const ANALYTICS = 1 << 5;
        const TIME = 1 << 6;
        const EVENTS = 1 << 7;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PluginCapability {
    Renderer,
    Ecs,
    Assets,
    Input,
    Scripts,
    Analytics,
    Time,
    Events,
    All,
}

impl PluginCapability {
    fn flag(self) -> CapabilityFlags {
        match self {
            PluginCapability::Renderer => CapabilityFlags::RENDERER,
            PluginCapability::Ecs => CapabilityFlags::ECS,
            PluginCapability::Assets => CapabilityFlags::ASSETS,
            PluginCapability::Input => CapabilityFlags::INPUT,
            PluginCapability::Scripts => CapabilityFlags::SCRIPTS,
            PluginCapability::Analytics => CapabilityFlags::ANALYTICS,
            PluginCapability::Time => CapabilityFlags::TIME,
            PluginCapability::Events => CapabilityFlags::EVENTS,
            PluginCapability::All => CapabilityFlags::all(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PluginCapability::Renderer => "renderer",
            PluginCapability::Ecs => "ecs",
            PluginCapability::Assets => "assets",
            PluginCapability::Input => "input",
            PluginCapability::Scripts => "scripts",
            PluginCapability::Analytics => "analytics",
            PluginCapability::Time => "time",
            PluginCapability::Events => "events",
            PluginCapability::All => "all",
        }
    }
}

impl From<&[PluginCapability]> for CapabilityFlags {
    fn from(list: &[PluginCapability]) -> Self {
        if list.is_empty() {
            default_capability_flags()
        } else if list.iter().any(|cap| matches!(cap, PluginCapability::All)) {
            CapabilityFlags::all()
        } else {
            list.iter().fold(CapabilityFlags::empty(), |acc, cap| acc | cap.flag())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginTrust {
    Full,
    Isolated,
}

impl Default for PluginTrust {
    fn default() -> Self {
        PluginTrust::Full
    }
}

impl PluginTrust {
    pub fn label(self) -> &'static str {
        match self {
            PluginTrust::Full => "Full",
            PluginTrust::Isolated => "Isolated",
        }
    }
}

fn default_capabilities() -> Vec<PluginCapability> {
    vec![
        PluginCapability::Renderer,
        PluginCapability::Ecs,
        PluginCapability::Assets,
        PluginCapability::Input,
        PluginCapability::Events,
        PluginCapability::Time,
    ]
}

fn default_capability_flags() -> CapabilityFlags {
    CapabilityFlags::from(default_capabilities().as_slice())
}

#[derive(Clone, Debug, Default)]
pub struct CapabilityViolationLog {
    pub count: u64,
    pub last_capability: Option<PluginCapability>,
}

#[derive(Clone)]
struct CapabilityTracker(Rc<RefCell<HashMap<String, CapabilityViolationLog>>>);

impl CapabilityTracker {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(HashMap::new())))
    }

    fn register(&self, name: &str) {
        self.0.borrow_mut().entry(name.to_string()).or_default();
    }

    fn log_violation(&self, name: &str, capability: PluginCapability) {
        let mut log = self.0.borrow_mut();
        let entry = log.entry(name.to_string()).or_default();
        entry.count += 1;
        entry.last_capability = Some(capability);
    }

    fn snapshot(&self) -> HashMap<String, CapabilityViolationLog> {
        self.0.borrow().clone()
    }
}

#[derive(Clone)]
pub struct CapabilityTrackerHandle(CapabilityTracker);

impl CapabilityTrackerHandle {
    fn new(inner: CapabilityTracker) -> Self {
        Self(inner)
    }

    fn tracker(&self) -> CapabilityTracker {
        self.0.clone()
    }
}

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

    pub fn unregister(&mut self, feature: &str) {
        self.features.remove(feature);
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
    renderer: &'a mut Renderer,
    ecs: &'a mut EcsWorld,
    assets: &'a mut AssetManager,
    input: &'a mut Input,
    material_registry: &'a mut MaterialRegistry,
    mesh_registry: &'a mut MeshRegistry,
    environment_registry: &'a mut EnvironmentRegistry,
    time: &'a Time,
    selected_entity: Option<Entity>,
    feature_registry: FeatureRegistryHandle,
    emit_event: fn(&mut EcsWorld, GameEvent),
    active_capabilities: CapabilityFlags,
    active_trust: PluginTrust,
    active_plugin: Option<String>,
    capability_tracker: CapabilityTracker,
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
        capability_tracker: CapabilityTrackerHandle,
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
            active_capabilities: CapabilityFlags::all(),
            active_trust: PluginTrust::Full,
            active_plugin: None,
            capability_tracker: capability_tracker.tracker(),
        }
    }

    pub fn features(&self) -> Ref<'_, FeatureRegistry> {
        self.feature_registry.borrow()
    }

    pub fn features_mut(&self) -> RefMut<'_, FeatureRegistry> {
        self.feature_registry.borrow_mut()
    }

    pub fn renderer_mut(&mut self) -> Result<&mut Renderer, CapabilityError> {
        self.require_capability(PluginCapability::Renderer)?;
        Ok(&mut *self.renderer)
    }

    pub fn ecs_mut(&mut self) -> Result<&mut EcsWorld, CapabilityError> {
        self.require_capability(PluginCapability::Ecs)?;
        Ok(&mut *self.ecs)
    }

    pub fn ecs(&self) -> Result<&EcsWorld, CapabilityError> {
        self.require_capability(PluginCapability::Ecs)?;
        Ok(&*self.ecs)
    }

    pub fn assets_mut(&mut self) -> Result<&mut AssetManager, CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        Ok(&mut *self.assets)
    }

    pub fn assets(&self) -> Result<&AssetManager, CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        Ok(&*self.assets)
    }

    pub fn input_mut(&mut self) -> Result<&mut Input, CapabilityError> {
        self.require_capability(PluginCapability::Input)?;
        Ok(&mut *self.input)
    }

    pub fn input(&self) -> Result<&Input, CapabilityError> {
        self.require_capability(PluginCapability::Input)?;
        Ok(&*self.input)
    }

    pub fn mesh_registry_mut(&mut self) -> Result<&mut MeshRegistry, CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        Ok(&mut *self.mesh_registry)
    }

    pub fn mesh_registry(&self) -> Result<&MeshRegistry, CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        Ok(&*self.mesh_registry)
    }

    pub fn material_registry_mut(&mut self) -> Result<&mut MaterialRegistry, CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        Ok(&mut *self.material_registry)
    }

    pub fn material_registry(&self) -> Result<&MaterialRegistry, CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        Ok(&*self.material_registry)
    }

    pub fn mesh_registry_and_renderer(
        &mut self,
    ) -> Result<(&mut MeshRegistry, &mut Renderer), CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        self.require_capability(PluginCapability::Renderer)?;
        Ok((&mut *self.mesh_registry, &mut *self.renderer))
    }

    pub fn mesh_registry_and_materials(
        &mut self,
    ) -> Result<(&mut MeshRegistry, &mut MaterialRegistry), CapabilityError> {
        self.require_capability(PluginCapability::Assets)?;
        Ok((&mut *self.mesh_registry, &mut *self.material_registry))
    }

    pub fn environment_registry_mut(&mut self) -> Result<&mut EnvironmentRegistry, CapabilityError> {
        self.require_capability(PluginCapability::Renderer)?;
        Ok(&mut *self.environment_registry)
    }

    pub fn time(&self) -> Result<&Time, CapabilityError> {
        self.require_capability(PluginCapability::Time)?;
        Ok(self.time)
    }

    pub fn selected_entity(&self) -> Option<Entity> {
        self.selected_entity
    }

    pub fn emit_event(&mut self, event: GameEvent) -> Result<(), CapabilityError> {
        self.require_capability(PluginCapability::Events)?;
        (self.emit_event)(self.ecs, event);
        Ok(())
    }

    pub fn emit_script_message(&mut self, message: impl Into<String>) -> Result<(), CapabilityError> {
        self.emit_event(GameEvent::ScriptMessage { message: message.into() })
    }

    pub fn renderer_api(&mut self) -> Result<RendererApi<'_>, CapabilityError> {
        let renderer = self.renderer_mut()?;
        Ok(RendererApi { renderer })
    }

    pub fn assets_api(&mut self) -> Result<AssetApi<'_>, CapabilityError> {
        let assets = self.assets_mut()?;
        Ok(AssetApi { assets })
    }

    pub(crate) fn set_active_plugin(
        &mut self,
        name: &str,
        capabilities: CapabilityFlags,
        trust: PluginTrust,
    ) {
        self.active_plugin = Some(name.to_string());
        self.active_capabilities = capabilities;
        self.active_trust = trust;
    }

    pub(crate) fn clear_active_plugin(&mut self) {
        self.active_plugin = None;
        self.active_capabilities = CapabilityFlags::all();
        self.active_trust = PluginTrust::Full;
    }

    fn require_capability(&self, capability: PluginCapability) -> Result<(), CapabilityError> {
        if self.active_capabilities.contains(capability.flag()) {
            Ok(())
        } else {
            if let Some(plugin) = self.active_plugin.as_deref() {
                self.capability_tracker.log_violation(plugin, capability);
            }
            Err(CapabilityError::new(self.active_plugin.as_deref(), capability))
        }
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

#[derive(Debug)]
pub struct CapabilityError {
    plugin: Option<String>,
    capability: PluginCapability,
}

impl CapabilityError {
    fn new(plugin: Option<&str>, capability: PluginCapability) -> Self {
        Self { plugin: plugin.map(|s| s.to_string()), capability }
    }
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(name) = &self.plugin {
            write!(f, "plugin '{name}' requested capability {:?}", self.capability)
        } else {
            write!(f, "plugin requested capability {:?}", self.capability)
        }
    }
}

impl std::error::Error for CapabilityError {}

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
    pub capabilities: Vec<PluginCapability>,
    pub trust: PluginTrust,
    pub state: PluginState,
}

pub struct PluginManager {
    plugins: Vec<PluginSlot>,
    features: Rc<RefCell<FeatureRegistry>>,
    capability_tracker: CapabilityTracker,
    statuses: Vec<PluginStatus>,
    loaded_names: HashSet<String>,
}

struct PluginSlot {
    name: String,
    plugin: Box<dyn EnginePlugin>,
    provides: Vec<String>,
    depends_on: Vec<String>,
    dynamic: bool,
    trust: PluginTrust,
    capabilities: CapabilityFlags,
    capability_list: Vec<PluginCapability>,
    _library: Option<Library>,
}

impl Default for PluginManager {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            features: Rc::new(RefCell::new(FeatureRegistry::with_engine_defaults())),
            capability_tracker: CapabilityTracker::new(),
            statuses: Vec::new(),
            loaded_names: HashSet::new(),
        }
    }
}

impl PluginManager {
    pub fn feature_handle(&self) -> FeatureRegistryHandle {
        FeatureRegistryHandle::new(self.features.clone())
    }

    pub fn capability_tracker_handle(&self) -> CapabilityTrackerHandle {
        CapabilityTrackerHandle::new(self.capability_tracker.clone())
    }

    pub fn capability_metrics(&self) -> HashMap<String, CapabilityViolationLog> {
        self.capability_tracker.snapshot()
    }

    pub fn register(&mut self, plugin: Box<dyn EnginePlugin>, ctx: &mut PluginContext<'_>) -> Result<()> {
        self.insert_plugin(plugin, None, false, Vec::new(), default_capabilities(), PluginTrust::Full, ctx)
    }

    pub fn register_with_features(
        &mut self,
        plugin: Box<dyn EnginePlugin>,
        provides: Vec<String>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<()> {
        self.insert_plugin(plugin, None, false, provides, default_capabilities(), PluginTrust::Full, ctx)
    }

    pub fn register_with_capabilities(
        &mut self,
        plugin: Box<dyn EnginePlugin>,
        provides: Vec<String>,
        capabilities: Vec<PluginCapability>,
        ctx: &mut PluginContext<'_>,
    ) -> Result<()> {
        self.insert_plugin(plugin, None, false, provides, capabilities, PluginTrust::Full, ctx)
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
                        capabilities: slot.capability_list.clone(),
                        trust: slot.trust,
                        state: PluginState::Loaded,
                    });
                }
                continue;
            }
            let entry_caps = entry.capabilities.clone();
            let entry_trust = entry.trust;
            if !entry.enabled {
                self.statuses.push(PluginStatus {
                    name: entry.name.clone(),
                    version: entry.version.clone(),
                    dynamic: true,
                    provides: entry.provides_features.clone(),
                    depends_on: Vec::new(),
                    capabilities: entry_caps.clone(),
                    trust: entry_trust,
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
                    capabilities: entry_caps.clone(),
                    trust: entry_trust,
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
                    capabilities: entry_caps.clone(),
                    trust: entry_trust,
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
                        capabilities: entry_caps.clone(),
                        trust: entry_trust,
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
            capabilities: default_capabilities(),
            trust: PluginTrust::Full,
            state: PluginState::Disabled(reason.to_string()),
        });
    }

    pub fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for slot in &mut self.plugins {
            ctx.set_active_plugin(&slot.name, slot.capabilities, slot.trust);
            if let Err(err) = slot.plugin.update(ctx, dt) {
                eprintln!("[plugin:{}] update failed: {err:?}", slot.name);
            }
            ctx.clear_active_plugin();
        }
    }

    pub fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        for slot in &mut self.plugins {
            ctx.set_active_plugin(&slot.name, slot.capabilities, slot.trust);
            if let Err(err) = slot.plugin.fixed_update(ctx, dt) {
                eprintln!("[plugin:{}] fixed_update failed: {err:?}", slot.name);
            }
            ctx.clear_active_plugin();
        }
    }

    pub fn handle_events(&mut self, ctx: &mut PluginContext<'_>, events: &[GameEvent]) {
        if events.is_empty() {
            return;
        }
        for slot in &mut self.plugins {
            ctx.set_active_plugin(&slot.name, slot.capabilities, slot.trust);
            if let Err(err) = slot.plugin.on_events(ctx, events) {
                eprintln!("[plugin:{}] event hook failed: {err:?}", slot.name);
            }
            ctx.clear_active_plugin();
        }
    }

    pub fn shutdown(&mut self, ctx: &mut PluginContext<'_>) {
        for slot in &mut self.plugins {
            ctx.set_active_plugin(&slot.name, slot.capabilities, slot.trust);
            if let Err(err) = slot.plugin.shutdown(ctx) {
                eprintln!("[plugin:{}] shutdown failed: {err:?}", slot.name);
            }
            ctx.clear_active_plugin();
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

    pub(crate) fn unload_dynamic_plugins(&mut self, ctx: &mut PluginContext<'_>) {
        if self.plugins.iter().all(|slot| !slot.dynamic) {
            self.clear_dynamic_statuses();
            return;
        }

        let mut removed_features = Vec::new();
        let mut retained = Vec::with_capacity(self.plugins.len());
        for mut slot in self.plugins.drain(..) {
            if slot.dynamic {
                ctx.set_active_plugin(&slot.name, slot.capabilities, slot.trust);
                if let Err(err) = slot.plugin.shutdown(ctx) {
                    eprintln!("[plugin:{}] shutdown failed during unload: {err:?}", slot.name);
                }
                ctx.clear_active_plugin();
                self.loaded_names.remove(&slot.name);
                removed_features.extend(slot.provides.clone());
            } else {
                retained.push(slot);
            }
        }
        self.plugins = retained;

        if removed_features.is_empty() {
            self.clear_dynamic_statuses();
            return;
        }

        let removed_unique: BTreeSet<String> = removed_features.into_iter().collect();
        let still_provided: HashSet<String> =
            self.plugins.iter().flat_map(|slot| slot.provides.iter().cloned()).collect();

        {
            let mut registry = self.features.borrow_mut();
            for feature in removed_unique {
                if DEFAULT_ENGINE_FEATURES.iter().any(|default| *default == feature.as_str()) {
                    continue;
                }
                if !still_provided.contains(&feature) {
                    registry.unregister(&feature);
                }
            }
        }

        self.clear_dynamic_statuses();
    }

    fn insert_plugin(
        &mut self,
        mut plugin: Box<dyn EnginePlugin>,
        library: Option<Library>,
        is_dynamic: bool,
        provides: Vec<String>,
        capabilities: Vec<PluginCapability>,
        trust: PluginTrust,
        ctx: &mut PluginContext<'_>,
    ) -> Result<()> {
        let name = plugin.name().to_string();
        if self.loaded_names.contains(&name) {
            bail!("plugin '{name}' already registered");
        }
        self.ensure_dependencies(plugin.depends_on(), &name)?;
        let capability_flags = CapabilityFlags::from(capabilities.as_slice());
        self.capability_tracker.register(&name);
        ctx.set_active_plugin(&name, capability_flags, trust);
        let build_result = plugin.build(ctx);
        ctx.clear_active_plugin();
        build_result?;
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
            capabilities: capabilities.clone(),
            trust,
            state: PluginState::Loaded,
        });
        self.plugins.push(PluginSlot {
            name,
            plugin,
            provides,
            depends_on: depends,
            dynamic: is_dynamic,
            trust,
            capabilities: capability_flags,
            capability_list: capabilities,
            _library: library,
        });
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

        self.insert_plugin(
            plugin,
            Some(library),
            true,
            entry.provides_features.clone(),
            entry.capabilities.clone(),
            entry.trust,
            ctx,
        )?;
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

#[derive(Debug, Serialize, Deserialize, Clone)]
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

    pub fn entries_mut(&mut self) -> &mut [PluginManifestEntry] {
        &mut self.plugins
    }

    pub fn entry_mut(&mut self, name: &str) -> Option<&mut PluginManifestEntry> {
        self.plugins.iter_mut().find(|entry| entry.name == name)
    }

    fn path_parent(&self) -> Option<&Path> {
        self.source_path.as_deref().and_then(|p| p.parent())
    }

    pub fn path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub fn save(&self) -> Result<()> {
        let path = self.source_path.as_ref().ok_or_else(|| anyhow!("plugin manifest path unavailable"))?;
        let json = serde_json::to_string_pretty(self)
            .context(format!("serializing plugin manifest '{}'", path.display()))?;
        fs::write(path, format!("{json}\n"))
            .with_context(|| format!("writing plugin manifest '{}'", path.display()))?;
        Ok(())
    }

    pub fn is_builtin_disabled(&self, name: &str) -> bool {
        self.disable_builtins.iter().any(|entry| entry == name)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
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
    #[serde(default = "default_capabilities")]
    pub capabilities: Vec<PluginCapability>,
    #[serde(default)]
    pub trust: PluginTrust,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct ManifestDynamicToggle {
    pub name: String,
    pub new_enabled: bool,
}

#[derive(Debug, Default)]
pub struct ManifestDynamicToggleOutcome {
    pub enabled: Vec<String>,
    pub disabled: Vec<String>,
    pub missing: Vec<String>,
    pub changed: bool,
}

pub fn apply_manifest_dynamic_toggles(
    manifest: &mut PluginManifest,
    toggles: &[ManifestDynamicToggle],
) -> ManifestDynamicToggleOutcome {
    let mut outcome = ManifestDynamicToggleOutcome::default();
    if toggles.is_empty() {
        return outcome;
    }
    let mut dedup: BTreeMap<&str, &ManifestDynamicToggle> = BTreeMap::new();
    for toggle in toggles {
        dedup.insert(toggle.name.as_str(), toggle);
    }
    for toggle in dedup.values() {
        match manifest.entry_mut(&toggle.name) {
            Some(entry) => {
                if entry.enabled != toggle.new_enabled {
                    entry.enabled = toggle.new_enabled;
                    outcome.changed = true;
                    if toggle.new_enabled {
                        outcome.enabled.push(toggle.name.clone());
                    } else {
                        outcome.disabled.push(toggle.name.clone());
                    }
                }
            }
            None => outcome.missing.push(toggle.name.clone()),
        }
    }
    outcome.enabled.sort();
    outcome.disabled.sort();
    outcome.missing.sort();
    outcome
}

#[derive(Debug, Clone)]
pub struct ManifestBuiltinToggle {
    pub name: String,
    pub disable: bool,
}

#[derive(Debug, Default)]
pub struct ManifestBuiltinToggleOutcome {
    pub disabled: Vec<String>,
    pub enabled: Vec<String>,
    pub changed: bool,
}

pub fn apply_manifest_builtin_toggles(
    manifest: &mut PluginManifest,
    toggles: &[ManifestBuiltinToggle],
) -> ManifestBuiltinToggleOutcome {
    let mut outcome = ManifestBuiltinToggleOutcome::default();
    if toggles.is_empty() {
        return outcome;
    }
    let mut dedup: BTreeMap<&str, &ManifestBuiltinToggle> = BTreeMap::new();
    for toggle in toggles {
        dedup.insert(toggle.name.as_str(), toggle);
    }
    let mut disabled: BTreeSet<String> = manifest.disable_builtins.iter().cloned().collect();
    for toggle in dedup.values() {
        let was_disabled = disabled.contains(&toggle.name);
        if toggle.disable {
            if !was_disabled {
                disabled.insert(toggle.name.clone());
                outcome.disabled.push(toggle.name.clone());
                outcome.changed = true;
            }
        } else if was_disabled {
            disabled.remove(&toggle.name);
            outcome.enabled.push(toggle.name.clone());
            outcome.changed = true;
        }
    }
    manifest.disable_builtins = disabled.into_iter().collect();
    outcome.disabled.sort();
    outcome.enabled.sort();
    outcome
}
