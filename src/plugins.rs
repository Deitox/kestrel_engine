use crate::assets::AssetManager;
use crate::ecs::EcsWorld;
use crate::environment::EnvironmentRegistry;
use crate::events::GameEvent;
use crate::input::Input;
use crate::material_registry::MaterialRegistry;
use crate::mesh_registry::MeshRegistry;
use crate::plugin_rpc::{
    recv_frame, send_frame, PluginHostRequest, PluginHostResponse, RpcAssetReadbackPayload,
    RpcAssetReadbackRequest, RpcAssetReadbackResponse, RpcComponentKind, RpcEntityFilter, RpcEntityInfo,
    RpcEntitySnapshot, RpcGameEvent, RpcIterEntitiesRequest, RpcIterEntitiesResponse, RpcIteratorCursor,
    RpcReadComponentsRequest, RpcReadComponentsResponse, RpcRequestId, RpcResponseData, RpcSnapshotFormat,
    RpcSpriteInfo,
};
use crate::renderer::Renderer;
use crate::time::Time;
use anyhow::{anyhow, bail, Context, Result};
use bevy_ecs::prelude::Entity;
use bitflags::bitflags;
use libloading::Library;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::io::{self, BufReader};
use std::mem;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const ISOLATED_RPC_TIMEOUT: Duration = Duration::from_secs(10);

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

    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "renderer" => Some(PluginCapability::Renderer),
            "ecs" => Some(PluginCapability::Ecs),
            "assets" => Some(PluginCapability::Assets),
            "input" => Some(PluginCapability::Input),
            "scripts" => Some(PluginCapability::Scripts),
            "analytics" => Some(PluginCapability::Analytics),
            "time" => Some(PluginCapability::Time),
            "events" => Some(PluginCapability::Events),
            "all" => Some(PluginCapability::All),
            _ => None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginTrust {
    #[default]
    Full,
    Isolated,
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
    pub last_timestamp: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct PluginCapabilityEvent {
    pub plugin: String,
    pub capability: PluginCapability,
    pub timestamp: SystemTime,
}

#[derive(Clone, Debug, Default)]
pub struct AssetReadbackStats {
    pub requests: u64,
    pub bytes: u64,
    pub cache_hits: u64,
    pub throttled: u64,
}

#[derive(Clone, Debug)]
pub struct PluginAssetReadbackEvent {
    pub plugin: String,
    pub kind: String,
    pub target: String,
    pub bytes: u64,
    pub duration_ms: f32,
    pub cache_hit: bool,
    pub timestamp: SystemTime,
}

#[derive(Clone, Debug)]
pub struct PluginWatchdogEvent {
    pub plugin: String,
    pub timestamp: SystemTime,
    pub elapsed_ms: f32,
    pub reason: String,
    pub last_request: String,
}

fn summarize_asset_payload(payload: &RpcAssetReadbackPayload) -> (String, String) {
    match payload {
        RpcAssetReadbackPayload::AtlasMeta { atlas_id } => ("atlas_meta".to_string(), atlas_id.clone()),
        RpcAssetReadbackPayload::AtlasBinary { atlas_id } => ("atlas_binary".to_string(), atlas_id.clone()),
        RpcAssetReadbackPayload::BlobRange { blob_id, .. } => ("blob_range".to_string(), blob_id.clone()),
    }
}

fn describe_panic(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(payload) => format!("unknown panic (type_id {:?})", (*payload).type_id()),
        },
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
enum AssetCacheKey {
    AtlasMeta(String),
    AtlasBinary(String),
    BlobRange { id: String, offset: u64, length: u64 },
}

#[derive(Clone)]
struct AssetCacheEntry {
    response: RpcAssetReadbackResponse,
    size_bytes: usize,
}

struct IsolatedAssetCache {
    entries: HashMap<AssetCacheKey, AssetCacheEntry>,
    order: VecDeque<AssetCacheKey>,
    capacity_bytes: usize,
    current_bytes: usize,
}

impl IsolatedAssetCache {
    fn new(capacity_bytes: usize) -> Self {
        Self { entries: HashMap::new(), order: VecDeque::new(), capacity_bytes, current_bytes: 0 }
    }

    fn get(&self, key: &AssetCacheKey) -> Option<RpcAssetReadbackResponse> {
        self.entries.get(key).map(|entry| entry.response.clone())
    }

    fn insert(&mut self, key: AssetCacheKey, response: RpcAssetReadbackResponse) {
        let size = response.bytes.len();
        if size > self.capacity_bytes {
            return;
        }
        let entry = AssetCacheEntry { response, size_bytes: size };
        self.current_bytes += size;
        if let Some(previous) = self.entries.insert(key.clone(), entry) {
            self.current_bytes = self.current_bytes.saturating_sub(previous.size_bytes);
        }
        self.order.retain(|existing| existing != &key);
        self.order.push_back(key);
        self.evict_if_needed();
    }

    fn evict_if_needed(&mut self) {
        while self.current_bytes > self.capacity_bytes {
            if let Some(key) = self.order.pop_front() {
                if let Some(entry) = self.entries.remove(&key) {
                    self.current_bytes = self.current_bytes.saturating_sub(entry.size_bytes);
                }
            } else {
                break;
            }
        }
    }
}

impl AssetCacheKey {
    fn from_payload(payload: &RpcAssetReadbackPayload) -> Self {
        match payload {
            RpcAssetReadbackPayload::AtlasMeta { atlas_id } => AssetCacheKey::AtlasMeta(atlas_id.clone()),
            RpcAssetReadbackPayload::AtlasBinary { atlas_id } => AssetCacheKey::AtlasBinary(atlas_id.clone()),
            RpcAssetReadbackPayload::BlobRange { blob_id, offset, length } => {
                AssetCacheKey::BlobRange { id: blob_id.clone(), offset: *offset, length: *length }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct RemoteEntityInfo {
    pub entity: Entity,
    pub scene_id: String,
    pub translation: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
    pub velocity: Option<[f32; 2]>,
    pub sprite: Option<RemoteSpriteInfo>,
}

#[derive(Debug, Clone)]
pub struct RemoteSpriteInfo {
    pub atlas: String,
    pub region: String,
}

impl From<RpcEntityInfo> for RemoteEntityInfo {
    fn from(info: RpcEntityInfo) -> Self {
        Self {
            entity: info.entity.into(),
            scene_id: info.scene_id,
            translation: info.translation,
            rotation: info.rotation,
            scale: info.scale,
            velocity: info.velocity,
            sprite: info.sprite.map(RemoteSpriteInfo::from),
        }
    }
}

impl From<RpcSpriteInfo> for RemoteSpriteInfo {
    fn from(info: RpcSpriteInfo) -> Self {
        Self { atlas: info.atlas, region: info.region }
    }
}

#[derive(Default)]
struct CapabilityTrackerInner {
    metrics: HashMap<String, CapabilityViolationLog>,
    events: VecDeque<PluginCapabilityEvent>,
    snapshot: Option<Arc<HashMap<String, CapabilityViolationLog>>>,
}

impl CapabilityTrackerInner {
    fn register(&mut self, name: &str) {
        self.metrics.entry(name.to_string()).or_default();
        self.snapshot = None;
    }

    fn log_violation(&mut self, name: &str, capability: PluginCapability) {
        let timestamp = SystemTime::now();
        let entry = self.metrics.entry(name.to_string()).or_default();
        entry.count += 1;
        entry.last_capability = Some(capability);
        entry.last_timestamp = Some(timestamp);
        self.events.push_front(PluginCapabilityEvent { plugin: name.to_string(), capability, timestamp });
        const CAPABILITY_EVENT_CAPACITY: usize = 64;
        while self.events.len() > CAPABILITY_EVENT_CAPACITY {
            self.events.pop_back();
        }
        self.snapshot = None;
    }

    fn snapshot(&mut self) -> Arc<HashMap<String, CapabilityViolationLog>> {
        if let Some(cache) = &self.snapshot {
            return Arc::clone(cache);
        }
        let arc = Arc::new(self.metrics.clone());
        self.snapshot = Some(Arc::clone(&arc));
        arc
    }

    fn drain_events(&mut self) -> Vec<PluginCapabilityEvent> {
        self.events.drain(..).collect()
    }
}

#[derive(Clone)]
struct CapabilityTracker(Rc<RefCell<CapabilityTrackerInner>>);

impl CapabilityTracker {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(CapabilityTrackerInner::default())))
    }

    fn register(&self, name: &str) {
        self.0.borrow_mut().register(name);
    }

    fn log_violation(&self, name: &str, capability: PluginCapability) {
        self.0.borrow_mut().log_violation(name, capability);
    }

    fn snapshot(&self) -> Arc<HashMap<String, CapabilityViolationLog>> {
        self.0.borrow_mut().snapshot()
    }

    fn drain_events(&self) -> Vec<PluginCapabilityEvent> {
        self.0.borrow_mut().drain_events()
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

    pub fn isolated() -> Self {
        Self(CapabilityTracker::new())
    }

    pub fn drain_events(&self) -> Vec<PluginCapabilityEvent> {
        self.0.drain_events()
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

    /// # Safety
    /// Caller must ensure the boxed plugin was allocated by this process and remains valid for the handle lifetime.
    pub unsafe fn from_box(plugin: Box<dyn EnginePlugin>) -> Self {
        Self::from_raw(Box::into_raw(plugin))
    }

    /// # Safety
    /// `raw` must be a valid, non-null pointer created from `Box<dyn EnginePlugin>` with matching vtable layout.
    pub unsafe fn from_raw(raw: *mut dyn EnginePlugin) -> Self {
        let erased: (*mut (), *mut ()) = mem::transmute(raw);
        Self { data: erased.0, vtable: erased.1 }
    }

    /// # Safety
    /// Returned pointer must eventually be converted back into a `Box<dyn EnginePlugin>` to avoid leaks.
    pub unsafe fn into_raw(self) -> *mut dyn EnginePlugin {
        mem::transmute((self.data, self.vtable))
    }

    /// # Safety
    /// Handle must contain a valid plugin pointer and vtable pair; using an invalid handle is undefined behavior.
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

    pub fn isolated() -> Self {
        Self(Rc::new(RefCell::new(FeatureRegistry::with_engine_defaults())))
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

    pub fn set_active_plugin(
        &mut self,
        name: &str,
        capabilities: CapabilityFlags,
        trust: PluginTrust,
    ) {
        self.active_plugin = Some(name.to_string());
        self.active_capabilities = capabilities;
        self.active_trust = trust;
    }

    pub fn clear_active_plugin(&mut self) {
        self.active_plugin = None;
        self.active_capabilities = CapabilityFlags::all();
        self.active_trust = PluginTrust::Full;
    }

    pub fn log_capability_violation(&self, plugin: &str, capability: PluginCapability) {
        self.capability_tracker.log_violation(plugin, capability);
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
    fn name(&self) -> &str;

    fn version(&self) -> &str {
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
    status_snapshot: Option<Arc<[PluginStatus]>>,
    loaded_names: HashSet<String>,
    asset_cache: IsolatedAssetCache,
    asset_metrics: HashMap<String, AssetReadbackStats>,
    asset_metrics_snapshot: Option<Arc<HashMap<String, AssetReadbackStats>>>,
    asset_readback_events: Vec<PluginAssetReadbackEvent>,
    ecs_query_history: HashMap<String, VecDeque<u64>>,
    ecs_history_snapshot: Option<Arc<HashMap<String, Vec<u64>>>>,
    last_asset_payload: HashMap<String, RpcAssetReadbackPayload>,
    watchdog_events: HashMap<String, VecDeque<PluginWatchdogEvent>>,
    pending_watchdog_events: Vec<PluginWatchdogEvent>,
    watchdog_snapshot: Option<Arc<HashMap<String, Vec<PluginWatchdogEvent>>>>,
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
    asset_filters: PluginAssetFilters,
    failed_reason: Option<String>,
    _library: Option<Library>,
}

impl PluginSlot {
    fn isolated_proxy(&mut self) -> Option<&mut IsolatedPluginProxy> {
        if self.trust != PluginTrust::Isolated {
            return None;
        }
        self.plugin.as_any_mut().downcast_mut::<IsolatedPluginProxy>()
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            features: Rc::new(RefCell::new(FeatureRegistry::with_engine_defaults())),
            capability_tracker: CapabilityTracker::new(),
            statuses: Vec::new(),
            status_snapshot: None,
            loaded_names: HashSet::new(),
            asset_cache: IsolatedAssetCache::new(32 * 1024 * 1024),
            asset_metrics: HashMap::new(),
            asset_metrics_snapshot: None,
            asset_readback_events: Vec::new(),
            ecs_query_history: HashMap::new(),
            ecs_history_snapshot: None,
            last_asset_payload: HashMap::new(),
            watchdog_events: HashMap::new(),
            pending_watchdog_events: Vec::new(),
            watchdog_snapshot: None,
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

    pub fn capability_metrics(&self) -> Arc<HashMap<String, CapabilityViolationLog>> {
        self.capability_tracker.snapshot()
    }

    pub fn drain_capability_events(&mut self) -> Vec<PluginCapabilityEvent> {
        self.capability_tracker.drain_events()
    }

    pub fn asset_readback_metrics(&mut self) -> Arc<HashMap<String, AssetReadbackStats>> {
        if let Some(snapshot) = &self.asset_metrics_snapshot {
            return Arc::clone(snapshot);
        }
        let arc = Arc::new(self.asset_metrics.clone());
        self.asset_metrics_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn ecs_query_history(&mut self) -> Arc<HashMap<String, Vec<u64>>> {
        if let Some(snapshot) = &self.ecs_history_snapshot {
            return Arc::clone(snapshot);
        }
        let map = self
            .ecs_query_history
            .iter()
            .map(|(plugin, log)| (plugin.clone(), log.iter().copied().collect()))
            .collect();
        let arc = Arc::new(map);
        self.ecs_history_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn watchdog_events(&mut self) -> Arc<HashMap<String, Vec<PluginWatchdogEvent>>> {
        if let Some(snapshot) = &self.watchdog_snapshot {
            return Arc::clone(snapshot);
        }
        let map = self
            .watchdog_events
            .iter()
            .map(|(plugin, log)| (plugin.clone(), log.iter().cloned().collect()))
            .collect();
        let arc = Arc::new(map);
        self.watchdog_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn clear_watchdog_events(&mut self, plugin_name: &str) {
        self.watchdog_events.remove(plugin_name);
        self.watchdog_snapshot = None;
    }

    pub fn drain_watchdog_events(&mut self) -> Vec<PluginWatchdogEvent> {
        std::mem::take(&mut self.pending_watchdog_events)
    }

    pub fn drain_asset_readback_events(&mut self) -> Vec<PluginAssetReadbackEvent> {
        std::mem::take(&mut self.asset_readback_events)
    }

    pub fn has_asset_readback_request(&self, plugin_name: &str) -> bool {
        self.last_asset_payload.contains_key(plugin_name)
    }

    pub fn retry_last_asset_readback(
        &mut self,
        plugin_name: &str,
    ) -> Result<Option<RpcAssetReadbackResponse>> {
        if let Some(payload) = self.last_asset_payload.get(plugin_name).cloned() {
            self.asset_readback(plugin_name, payload).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn pending_asset_readback_plugins(&self) -> HashSet<String> {
        self.last_asset_payload.keys().cloned().collect()
    }

    pub fn query_isolated_entity_info(
        &mut self,
        plugin_name: &str,
        entity: Entity,
    ) -> Result<Option<RemoteEntityInfo>> {
        let idx = self
            .plugins
            .iter()
            .position(|slot| slot.name == plugin_name)
            .ok_or_else(|| anyhow!("plugin '{plugin_name}' not registered"))?;
        let (result, watchdog, caps) = {
            let slot = self.plugins.get_mut(idx).expect("slot index valid");
            let proxy = slot
                .isolated_proxy()
                .ok_or_else(|| anyhow!("plugin '{plugin_name}' is not running in isolated mode"))?;
            match proxy.query_entity_info(entity) {
                Ok(info) => (Ok(info), proxy.take_watchdog_event(), proxy.take_capability_violations()),
                Err(err) => (Err(err), proxy.take_watchdog_event(), proxy.take_capability_violations()),
            }
        };
        self.log_isolated_capability_violations(plugin_name, caps);
        if let Some(event) = watchdog {
            self.log_watchdog_event(event);
        }
        result
    }

    pub fn read_isolated_components(
        &mut self,
        plugin_name: &str,
        entity: Entity,
        components: Vec<RpcComponentKind>,
        format: RpcSnapshotFormat,
    ) -> Result<Option<RpcEntitySnapshot>> {
        let idx = self
            .plugins
            .iter()
            .position(|slot| slot.name == plugin_name)
            .ok_or_else(|| anyhow!("plugin '{plugin_name}' not registered"))?;
        let (response, watchdog, caps) = {
            let slot = self.plugins.get_mut(idx).expect("slot index valid");
            let proxy = slot
                .isolated_proxy()
                .ok_or_else(|| anyhow!("plugin '{plugin_name}' is not running in isolated mode"))?;
            match proxy.read_components(entity, components, format) {
                Ok(resp) => (Ok(resp), proxy.take_watchdog_event(), proxy.take_capability_violations()),
                Err(err) => (Err(err), proxy.take_watchdog_event(), proxy.take_capability_violations()),
            }
        };
        self.log_isolated_capability_violations(plugin_name, caps);
        if let Some(event) = watchdog {
            self.log_watchdog_event(event);
        }
        match response {
            Ok(response) => {
                if let Some(snapshot) = response.snapshot.as_ref() {
                    let logged: Entity = snapshot.entity.into();
                    self.log_ecs_entities(plugin_name, [logged]);
                }
                Ok(response.snapshot)
            }
            Err(err) => Err(err),
        }
    }

    pub fn iter_isolated_entities(
        &mut self,
        plugin_name: &str,
        filter: RpcEntityFilter,
        cursor: Option<RpcIteratorCursor>,
        limit: u32,
        components: Vec<RpcComponentKind>,
        format: RpcSnapshotFormat,
    ) -> Result<RpcIterEntitiesResponse> {
        let idx = self
            .plugins
            .iter()
            .position(|slot| slot.name == plugin_name)
            .ok_or_else(|| anyhow!("plugin '{plugin_name}' not registered"))?;
        let (response, watchdog, caps) = {
            let slot = self.plugins.get_mut(idx).expect("slot index valid");
            let proxy = slot
                .isolated_proxy()
                .ok_or_else(|| anyhow!("plugin '{plugin_name}' is not running in isolated mode"))?;
            match proxy.iter_entities(filter, cursor, limit, components, format) {
                Ok(resp) => (Ok(resp), proxy.take_watchdog_event(), proxy.take_capability_violations()),
                Err(err) => (Err(err), proxy.take_watchdog_event(), proxy.take_capability_violations()),
            }
        };
        self.log_isolated_capability_violations(plugin_name, caps);
        if let Some(event) = watchdog {
            self.log_watchdog_event(event);
        }
        match response {
            Ok(response) => {
                if !response.snapshots.is_empty() {
                    let entities = response.snapshots.iter().map(|snapshot| {
                        let entity: Entity = snapshot.entity.into();
                        entity
                    });
                    self.log_ecs_entities(plugin_name, entities);
                }
                Ok(response)
            }
            Err(err) => Err(err),
        }
    }

    pub fn asset_readback(
        &mut self,
        plugin_name: &str,
        payload: RpcAssetReadbackPayload,
    ) -> Result<RpcAssetReadbackResponse> {
        self.last_asset_payload.insert(plugin_name.to_string(), payload.clone());
        let key = AssetCacheKey::from_payload(&payload);
        if let Some(hit) = self.asset_cache.get(&key) {
            let stats = self.asset_metrics.entry(plugin_name.to_string()).or_default();
            stats.cache_hits += 1;
            self.asset_metrics_snapshot = None;
            self.record_asset_readback_event(
                plugin_name,
                &payload,
                hit.byte_length,
                Duration::from_secs(0),
                true,
            );
            return Ok(hit);
        }
        let idx = self
            .plugins
            .iter()
            .position(|slot| slot.name == plugin_name)
            .ok_or_else(|| anyhow!("plugin '{plugin_name}' not registered"))?;
        let (capabilities, filters, trust) = {
            let slot = self.plugins.get(idx).expect("slot index valid");
            (slot.capabilities, slot.asset_filters.clone(), slot.trust)
        };
        if !capabilities.contains(PluginCapability::Assets.flag()) {
            bail!("plugin '{plugin_name}' missing asset capability for readback");
        }
        ensure_asset_filter_allows(&filters, &payload, trust)?;
        let (result, watchdog, caps, elapsed) = {
            let slot = self.plugins.get_mut(idx).expect("slot index valid");
            let proxy = slot
                .isolated_proxy()
                .ok_or_else(|| anyhow!("plugin '{plugin_name}' is not running in isolated mode"))?;
            let start = Instant::now();
            let result = proxy.asset_readback(payload.clone());
            let elapsed = start.elapsed();
            let watchdog = proxy.take_watchdog_event();
            let caps = proxy.take_capability_violations();
            (result, watchdog, caps, elapsed)
        };
        self.log_isolated_capability_violations(plugin_name, caps);
        if let Some(event) = watchdog {
            self.log_watchdog_event(event);
        }
        match result {
            Ok(response) => {
                let stats = self.asset_metrics.entry(plugin_name.to_string()).or_default();
                stats.requests += 1;
                stats.bytes += response.byte_length;
                self.asset_metrics_snapshot = None;
                self.asset_cache.insert(key, response.clone());
                self.record_asset_readback_event(
                    plugin_name,
                    &payload,
                    response.byte_length,
                    elapsed,
                    false,
                );
                Ok(response)
            }
            Err(err) => {
                if err.to_string().contains("asset readback budget exceeded") {
                    let stats = self.asset_metrics.entry(plugin_name.to_string()).or_default();
                    stats.throttled += 1;
                    self.asset_metrics_snapshot = None;
                }
                Err(err)
            }
        }
    }

    fn log_ecs_entities(&mut self, plugin_name: &str, entities: impl IntoIterator<Item = Entity>) {
        let log = self.ecs_query_history.entry(plugin_name.to_string()).or_default();
        for entity in entities {
            log.push_front(entity.to_bits());
        }
        const MAX_ENTRIES: usize = 16;
        while log.len() > MAX_ENTRIES {
            log.pop_back();
        }
        self.ecs_history_snapshot = None;
    }

    fn log_watchdog_event(&mut self, event: PluginWatchdogEvent) {
        let log = self.watchdog_events.entry(event.plugin.clone()).or_default();
        log.push_front(event.clone());
        const MAX_EVENTS: usize = 10;
        while log.len() > MAX_EVENTS {
            log.pop_back();
        }
        self.pending_watchdog_events.push(event);
        self.watchdog_snapshot = None;
    }

    fn log_isolated_capability_violations(
        &mut self,
        plugin_name: &str,
        caps: Vec<PluginCapability>,
    ) {
        if caps.is_empty() {
            return;
        }
        for cap in caps {
            self.capability_tracker.log_violation(plugin_name, cap);
        }
    }

    fn mark_plugin_failed(&mut self, idx: usize, reason: String) {
        if idx >= self.plugins.len() {
            return;
        }
        if self.plugins[idx].failed_reason.is_some() {
            return;
        }
        let plugin_name = self.plugins[idx].name.clone();
        self.plugins[idx].failed_reason = Some(reason.clone());
        self.update_status_state(&plugin_name, PluginState::Failed(reason.clone()));
        self.log_watchdog_event(PluginWatchdogEvent {
            plugin: plugin_name,
            timestamp: SystemTime::now(),
            elapsed_ms: 0.0,
            reason,
            last_request: "panic".to_string(),
        });
    }

    fn update_status_state(&mut self, plugin_name: &str, state: PluginState) {
        if let Some(status) = self.statuses.iter_mut().find(|status| status.name == plugin_name) {
            status.state = state;
            self.invalidate_status_cache();
            return;
        }
        if let Some(slot) = self.plugins.iter().find(|slot| slot.name == plugin_name) {
            self.push_status(PluginStatus {
                name: slot.name.clone(),
                version: Some(slot.plugin.version().to_string()),
                dynamic: slot.dynamic,
                provides: slot.provides.clone(),
                depends_on: slot.depends_on.clone(),
                capabilities: slot.capability_list.clone(),
                trust: slot.trust,
                state,
            });
        }
    }

    fn record_asset_readback_event(
        &mut self,
        plugin_name: &str,
        payload: &RpcAssetReadbackPayload,
        bytes: u64,
        elapsed: Duration,
        cache_hit: bool,
    ) {
        let (kind, target) = summarize_asset_payload(payload);
        self.asset_readback_events.push(PluginAssetReadbackEvent {
            plugin: plugin_name.to_string(),
            kind,
            target,
            bytes,
            duration_ms: elapsed.as_secs_f32() * 1000.0,
            cache_hit,
            timestamp: SystemTime::now(),
        });
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
                    self.push_status(PluginStatus {
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
                self.push_status(PluginStatus {
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
                self.push_status(PluginStatus {
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
                self.push_status(PluginStatus {
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
                    self.push_status(PluginStatus {
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
        self.push_status(PluginStatus {
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
        let mut watchdog_events = Vec::new();
        let mut panicked = Vec::new();
        for idx in 0..self.plugins.len() {
            if self.plugins[idx].failed_reason.is_some() {
                continue;
            }
            let plugin_name = self.plugins[idx].name.clone();
            let capability_flags = self.plugins[idx].capabilities;
            let trust = self.plugins[idx].trust;
            ctx.set_active_plugin(&plugin_name, capability_flags, trust);
            let result = {
                let slot = &mut self.plugins[idx];
                catch_unwind(AssertUnwindSafe(|| slot.plugin.update(ctx, dt)))
            };
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    eprintln!("[plugin:{}] update failed: {err:?}", plugin_name);
                    if let Some(event) =
                        self.plugins[idx].isolated_proxy().and_then(|proxy| proxy.take_watchdog_event())
                    {
                        watchdog_events.push(event);
                    }
                }
                Err(payload) => {
                    let summary = format!("update panicked: {}", describe_panic(payload));
                    eprintln!("[plugin:{}] {summary}", plugin_name);
                    panicked.push((idx, summary));
                }
            }
            ctx.clear_active_plugin();
        }
        for event in watchdog_events {
            self.log_watchdog_event(event);
        }
        for (idx, reason) in panicked {
            self.mark_plugin_failed(idx, reason);
        }
    }

    pub fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) {
        let mut watchdog_events = Vec::new();
        let mut panicked = Vec::new();
        for idx in 0..self.plugins.len() {
            if self.plugins[idx].failed_reason.is_some() {
                continue;
            }
            let plugin_name = self.plugins[idx].name.clone();
            let capability_flags = self.plugins[idx].capabilities;
            let trust = self.plugins[idx].trust;
            ctx.set_active_plugin(&plugin_name, capability_flags, trust);
            let result = {
                let slot = &mut self.plugins[idx];
                catch_unwind(AssertUnwindSafe(|| slot.plugin.fixed_update(ctx, dt)))
            };
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    eprintln!("[plugin:{}] fixed_update failed: {err:?}", plugin_name);
                    if let Some(event) =
                        self.plugins[idx].isolated_proxy().and_then(|proxy| proxy.take_watchdog_event())
                    {
                        watchdog_events.push(event);
                    }
                }
                Err(payload) => {
                    let summary = format!("fixed_update panicked: {}", describe_panic(payload));
                    eprintln!("[plugin:{}] {summary}", plugin_name);
                    panicked.push((idx, summary));
                }
            }
            ctx.clear_active_plugin();
        }
        for event in watchdog_events {
            self.log_watchdog_event(event);
        }
        for (idx, reason) in panicked {
            self.mark_plugin_failed(idx, reason);
        }
    }

    pub fn handle_events(&mut self, ctx: &mut PluginContext<'_>, events: &[GameEvent]) {
        if events.is_empty() {
            return;
        }
        let mut watchdog_events = Vec::new();
        let mut panicked = Vec::new();
        for idx in 0..self.plugins.len() {
            if self.plugins[idx].failed_reason.is_some() {
                continue;
            }
            let plugin_name = self.plugins[idx].name.clone();
            let capability_flags = self.plugins[idx].capabilities;
            let trust = self.plugins[idx].trust;
            ctx.set_active_plugin(&plugin_name, capability_flags, trust);
            let result = {
                let slot = &mut self.plugins[idx];
                catch_unwind(AssertUnwindSafe(|| slot.plugin.on_events(ctx, events)))
            };
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    eprintln!("[plugin:{}] event hook failed: {err:?}", plugin_name);
                    if let Some(event) =
                        self.plugins[idx].isolated_proxy().and_then(|proxy| proxy.take_watchdog_event())
                    {
                        watchdog_events.push(event);
                    }
                }
                Err(payload) => {
                    let summary = format!("event hook panicked: {}", describe_panic(payload));
                    eprintln!("[plugin:{}] {summary}", plugin_name);
                    panicked.push((idx, summary));
                }
            }
            ctx.clear_active_plugin();
        }
        for event in watchdog_events {
            self.log_watchdog_event(event);
        }
        for (idx, reason) in panicked {
            self.mark_plugin_failed(idx, reason);
        }
    }

    pub fn shutdown(&mut self, ctx: &mut PluginContext<'_>) {
        for slot in &mut self.plugins {
            if slot.failed_reason.is_some() {
                continue;
            }
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

    fn invalidate_status_cache(&mut self) {
        self.status_snapshot = None;
    }

    fn push_status(&mut self, status: PluginStatus) {
        self.statuses.push(status);
        self.invalidate_status_cache();
    }

    pub fn statuses(&self) -> &[PluginStatus] {
        &self.statuses
    }

    pub fn status_snapshot(&mut self) -> Arc<[PluginStatus]> {
        if let Some(cache) = &self.status_snapshot {
            return Arc::clone(cache);
        }
        let arc = Arc::from(self.statuses.clone().into_boxed_slice());
        self.status_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub fn is_plugin_loaded(&self, name: &str) -> bool {
        self.plugins.iter().any(|slot| slot.name == name)
    }

    pub fn unload_dynamic(&mut self, ctx: &mut PluginContext<'_>) {
        self.unload_dynamic_plugins(ctx);
    }

    pub fn clear_dynamic_statuses(&mut self) {
        self.statuses.retain(|status| !status.dynamic);
        self.invalidate_status_cache();
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
                if DEFAULT_ENGINE_FEATURES.contains(&feature.as_str()) {
                    continue;
                }
                if !still_provided.contains(&feature) {
                    registry.unregister(&feature);
                }
            }
        }

        self.clear_dynamic_statuses();
    }

    #[allow(clippy::too_many_arguments)]
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
        self.push_status(PluginStatus {
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
            asset_filters: PluginAssetFilters::default(),
            failed_reason: None,
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
        if entry.trust == PluginTrust::Isolated {
            return self.load_isolated_entry(entry, plugin_path, ctx);
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

    fn load_isolated_entry(
        &mut self,
        entry: &PluginManifestEntry,
        plugin_path: PathBuf,
        ctx: &mut PluginContext<'_>,
    ) -> Result<String> {
        let proxy = IsolatedPluginProxy::new(entry, plugin_path)?;
        self.insert_plugin(
            Box::new(proxy),
            None,
            true,
            entry.provides_features.clone(),
            entry.capabilities.clone(),
            entry.trust,
            ctx,
        )?;
        if let Some(slot) = self.plugins.iter_mut().find(|slot| slot.name == entry.name) {
            slot.asset_filters = entry.asset_filters.clone();
        }
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

struct IsolatedPluginProxy {
    name: String,
    version: String,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    terminated: bool,
    next_request_id: RpcRequestId,
    asset_budget: AssetReadbackBudget,
    last_request_desc: Option<String>,
    pending_watchdog: Option<PluginWatchdogEvent>,
    pending_capability_violations: Vec<PluginCapability>,
}

struct AssetReadbackBudget {
    window: Duration,
    max_requests: u32,
    max_bytes: u64,
    requests: u32,
    bytes: u64,
    window_start: Instant,
}

impl AssetReadbackBudget {
    fn new(max_requests: u32, max_bytes: u64, window: Duration) -> Self {
        Self { window, max_requests, max_bytes, requests: 0, bytes: 0, window_start: Instant::now() }
    }

    fn reset_if_needed(&mut self) {
        if self.window_start.elapsed() >= self.window {
            self.window_start = Instant::now();
            self.requests = 0;
            self.bytes = 0;
        }
    }

    fn begin_request(&mut self) -> Result<()> {
        self.reset_if_needed();
        if self.requests >= self.max_requests {
            bail!("isolated asset readback budget exceeded (request count)");
        }
        self.requests += 1;
        Ok(())
    }

    fn finalize(&mut self, byte_len: u64) -> Result<()> {
        self.bytes = self.bytes.saturating_add(byte_len);
        if self.bytes > self.max_bytes {
            bail!("isolated asset readback budget exceeded (byte budget)");
        }
        Ok(())
    }
}

impl IsolatedPluginProxy {
    fn new(entry: &PluginManifestEntry, plugin_path: PathBuf) -> Result<Self> {
        let version = entry.version.clone().unwrap_or_else(|| "0.1.0".to_string());
        let host_path = Self::host_binary_path().context("resolve isolated host binary")?;
        let mut command = Command::new(host_path);
        command
            .arg("--plugin")
            .arg(&plugin_path)
            .arg("--name")
            .arg(entry.name.as_str())
            .arg("--trust")
            .arg(entry.trust.label().to_ascii_lowercase());
        for capability in &entry.capabilities {
            command.arg("--cap").arg(capability.label());
        }
        let cwd = env::current_dir().context("resolve working directory for isolated host")?;
        command.current_dir(&cwd);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to spawn isolated host for plugin '{}' ({})",
                    entry.name,
                    plugin_path.display()
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("isolated host missing stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("isolated host missing stdout"))?;
        Ok(Self {
            name: entry.name.clone(),
            version,
            child,
            stdin: Some(stdin),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))),
            terminated: false,
            next_request_id: 1,
            asset_budget: AssetReadbackBudget::new(8, 4 * 1024 * 1024, Duration::from_millis(250)),
            last_request_desc: None,
            pending_watchdog: None,
            pending_capability_violations: Vec::new(),
        })
    }

    fn host_binary_path() -> Result<PathBuf> {
        if let Ok(explicit) = env::var("CARGO_BIN_EXE_kestrel_plugin_host") {
            let candidate = PathBuf::from(explicit);
            if candidate.exists() {
                return Ok(candidate);
            }
        }
        let current = env::current_exe().context("locate engine executable")?;
        let filename = if cfg!(windows) { "kestrel_plugin_host.exe" } else { "kestrel_plugin_host" };
        let mut host_path = current.clone();
        host_path.set_file_name(filename);
        if host_path.exists() {
            return Ok(host_path);
        }
        if let Some(parent) = current.parent().and_then(|p| p.parent()) {
            let fallback = parent.join(filename);
            if fallback.exists() {
                return Ok(fallback);
            }
        }
        bail!("isolated host binary '{}' not found", host_path.display())
    }

    fn call_remote(
        &mut self,
        request: PluginHostRequest,
    ) -> Result<(Vec<GameEvent>, Vec<PluginCapability>, Option<RpcResponseData>)> {
        if self.terminated {
            bail!("isolated plugin host already terminated");
        }
        let stdin = self.stdin.as_mut().ok_or_else(|| anyhow!("isolated host stdin closed"))?;
        let summary = Self::summarize_request(&request);
        self.last_request_desc = Some(summary);
        send_frame(stdin, &request).context("send isolated plugin request")?;
        let start = Instant::now();
        let stdout = Arc::clone(&self.stdout);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = {
                let mut guard = stdout.lock().expect("stdout mutex poisoned");
                recv_frame(&mut *guard)
            };
            let _ = tx.send(result);
        });
        let response: PluginHostResponse = match rx.recv_timeout(ISOLATED_RPC_TIMEOUT) {
            Ok(result) => result.context("recv isolated plugin response")?,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let elapsed = start.elapsed();
                self.record_watchdog_event(
                    format!("RPC timeout after {:.1} ms", elapsed.as_secs_f32() * 1000.0),
                    elapsed,
                );
                self.terminated = true;
                let _ = self.child.kill();
                bail!("isolated plugin '{}' exceeded RPC timeout ({elapsed:?})", self.name);
            }
            Err(err) => {
                bail!("isolated plugin recv channel failed: {err}");
            }
        };
        self.last_request_desc = None;
        match response {
            PluginHostResponse::Ok { events, capability_violations, data } => {
                let caps: Vec<PluginCapability> =
                    capability_violations.into_iter().map(|evt| evt.capability).collect();
                self.pending_capability_violations = caps.clone();
                Ok((events.into_iter().map(Into::into).collect(), caps, data))
            }
            PluginHostResponse::Error { message, capability_violations } => {
                self.pending_capability_violations =
                    capability_violations.into_iter().map(|evt| evt.capability).collect();
                bail!("isolated plugin error: {message}")
            }
        }
    }

    fn record_watchdog_event(&mut self, reason: String, elapsed: Duration) {
        let last_request = self.last_request_desc.clone().unwrap_or_else(|| "unknown request".to_string());
        self.pending_watchdog = Some(PluginWatchdogEvent {
            plugin: self.name.to_string(),
            timestamp: SystemTime::now(),
            elapsed_ms: elapsed.as_secs_f32() * 1000.0,
            reason,
            last_request,
        });
    }

    fn take_watchdog_event(&mut self) -> Option<PluginWatchdogEvent> {
        self.pending_watchdog.take()
    }

    fn take_capability_violations(&mut self) -> Vec<PluginCapability> {
        std::mem::take(&mut self.pending_capability_violations)
    }

    fn summarize_request(request: &PluginHostRequest) -> String {
        match request {
            PluginHostRequest::Build => "Build".to_string(),
            PluginHostRequest::Shutdown => "Shutdown".to_string(),
            PluginHostRequest::Update { dt } => format!("Update(dt={dt:.3})"),
            PluginHostRequest::FixedUpdate { dt } => format!("FixedUpdate(dt={dt:.3})"),
            PluginHostRequest::OnEvents { events } => format!("OnEvents(count={})", events.len()),
            PluginHostRequest::QueryEntityInfo { entity } => {
                let entity: Entity = (*entity).into();
                format!("QueryEntityInfo(entity={})", entity.index())
            }
            PluginHostRequest::ReadComponents(payload) => {
                let entity: Entity = payload.entity.into();
                format!(
                    "ReadComponents(entity={}, comps={}, format={:?})",
                    entity.index(),
                    payload.components.len(),
                    payload.format
                )
            }
            PluginHostRequest::IterEntities(payload) => {
                format!(
                    "IterEntities(filter_comps={}, limit={})",
                    payload.filter.components.len(),
                    payload.limit
                )
            }
            PluginHostRequest::AssetReadback(payload) => {
                let (kind, target) = summarize_asset_payload(&payload.payload);
                format!("AssetReadback(kind={kind}, target={target})")
            }
        }
    }

    fn ensure_shutdown(&mut self) {
        if self.terminated {
            return;
        }
        if let Err(err) = self.call_remote(PluginHostRequest::Shutdown).map(|_| ()) {
            eprintln!("[plugin:{}] failed to shutdown isolated host: {err:?}", self.name);
        }
        self.terminated = true;
        self.stdin.take();
    }

    fn forward_with_ctx(&mut self, ctx: &mut PluginContext<'_>, request: PluginHostRequest) -> Result<()> {
        match self.call_remote(request) {
            Ok((events, caps, _)) => {
                for cap in caps {
                    ctx.log_capability_violation(&self.name, cap);
                }
                self.pending_capability_violations.clear();
                self.relay_events(ctx, events)
            }
            Err(err) => {
                for cap in self.take_capability_violations() {
                    ctx.log_capability_violation(&self.name, cap);
                }
                Err(err)
            }
        }
    }

    fn relay_events(&self, ctx: &mut PluginContext<'_>, events: Vec<GameEvent>) -> Result<()> {
        for event in events {
            ctx.emit_event(event)?;
        }
        Ok(())
    }

    fn query_entity_info(&mut self, entity: Entity) -> Result<Option<RemoteEntityInfo>> {
        let (events, _caps, payload) =
            self.call_remote(PluginHostRequest::QueryEntityInfo { entity: entity.into() })?;
        if !events.is_empty() {
            eprintln!(
                "[plugin:{}] query_entity_info returned unexpected events ({})",
                self.name,
                events.len()
            );
        }
        #[allow(unreachable_patterns)]
        match payload {
            Some(RpcResponseData::EntityInfo(info)) => Ok(info.map(RemoteEntityInfo::from)),
            Some(other) => bail!("unexpected payload from isolated host: {other:?}"),
            None => Ok(None),
        }
    }

    fn read_components(
        &mut self,
        entity: Entity,
        components: Vec<RpcComponentKind>,
        format: RpcSnapshotFormat,
    ) -> Result<RpcReadComponentsResponse> {
        let request_id = self.take_request_id();
        let request = PluginHostRequest::ReadComponents(RpcReadComponentsRequest {
            request_id,
            entity: entity.into(),
            components,
            format,
        });
        let (events, _caps, payload) = self.call_remote(request)?;
        if !events.is_empty() {
            eprintln!("[plugin:{}] read_components returned unexpected events ({})", self.name, events.len());
        }
        match payload {
            Some(RpcResponseData::ReadComponents(response)) if response.request_id == request_id => {
                Ok(response)
            }
            Some(other) => bail!("unexpected payload from isolated host: {other:?}"),
            None => bail!("isolated host returned no payload for ReadComponents"),
        }
    }

    fn iter_entities(
        &mut self,
        filter: RpcEntityFilter,
        cursor: Option<RpcIteratorCursor>,
        limit: u32,
        components: Vec<RpcComponentKind>,
        format: RpcSnapshotFormat,
    ) -> Result<RpcIterEntitiesResponse> {
        let request_id = self.take_request_id();
        let request = PluginHostRequest::IterEntities(RpcIterEntitiesRequest {
            request_id,
            filter,
            cursor,
            limit,
            components,
            format,
        });
        let (events, _caps, payload) = self.call_remote(request)?;
        if !events.is_empty() {
            eprintln!("[plugin:{}] iter_entities returned unexpected events ({})", self.name, events.len());
        }
        match payload {
            Some(RpcResponseData::IterEntities(response)) if response.request_id == request_id => {
                Ok(response)
            }
            Some(other) => bail!("unexpected payload from isolated host: {other:?}"),
            None => bail!("isolated host returned no payload for IterEntities"),
        }
    }

    fn asset_readback(&mut self, payload: RpcAssetReadbackPayload) -> Result<RpcAssetReadbackResponse> {
        self.asset_budget.begin_request()?;
        let request_id = self.take_request_id();
        let request = PluginHostRequest::AssetReadback(RpcAssetReadbackRequest { request_id, payload });
        let (events, _caps, response) = self.call_remote(request)?;
        if !events.is_empty() {
            eprintln!("[plugin:{}] asset_readback returned unexpected events ({})", self.name, events.len());
        }
        match response {
            Some(RpcResponseData::AssetReadback(payload)) if payload.request_id == request_id => {
                self.asset_budget.finalize(payload.byte_length)?;
                Ok(payload)
            }
            Some(other) => bail!("unexpected payload from isolated host: {other:?}"),
            None => bail!("isolated host returned no payload for AssetReadback"),
        }
    }

    fn take_request_id(&mut self) -> RpcRequestId {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        request_id
    }
}

impl EnginePlugin for IsolatedPluginProxy {
    fn name(&self) -> &str {
        &self.name
    }

    fn version(&self) -> &str {
        &self.version
    }

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        self.forward_with_ctx(ctx, PluginHostRequest::Build)
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        self.forward_with_ctx(ctx, PluginHostRequest::Update { dt })
    }

    fn fixed_update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        self.forward_with_ctx(ctx, PluginHostRequest::FixedUpdate { dt })
    }

    fn on_events(&mut self, ctx: &mut PluginContext<'_>, events: &[GameEvent]) -> Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let payload: Vec<RpcGameEvent> = events.iter().cloned().map(RpcGameEvent::from).collect();
        self.forward_with_ctx(ctx, PluginHostRequest::OnEvents { events: payload })
    }

    fn shutdown(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        if !self.terminated {
            self.forward_with_ctx(ctx, PluginHostRequest::Shutdown)?;
            self.terminated = true;
            self.stdin.take();
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl Drop for IsolatedPluginProxy {
    fn drop(&mut self) {
        self.ensure_shutdown();
        let _ = self.child.kill();
        let _ = self.child.wait();
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
    #[serde(default)]
    pub asset_filters: PluginAssetFilters,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginAssetFilters {
    #[serde(default)]
    pub atlases: Vec<String>,
    #[serde(default)]
    pub blobs: Vec<String>,
}

impl PluginAssetFilters {
    pub fn allows_atlas(&self, atlas_id: &str) -> bool {
        if self.atlases.is_empty() {
            return true;
        }
        self.atlases.iter().any(|pattern| matches_asset_pattern(pattern, atlas_id))
    }

    pub fn allows_blob(&self, blob_id: &str, trust: PluginTrust) -> bool {
        if self.blobs.is_empty() {
            return trust != PluginTrust::Isolated;
        }
        self.blobs.iter().any(|pattern| matches_asset_pattern(pattern, blob_id))
    }
}

fn matches_asset_pattern(pattern: &str, value: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        value.starts_with(prefix)
    } else {
        pattern == value
    }
}

fn ensure_asset_filter_allows(
    filters: &PluginAssetFilters,
    payload: &RpcAssetReadbackPayload,
    trust: PluginTrust,
) -> Result<()> {
    match payload {
        RpcAssetReadbackPayload::AtlasMeta { atlas_id }
        | RpcAssetReadbackPayload::AtlasBinary { atlas_id } => {
            if filters.allows_atlas(atlas_id) {
                Ok(())
            } else {
                bail!("asset readback blocked by manifest filters for atlas '{atlas_id}'")
            }
        }
        RpcAssetReadbackPayload::BlobRange { blob_id, .. } => {
            if filters.allows_blob(blob_id, trust) {
                Ok(())
            } else {
                bail!("asset readback blocked by manifest filters for blob '{blob_id}'")
            }
        }
    }
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
