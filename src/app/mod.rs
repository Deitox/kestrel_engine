mod animation_keyframe_panel;
mod animation_tooling;
mod animation_watch;
mod asset_watch_tooling;
mod atlas_watch;
mod editor_shell;
mod editor_ui;
mod gizmo_interaction;
mod inspector_tooling;
mod mesh_preview_tooling;
mod plugin_host;
mod plugin_runtime;
mod prefab_tooling;
mod runtime_loop;
mod script_console;
mod telemetry_tooling;

use self::animation_keyframe_panel::{
    AnimationKeyframePanelState, AnimationPanelCommand, AnimationTrackBinding, AnimationTrackId,
    AnimationTrackKind, AnimationTrackSummary, KeyframeDetail, KeyframeId, KeyframeValue,
};
use self::animation_watch::{AnimationAssetKind, AnimationAssetWatcher};
use self::atlas_watch::AtlasHotReload;
use self::editor_shell::{
    EditorShell, EditorUiState, EditorUiStateParams, EmitterUiDefaults, ScriptDebuggerStatus,
    ScriptHandleBinding,
};
use self::plugin_host::{BuiltinPluginFactory, PluginHost};
use self::plugin_runtime::{PluginContextInputs, PluginRuntime};
use self::runtime_loop::{RuntimeLoop, RuntimeTick};
pub(crate) use self::telemetry_tooling::FrameBudgetSnapshot;
use self::telemetry_tooling::GpuTimingFrame;
#[cfg(feature = "alloc_profiler")]
use crate::alloc_profiler;
use crate::analytics::{
    AnalyticsPlugin, AnimationBudgetSample, KeyframeEditorEvent, KeyframeEditorEventKind,
    KeyframeEditorTrackKind, KeyframeEditorUsageSnapshot,
};
use crate::animation_validation::{
    AnimationValidationEvent, AnimationValidationSeverity, AnimationValidator,
};
use crate::assets::skeletal;
use crate::assets::{parse_animation_clip_bytes, parse_animation_graph_bytes};
use crate::assets::{
    AnimationClip, AnimationGraphAsset, AssetManager, ClipInterpolation, ClipKeyframe, ClipScalarTrack,
    ClipSegment, ClipVec2Track, ClipVec4Track, SpriteTimeline, TextureAtlasDiagnostics,
};
use crate::audio::{AudioHealthSnapshot, AudioListenerState, AudioPlugin, AudioSpatialConfig};
use crate::camera::Camera2D;
use crate::camera3d::Camera3D;
use crate::config::{AppConfig, AppConfigOverrides, SpriteGuardrailMode};
use crate::ecs::{
    AnimationTime, ClipInstance, EcsWorld, EntityInfo, ForceField, InstanceData, MeshLightingInfo,
    ParticleAttractor, ParticleCaps, SkeletonInstance, SpriteAnimation, SpriteAnimationInfo, SpriteInstance,
};
use crate::environment::EnvironmentRegistry;
use crate::events::{AudioEmitter, GameEvent};
use crate::gizmo::{GizmoInteraction, GizmoMode};
use crate::input::{Input, InputEvent};
use crate::material_registry::{MaterialGpu, MaterialRegistry};
use crate::mesh_preview::{MeshControlMode, MeshPreviewPlugin};
use crate::mesh_registry::MeshRegistry;
use crate::plugins::{
    ManifestBuiltinToggle, ManifestDynamicToggle, PluginAssetReadbackEvent, PluginCapabilityEvent,
    PluginContext, PluginManager, PluginWatchdogEvent,
};
use crate::prefab::{PrefabFormat, PrefabLibrary};
use crate::renderer::{
    GpuPassTiming, MeshDraw, RenderViewport, Renderer, ScenePointLight, SpriteBatch, MAX_SHADOW_CASCADES,
};
use crate::scene::{
    EnvironmentDependency, Scene, SceneCamera2D, SceneCameraBookmark, SceneDependencies, SceneEntityId,
    SceneEnvironment, SceneLightingData, SceneMetadata, ScenePointLightData, SceneShadowData,
    SceneViewportMode, Vec2Data,
};
use crate::scripts::{ScriptCommand, ScriptHandle, ScriptPlugin};
use crate::time::Time;
use bevy_ecs::prelude::Entity;
use glam::{Mat4, Vec2, Vec3, Vec4};

use anyhow::{anyhow, Context, Result};
use std::cell::{Ref, RefMut};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
#[cfg(feature = "alloc_profiler")]
use std::env;
use std::fs;
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};

// egui
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use egui_winit::State as EguiWinit;

const CAMERA_BASE_HALF_HEIGHT: f32 = 1.2;
const MAX_PENDING_ANIMATION_RELOADS_PER_KIND: usize = 32;
const ANIMATION_RELOAD_WORKER_QUEUE_DEPTH: usize = 8;
const PLUGIN_MANIFEST_PATH: &str = "config/plugins.json";
const INPUT_CONFIG_PATH: &str = "config/input.json";
const SCRIPT_CONSOLE_CAPACITY: usize = 200;
const SCRIPT_HISTORY_CAPACITY: usize = 64;
const BINARY_PREFABS_ENABLED: bool = cfg!(feature = "binary_scene");
const MAX_FIXED_TIMESTEP_BACKLOG: f32 = 0.5;

struct SkeletonPlaybackSnapshot {
    entity: Entity,
    clip_key: Option<String>,
    time: f32,
    playing: bool,
    speed: f32,
    group: Option<String>,
}

fn default_graph_key(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ViewportCameraMode {
    Ortho2D,
    Perspective3D,
}

impl Default for ViewportCameraMode {
    fn default() -> Self {
        ViewportCameraMode::Ortho2D
    }
}

impl ViewportCameraMode {
    fn label(self) -> &'static str {
        match self {
            ViewportCameraMode::Ortho2D => "Orthographic 2D",
            ViewportCameraMode::Perspective3D => "Perspective 3D",
        }
    }
}

impl From<ViewportCameraMode> for SceneViewportMode {
    fn from(mode: ViewportCameraMode) -> Self {
        match mode {
            ViewportCameraMode::Ortho2D => SceneViewportMode::Ortho2D,
            ViewportCameraMode::Perspective3D => SceneViewportMode::Perspective3D,
        }
    }
}

impl From<SceneViewportMode> for ViewportCameraMode {
    fn from(mode: SceneViewportMode) -> Self {
        match mode {
            SceneViewportMode::Ortho2D => ViewportCameraMode::Ortho2D,
            SceneViewportMode::Perspective3D => ViewportCameraMode::Perspective3D,
        }
    }
}

#[derive(Clone, Copy)]
struct Viewport {
    origin: Vec2,
    size: Vec2,
}

impl Viewport {
    fn new(origin: Vec2, size: Vec2) -> Self {
        Self { origin, size }
    }

    fn contains(&self, point: Vec2) -> bool {
        point.x >= self.origin.x
            && point.y >= self.origin.y
            && point.x <= self.origin.x + self.size.x
            && point.y <= self.origin.y + self.size.y
    }

    fn size_physical(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(self.size.x.max(1.0).round() as u32, self.size.y.max(1.0).round() as u32)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ScriptConsoleEntry {
    pub kind: ScriptConsoleKind,
    pub text: String,
}

#[derive(Clone)]
struct ClipEditRecord {
    clip_key: String,
    before: Arc<AnimationClip>,
    after: Arc<AnimationClip>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptConsoleKind {
    Input,
    Output,
    Error,
    Log,
}

#[derive(Debug, Clone)]
struct CameraBookmark {
    name: String,
    position: Vec2,
    zoom: f32,
}

impl CameraBookmark {
    fn to_scene(&self) -> SceneCameraBookmark {
        SceneCameraBookmark {
            name: self.name.clone(),
            position: Vec2Data::from(self.position),
            zoom: self.zoom,
        }
    }

    fn from_scene(bookmark: &SceneCameraBookmark) -> Self {
        Self {
            name: bookmark.name.clone(),
            position: Vec2::from(bookmark.position.clone()),
            zoom: bookmark.zoom,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct FrameTimingSample {
    pub frame_ms: f32,
    pub update_ms: f32,
    pub fixed_ms: f32,
    pub render_ms: f32,
    pub ui_ms: f32,
}

pub async fn run() -> Result<()> {
    run_with_overrides(AppConfigOverrides::default()).await
}

pub async fn run_with_overrides(overrides: AppConfigOverrides) -> Result<()> {
    let mut config = AppConfig::load_or_default("config/app.json");
    let precedence_note = "Precedence: CLI overrides > config/app.json > defaults.";
    if overrides.is_empty() {
        println!("[config] {precedence_note} No CLI overrides supplied.");
    } else {
        let fields = overrides.applied_fields();
        if !fields.is_empty() {
            println!("[config] {precedence_note} CLI overrides applied for: {}.", fields.join(", "));
        }
    }
    config.apply_overrides(&overrides);
    let event_loop = EventLoop::new().context("Failed to create winit event loop")?;
    let mut app = App::new(config).await;
    event_loop.run_app(&mut app).context("Event loop execution failed")?;
    Ok(())
}

pub struct App {
    pub(crate) renderer: Renderer,
    pub(crate) ecs: EcsWorld,
    runtime_loop: RuntimeLoop,
    pub(crate) input: Input,
    assets: AssetManager,
    prefab_library: PrefabLibrary,
    environment_registry: EnvironmentRegistry,
    persistent_environments: HashSet<String>,
    scene_environment_ref: Option<String>,
    active_environment_key: String,
    environment_intensity: f32,
    should_close: bool,

    // egui
    editor_shell: EditorShell,

    // Plugins
    plugin_runtime: PluginRuntime,

    // Camera / selection
    pub(crate) camera: Camera2D,
    pub(crate) viewport_camera_mode: ViewportCameraMode,
    camera_follow_target: Option<SceneEntityId>,

    // Configuration
    config: AppConfig,

    scene_atlas_refs: HashSet<String>,
    persistent_atlases: HashSet<String>,
    scene_clip_refs: HashMap<String, usize>,
    scene_mesh_refs: HashSet<String>,
    pub(crate) scene_material_refs: HashSet<String>,

    pub(crate) material_registry: MaterialRegistry,
    pub(crate) mesh_registry: MeshRegistry,

    viewport: Viewport,
    #[cfg(feature = "alloc_profiler")]
    last_alloc_snapshot: alloc_profiler::AllocationSnapshot,
    #[cfg(feature = "alloc_profiler")]
    frame_budget_capture: Option<FrameBudgetCaptureScript>,

    // Particles
    emitter_entity: Option<Entity>,

    sprite_atlas_views: HashMap<String, Arc<wgpu::TextureView>>,
    atlas_hot_reload: Option<AtlasHotReload>,
    animation_asset_watcher: Option<AnimationAssetWatcher>,
    animation_watch_roots_queue: Vec<(PathBuf, AnimationAssetKind)>,
    animation_watch_roots_pending: HashSet<(PathBuf, AnimationAssetKind)>,
    animation_watch_roots_registered: HashSet<(PathBuf, AnimationAssetKind)>,
    animation_reload_pending: HashSet<(PathBuf, AnimationAssetKind)>,
    animation_reload_queue: AnimationReloadQueue,
    animation_reload_worker: Option<AnimationReloadWorker>,
    animation_validation_worker: Option<AnimationValidationWorker>,
    sprite_guardrail_mode: SpriteGuardrailMode,
    sprite_guardrail_max_pixels: f32,
    sprite_batch_map: HashMap<Arc<str>, Vec<InstanceData>>,
    sprite_batch_pool: Vec<Vec<InstanceData>>,
    sprite_batch_order: Vec<Arc<str>>,
}

impl App {
    fn editor_ui_state(&self) -> Ref<'_, EditorUiState> {
        self.editor_shell.ui_state()
    }

    fn editor_ui_state_mut(&self) -> RefMut<'_, EditorUiState> {
        self.editor_shell.ui_state_mut()
    }

    fn with_editor_ui_state_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut EditorUiState) -> R,
    {
        let mut state = self.editor_ui_state_mut();
        f(&mut state)
    }

    fn selected_entity(&self) -> Option<Entity> {
        self.editor_ui_state().selected_entity
    }

    fn set_selected_entity(&self, entity: Option<Entity>) {
        self.editor_ui_state_mut().selected_entity = entity;
    }

    fn gizmo_mode(&self) -> GizmoMode {
        self.editor_ui_state().gizmo_mode
    }

    fn set_gizmo_mode(&self, mode: GizmoMode) {
        self.with_editor_ui_state_mut(|state| {
            if state.gizmo_mode != mode {
                state.gizmo_mode = mode;
                state.gizmo_interaction = None;
            }
        });
    }

    fn gizmo_interaction(&self) -> Option<GizmoInteraction> {
        self.editor_ui_state().gizmo_interaction
    }

    fn set_gizmo_interaction(&self, interaction: Option<GizmoInteraction>) {
        self.editor_ui_state_mut().gizmo_interaction = interaction;
    }

    fn take_gizmo_interaction(&self) -> Option<GizmoInteraction> {
        self.with_editor_ui_state_mut(|state| state.gizmo_interaction.take())
    }

    fn record_frame_timing_sample(&self, sample: FrameTimingSample) {
        self.with_editor_ui_state_mut(|state| state.frame_profiler.push(sample));
    }

    fn latest_frame_timing(&self) -> Option<FrameTimingSample> {
        self.editor_ui_state().frame_profiler.latest()
    }

    fn camera_bookmarks(&self) -> Vec<CameraBookmark> {
        self.editor_ui_state().camera_bookmarks.clone()
    }

    fn active_camera_bookmark(&self) -> Option<String> {
        self.editor_ui_state().active_camera_bookmark.clone()
    }

    fn set_active_camera_bookmark(&self, bookmark: Option<String>) {
        self.editor_ui_state_mut().active_camera_bookmark = bookmark;
    }

    fn update_gpu_timing_snapshots(&self, timings: Vec<GpuPassTiming>) {
        if timings.is_empty() {
            return;
        }
        let arc_timings = Arc::from(timings.clone().into_boxed_slice());
        self.with_editor_ui_state_mut(|state| {
            state.gpu_timings = Arc::clone(&arc_timings);
            state.gpu_frame_counter = state.gpu_frame_counter.saturating_add(1);
            state
                .gpu_timing_history
                .push_back(GpuTimingFrame { frame_index: state.gpu_frame_counter, timings });
            while state.gpu_timing_history.len() > state.gpu_timing_history_capacity {
                state.gpu_timing_history.pop_front();
            }
        });
    }

    fn set_ui_scene_status(&self, message: impl Into<String>) {
        self.editor_ui_state_mut().ui_scene_status = Some(message.into());
    }

    fn preview_sprite_events(&mut self, atlas: &str, timeline: &str, frame_index: usize) {
        if let Some(timeline_data) = self.assets.atlas_timeline(atlas, timeline) {
            if let Some(frame) = timeline_data.frames.get(frame_index) {
                if frame.events.is_empty() {
                    self.set_inspector_status(Some(format!(
                        "Preview events: none (frame {})",
                        frame_index + 1
                    )));
                } else {
                    let joined = frame.events.join(", ");
                    println!(
                        "[animation] Preview events for {}::{} frame {} => {}",
                        atlas,
                        timeline,
                        frame_index + 1,
                        joined
                    );
                    self.set_inspector_status(Some(format!("Preview events: {}", joined)));
                }
            } else {
                self.set_inspector_status(Some("Preview events unavailable for frame.".to_string()));
            }
        } else {
            self.set_inspector_status(Some("Timeline unavailable for preview.".to_string()));
        }
    }

    fn set_sprite_guardrail_status(&self, status: Option<String>) {
        self.with_editor_ui_state_mut(|state| state.sprite_guardrail_status = status);
    }

    pub fn hot_reload_atlas(&mut self, key: &str) -> Result<(usize, TextureAtlasDiagnostics)> {
        let diagnostics = self.assets.reload_atlas(key)?;
        self.invalidate_atlas_view(key);
        let refreshed = self.ecs.refresh_sprite_animations_for_atlas(key, &self.assets);
        Ok((refreshed, diagnostics))
    }

    fn drain_animation_validation_results(&mut self) {
        let Some(worker) = self.animation_validation_worker.as_ref() else {
            return;
        };
        for result in worker.drain() {
            self.handle_validation_events(result.kind.label(), result.path.as_path(), result.events);
        }
    }

    fn prepare_animation_reload_request(
        &self,
        path: PathBuf,
        kind: AnimationAssetKind,
    ) -> Option<AnimationReloadRequest> {
        let key = match kind {
            AnimationAssetKind::Clip => self.assets.clip_key_for_source_path(&path)?,
            AnimationAssetKind::Graph => {
                self.assets.graph_key_for_source_path(&path).unwrap_or_else(|| default_graph_key(&path))
            }
            AnimationAssetKind::Skeletal => self.assets.skeleton_key_for_source_path(&path)?,
        };
        Some(AnimationReloadRequest { path, key, kind, skip_validation: false })
    }

    fn enqueue_animation_reload(&mut self, request: AnimationReloadRequest) {
        let pending_key = (request.path.clone(), request.kind);
        if !self.animation_reload_pending.insert(pending_key.clone()) {
            return;
        }
        if let Some(evicted) = self.animation_reload_queue.enqueue(request) {
            self.animation_reload_pending.remove(&(evicted.path.clone(), evicted.kind));
            eprintln!(
                "[animation] dropping stale reload for {} ({}) - superseded by newer events",
                evicted.path.display(),
                evicted.kind.label()
            );
        }
        self.dispatch_animation_reload_queue();
    }

    fn dispatch_animation_reload_queue(&mut self) {
        loop {
            let Some(request) = self.animation_reload_queue.pop_next() else {
                break;
            };
            match self.try_submit_animation_reload(request) {
                Ok(()) => continue,
                Err(request) => {
                    if let Some(evicted) = self.animation_reload_queue.push_front(request) {
                        self.animation_reload_pending.remove(&(evicted.path.clone(), evicted.kind));
                        eprintln!(
                            "[animation] dropping queued reload for {} ({}) - queue saturated",
                            evicted.path.display(),
                            evicted.kind.label()
                        );
                    }
                    break;
                }
            }
        }
    }

    fn try_submit_animation_reload(
        &mut self,
        request: AnimationReloadRequest,
    ) -> Result<(), AnimationReloadRequest> {
        if let Some(worker) = self.animation_reload_worker.as_ref() {
            match worker.submit(AnimationReloadJob { request }) {
                Ok(()) => Ok(()),
                Err(job) => Err(job.request),
            }
        } else {
            let result = run_animation_reload_job(AnimationReloadJob { request });
            self.apply_animation_reload_result(result);
            Ok(())
        }
    }

    fn drain_animation_reload_results(&mut self) {
        if let Some(worker) = self.animation_reload_worker.as_ref() {
            for result in worker.drain() {
                self.apply_animation_reload_result(result);
            }
        }
    }

    fn apply_animation_reload_result(&mut self, result: AnimationReloadResult) {
        self.animation_reload_pending.remove(&(result.request.path.clone(), result.request.kind));
        match result.data {
            Ok(AnimationReloadData::Clip { clip, bytes }) => {
                let key = result.request.key.clone();
                let path_string = result.request.path.to_string_lossy().to_string();
                self.assets.replace_clip(&key, &path_string, clip);
                self.queue_animation_watch_root(&result.request.path, AnimationAssetKind::Clip);
                if let Some(updated) = self.assets.clip(&key) {
                    let canonical = Arc::new(updated.clone());
                    {
                        let mut state = self.editor_ui_state_mut();
                        state.clip_edit_overrides.remove(&key);
                        state.clip_dirty.remove(&key);
                        state.animation_clip_status =
                            Some(format!("Reloaded clip '{}' from {}", key, result.request.path.display()));
                    }
                    self.apply_clip_override_to_instances(&key, Arc::clone(&canonical));
                }
                if !result.request.skip_validation {
                    self.enqueue_animation_validation_job(AnimationAssetReload {
                        path: result.request.path.clone(),
                        kind: AnimationAssetKind::Clip,
                        bytes: Some(bytes),
                    });
                }
            }
            Ok(AnimationReloadData::Graph { graph, bytes }) => {
                let key = result.request.key.clone();
                let path_string = result.request.path.to_string_lossy().to_string();
                self.assets.replace_animation_graph(&key, &path_string, graph);
                self.queue_animation_watch_root(&result.request.path, AnimationAssetKind::Graph);
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status = Some(format!(
                        "Reloaded animation graph '{}' from {}",
                        key,
                        result.request.path.display()
                    ));
                });
                if !result.request.skip_validation {
                    self.enqueue_animation_validation_job(AnimationAssetReload {
                        path: result.request.path.clone(),
                        kind: AnimationAssetKind::Graph,
                        bytes: Some(bytes),
                    });
                }
            }
            Ok(AnimationReloadData::Skeletal { import }) => {
                let key = result.request.key.clone();
                let path_string = result.request.path.to_string_lossy().to_string();
                self.assets.replace_skeleton_from_import(&key, &path_string, import);
                self.queue_animation_watch_root(&result.request.path, AnimationAssetKind::Skeletal);
                let mut snapshots: Vec<SkeletonPlaybackSnapshot> = Vec::new();
                {
                    let mut query = self.ecs.world.query::<(Entity, &SkeletonInstance)>();
                    for (entity, instance) in query.iter(&self.ecs.world) {
                        if instance.skeleton_key.as_ref() == key.as_str() {
                            snapshots.push(SkeletonPlaybackSnapshot {
                                entity,
                                clip_key: instance.active_clip_key.as_ref().map(|k| k.as_ref().to_string()),
                                time: instance.time,
                                playing: instance.playing,
                                speed: instance.speed,
                                group: instance.group.clone(),
                            });
                        }
                    }
                }
                for snapshot in snapshots {
                    self.ecs.set_skeleton(snapshot.entity, &self.assets, &key);
                    if let Some(ref clip_key) = snapshot.clip_key {
                        let _ = self.ecs.set_skeleton_clip(snapshot.entity, &self.assets, clip_key);
                        let _ = self.ecs.set_skeleton_clip_time(snapshot.entity, snapshot.time);
                        let _ = self.ecs.set_skeleton_clip_playing(snapshot.entity, snapshot.playing);
                        let _ = self.ecs.set_skeleton_clip_speed(snapshot.entity, snapshot.speed);
                        let _ = self.ecs.set_skeleton_clip_group(snapshot.entity, snapshot.group.as_deref());
                    }
                }
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status =
                        Some(format!("Reloaded skeleton '{}' from {}", key, result.request.path.display()));
                });
                if !result.request.skip_validation {
                    self.enqueue_animation_validation_job(AnimationAssetReload {
                        path: result.request.path.clone(),
                        kind: AnimationAssetKind::Skeletal,
                        bytes: None,
                    });
                }
            }
            Err(err) => {
                eprintln!("[animation] reload failed for {}: {err:?}", result.request.path.display());
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status = Some(format!(
                        "Reload failed for {} from {}: {err}",
                        result.request.key,
                        result.request.path.display()
                    ));
                });
            }
        }
    }

    fn enqueue_animation_validation_job(&mut self, reload: AnimationAssetReload) {
        let mut job = AnimationValidationJob { path: reload.path, kind: reload.kind, bytes: reload.bytes };
        if let Some(worker) = self.animation_validation_worker.as_ref() {
            match worker.submit(job) {
                Ok(()) => return,
                Err(returned) => job = returned,
            }
        }
        let result = run_animation_validation_job(job);
        self.handle_validation_events(result.kind.label(), result.path.as_path(), result.events);
    }

    fn handle_validation_events(
        &mut self,
        context: &str,
        path: &Path,
        events: Vec<AnimationValidationEvent>,
    ) {
        if events.is_empty() {
            eprintln!(
                "[animation] detected change for {} ({context}) but no validations ran",
                path.display()
            );
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status =
                    Some(format!("Detected {context} change but no validators ran: {}", path.display()));
            });
            return;
        }
        for event in events {
            self.with_editor_ui_state_mut(|state| {
                state.pending_animation_validation_events.push(event.clone())
            });
            self.log_animation_validation_event(event);
        }
    }

    fn record_atlas_validation_results(&mut self, key: &str, diagnostics: TextureAtlasDiagnostics) {
        let Some(source_path) = self.assets.atlas_source(key).map(|s| s.to_string()) else {
            eprintln!("[animation] atlas '{key}' hot-reloaded without a recorded source path");
            return;
        };
        let path_buf = PathBuf::from(&source_path);
        if self.consume_validation_suppression(&path_buf) {
            return;
        }
        let mut events = Vec::new();
        let info_message = if let Some(snapshot) = self.assets.atlas_snapshot(key) {
            let region_count = snapshot.regions.len();
            let timeline_count = snapshot.animations.len();
            let image_label = snapshot.image_path.display().to_string();
            format!(
                "Parsed atlas '{key}' with {region_count} region{} and {timeline_count} timeline{} (image: {image_label}).",
                if region_count == 1 { "" } else { "s" },
                if timeline_count == 1 { "" } else { "s" }
            )
        } else {
            format!("Reloaded atlas '{key}' ({source_path})")
        };
        events.push(AnimationValidationEvent {
            severity: AnimationValidationSeverity::Info,
            path: path_buf.clone(),
            message: info_message,
        });
        for warning in diagnostics.warnings {
            events.push(AnimationValidationEvent {
                severity: AnimationValidationSeverity::Warning,
                path: path_buf.clone(),
                message: warning,
            });
        }
        for event in events {
            self.with_editor_ui_state_mut(|state| {
                state.pending_animation_validation_events.push(event.clone())
            });
            self.log_animation_validation_event(event);
        }
    }

    fn suppress_validation_for_path(&mut self, path: &Path) {
        let normalized = Self::normalize_validation_path(path);
        self.with_editor_ui_state_mut(|state| {
            state.suppressed_validation_paths.insert(normalized);
        });
    }

    fn consume_validation_suppression(&mut self, path: &Path) -> bool {
        let normalized = Self::normalize_validation_path(path);
        self.with_editor_ui_state_mut(|state| state.suppressed_validation_paths.remove(&normalized))
    }

    fn normalize_validation_path(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    fn log_animation_validation_event(&mut self, event: AnimationValidationEvent) {
        let severity = event.severity.to_string();
        let formatted =
            format!("[animation] validation {severity} for {}: {}", event.path.display(), event.message);
        eprintln!("{formatted}");
        self.with_editor_ui_state_mut(|state| state.animation_clip_status = Some(formatted.clone()));
        if matches!(event.severity, AnimationValidationSeverity::Warning | AnimationValidationSeverity::Error)
        {
            self.set_inspector_status(Some(formatted));
        }
    }

    fn drain_animation_validation_events(&mut self) -> Vec<AnimationValidationEvent> {
        self.with_editor_ui_state_mut(|state| std::mem::take(&mut state.pending_animation_validation_events))
    }

    fn process_atlas_hot_reload_events(&mut self) {
        let keys = if let Some(watcher) = self.atlas_hot_reload.as_mut() {
            watcher.drain_keys()
        } else {
            Vec::new()
        };
        if keys.is_empty() {
            return;
        }
        let mut unique = keys;
        unique.sort();
        unique.dedup();
        for key in unique {
            match self.hot_reload_atlas(&key) {
                Ok((updated, diagnostics)) => {
                    println!(
                        "[assets] Hot reloaded atlas '{key}' ({updated} animation component{} refreshed)",
                        if updated == 1 { "" } else { "s" }
                    );
                    self.record_atlas_validation_results(&key, diagnostics);
                }
                Err(err) => {
                    eprintln!("[assets] Failed to hot reload atlas '{key}': {err}");
                }
            }
        }
    }

    fn refresh_camera_follow(&mut self) -> bool {
        let Some(target_id) = self.camera_follow_target.as_ref().map(|id| id.as_str().to_string()) else {
            return false;
        };
        let Some(entity) = self.ecs.find_entity_by_scene_id(&target_id) else {
            return false;
        };
        let Some(info) = self.ecs.entity_info(entity) else {
            return false;
        };
        self.camera.position = info.translation;
        true
    }

    fn apply_camera_bookmark_by_name(&mut self, name: &str) -> bool {
        let bookmark = {
            let state = self.editor_ui_state();
            state.camera_bookmarks.iter().find(|b| b.name == name).cloned()
        };
        if let Some(bookmark) = bookmark {
            self.camera.position = bookmark.position;
            self.camera.set_zoom(bookmark.zoom);
            self.set_active_camera_bookmark(Some(bookmark.name.clone()));
            self.camera_follow_target = None;
            true
        } else {
            false
        }
    }

    fn upsert_camera_bookmark(&mut self, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        let bookmark_name = trimmed.to_string();
        let position = self.camera.position;
        let zoom = self.camera.zoom;
        self.with_editor_ui_state_mut(|state| {
            if let Some(existing) = state.camera_bookmarks.iter_mut().find(|b| b.name == trimmed) {
                existing.position = position;
                existing.zoom = zoom;
            } else {
                state.camera_bookmarks.push(CameraBookmark { name: bookmark_name.clone(), position, zoom });
                state.camera_bookmarks.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            state.active_camera_bookmark = Some(bookmark_name);
        });
        self.camera_follow_target = None;
        true
    }

    fn delete_camera_bookmark(&mut self, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        let mut removed = false;
        self.with_editor_ui_state_mut(|state| {
            let before = state.camera_bookmarks.len();
            state.camera_bookmarks.retain(|bookmark| bookmark.name != trimmed);
            if state.camera_bookmarks.len() != before {
                if state.active_camera_bookmark.as_deref() == Some(trimmed) {
                    state.active_camera_bookmark = None;
                }
                removed = true;
            }
        });
        removed
    }

    fn set_camera_follow_scene_id(&mut self, scene_id: SceneEntityId) -> bool {
        self.camera_follow_target = Some(scene_id);
        if self.refresh_camera_follow() {
            self.set_active_camera_bookmark(None);
            true
        } else {
            self.camera_follow_target = None;
            false
        }
    }

    fn clear_camera_follow(&mut self) {
        self.camera_follow_target = None;
    }

    fn reload_dynamic_plugins(&mut self) {
        let result =
            self.with_plugin_runtime(|host, manager, ctx| host.reload_dynamic_from_disk(manager, ctx));
        match result {
            Ok(newly_loaded) => {
                if newly_loaded.is_empty() {
                    self.set_ui_scene_status("Plugin manifest reloaded".to_string());
                } else {
                    self.set_ui_scene_status(format!("Loaded plugins: {}", newly_loaded.join(", ")));
                }
            }
            Err(err) => {
                self.set_ui_scene_status(format!("Plugin reload failed: {err}"));
            }
        }
    }

    fn apply_plugin_toggles(&mut self, toggles: &[editor_ui::PluginToggleRequest]) {
        if toggles.is_empty() {
            return;
        }
        let mut dynamic_requests = Vec::new();
        let mut builtin_requests = Vec::new();
        for toggle in toggles {
            match &toggle.kind {
                editor_ui::PluginToggleKind::Dynamic { new_enabled } => dynamic_requests
                    .push(ManifestDynamicToggle { name: toggle.name.clone(), new_enabled: *new_enabled }),
                editor_ui::PluginToggleKind::Builtin { disable } => builtin_requests
                    .push(ManifestBuiltinToggle { name: toggle.name.clone(), disable: *disable }),
            }
        }
        let summary =
            match self.plugin_host_mut().apply_manifest_toggles(&dynamic_requests, &builtin_requests) {
                Ok(summary) => summary,
                Err(err) => {
                    self.set_ui_scene_status(format!("Plugin manifest update failed: {err}"));
                    if let Err(load_err) = self.plugin_host_mut().reload_manifest_from_disk() {
                        eprintln!("[plugin] failed to reload manifest after error: {load_err:?}");
                    }
                    return;
                }
            };
        if !summary.changed() {
            if !summary.dynamic.missing.is_empty() {
                self.set_ui_scene_status(format!(
                    "Plugin toggle skipped; missing manifest entr{} {}",
                    if summary.dynamic.missing.len() == 1 { "y:" } else { "ies:" },
                    summary.dynamic.missing.join(", ")
                ));
                if let Err(err) = self.plugin_host_mut().reload_manifest_from_disk() {
                    eprintln!("[plugin] failed to reload manifest after missing entries: {err:?}");
                }
            } else {
                self.set_ui_scene_status("Plugin manifest unchanged.".to_string());
            }
            return;
        }
        self.reload_dynamic_plugins();
        let mut parts = Vec::new();
        if !summary.dynamic.enabled.is_empty() {
            parts.push(format!("enabled {}", summary.dynamic.enabled.join(", ")));
        }
        if !summary.dynamic.disabled.is_empty() {
            parts.push(format!("disabled {}", summary.dynamic.disabled.join(", ")));
        }
        if !summary.builtin.enabled.is_empty() {
            parts.push(format!("enabled built-ins {}", summary.builtin.enabled.join(", ")));
        }
        if !summary.builtin.disabled.is_empty() {
            parts.push(format!("disabled built-ins {}", summary.builtin.disabled.join(", ")));
        }
        if !summary.dynamic.missing.is_empty() {
            parts.push(format!(
                "skipped unknown entr{} {}",
                if summary.dynamic.missing.len() == 1 { "y" } else { "ies" },
                summary.dynamic.missing.join(", ")
            ));
        }
        if summary.builtin.changed {
            parts.push("restart required for built-in changes".to_string());
        }
        if parts.is_empty() {
            self.set_ui_scene_status("Plugin manifest updated.".to_string());
        } else {
            self.set_ui_scene_status(format!("Plugin manifest {}", parts.join("; ")));
        }
    }
    pub async fn new(config: AppConfig) -> Self {
        let mut renderer = Renderer::new(&config.window).await;
        {
            let shadow_cfg = &config.shadow;
            let lighting = renderer.lighting_mut();
            lighting.shadow_cascade_count = shadow_cfg.cascade_count.clamp(1, MAX_SHADOW_CASCADES as u32);
            lighting.shadow_resolution = shadow_cfg.resolution.clamp(256, 8192);
            lighting.shadow_split_lambda = shadow_cfg.split_lambda.clamp(0.0, 1.0);
            lighting.shadow_pcf_radius = shadow_cfg.pcf_radius.clamp(0.0, 10.0);
        }
        renderer.mark_shadow_settings_dirty();
        let lighting_state = renderer.lighting().clone();
        let editor_lighting_state = lighting_state.clone();
        let particle_config = config.particles.clone();
        let editor_cfg = config.editor.clone();
        let mut ecs = EcsWorld::new();
        ecs.set_particle_caps(ParticleCaps::new(
            particle_config.max_spawn_per_frame,
            particle_config.max_total,
            particle_config.max_emitter_backlog,
        ));
        let emitter = ecs.spawn_demo_scene();
        let initial_events = ecs.drain_events();
        let emitter_snapshot = ecs.emitter_snapshot(emitter);
        let (
            ui_emitter_rate,
            ui_emitter_spread,
            ui_emitter_speed,
            ui_emitter_lifetime,
            ui_emitter_start_size,
            ui_emitter_end_size,
            ui_emitter_start_color,
            ui_emitter_end_color,
        ) = if let Some(snapshot) = emitter_snapshot {
            (
                snapshot.rate,
                snapshot.spread,
                snapshot.speed,
                snapshot.lifetime,
                snapshot.start_size,
                snapshot.end_size,
                snapshot.start_color.to_array(),
                snapshot.end_color.to_array(),
            )
        } else {
            (
                35.0,
                std::f32::consts::PI / 3.0,
                0.8,
                1.2,
                0.18,
                0.05,
                [1.0, 0.8, 0.2, 0.8],
                [1.0, 0.2, 0.2, 0.0],
            )
        };
        let emitter_defaults = EmitterUiDefaults {
            rate: ui_emitter_rate,
            spread: ui_emitter_spread,
            speed: ui_emitter_speed,
            lifetime: ui_emitter_lifetime,
            start_size: ui_emitter_start_size,
            end_size: ui_emitter_end_size,
            start_color: ui_emitter_start_color,
            end_color: ui_emitter_end_color,
        };
        let runtime_loop = RuntimeLoop::new(Time::new(), 1.0 / 60.0);
        let mut input = Input::from_config(INPUT_CONFIG_PATH);
        let mut assets = AssetManager::new();
        let mut prefab_library = PrefabLibrary::new("assets/prefabs");
        if let Err(err) = prefab_library.refresh() {
            eprintln!("[prefab] failed to scan prefabs: {err:?}");
        }
        let mut environment_registry = EnvironmentRegistry::new();
        let default_environment_key = environment_registry.default_key().to_string();
        let default_environment_intensity = 1.0;
        let mut persistent_environments = HashSet::new();
        persistent_environments.insert(default_environment_key.clone());
        match environment_registry.load_directory("assets/environments") {
            Ok(keys) => {
                for key in keys {
                    persistent_environments.insert(key);
                }
            }
            Err(err) => eprintln!("[environment] failed to scan assets/environments: {err:?}"),
        }
        let environment_intensity = default_environment_intensity;
        let mut material_registry = MaterialRegistry::new();
        let mut mesh_registry = MeshRegistry::new(&mut material_registry);
        let scene_material_refs = HashSet::new();
        let scene_clip_refs = HashMap::new();
        let ui_state = EditorUiState::new(EditorUiStateParams {
            emitter_defaults,
            particle_config: particle_config.clone(),
            lighting_state: editor_lighting_state,
            environment_intensity,
            editor_config: editor_cfg.clone(),
        });
        let editor_shell = EditorShell::new(ui_state);

        let plugin_host = PluginHost::new(PLUGIN_MANIFEST_PATH);
        let plugin_manager = PluginManager::default();
        let mut plugin_runtime = PluginRuntime::new(plugin_host, plugin_manager);
        let script_path = PathBuf::from("assets/scripts/main.rhai");
        let mut builtin_plugins = Vec::new();
        builtin_plugins
            .push(BuiltinPluginFactory::new("mesh_preview", || Box::new(MeshPreviewPlugin::new())));
        builtin_plugins.push(BuiltinPluginFactory::new("analytics", || Box::new(AnalyticsPlugin::default())));
        {
            let path = script_path.clone();
            builtin_plugins.push(BuiltinPluginFactory::new("scripts", move || {
                Box::new(ScriptPlugin::new(path.clone()))
            }));
        }
        builtin_plugins.push(BuiltinPluginFactory::new("audio", || Box::new(AudioPlugin::new(16))));
        plugin_runtime.with_context(
            PluginContextInputs {
                renderer: &mut renderer,
                ecs: &mut ecs,
                assets: &mut assets,
                input: &mut input,
                material_registry: &mut material_registry,
                mesh_registry: &mut mesh_registry,
                environment_registry: &mut environment_registry,
                time: runtime_loop.time(),
                event_emitter: Self::emit_event_for_plugin,
                selected_entity: None,
            },
            |host, manager, ctx| {
                host.register_builtins(manager, ctx, &builtin_plugins);
            },
        );
        if !initial_events.is_empty() {
            plugin_runtime.with_context(
                PluginContextInputs {
                    renderer: &mut renderer,
                    ecs: &mut ecs,
                    assets: &mut assets,
                    input: &mut input,
                    material_registry: &mut material_registry,
                    mesh_registry: &mut mesh_registry,
                    environment_registry: &mut environment_registry,
                    time: runtime_loop.time(),
                    event_emitter: Self::emit_event_for_plugin,
                    selected_entity: None,
                },
                |_, manager, ctx| {
                    manager.handle_events(ctx, &initial_events);
                },
            );
        }

        let atlas_hot_reload = match AtlasHotReload::new() {
            Ok(watcher) => Some(watcher),
            Err(err) => {
                eprintln!("[assets] atlas hot-reload disabled: {err}");
                None
            }
        };
        let animation_asset_watcher = Self::init_animation_asset_watcher();
        let animation_reload_worker = AnimationReloadWorker::new();
        let animation_reload_queue = AnimationReloadQueue::new(MAX_PENDING_ANIMATION_RELOADS_PER_KIND);
        let animation_validation_worker = AnimationValidationWorker::new();

        let mut camera = Camera2D::new(CAMERA_BASE_HALF_HEIGHT);
        camera.set_zoom_limits(editor_cfg.camera_zoom_min, editor_cfg.camera_zoom_max);

        #[cfg(feature = "alloc_profiler")]
        let frame_budget_capture = FrameBudgetCaptureScript::from_env();

        let mut app = Self {
            renderer,
            ecs,
            runtime_loop,
            input,
            assets,
            prefab_library,
            environment_registry,
            persistent_environments,
            scene_environment_ref: None,
            active_environment_key: default_environment_key.clone(),
            environment_intensity,
            should_close: false,
            editor_shell,
            plugin_runtime,
            camera,
            viewport_camera_mode: ViewportCameraMode::default(),
            camera_follow_target: None,
            scene_atlas_refs: HashSet::new(),
            persistent_atlases: HashSet::new(),
            scene_clip_refs,
            scene_mesh_refs: HashSet::new(),
            scene_material_refs,
            material_registry,
            mesh_registry,
            viewport: Viewport::new(
                Vec2::ZERO,
                Vec2::new(config.window.width as f32, config.window.height as f32),
            ),
            config,
            emitter_entity: Some(emitter),
            sprite_atlas_views: HashMap::new(),
            atlas_hot_reload,
            animation_asset_watcher,
            animation_watch_roots_queue: Vec::new(),
            animation_watch_roots_pending: HashSet::new(),
            animation_watch_roots_registered: HashSet::new(),
            animation_reload_pending: HashSet::new(),
            animation_reload_queue,
            animation_reload_worker,
            animation_validation_worker,
            sprite_guardrail_mode: editor_cfg.sprite_guardrail_mode,
            sprite_guardrail_max_pixels: editor_cfg.sprite_guard_max_pixels,
            #[cfg(feature = "alloc_profiler")]
            last_alloc_snapshot: alloc_profiler::allocation_snapshot(),
            #[cfg(feature = "alloc_profiler")]
            frame_budget_capture,
            sprite_batch_map: HashMap::new(),
            sprite_batch_pool: Vec::new(),
            sprite_batch_order: Vec::new(),
        };
        app.seed_animation_watch_roots();
        app.sync_animation_asset_watch_roots();
        app.apply_particle_caps();
        app.apply_editor_camera_settings();
        app.report_audio_startup_status();
        app
    }

    fn record_events(&mut self) {
        let listener = self.current_audio_listener_state();
        if let Some(audio) = self.audio_plugin_mut() {
            audio.set_listener_state(listener);
        }
        let events = self
            .ecs
            .drain_events()
            .into_iter()
            .map(|e| self.enrich_event_audio(e))
            .collect::<Vec<_>>();
        if events.is_empty() {
            return;
        }
        self.with_plugins(|plugins, ctx| plugins.handle_events(ctx, &events));
    }

    fn current_audio_listener_state(&self) -> AudioListenerState {
        match self.viewport_camera_mode {
            ViewportCameraMode::Ortho2D => AudioListenerState {
                position: Vec3::new(self.camera.position.x, self.camera.position.y, 0.0),
                forward: Vec3::new(0.0, 0.0, -1.0),
                up: Vec3::Y,
            },
            ViewportCameraMode::Perspective3D => {
                if let Some(cam) = self.mesh_preview_plugin().map(|p| p.mesh_camera().clone()) {
                    let forward = (cam.target - cam.position).normalize_or_zero();
                    AudioListenerState { position: cam.position, forward, up: cam.up }
                } else {
                    AudioListenerState { position: Vec3::new(0.0, 0.0, 5.0), forward: Vec3::new(0.0, 0.0, -1.0), up: Vec3::Y }
                }
            }
        }
    }

    fn enrich_event_audio(&self, event: GameEvent) -> GameEvent {
        const DEFAULT_MAX_DISTANCE: f32 = 25.0;
        match event {
            GameEvent::SpriteSpawned { entity, atlas, region, audio } => {
                let audio = audio.or_else(|| {
                    self.ecs
                        .entity_world_position3d(entity)
                        .map(|position| AudioEmitter { position, max_distance: DEFAULT_MAX_DISTANCE })
                });
                GameEvent::SpriteSpawned { entity, atlas, region, audio }
            }
            GameEvent::CollisionStarted { a, b, audio } => {
                let audio = audio.or_else(|| match (self.ecs.entity_world_position3d(a), self.ecs.entity_world_position3d(b)) {
                    (Some(pa), Some(pb)) => {
                        let mid = (pa + pb) * 0.5;
                        Some(AudioEmitter { position: mid, max_distance: DEFAULT_MAX_DISTANCE })
                    }
                    _ => None,
                });
                GameEvent::CollisionStarted { a, b, audio }
            }
            GameEvent::CollisionEnded { a, b, audio } => {
                let audio = audio.or_else(|| match (self.ecs.entity_world_position3d(a), self.ecs.entity_world_position3d(b)) {
                    (Some(pa), Some(pb)) => {
                        let mid = (pa + pb) * 0.5;
                        Some(AudioEmitter { position: mid, max_distance: DEFAULT_MAX_DISTANCE })
                    }
                    _ => None,
                });
                GameEvent::CollisionEnded { a, b, audio }
            }
            GameEvent::CollisionForce { a, b, force, audio } => {
                let audio = audio.or_else(|| match (self.ecs.entity_world_position3d(a), self.ecs.entity_world_position3d(b)) {
                    (Some(pa), Some(pb)) => {
                        let mid = (pa + pb) * 0.5;
                        Some(AudioEmitter { position: mid, max_distance: DEFAULT_MAX_DISTANCE })
                    }
                    _ => None,
                });
                GameEvent::CollisionForce { a, b, force, audio }
            }
            other => other,
        }
    }

    fn apply_sprite_guardrails(
        &mut self,
        sprite_instances: Vec<SpriteInstance>,
        viewport_size: PhysicalSize<u32>,
    ) -> Vec<SpriteInstance> {
        if sprite_instances.is_empty()
            || self.viewport_camera_mode != ViewportCameraMode::Ortho2D
            || viewport_size.width == 0
            || viewport_size.height == 0
            || self.sprite_guardrail_mode == SpriteGuardrailMode::Off
        {
            self.set_sprite_guardrail_status(None);
            return sprite_instances;
        }

        let Some(guardrail_projection) = SpriteGuardrailProjection::new(&self.camera, viewport_size) else {
            self.set_sprite_guardrail_status(None);
            return sprite_instances;
        };

        let threshold = self.sprite_guardrail_max_pixels.max(64.0);
        let mut filtered = Vec::with_capacity(sprite_instances.len());
        let mut largest_hit: f32 = 0.0;
        let mut culled = 0usize;
        for instance in sprite_instances {
            let mut oversized = false;
            let extent = guardrail_projection.extent(instance.world_half_extent);
            if extent > threshold {
                oversized = true;
                largest_hit = largest_hit.max(extent);
            }
            if oversized && self.sprite_guardrail_mode == SpriteGuardrailMode::Strict {
                culled += 1;
                continue;
            }
            filtered.push(instance);
        }

        if largest_hit > threshold {
            let status = match self.sprite_guardrail_mode {
                SpriteGuardrailMode::Warn => Some(format!(
                    "Zoom guardrail: sprite spans {:.0}px (limit {:.0}px).",
                    largest_hit, threshold
                )),
                SpriteGuardrailMode::Clamp => {
                    let prev_zoom = self.camera.zoom;
                    let ratio = (threshold / largest_hit).clamp(0.1, 1.0);
                    if ratio < 0.999 {
                        let desired_zoom = prev_zoom * ratio;
                        self.camera.set_zoom(desired_zoom);
                        self.set_active_camera_bookmark(None);
                        self.camera_follow_target = None;
                        Some(format!(
                            "Zoom guardrail clamped camera to {:.2} (sprite {:.0}px, limit {:.0}px).",
                            self.camera.zoom, largest_hit, threshold
                        ))
                    } else {
                        Some(format!(
                            "Zoom guardrail: sprite spans {:.0}px (limit {:.0}px).",
                            largest_hit, threshold
                        ))
                    }
                }
                SpriteGuardrailMode::Strict => Some(format!(
                    "Zoom guardrail hiding {culled} sprite(s) > {:.0}px (limit {:.0}px).",
                    largest_hit, threshold
                )),
                SpriteGuardrailMode::Off => None,
            };
            self.set_sprite_guardrail_status(status);
        } else {
            self.set_sprite_guardrail_status(None);
        }

        filtered
    }

    fn take_sprite_batch_buffer(&mut self) -> Vec<InstanceData> {
        self.sprite_batch_pool.pop().unwrap_or_else(Vec::new)
    }

    fn recycle_sprite_batch_buffers(&mut self) {
        if self.sprite_batch_map.is_empty() && self.sprite_batch_order.is_empty() {
            return;
        }
        for (_, mut instances) in self.sprite_batch_map.drain() {
            instances.clear();
            self.sprite_batch_pool.push(instances);
        }
        self.sprite_batch_order.clear();
    }
    fn apply_editor_camera_settings(&mut self) {
        let (zoom_min, zoom_max, guard_pixels, guard_mode) = {
            let mut state = self.editor_ui_state_mut();
            state.ui_camera_zoom_min = state.ui_camera_zoom_min.clamp(0.05, 20.0);
            state.ui_camera_zoom_max =
                state.ui_camera_zoom_max.max(state.ui_camera_zoom_min + 0.01).min(40.0);
            state.ui_sprite_guard_pixels = state.ui_sprite_guard_pixels.clamp(256.0, 8192.0);
            (
                state.ui_camera_zoom_min,
                state.ui_camera_zoom_max,
                state.ui_sprite_guard_pixels,
                state.ui_sprite_guard_mode,
            )
        };
        self.camera.set_zoom_limits(zoom_min, zoom_max);
        self.sprite_guardrail_mode = guard_mode;
        self.sprite_guardrail_max_pixels = guard_pixels;
        self.config.editor.camera_zoom_min = zoom_min;
        self.config.editor.camera_zoom_max = zoom_max;
        self.config.editor.sprite_guard_max_pixels = guard_pixels;
        self.config.editor.sprite_guardrail_mode = guard_mode;
    }

    fn apply_editor_lighting_settings(&mut self) {
        let (
            ui_light_direction,
            ui_light_color,
            ui_light_ambient,
            ui_light_exposure,
            ui_shadow_distance,
            ui_shadow_bias,
            ui_shadow_strength,
            ui_shadow_cascade_count,
            ui_shadow_resolution,
            ui_shadow_split_lambda,
            ui_shadow_pcf_radius,
        ) = {
            let state = self.editor_ui_state();
            (
                state.ui_light_direction,
                state.ui_light_color,
                state.ui_light_ambient,
                state.ui_light_exposure,
                state.ui_shadow_distance,
                state.ui_shadow_bias,
                state.ui_shadow_strength,
                state.ui_shadow_cascade_count,
                state.ui_shadow_resolution,
                state.ui_shadow_split_lambda,
                state.ui_shadow_pcf_radius,
            )
        };
        let default_dir = glam::Vec3::new(0.4, 0.8, 0.35).normalize();
        let mut direction = ui_light_direction;
        if !direction.is_finite() || direction.length_squared() < 1e-4 {
            direction = default_dir;
        } else {
            direction = direction.normalize_or_zero();
            if direction.length_squared() < 1e-4 {
                direction = default_dir;
            }
        }
        let lighting = self.renderer.lighting_mut();
        lighting.direction = direction;
        lighting.color = ui_light_color;
        lighting.ambient = ui_light_ambient;
        lighting.exposure = ui_light_exposure;
        lighting.shadow_distance = ui_shadow_distance.clamp(1.0, 500.0);
        lighting.shadow_bias = ui_shadow_bias.clamp(0.00005, 0.05);
        lighting.shadow_strength = ui_shadow_strength.clamp(0.0, 1.0);
        lighting.shadow_cascade_count = ui_shadow_cascade_count.clamp(1, MAX_SHADOW_CASCADES as u32);
        lighting.shadow_resolution = ui_shadow_resolution.clamp(256, 8192);
        lighting.shadow_split_lambda = ui_shadow_split_lambda.clamp(0.0, 1.0);
        lighting.shadow_pcf_radius = ui_shadow_pcf_radius.clamp(0.0, 10.0);
        self.renderer.mark_shadow_settings_dirty();
    }

    fn export_gpu_timings_csv<P: AsRef<std::path::Path>>(&self, path: P) -> Result<PathBuf> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Creating GPU timing export directory {}", parent.display()))?;
            }
        }
        let rows = {
            let state = self.editor_ui_state();
            if state.gpu_timing_history.is_empty() {
                return Err(anyhow!("No GPU timing samples available to export."));
            }
            let mut rows = String::from("frame,label,duration_ms\n");
            for frame in &state.gpu_timing_history {
                for timing in &frame.timings {
                    rows.push_str(&format!(
                        "{},{},{:.4}\n",
                        frame.frame_index, timing.label, timing.duration_ms
                    ));
                }
            }
            rows
        };
        fs::write(path, rows.as_bytes())
            .with_context(|| format!("Writing GPU timing export {}", path.display()))?;
        Ok(path.to_path_buf())
    }

    fn report_audio_startup_status(&mut self) {
        let Some(snapshot) = self.audio_plugin().map(|audio| audio.health_snapshot()) else {
            return;
        };
        if snapshot.playback_available {
            return;
        }
        if snapshot.last_error.is_none() {
            return;
        }
        let mut parts = Vec::new();
        if let Some(name) = snapshot.device_name.as_deref() {
            parts.push(format!("device: {name}"));
        }
        if let Some(rate) = snapshot.sample_rate_hz {
            parts.push(format!("sample rate: {rate} Hz"));
        }
        let detail_suffix = if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) };
        let mut message =
            format!("[audio] Output initialization failed{detail_suffix}. Audio triggers disabled.");
        if let Some(err) = snapshot.last_error.as_deref() {
            message.push_str(&format!(" Last error: {err}"));
        }
        self.ecs.push_event(GameEvent::ScriptMessage { message });
        self.record_events();
    }

    fn atlas_view(&mut self, key: &str) -> Result<Arc<wgpu::TextureView>> {
        if let Some(view) = self.sprite_atlas_views.get(key) {
            return Ok(view.clone());
        }
        let view = self.assets.atlas_texture_view(key)?;
        let arc = Arc::new(view);
        self.sprite_atlas_views.insert(key.to_string(), arc.clone());
        Ok(arc)
    }

    fn invalidate_atlas_view(&mut self, key: &str) {
        if self.sprite_atlas_views.remove(key).is_some() {
            self.renderer.invalidate_sprite_bind_group(key);
        }
    }

    fn clear_atlas_view_cache(&mut self) {
        if !self.sprite_atlas_views.is_empty() {
            self.sprite_atlas_views.clear();
            self.renderer.clear_sprite_bind_cache();
        }
    }

    fn with_plugin_runtime<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut PluginHost, &mut PluginManager, &mut PluginContext<'_>) -> R,
    {
        let selected_entity = self.selected_entity();
        self.plugin_runtime.with_context(
            PluginContextInputs {
                renderer: &mut self.renderer,
                ecs: &mut self.ecs,
                assets: &mut self.assets,
                input: &mut self.input,
                material_registry: &mut self.material_registry,
                mesh_registry: &mut self.mesh_registry,
                environment_registry: &mut self.environment_registry,
                time: self.runtime_loop.time(),
                event_emitter: Self::emit_event_for_plugin,
                selected_entity,
            },
            f,
        )
    }

    fn with_plugins<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut PluginManager, &mut PluginContext<'_>) -> R,
    {
        self.with_plugin_runtime(|_, manager, ctx| f(manager, ctx))
    }

    fn plugin_host(&self) -> &PluginHost {
        self.plugin_runtime.host()
    }

    fn plugin_host_mut(&mut self) -> &mut PluginHost {
        self.plugin_runtime.host_mut()
    }

    fn plugin_manager(&self) -> &PluginManager {
        self.plugin_runtime.manager()
    }

    fn plugin_manager_mut(&mut self) -> &mut PluginManager {
        self.plugin_runtime.manager_mut()
    }

    fn emit_event_for_plugin(ecs: &mut EcsWorld, event: GameEvent) {
        ecs.push_event(event);
    }

    fn audio_plugin(&self) -> Option<&AudioPlugin> {
        self.plugin_manager().get::<AudioPlugin>()
    }

    fn audio_plugin_mut(&mut self) -> Option<&mut AudioPlugin> {
        self.plugin_manager_mut().get_mut::<AudioPlugin>()
    }

    fn analytics_plugin(&self) -> Option<&AnalyticsPlugin> {
        self.plugin_manager().get::<AnalyticsPlugin>()
    }

    fn analytics_plugin_mut(&mut self) -> Option<&mut AnalyticsPlugin> {
        self.plugin_manager_mut().get_mut::<AnalyticsPlugin>()
    }

    fn mesh_preview_plugin(&self) -> Option<&MeshPreviewPlugin> {
        self.plugin_manager().get::<MeshPreviewPlugin>()
    }

    fn mesh_preview_plugin_mut(&mut self) -> Option<&mut MeshPreviewPlugin> {
        self.plugin_manager_mut().get_mut::<MeshPreviewPlugin>()
    }

    fn script_plugin(&self) -> Option<&ScriptPlugin> {
        self.plugin_manager().get::<ScriptPlugin>()
    }

    fn script_plugin_mut(&mut self) -> Option<&mut ScriptPlugin> {
        self.plugin_manager_mut().get_mut::<ScriptPlugin>()
    }

    fn drain_script_commands(&mut self) -> Vec<ScriptCommand> {
        self.script_plugin_mut().map(|plugin| plugin.take_commands()).unwrap_or_default()
    }

    fn drain_script_logs(&mut self) -> Vec<String> {
        self.script_plugin_mut().map(|plugin| plugin.take_logs()).unwrap_or_default()
    }

    fn register_script_spawn(&mut self, handle: ScriptHandle, entity: Entity) {
        if let Some(plugin) = self.script_plugin_mut() {
            plugin.register_spawn_result(handle, entity);
        }
    }

    fn forget_script_handle(&mut self, handle: ScriptHandle) {
        if let Some(plugin) = self.script_plugin_mut() {
            plugin.forget_handle(handle);
        }
    }

    fn resolve_script_handle(&self, handle: ScriptHandle) -> Option<Entity> {
        self.script_plugin().and_then(|plugin| plugin.resolve_handle(handle))
    }

    fn refresh_editor_analytics_state(&mut self) {
        let mut shadow_pass_metric = None;
        let mut mesh_pass_metric = None;
        let mut plugin_capability_metrics = Arc::new(HashMap::new());
        let mut plugin_capability_events = Arc::from(Vec::<PluginCapabilityEvent>::new().into_boxed_slice());
        let mut plugin_asset_readbacks = Arc::from(Vec::<PluginAssetReadbackEvent>::new().into_boxed_slice());
        let mut plugin_watchdog_events = Arc::from(Vec::<PluginWatchdogEvent>::new().into_boxed_slice());
        let mut animation_validation_log =
            Arc::from(Vec::<AnimationValidationEvent>::new().into_boxed_slice());
        let mut animation_budget_sample = None;
        let mut light_cluster_metrics_overlay = None;
        let mut keyframe_editor_usage: Option<KeyframeEditorUsageSnapshot> = None;
        let mut keyframe_event_log = Arc::from(Vec::<KeyframeEditorEvent>::new().into_boxed_slice());

        if let Some(analytics) = self.analytics_plugin_mut() {
            shadow_pass_metric = analytics.gpu_pass_metric("Shadow pass");
            mesh_pass_metric = analytics.gpu_pass_metric("Mesh pass");
            plugin_capability_metrics = analytics.plugin_capability_metrics();
            plugin_capability_events = analytics.plugin_capability_events_arc();
            plugin_asset_readbacks = analytics.plugin_asset_readbacks_arc();
            plugin_watchdog_events = analytics.plugin_watchdog_events_arc();
            animation_validation_log = analytics.animation_validation_events_arc();
            animation_budget_sample = analytics.animation_budget_sample();
            light_cluster_metrics_overlay = analytics.light_cluster_metrics();
            keyframe_editor_usage = Some(analytics.keyframe_editor_usage());
            keyframe_event_log = analytics.keyframe_editor_events_arc();
        }

        self.with_editor_ui_state_mut(|state| {
            state.shadow_pass_metric = shadow_pass_metric;
            state.mesh_pass_metric = mesh_pass_metric;
            state.plugin_capability_metrics = plugin_capability_metrics;
            state.plugin_capability_events = plugin_capability_events;
            state.plugin_asset_readbacks = plugin_asset_readbacks;
            state.plugin_watchdog_events = plugin_watchdog_events;
            state.animation_validation_log = animation_validation_log;
            state.animation_budget_sample = animation_budget_sample;
            state.light_cluster_metrics_overlay = light_cluster_metrics_overlay;
            state.keyframe_editor_usage = keyframe_editor_usage;
            state.keyframe_event_log = keyframe_event_log;
        });
    }

    fn refresh_editor_plugin_state(&mut self) {
        let plugin_manifest_error = self.plugin_host().manifest_error().map(|err| err.to_string());
        let (plugin_manifest_entries, plugin_manifest_disabled_builtins, plugin_manifest_path) =
            if let Some(manifest) = self.plugin_host().manifest() {
                (
                    Some(Arc::from(manifest.entries().to_vec().into_boxed_slice())),
                    Some(manifest.disabled_builtins().map(|entry| entry.to_string()).collect::<HashSet<_>>()),
                    manifest.path().map(|path| path.display().to_string()),
                )
            } else {
                (None, None, None)
            };

        let (
            plugin_statuses,
            plugin_asset_metrics,
            plugin_ecs_history,
            plugin_watchdog_map,
            plugin_asset_requestable,
        ) = {
            let manager = self.plugin_runtime.manager_mut();
            (
                manager.status_snapshot(),
                manager.asset_readback_metrics(),
                manager.ecs_query_history(),
                manager.watchdog_events(),
                manager.pending_asset_readback_plugins(),
            )
        };

        self.with_editor_ui_state_mut(|state| {
            state.plugin_manifest_error = plugin_manifest_error;
            state.plugin_manifest_entries = plugin_manifest_entries;
            state.plugin_manifest_disabled_builtins = plugin_manifest_disabled_builtins;
            state.plugin_manifest_path = plugin_manifest_path;
            state.plugin_statuses = plugin_statuses;
            state.plugin_asset_metrics = plugin_asset_metrics;
            state.plugin_ecs_history = plugin_ecs_history;
            state.plugin_watchdog_map = plugin_watchdog_map;
            state.plugin_asset_requestable = plugin_asset_requestable;
        });
    }

    fn refresh_script_debugger_state(&mut self) {
        let snapshot = if let Some(plugin) = self.script_plugin() {
            let handles = plugin
                .handles_snapshot()
                .into_iter()
                .map(|(handle, entity)| {
                    let scene_id = self.ecs.entity_info(entity).map(|info| info.scene_id);
                    ScriptHandleBinding { handle, scene_id }
                })
                .collect();
            ScriptDebuggerStatus {
                available: true,
                script_path: Some(plugin.script_path().display().to_string()),
                enabled: plugin.enabled(),
                paused: plugin.paused(),
                last_error: plugin.last_error().map(|err| err.to_string()),
                handles,
            }
        } else {
            ScriptDebuggerStatus::default()
        };
        self.with_editor_ui_state_mut(|state| {
            state.script_debugger_status = snapshot;
        });
    }

    #[cfg(feature = "alloc_profiler")]
    fn log_allocation_delta(delta: alloc_profiler::AllocationDelta) {
        if delta.allocated_bytes == 0 && delta.deallocated_bytes == 0 {
            return;
        }
        let net = delta.net_bytes();
        eprintln!(
            "[alloc] frame delta: +{}B allocated, +{}B freed (net {:+} B)",
            delta.allocated_bytes, delta.deallocated_bytes, net
        );
    }

    fn should_keep_environment(&self, key: &str) -> bool {
        if key.is_empty() {
            return true;
        }
        self.persistent_environments.contains(key) || self.scene_environment_ref.as_deref() == Some(key)
    }

    fn set_active_environment(&mut self, key: &str, intensity: f32) -> Result<()> {
        let intensity = intensity.max(0.0);
        if self.environment_registry.definition(key).is_none() {
            return Err(anyhow!("Environment '{}' is not available", key));
        }
        if self.active_environment_key == key {
            self.environment_intensity = intensity;
            self.editor_ui_state_mut().ui_environment_intensity = intensity;
            if self.renderer.environment_parameters().is_some() {
                self.renderer.set_environment_intensity(intensity);
            } else {
                self.bind_environment(key, intensity)?;
            }
            return Ok(());
        }
        self.bind_environment(key, intensity)?;
        let previous = std::mem::replace(&mut self.active_environment_key, key.to_string());
        self.environment_intensity = intensity;
        self.editor_ui_state_mut().ui_environment_intensity = intensity;
        if previous != self.active_environment_key && !self.should_keep_environment(&previous) {
            self.environment_registry.release(&previous);
        }
        Ok(())
    }

    fn apply_environment_to_renderer(&mut self) -> Result<()> {
        self.bind_environment(&self.active_environment_key.clone(), self.environment_intensity)
    }

    fn bind_environment(&mut self, key: &str, intensity: f32) -> Result<()> {
        if self.renderer.device().is_err() {
            return Ok(());
        }
        let env_gpu = self.environment_registry.ensure_gpu(key, &mut self.renderer)?;
        self.renderer.set_environment(&env_gpu, intensity)?;
        Ok(())
    }

    fn set_viewport_camera_mode(&mut self, mode: ViewportCameraMode) {
        if self.viewport_camera_mode == mode {
            return;
        }
        self.viewport_camera_mode = mode;
        if mode == ViewportCameraMode::Perspective3D {
            self.with_plugins(|plugins, ctx| {
                if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                    if plugin.mesh_control_mode() == MeshControlMode::Disabled {
                        if let Err(err) = plugin.set_mesh_control_mode(ctx, MeshControlMode::Orbit) {
                            eprintln!("[mesh_preview] set_mesh_control_mode failed: {err:?}");
                        }
                    }
                }
            });
        }
    }

    fn apply_vsync_toggle(&mut self, enabled: bool) {
        if self.renderer.vsync_enabled() == enabled {
            self.config.window.vsync = enabled;
            return;
        }
        match self.renderer.set_vsync(enabled) {
            Ok(()) => {
                self.config.window.vsync = enabled;
                self.set_ui_scene_status(format!("VSync {}", if enabled { "enabled" } else { "disabled" }));
            }
            Err(err) => {
                eprintln!("Failed to update VSync: {err:?}");
                self.set_ui_scene_status(format!("Failed to update VSync: {err}"));
            }
        }
    }

    fn apply_particle_caps(&mut self) {
        let (max_spawn_per_frame, max_total, max_emitter_backlog) = {
            let mut state = self.editor_ui_state_mut();
            if state.ui_particle_max_spawn_per_frame > state.ui_particle_max_total {
                state.ui_particle_max_spawn_per_frame = state.ui_particle_max_total;
            }
            (
                state.ui_particle_max_spawn_per_frame,
                state.ui_particle_max_total,
                state.ui_particle_max_emitter_backlog,
            )
        };
        let caps = ParticleCaps::new(max_spawn_per_frame, max_total, max_emitter_backlog);
        self.ecs.set_particle_caps(caps);
    }

    fn sync_emitter_ui(&mut self) {
        if let Some(entity) = self.ecs.first_emitter() {
            self.emitter_entity = Some(entity);
            if let Some(snapshot) = self.ecs.emitter_snapshot(entity) {
                let mut state = self.editor_ui_state_mut();
                state.ui_emitter_rate = snapshot.rate;
                state.ui_emitter_spread = snapshot.spread;
                state.ui_emitter_speed = snapshot.speed;
                state.ui_emitter_lifetime = snapshot.lifetime;
                state.ui_emitter_start_size = snapshot.start_size;
                state.ui_emitter_end_size = snapshot.end_size;
                state.ui_emitter_start_color = snapshot.start_color.to_array();
                state.ui_emitter_end_color = snapshot.end_color.to_array();
            }
        } else {
            self.emitter_entity = None;
        }
    }

    fn update_scene_dependencies(&mut self, deps: &SceneDependencies) -> Result<()> {
        let fingerprint = deps.fingerprints();
        let cached_fingerprint = {
            let state = self.editor_ui_state();
            state.scene_dependency_fingerprints.clone()
        };
        if cached_fingerprint.as_ref() == Some(&fingerprint) {
            self.with_editor_ui_state_mut(|state| state.scene_dependencies = Some(deps.clone()));
            return Ok(());
        }
        let atlas_dirty = cached_fingerprint.as_ref().map_or(true, |fp| fp.atlases != fingerprint.atlases);
        let clip_dirty = cached_fingerprint.as_ref().map_or(true, |fp| fp.clips != fingerprint.clips);
        let mesh_dirty = cached_fingerprint.as_ref().map_or(true, |fp| fp.meshes != fingerprint.meshes);
        let material_dirty =
            cached_fingerprint.as_ref().map_or(true, |fp| fp.materials != fingerprint.materials);
        let environment_dirty =
            cached_fingerprint.as_ref().map_or(true, |fp| fp.environments != fingerprint.environments);

        if atlas_dirty {
            let previous = self.scene_atlas_refs.clone();
            let mut next = self.persistent_atlases.clone();
            for dep in deps.atlas_dependencies() {
                let key = dep.key().to_string();
                if !next.contains(&key) {
                    if !previous.contains(&key) {
                        self.assets
                            .retain_atlas(dep.key(), dep.path())
                            .with_context(|| format!("Failed to retain atlas '{}'", dep.key()))?;
                    }
                    next.insert(key);
                }
            }
            for key in previous {
                if !next.contains(&key) && !self.persistent_atlases.contains(&key) {
                    self.assets.release_atlas(&key);
                    self.invalidate_atlas_view(&key);
                }
            }
            self.scene_atlas_refs = next;
            self.with_editor_ui_state_mut(|state| state.scene_atlas_snapshot = None);
        }

        if clip_dirty {
            let mut required_clips: HashMap<String, (usize, Option<PathBuf>)> = HashMap::new();
            for dep in deps.clip_dependencies() {
                let entry =
                    required_clips.entry(dep.key().to_string()).or_insert((0, dep.path().map(PathBuf::from)));
                entry.0 = entry.0.saturating_add(1);
            }
            let mut clip_watch_updates: Vec<PathBuf> = Vec::new();
            for (key, (count, path)) in required_clips.iter() {
                let entry = self.scene_clip_refs.entry(key.clone()).or_insert(0);
                if *entry == 0 {
                    self.assets
                        .retain_clip(key, path.as_ref().and_then(|p| p.to_str()))
                        .with_context(|| format!("Failed to retain clip '{key}'"))?;
                    if let Some(path) = path {
                        clip_watch_updates.push(path.clone());
                    }
                }
                *entry = *count;
            }
            for path in clip_watch_updates {
                self.queue_animation_watch_root(&path, AnimationAssetKind::Clip);
            }
            self.scene_clip_refs.retain(|key, _| {
                if required_clips.contains_key(key) {
                    true
                } else {
                    self.assets.release_clip(key);
                    false
                }
            });
        }

        if mesh_dirty {
            let previous_mesh = self.scene_mesh_refs.clone();
            let mut next_mesh = HashSet::new();
            let mut newly_required: Vec<String> = Vec::new();
            for dep in deps.mesh_dependencies() {
                let key = dep.key().to_string();
                if next_mesh.insert(key.clone()) {
                    self.mesh_registry
                        .ensure_mesh(dep.key(), dep.path(), &mut self.material_registry)
                        .with_context(|| format!("Failed to prepare mesh '{}'", dep.key()))?;
                    if !previous_mesh.contains(&key) {
                        newly_required.push(key);
                    }
                }
            }
            for key in previous_mesh {
                if !next_mesh.contains(&key) {
                    self.mesh_registry.release_mesh(&key);
                }
            }
            for key in &newly_required {
                self.mesh_registry
                    .retain_mesh(key, None, &mut self.material_registry)
                    .with_context(|| format!("Failed to retain mesh '{key}'"))?;
            }
            self.scene_mesh_refs = next_mesh;
            self.with_editor_ui_state_mut(|state| state.scene_mesh_snapshot = None);
        }

        if material_dirty {
            let persistent_materials: HashSet<String> = self
                .mesh_preview_plugin()
                .map(|plugin| plugin.persistent_materials().iter().cloned().collect())
                .unwrap_or_default();
            let previous_materials = self.scene_material_refs.clone();
            let mut next_materials = persistent_materials.clone();
            for dep in deps.material_dependencies() {
                let key = dep.key().to_string();
                if next_materials.insert(key.clone()) {
                    if !previous_materials.contains(&key) {
                        self.material_registry
                            .retain(&key)
                            .with_context(|| format!("Failed to retain material '{key}'"))?;
                    }
                }
            }
            for key in previous_materials {
                if !next_materials.contains(&key) && !persistent_materials.contains(&key) {
                    self.material_registry.release(&key);
                }
            }
            self.scene_material_refs = next_materials;
        }

        if environment_dirty {
            let previous_environment = self.scene_environment_ref.clone();
            let mut next_environment = None;
            if let Some(dep) = deps.environment_dependency() {
                let key = dep.key().to_string();
                self.environment_registry
                    .retain(dep.key(), dep.path())
                    .with_context(|| format!("Failed to retain environment '{}'", dep.key()))?;
                if self.renderer.device().is_ok() {
                    self.environment_registry
                        .ensure_gpu(dep.key(), &mut self.renderer)
                        .with_context(|| format!("Failed to prepare environment '{}'", dep.key()))?;
                }
                next_environment = Some(key);
            }
            if let Some(prev) = previous_environment {
                if Some(prev.clone()) != next_environment && !self.persistent_environments.contains(&prev) {
                    self.environment_registry.release(&prev);
                }
            }
            self.scene_environment_ref = next_environment;
        }

        let deps_clone = deps.clone();
        let fingerprint_clone = fingerprint.clone();
        self.with_editor_ui_state_mut(|state| {
            state.scene_dependencies = Some(deps_clone);
            state.scene_dependency_fingerprints = Some(fingerprint_clone);
        });
        Ok(())
    }

    fn capture_scene_metadata(&self) -> SceneMetadata {
        let mut metadata = SceneMetadata::default();
        metadata.viewport = SceneViewportMode::from(self.viewport_camera_mode);
        metadata.camera2d =
            Some(SceneCamera2D { position: Vec2Data::from(self.camera.position), zoom: self.camera.zoom });
        let camera_bookmarks = self.camera_bookmarks();
        metadata.camera_bookmarks = camera_bookmarks.iter().map(CameraBookmark::to_scene).collect();
        metadata.active_camera_bookmark =
            if self.camera_follow_target.is_none() { self.active_camera_bookmark() } else { None };
        metadata.camera_follow_entity = self.camera_follow_target.clone();
        if let Some(plugin) = self.mesh_preview_plugin() {
            metadata.preview_camera = Some(plugin.capture_preview_camera());
        }
        let lighting = self.renderer.lighting();
        metadata.lighting = Some(SceneLightingData {
            direction: lighting.direction.into(),
            color: lighting.color.into(),
            ambient: lighting.ambient.into(),
            exposure: lighting.exposure,
            shadow: SceneShadowData {
                distance: lighting.shadow_distance,
                bias: lighting.shadow_bias,
                strength: lighting.shadow_strength,
                cascade_count: lighting.shadow_cascade_count,
                resolution: lighting.shadow_resolution,
                split_lambda: lighting.shadow_split_lambda,
                pcf_radius: lighting.shadow_pcf_radius,
            },
            point_lights: lighting
                .point_lights
                .iter()
                .map(|light| ScenePointLightData {
                    position: light.position.into(),
                    color: light.color.into(),
                    radius: light.radius,
                    intensity: light.intensity,
                })
                .collect(),
        });
        metadata.environment =
            Some(SceneEnvironment::new(self.active_environment_key.clone(), self.environment_intensity));
        metadata
    }

    fn apply_scene_metadata(&mut self, metadata: &SceneMetadata) {
        self.set_viewport_camera_mode(ViewportCameraMode::from(metadata.viewport));
        if let Some(cam2d) = metadata.camera2d.as_ref() {
            self.camera.position = Vec2::from(cam2d.position.clone());
            self.camera.set_zoom(cam2d.zoom);
        }
        self.with_editor_ui_state_mut(|state| {
            state.camera_bookmarks =
                metadata.camera_bookmarks.iter().map(CameraBookmark::from_scene).collect();
            state.camera_bookmarks.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        });
        self.camera_follow_target = metadata.camera_follow_entity.clone();
        if self.camera_follow_target.is_some() && !self.refresh_camera_follow() {
            self.camera_follow_target = None;
        }
        if self.camera_follow_target.is_none() {
            if let Some(active) = metadata.active_camera_bookmark.as_deref() {
                if !self.apply_camera_bookmark_by_name(active) {
                    self.set_active_camera_bookmark(None);
                }
            } else {
                self.set_active_camera_bookmark(None);
            }
        } else {
            self.set_active_camera_bookmark(None);
        }
        if let Some(preview) = metadata.preview_camera.as_ref() {
            if let Some(plugin) = self.mesh_preview_plugin_mut() {
                plugin.apply_preview_camera(preview);
            }
        }
        if let Some(lighting) = metadata.lighting.as_ref() {
            let (mut direction, color, ambient, exposure, shadow, point_lights) = lighting.components();
            if !direction.is_finite() || direction.length_squared() < 1e-4 {
                direction = glam::Vec3::new(0.4, 0.8, 0.35).normalize();
            }
            direction = direction.normalize_or_zero();
            {
                let lighting_mut = self.renderer.lighting_mut();
                lighting_mut.direction = direction;
                lighting_mut.color = color;
                lighting_mut.ambient = ambient;
                lighting_mut.exposure = exposure;
                lighting_mut.shadow_distance = shadow.distance.clamp(1.0, 500.0);
                lighting_mut.shadow_bias = shadow.bias.clamp(0.00005, 0.05);
                lighting_mut.shadow_strength = shadow.strength.clamp(0.0, 1.0);
                lighting_mut.shadow_cascade_count = shadow.cascade_count.clamp(1, MAX_SHADOW_CASCADES as u32);
                lighting_mut.shadow_resolution = shadow.resolution.clamp(256, 8192);
                lighting_mut.shadow_split_lambda = shadow.split_lambda.clamp(0.0, 1.0);
                lighting_mut.shadow_pcf_radius = shadow.pcf_radius.clamp(0.0, 10.0);
                lighting_mut.point_lights = point_lights
                    .into_iter()
                    .map(|data| ScenePointLight {
                        position: Vec3::from(data.position),
                        color: Vec3::from(data.color),
                        radius: data.radius.max(0.0),
                        intensity: data.intensity.max(0.0),
                    })
                    .collect();
            }
            let renderer_lighting = self.renderer.lighting();
            {
                let mut state = self.editor_ui_state_mut();
                state.ui_light_direction = renderer_lighting.direction;
                state.ui_light_color = renderer_lighting.color;
                state.ui_light_ambient = renderer_lighting.ambient;
                state.ui_light_exposure = renderer_lighting.exposure;
                state.ui_shadow_distance = renderer_lighting.shadow_distance;
                state.ui_shadow_bias = renderer_lighting.shadow_bias;
                state.ui_shadow_strength = renderer_lighting.shadow_strength;
                state.ui_shadow_cascade_count = renderer_lighting.shadow_cascade_count;
                state.ui_shadow_resolution = renderer_lighting.shadow_resolution;
                state.ui_shadow_split_lambda = renderer_lighting.shadow_split_lambda;
                state.ui_shadow_pcf_radius = renderer_lighting.shadow_pcf_radius;
            }
            self.renderer.mark_shadow_settings_dirty();
        }
        if let Some(environment) = metadata.environment.as_ref() {
            let intensity = environment.intensity.max(0.0);
            if let Err(err) = self.set_active_environment(&environment.key, intensity) {
                self.set_ui_scene_status(format!("Environment '{}' unavailable: {err}", environment.key));
            }
        } else {
            let default_key = self.environment_registry.default_key().to_string();
            if let Err(err) = self.set_active_environment(&default_key, 1.0) {
                eprintln!("[environment] failed to restore default environment: {err:?}");
            }
        }
    }

    fn clear_scene_atlases(&mut self) {
        let to_release: Vec<String> = self
            .scene_atlas_refs
            .iter()
            .filter(|key| !self.persistent_atlases.contains(*key))
            .cloned()
            .collect();
        for key in to_release {
            self.assets.release_atlas(&key);
            self.invalidate_atlas_view(&key);
        }
        self.scene_atlas_refs = self.persistent_atlases.clone();
        self.with_editor_ui_state_mut(|state| state.scene_atlas_snapshot = None);
        let persistent_meshes: HashSet<String> = self
            .mesh_preview_plugin()
            .map(|plugin| plugin.persistent_meshes().iter().cloned().collect())
            .unwrap_or_default();
        let mesh_to_release: Vec<String> =
            self.scene_mesh_refs.iter().filter(|key| !persistent_meshes.contains(*key)).cloned().collect();
        for key in &mesh_to_release {
            self.mesh_registry.release_mesh(key);
        }
        self.scene_mesh_refs = persistent_meshes.clone();
        self.with_editor_ui_state_mut(|state| state.scene_mesh_snapshot = None);

        let persistent_materials: HashSet<String> = self
            .mesh_preview_plugin()
            .map(|plugin| plugin.persistent_materials().iter().cloned().collect())
            .unwrap_or_default();
        let material_to_release: Vec<String> = self
            .scene_material_refs
            .iter()
            .filter(|key| !persistent_materials.contains(*key))
            .cloned()
            .collect();
        for key in &material_to_release {
            self.material_registry.release(key);
        }
        self.scene_material_refs = persistent_materials;
        self.clear_scene_clips();
    }

    fn clear_scene_clips(&mut self) {
        let clips: Vec<String> = self.scene_clip_refs.keys().cloned().collect();
        for key in clips {
            self.assets.release_clip(&key);
        }
        self.scene_clip_refs.clear();
        self.with_editor_ui_state_mut(|state| state.scene_clip_snapshot = None);
    }

    fn viewport_physical_size(&self) -> PhysicalSize<u32> {
        self.viewport.size_physical()
    }

    fn screen_to_viewport(&self, screen: Vec2) -> Option<Vec2> {
        if self.viewport.contains(screen) {
            Some(screen - self.viewport.origin)
        } else {
            None
        }
    }

    fn update_viewport(&mut self, origin: Vec2, size: Vec2) {
        let clamped = Vec2::new(size.x.max(1.0), size.y.max(1.0));
        self.viewport = Viewport::new(origin, clamped);
    }

    fn resolve_material_for_mesh(&self, mesh_key: &str, override_key: Option<&String>) -> String {
        if let Some(material) = override_key {
            if self.material_registry.has(material.as_str()) {
                return material.clone();
            }
        }
        if let Some(subsets) = self.mesh_registry.mesh_subsets(mesh_key) {
            for subset in subsets {
                if let Some(material_key) = subset.material.as_ref() {
                    if self.material_registry.has(material_key.as_str()) {
                        return material_key.clone();
                    }
                }
            }
        }
        self.material_registry.default_key().to_string()
    }

    fn mesh_camera_forward(&self) -> Vec3 {
        self.mesh_preview_plugin().map(|plugin| plugin.mesh_camera_forward()).unwrap_or(Vec3::Z)
    }

    fn intersect_ray_plane(origin: Vec3, dir: Vec3, plane_origin: Vec3, plane_normal: Vec3) -> Option<Vec3> {
        let denom = plane_normal.dot(dir);
        if denom.abs() < 1e-4 {
            return None;
        }
        let t = (plane_origin - origin).dot(plane_normal) / denom;
        if t < 0.0 {
            return None;
        }
        Some(origin + dir * t)
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Err(err) = self.renderer.ensure_window(event_loop) {
            eprintln!("Renderer initialization error: {err:?}");
            self.should_close = true;
            return;
        }
        let (device, queue) = match self.renderer.device_and_queue() {
            Ok(pair) => pair,
            Err(err) => {
                eprintln!("Renderer missing device/queue: {err:?}");
                self.should_close = true;
                return;
            }
        };
        self.assets.set_device(device, queue);
        self.clear_atlas_view_cache();
        if let Err(err) = self.apply_environment_to_renderer() {
            eprintln!(
                "[environment] failed to bind active environment '{}': {err:?}",
                self.active_environment_key
            );
        }
        if !self.scene_atlas_refs.contains("main") {
            match self.assets.retain_atlas("main", Some("assets/images/atlas.json")) {
                Ok(()) => {
                    self.scene_atlas_refs.insert("main".to_string());
                    self.persistent_atlases.insert("main".to_string());
                }
                Err(err) => {
                    eprintln!("Failed to retain atlas: {err:?}");
                    self.should_close = true;
                    return;
                }
            }
        }
        if let Some(emitter) = self.emitter_entity {
            let has_animation = self
                .ecs
                .entity_info(emitter)
                .and_then(|info| info.sprite.and_then(|sprite| sprite.animation))
                .is_some();
            if !has_animation {
                if self.ecs.set_sprite_timeline(emitter, &self.assets, Some("demo_cycle")) {
                    self.ecs.set_sprite_animation_speed(emitter, 0.85);
                }
            }
        }
        let atlas_view = match self.assets.atlas_texture_view("main") {
            Ok(view) => view,
            Err(err) => {
                eprintln!("Failed to create atlas texture view: {err:?}");
                self.should_close = true;
                return;
            }
        };
        self.sprite_atlas_views.insert("main".to_string(), Arc::new(atlas_view.clone()));
        let sampler = self.assets.default_sampler().clone();
        if let Err(err) = self.renderer.init_sprite_pipeline_with_atlas(atlas_view, sampler) {
            eprintln!("Failed to initialize sprite pipeline: {err:?}");
            self.should_close = true;
            return;
        }

        if self.editor_shell.egui_winit.is_none() {
            if let Some(window) = self.renderer.window() {
                let state = EguiWinit::new(
                    self.editor_shell.egui_ctx.clone(),
                    egui::ViewportId::ROOT,
                    window,
                    Some(self.renderer.pixels_per_point()),
                    window.theme(),
                    None,
                );
                self.editor_shell.egui_winit = Some(state);
            }
        }

        // egui painter
        let egui_renderer = match (self.renderer.device(), self.renderer.surface_format()) {
            (Ok(device), Ok(format)) => EguiRenderer::new(device, format, RendererOptions::default()),
            (Err(err), _) | (_, Err(err)) => {
                eprintln!("Unable to initialize egui renderer: {err:?}");
                self.should_close = true;
                return;
            }
        };
        self.editor_shell.egui_renderer = Some(egui_renderer);
        let ui_scale = self.editor_ui_state().ui_scale;
        let size = self.renderer.size();
        self.editor_shell.egui_screen = Some(ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: self.renderer.pixels_per_point() * ui_scale,
        });

        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.ensure_preview_gpu(ctx) {
                    eprintln!("[mesh_preview] ensure_preview_gpu failed: {err:?}");
                }
            }
        });
        if let Err(err) = self.renderer.init_mesh_pipeline() {
            eprintln!("Failed to initialize mesh pipeline: {err:?}");
        }
    }

    fn window_event(&mut self, _el: &ActiveEventLoop, id: winit::window::WindowId, event: WindowEvent) {
        // egui wants the events too
        let mut consumed = false;
        let input_event = InputEvent::from_window_event(&event);
        let is_cursor_event = matches!(&input_event, InputEvent::CursorPos { .. });
        if let (Some(window), Some(state)) = (self.renderer.window(), self.editor_shell.egui_winit.as_mut()) {
            if id == window.id() {
                let resp = state.on_window_event(window, &event);
                if resp.consumed {
                    consumed = true;
                }
            }
        }
        if !consumed || is_cursor_event {
            self.input.push(input_event);
        }

        if consumed {
            return;
        }

        match &event {
            WindowEvent::CloseRequested => self.should_close = true,
            WindowEvent::Resized(size) => {
                self.renderer.resize(*size);
                let ui_scale = self.editor_ui_state().ui_scale;
                if let Some(sd) = &mut self.editor_shell.egui_screen {
                    sd.size_in_pixels = [size.width, size.height];
                    sd.pixels_per_point = self.renderer.pixels_per_point() * ui_scale;
                }
            }
            WindowEvent::KeyboardInput { event: KeyEvent { logical_key, state, .. }, .. } => {
                if let Key::Named(NamedKey::Escape) = logical_key {
                    if *state == ElementState::Pressed {
                        self.should_close = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _e: &ActiveEventLoop, _dev: winit::event::DeviceId, ev: DeviceEvent) {
        self.input.push(InputEvent::from_device_event(&ev));
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.should_close {
            event_loop.exit();
            return;
        }
        let RuntimeTick { dt, dropped_backlog } = self.runtime_loop.tick(MAX_FIXED_TIMESTEP_BACKLOG);
        if let Some(dropped) = dropped_backlog {
            eprintln!("[time] Dropping {:.3}s of fixed-step backlog to maintain responsiveness", dropped);
        }
        self.sync_atlas_hot_reload();
        self.process_atlas_hot_reload_events();
        self.process_animation_asset_watchers();
        self.ecs.profiler_begin_frame();
        let frame_start = Instant::now();
        let mut fixed_time_ms = 0.0;
        #[allow(unused_assignments)]
        let mut update_time_ms = 0.0;
        #[allow(unused_assignments)]
        let mut render_time_ms = 0.0;
        let mut ui_time_ms = 0.0;
        #[cfg(feature = "alloc_profiler")]
        let alloc_snapshot = alloc_profiler::allocation_snapshot();
        #[cfg(feature = "alloc_profiler")]
        let alloc_delta = alloc_snapshot.delta_since(self.last_alloc_snapshot);
        #[cfg(feature = "alloc_profiler")]
        {
            self.last_alloc_snapshot = alloc_snapshot;
        }

        if let Some(entity) = self.selected_entity() {
            if !self.ecs.entity_exists(entity) {
                self.set_selected_entity(None);
            }
        }

        let (ui_auto_spawn_rate, ui_spawn_per_press) = {
            let state = self.editor_ui_state();
            (state.ui_auto_spawn_rate, state.ui_spawn_per_press)
        };
        if ui_auto_spawn_rate > 0.0 {
            let to_spawn = (ui_auto_spawn_rate * dt) as i32;
            if to_spawn > 0 {
                self.ecs.spawn_burst(&self.assets, to_spawn as usize);
            }
        }

        if self.input.take_space_pressed() {
            self.ecs.spawn_burst(&self.assets, ui_spawn_per_press as usize);
        }
        if self.input.take_b_pressed() {
            self.ecs.spawn_burst(&self.assets, (ui_spawn_per_press * 5).max(1000) as usize);
        }

        self.with_plugins(|plugins, ctx| plugins.update(ctx, dt));
        let capability_metrics = self.plugin_manager().capability_metrics();
        let capability_events = self.plugin_manager_mut().drain_capability_events();
        let watchdog_alerts = self.plugin_manager_mut().drain_watchdog_events();
        let asset_readback_alerts = self.plugin_manager_mut().drain_asset_readback_events();
        let animation_validation_alerts = self.drain_animation_validation_events();
        if let Some(analytics) = self.analytics_plugin_mut() {
            #[cfg(feature = "alloc_profiler")]
            {
                analytics.record_allocation_delta(alloc_delta);
            }
            analytics.record_plugin_capability_metrics(capability_metrics);
            if !capability_events.is_empty() {
                analytics.record_plugin_capability_events(capability_events);
            }
            if !asset_readback_alerts.is_empty() {
                analytics.record_plugin_asset_readbacks(asset_readback_alerts);
            }
            if !watchdog_alerts.is_empty() {
                analytics.record_plugin_watchdog_events(watchdog_alerts);
            }
            if !animation_validation_alerts.is_empty() {
                analytics.record_animation_validation_events(animation_validation_alerts);
            }
        }
        #[cfg(feature = "alloc_profiler")]
        Self::log_allocation_delta(alloc_delta);

        if self.camera_follow_target.is_some() && !self.refresh_camera_follow() {
            self.camera_follow_target = None;
        }

        let window_size = self.renderer.size();
        let viewport_size = self.viewport_physical_size();
        let cursor_screen = self.input.cursor_position().map(|(sx, sy)| Vec2::new(sx, sy));
        let cursor_viewport = cursor_screen.and_then(|pos| self.screen_to_viewport(pos));
        let cursor_world_2d = if self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
            cursor_viewport.and_then(|pos| self.camera.screen_to_world(pos, viewport_size))
        } else {
            None
        };
        let mesh_camera = self.mesh_preview_plugin().map(|plugin| plugin.mesh_camera().clone());
        let mesh_control_mode =
            self.mesh_preview_plugin().map(|plugin| plugin.mesh_control_mode()).unwrap_or_default();
        let cursor_ray = if self.viewport_camera_mode == ViewportCameraMode::Perspective3D {
            if let (Some(pos), Some(camera)) = (cursor_viewport, mesh_camera.as_ref()) {
                camera.screen_ray(pos, viewport_size)
            } else {
                None
            }
        } else {
            None
        };
        let cursor_in_viewport = cursor_viewport.is_some();
        let mut selected_info = self.selected_entity().and_then(|entity| self.ecs.entity_info(entity));
        let mut selection_bounds_2d =
            self.selected_entity().and_then(|entity| self.ecs.entity_bounds(entity));
        let mesh_center_world = selected_info.as_ref().and_then(|info| {
            info.mesh_transform
                .as_ref()
                .map(|tx| tx.translation)
                .or_else(|| Some(Vec3::new(info.translation.x, info.translation.y, 0.0)))
        });
        let gizmo_center_viewport = match self.viewport_camera_mode {
            ViewportCameraMode::Ortho2D => selected_info
                .as_ref()
                .and_then(|info| self.camera.world_to_screen_pixels(info.translation, viewport_size)),
            ViewportCameraMode::Perspective3D => {
                if let Some(camera) = mesh_camera.as_ref() {
                    mesh_center_world.and_then(|center| camera.project_point(center, viewport_size))
                } else {
                    None
                }
            }
        };
        let prev_selected_entity = self.selected_entity();
        let prev_gizmo_interaction = self.gizmo_interaction();
        let prev_selection_details = selected_info.clone();
        let prev_selection_bounds_2d = selection_bounds_2d;

        if self.viewport_camera_mode == ViewportCameraMode::Ortho2D
            && mesh_control_mode == MeshControlMode::Disabled
        {
            if let Some(delta) = self.input.consume_wheel_delta() {
                self.camera.apply_scroll_zoom(delta);
                self.set_active_camera_bookmark(None);
            }

            if self.input.right_mouse_held() {
                let (dx, dy) = self.input.mouse_delta;
                if dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON {
                    self.camera.pan_screen_delta(Vec2::new(dx, dy), viewport_size);
                    self.set_active_camera_bookmark(None);
                    self.camera_follow_target = None;
                }
            }
        }

        let gizmo_update = self.update_gizmo_interactions(
            viewport_size,
            cursor_world_2d,
            cursor_viewport,
            cursor_ray,
            cursor_in_viewport,
            mesh_center_world,
            gizmo_center_viewport,
            &selected_info,
        );
        let hovered_scale_kind = gizmo_update.hovered_scale_kind;
        let selection_changed = self.selected_entity() != prev_selected_entity;
        let gizmo_changed = self.gizmo_interaction() != prev_gizmo_interaction;
        selected_info = self.selected_entity().and_then(|entity| self.ecs.entity_info(entity));
        selection_bounds_2d = self.selected_entity().and_then(|entity| self.ecs.entity_bounds(entity));

        let (cell_size, use_quadtree, density_threshold) = {
            let state = self.editor_ui_state();
            (state.ui_cell_size.max(0.05), state.ui_spatial_use_quadtree, state.ui_spatial_density_threshold)
        };
        self.ecs.set_spatial_cell(cell_size);
        self.ecs.set_spatial_quadtree_enabled(use_quadtree);
        self.ecs.set_spatial_density_threshold(density_threshold);
        if let Some(emitter) = self.emitter_entity {
            let (
                emitter_rate,
                emitter_spread,
                emitter_speed,
                emitter_lifetime,
                emitter_start_size,
                emitter_end_size,
                emitter_start_color,
                emitter_end_color,
            ) = {
                let state = self.editor_ui_state();
                (
                    state.ui_emitter_rate,
                    state.ui_emitter_spread,
                    state.ui_emitter_speed,
                    state.ui_emitter_lifetime,
                    state.ui_emitter_start_size,
                    state.ui_emitter_end_size,
                    state.ui_emitter_start_color,
                    state.ui_emitter_end_color,
                )
            };
            self.ecs.set_emitter_rate(emitter, emitter_rate);
            self.ecs.set_emitter_spread(emitter, emitter_spread);
            self.ecs.set_emitter_speed(emitter, emitter_speed);
            self.ecs.set_emitter_lifetime(emitter, emitter_lifetime);
            self.ecs.set_emitter_colors(
                emitter,
                Vec4::from_array(emitter_start_color),
                Vec4::from_array(emitter_end_color),
            );
            self.ecs.set_emitter_sizes(emitter, emitter_start_size, emitter_end_size);
        }
        let commands = self.drain_script_commands();
        self.apply_script_commands(commands);
        for message in self.drain_script_logs() {
            self.push_script_console(ScriptConsoleKind::Log, format!("[log] {message}"));
            self.ecs.push_event(GameEvent::ScriptMessage { message });
        }

        while let Some(fixed_dt) = self.runtime_loop.pop_fixed_step() {
            let fixed_start = Instant::now();
            self.ecs.fixed_step(fixed_dt);
            fixed_time_ms += fixed_start.elapsed().as_secs_f32() * 1000.0;
            let plugin_fixed_start = Instant::now();
            self.with_plugins(|plugins, ctx| plugins.fixed_update(ctx, fixed_dt));
            fixed_time_ms += plugin_fixed_start.elapsed().as_secs_f32() * 1000.0;
        }
        let update_start = Instant::now();
        self.ecs.update(dt);
        update_time_ms = update_start.elapsed().as_secs_f32() * 1000.0;
        if self.camera_follow_target.is_some() && !self.refresh_camera_follow() {
            self.camera_follow_target = None;
        }
        self.record_events();
        let particle_budget_snapshot = self.ecs.particle_budget_metrics();
        let sprite_perf_sample = self.ecs.sprite_anim_perf_sample();
        let spatial_metrics_snapshot = self.ecs.spatial_metrics();
        if let Some(analytics) = self.analytics_plugin_mut() {
            analytics.record_particle_budget(particle_budget_snapshot);
            analytics.record_spatial_metrics(spatial_metrics_snapshot);
        }

        let sprite_instances = match self.ecs.collect_sprite_instances(&self.assets) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("Instance collection error: {err:?}");
                self.input.clear_frame();
                return;
            }
        };
        let sprite_instances = self.apply_sprite_guardrails(sprite_instances, viewport_size);
        self.recycle_sprite_batch_buffers();
        for instance in sprite_instances {
            let (atlas_key, gpu_data) = instance.into_gpu();
            if let Some(existing) = self.sprite_batch_map.get_mut(&atlas_key) {
                existing.push(gpu_data);
            } else {
                let mut bucket = self.take_sprite_batch_buffer();
                bucket.push(gpu_data);
                self.sprite_batch_order.push(Arc::clone(&atlas_key));
                self.sprite_batch_map.insert(atlas_key, bucket);
            }
        }
        let mut instances: Vec<InstanceData> = Vec::new();
        let total_instances: usize = self.sprite_batch_map.values().map(|bucket| bucket.len()).sum();
        instances.reserve(total_instances);
        let mut sprite_batches: Vec<SpriteBatch> = Vec::new();
        let mut ordered_keys = mem::take(&mut self.sprite_batch_order);
        for atlas in ordered_keys.drain(..) {
            let mut batch_instances = match self.sprite_batch_map.remove(&atlas) {
                Some(bucket) => bucket,
                None => continue,
            };
            if batch_instances.is_empty() {
                self.sprite_batch_pool.push(batch_instances);
                continue;
            }
            let start_len = instances.len();
            instances.append(&mut batch_instances);
            if instances.len() > u32::MAX as usize {
                eprintln!("Too many sprite instances to render ({}).", instances.len());
                instances.truncate(start_len);
                batch_instances.clear();
                self.sprite_batch_pool.push(batch_instances);
                break;
            }
            let start = start_len as u32;
            let end = instances.len() as u32;
            match self.atlas_view(atlas.as_ref()) {
                Ok(view) => {
                    sprite_batches.push(SpriteBatch { atlas: Arc::clone(&atlas), range: start..end, view });
                }
                Err(err) => {
                    eprintln!("Atlas '{}' unavailable for rendering: {err:?}", atlas.as_ref());
                    instances.truncate(start_len);
                    self.invalidate_atlas_view(atlas.as_ref());
                }
            }
            batch_instances.clear();
            self.sprite_batch_pool.push(batch_instances);
        }
        self.sprite_batch_order = ordered_keys;
        let render_viewport = RenderViewport {
            origin: (self.viewport.origin.x, self.viewport.origin.y),
            size: (self.viewport.size.x, self.viewport.size.y),
        };
        let view_proj = self.camera.view_projection(viewport_size);
        let default_material_key = self.material_registry.default_key().to_string();
        let mut mesh_draw_infos: Vec<(String, Mat4, MeshLightingInfo, String, Option<Arc<[Mat4]>>)> =
            Vec::new();
        if let Some((preview_key, preview_model)) = self
            .mesh_preview_plugin()
            .map(|plugin| (plugin.preview_mesh_key().to_string(), *plugin.mesh_model()))
        {
            match self.mesh_registry.ensure_gpu(&preview_key, &mut self.renderer) {
                Ok(_) => {
                    let material_key = self.resolve_material_for_mesh(&preview_key, None);
                    mesh_draw_infos.push((
                        preview_key,
                        preview_model,
                        MeshLightingInfo::default(),
                        material_key,
                        None,
                    ));
                }
                Err(err) => {
                    self.set_mesh_status(format!("Mesh upload failed: {err}"));
                }
            }
        }
        let scene_meshes = self.ecs.collect_mesh_instances();
        for instance in scene_meshes {
            match self.mesh_registry.ensure_gpu(&instance.key, &mut self.renderer) {
                Ok(_) => {
                    let material_key =
                        self.resolve_material_for_mesh(&instance.key, instance.material.as_ref());
                    let skin_palette = instance.skin.as_ref().map(|skin| skin.palette.clone());
                    mesh_draw_infos.push((
                        instance.key.clone(),
                        instance.model,
                        instance.lighting,
                        material_key,
                        skin_palette,
                    ));
                }
                Err(err) => {
                    eprintln!("[mesh] Unable to prepare '{}': {err:?}", instance.key);
                }
            }
        }
        let mut mesh_draws: Vec<MeshDraw> = Vec::new();
        let mut material_cache: HashMap<String, Arc<MaterialGpu>> = HashMap::new();
        for (key, model, lighting, material_key, skin_palette) in mesh_draw_infos {
            let mesh = match self.mesh_registry.gpu_mesh(&key) {
                Some(mesh) => mesh,
                None => continue,
            };
            let material_gpu = if let Some(existing) = material_cache.get(&material_key) {
                existing.clone()
            } else {
                match self.material_registry.prepare_material_gpu(&material_key, &mut self.renderer) {
                    Ok(gpu) => {
                        material_cache.insert(material_key.clone(), gpu.clone());
                        gpu
                    }
                    Err(err) => {
                        eprintln!("[material] Failed to prepare '{material_key}': {err:?}");
                        let fallback_gpu =
                            if let Some(existing_default) = material_cache.get(&default_material_key) {
                                existing_default.clone()
                            } else {
                                match self
                                    .material_registry
                                    .prepare_material_gpu(&default_material_key, &mut self.renderer)
                                {
                                    Ok(gpu) => {
                                        material_cache.insert(default_material_key.clone(), gpu.clone());
                                        gpu
                                    }
                                    Err(default_err) => {
                                        eprintln!(
                                            "[material] Failed to prepare default material: {default_err:?}"
                                        );
                                        continue;
                                    }
                                }
                            };
                        material_cache.insert(material_key.clone(), fallback_gpu.clone());
                        fallback_gpu
                    }
                }
            };
            let casts_shadows = lighting.cast_shadows;
            mesh_draws.push(MeshDraw {
                mesh,
                model,
                lighting,
                material: material_gpu,
                casts_shadows,
                skin_palette,
            });
        }
        let mesh_camera_opt = if mesh_draws.is_empty() { None } else { mesh_camera.as_ref() };
        let render_start = Instant::now();
        let frame = match self.renderer.render_frame(
            &instances,
            &sprite_batches,
            self.assets.default_sampler(),
            view_proj,
            render_viewport,
            &mesh_draws,
            mesh_camera_opt,
        ) {
            Ok(frame) => frame,
            Err(err) => {
                eprintln!("Render error: {err:?}");
                self.input.clear_frame();
                return;
            }
        };
        render_time_ms = render_start.elapsed().as_secs_f32() * 1000.0;

        let palette_upload_stats = self.renderer.take_palette_upload_metrics();
        let light_cluster_snapshot = *self.renderer.light_cluster_metrics();
        if let Some(analytics) = self.analytics_plugin_mut() {
            analytics.record_light_cluster_metrics(light_cluster_snapshot);
        }
        if self.editor_shell.egui_winit.is_none() {
            frame.present();
            let frame_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
            self.record_frame_timing_sample(FrameTimingSample {
                frame_ms,
                update_ms: update_time_ms,
                fixed_ms: fixed_time_ms,
                render_ms: render_time_ms,
                ui_ms: ui_time_ms,
            });
            return;
        }

        let raw_input = {
            let Some(window) = self.renderer.window() else {
                return;
            };
            self.editor_shell.egui_winit.as_mut().unwrap().take_egui_input(window)
        };
        let base_pixels_per_point = self.renderer.pixels_per_point();
        let ui_scale = self.editor_ui_state().ui_scale;
        self.editor_shell.egui_ctx.set_pixels_per_point(base_pixels_per_point * ui_scale);
        let ui_pixels_per_point = self.editor_shell.egui_ctx.pixels_per_point();
        if let Some(screen) = self.editor_shell.egui_screen.as_mut() {
            screen.pixels_per_point = ui_pixels_per_point;
        };
        let hist_points = self.frame_plot_points_arc();
        let spatial_metrics = self.analytics_plugin().and_then(|plugin| plugin.spatial_metrics());
        #[cfg(feature = "alloc_profiler")]
        let allocation_delta = self.analytics_plugin().and_then(|plugin| plugin.allocation_delta());
        let system_timings = self.ecs.system_timings();
        let sprite_eval_ms = system_timings
            .iter()
            .find(|timing| timing.name == "sys_drive_sprite_animations")
            .map(|timing| timing.last_ms);
        let sprite_pack_ms = system_timings
            .iter()
            .find(|timing| timing.name == "sys_apply_sprite_frame_states")
            .map(|timing| timing.last_ms);
        let sprite_upload_ms = {
            let state = self.editor_ui_state();
            state
                .gpu_timings
                .iter()
                .find(|timing| timing.label == "Sprite pass")
                .map(|timing| timing.duration_ms)
        };
        let entity_count = self.ecs.entity_count();
        let instances_drawn = instances.len();
        let orbit_target =
            self.mesh_preview_plugin().map(|plugin| plugin.mesh_orbit().target).unwrap_or(Vec3::ZERO);
        let mesh_camera_for_ui = mesh_camera.clone().unwrap_or_else(|| {
            Camera3D::new(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, 60.0_f32.to_radians(), 0.1, 100.0)
        });
        let camera_position = self.camera.position;
        let camera_zoom = self.camera.zoom;
        self.sync_script_error_state();
        let recent_events: Arc<[GameEvent]> = if let Some(plugin) = self.analytics_plugin_mut() {
            plugin.recent_events_snapshot()
        } else {
            Arc::<[GameEvent]>::from([])
        };
        let (audio_triggers, audio_enabled, audio_health, audio_plugin_present, audio_spatial_config) =
            if let Some(audio) = self.audio_plugin() {
                (
                    audio.recent_triggers().cloned().collect(),
                    audio.enabled(),
                    audio.health_snapshot(),
                    true,
                    audio.spatial_config(),
                )
            } else {
                (
                    Vec::new(),
                    false,
                    AudioHealthSnapshot::default(),
                    false,
                    AudioSpatialConfig { enabled: false, min_distance: 0.1, max_distance: 25.0, pan_width: 10.0 },
                )
            };
        let (mesh_keys, environment_options, prefab_entries) = self.with_editor_ui_state_mut(|state| {
            let mesh = state.telemetry_cache.mesh_keys(&self.mesh_registry);
            let env = state.telemetry_cache.environment_options(&self.environment_registry);
            let prefabs = state.telemetry_cache.prefab_entries(&self.prefab_library);
            (mesh, env, prefabs)
        });
        let clip_keys_list = self.assets.clip_keys();
        let clip_assets_map: HashMap<String, editor_ui::ClipAssetSummary> = clip_keys_list
            .iter()
            .map(|key| {
                let source = self.assets.clip_source(key).map(|s| s.to_string());
                let markers = self
                    .assets
                    .clip(key)
                    .map(|clip| {
                        let mut markers = Vec::new();
                        if let Some(track) = clip.translation.as_ref() {
                            markers.extend(track.keyframes.iter().map(|kf| kf.time));
                        }
                        if let Some(track) = clip.rotation.as_ref() {
                            markers.extend(track.keyframes.iter().map(|kf| kf.time));
                        }
                        if let Some(track) = clip.scale.as_ref() {
                            markers.extend(track.keyframes.iter().map(|kf| kf.time));
                        }
                        if let Some(track) = clip.tint.as_ref() {
                            markers.extend(track.keyframes.iter().map(|kf| kf.time));
                        }
                        markers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
                        markers.dedup_by(|a, b| (*a - *b).abs() <= 1e-4);
                        Arc::from(markers.into_boxed_slice())
                    })
                    .unwrap_or_else(|| Arc::from(Vec::<f32>::new().into_boxed_slice()));
                (key.clone(), editor_ui::ClipAssetSummary { source, keyframe_markers: markers })
            })
            .collect();
        let clip_keys: Arc<[String]> = Arc::from(clip_keys_list.clone().into_boxed_slice());
        let clip_assets = Arc::new(clip_assets_map);
        let skeleton_keys_list = self.assets.skeleton_keys();
        let skeleton_assets_map: HashMap<String, editor_ui::SkeletonAssetSummary> = skeleton_keys_list
            .iter()
            .map(|key| {
                let clip_keys =
                    self.assets.skeletal_clip_keys_for(key).map(|keys| keys.to_vec()).unwrap_or_default();
                let source = self.assets.skeleton_source(key).map(|s| s.to_string());
                (
                    key.clone(),
                    editor_ui::SkeletonAssetSummary {
                        source,
                        clip_keys: Arc::from(clip_keys.into_boxed_slice()),
                    },
                )
            })
            .collect();
        let skeleton_keys: Arc<[String]> = Arc::from(skeleton_keys_list.clone().into_boxed_slice());
        let skeleton_assets = Arc::new(skeleton_assets_map);
        let atlas_keys_list = self.assets.atlas_keys();
        let atlas_assets_map: HashMap<String, editor_ui::AtlasAssetSummary> = atlas_keys_list
            .iter()
            .map(|key| {
                let mut timelines = self.assets.atlas_timeline_names(key);
                timelines.sort();
                timelines.dedup();
                let source = self.assets.atlas_source(key).map(|s| s.to_string());
                (
                    key.clone(),
                    editor_ui::AtlasAssetSummary {
                        source,
                        timeline_names: Arc::from(timelines.into_boxed_slice()),
                    },
                )
            })
            .collect();
        let atlas_keys: Arc<[String]> = Arc::from(atlas_keys_list.clone().into_boxed_slice());
        let atlas_assets = Arc::new(atlas_assets_map);
        let skeleton_entities: Arc<[editor_ui::SkeletonEntityBinding]> = Arc::from(
            self.ecs
                .skeleton_entities()
                .into_iter()
                .map(|(entity, scene_id)| editor_ui::SkeletonEntityBinding { entity, scene_id })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let mut material_options: Vec<editor_ui::MaterialOption> = self
            .material_registry
            .keys()
            .map(|key| {
                let label = self
                    .material_registry
                    .definition(key)
                    .map(|def| def.label.clone())
                    .unwrap_or_else(|| key.to_string());
                editor_ui::MaterialOption { key: key.to_string(), label }
            })
            .collect();
        material_options.sort_by(|a, b| a.label.cmp(&b.label).then_with(|| a.key.cmp(&b.key)));
        let material_options: Arc<[editor_ui::MaterialOption]> =
            Arc::from(material_options.into_boxed_slice());
        let mesh_subset_map: HashMap<String, Arc<[editor_ui::MeshSubsetEntry]>> = self
            .mesh_registry
            .keys()
            .filter_map(|key| {
                self.mesh_registry.mesh_subsets(key).map(|subsets| {
                    let entries: Vec<editor_ui::MeshSubsetEntry> = subsets
                        .iter()
                        .map(|subset| editor_ui::MeshSubsetEntry {
                            name: subset.name.clone(),
                            index_offset: subset.index_offset,
                            index_count: subset.index_count,
                            material: subset.material.clone(),
                        })
                        .collect();
                    (key.to_string(), Arc::from(entries.into_boxed_slice()))
                })
            })
            .collect();
        let mesh_subsets = Arc::new(mesh_subset_map);
        let input_modifiers =
            editor_ui::InputModifierState { ctrl: self.input.ctrl_held(), shift: self.input.shift_held() };
        let scene_history_list = self.scene_history_arc();
        let atlas_snapshot = self.scene_atlas_refs_arc();
        let mesh_snapshot = self.scene_mesh_refs_arc();
        let clip_snapshot = self.scene_clip_refs_arc();
        let active_environment = self.active_environment_key.clone();
        let (debug_show_spatial_hash_state, debug_show_colliders_state) = {
            let state = self.editor_ui_state();
            (state.debug_show_spatial_hash, state.debug_show_colliders)
        };
        let collider_rects =
            if debug_show_colliders_state && self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
                self.ecs.collider_rects()
            } else {
                Vec::new()
            };
        let spatial_hash_rects =
            if debug_show_spatial_hash_state && self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
                self.ecs.spatial_hash_rects()
            } else {
                Vec::new()
            };
        if !BINARY_PREFABS_ENABLED {
            let mut state = self.editor_ui_state_mut();
            if state.prefab_format == PrefabFormat::Binary {
                state.prefab_format = PrefabFormat::Json;
            }
        }
        let transform_metrics = self.ecs.transform_clip_metrics();
        let skeletal_metrics = self.ecs.skeletal_metrics();
        let transform_eval_ms = system_timings
            .iter()
            .find(|timing| timing.name == "sys_drive_transform_clips")
            .map(|timing| timing.last_ms)
            .unwrap_or(0.0);
        let skeletal_eval_ms = system_timings
            .iter()
            .find(|timing| timing.name == "sys_drive_skeletal_clips")
            .map(|timing| timing.last_ms)
            .unwrap_or(0.0);
        let sprite_animator_count = sprite_perf_sample.map(|perf| perf.total_animators()).unwrap_or(0);
        let palette_upload_ms =
            if palette_upload_stats.calls > 0 { Some(palette_upload_stats.total_cpu_ms) } else { None };
        if let Some(analytics) = self.analytics_plugin_mut() {
            analytics.record_animation_budget_sample(AnimationBudgetSample {
                sprite_eval_ms: sprite_eval_ms.unwrap_or(0.0),
                sprite_pack_ms: sprite_pack_ms.unwrap_or(0.0),
                sprite_upload_ms,
                transform_eval_ms,
                skeletal_eval_ms,
                palette_upload_ms,
                sprite_animators: sprite_animator_count,
                transform_clip_count: transform_metrics.clip_count,
                skeletal_instance_count: skeletal_metrics.skeleton_count,
                skeletal_bone_count: skeletal_metrics.bone_count,
                palette_upload_calls: palette_upload_stats.calls,
                palette_uploaded_joints: palette_upload_stats.joints_uploaded,
            });
        }
        self.refresh_editor_analytics_state();
        self.refresh_editor_plugin_state();
        self.refresh_script_debugger_state();
        let latest_frame_timing = self.latest_frame_timing();
        let (frame_budget_idle, frame_budget_panel, frame_budget_status) = {
            let state = self.editor_ui_state();
            (
                state.frame_budget_idle_snapshot.as_ref().map(Self::frame_budget_snapshot_view),
                state.frame_budget_panel_snapshot.as_ref().map(Self::frame_budget_snapshot_view),
                state.frame_budget_status.clone(),
            )
        };

        let (id_lookup_input_state, id_lookup_active_state) = {
            let state = self.editor_ui_state();
            (state.id_lookup_input.clone(), state.id_lookup_active)
        };
        let (script_debugger_open, script_repl_input, script_repl_history_index, script_focus_repl) = {
            let state = self.editor_ui_state();
            (
                state.script_debugger_open,
                state.script_repl_input.clone(),
                state.script_repl_history_index,
                state.script_focus_repl,
            )
        };
        let script_repl_history = self.script_repl_history_arc();
        let script_console_entries = self.script_console_entries();
        let script_debugger_status = {
            let state = self.editor_ui_state();
            state.script_debugger_status.clone()
        };

        let (
            shadow_pass_metric,
            mesh_pass_metric,
            plugin_capability_metrics,
            plugin_capability_events,
            plugin_asset_readback_log,
            plugin_watchdog_log,
            plugin_manifest_error,
            plugin_manifest_entries,
            plugin_manifest_disabled_builtins,
            plugin_manifest_path,
            plugin_statuses,
            plugin_asset_metrics,
            plugin_ecs_history,
            plugin_watchdog_map,
            plugin_asset_requestable,
            animation_validation_log,
            animation_budget_sample,
            light_cluster_metrics_overlay,
            keyframe_editor_usage,
            keyframe_event_log,
        ) = {
            let state = self.editor_ui_state();
            (
                state.shadow_pass_metric,
                state.mesh_pass_metric,
                Arc::clone(&state.plugin_capability_metrics),
                Arc::clone(&state.plugin_capability_events),
                Arc::clone(&state.plugin_asset_readbacks),
                Arc::clone(&state.plugin_watchdog_events),
                state.plugin_manifest_error.clone(),
                state.plugin_manifest_entries.as_ref().map(Arc::clone),
                state.plugin_manifest_disabled_builtins.clone(),
                state.plugin_manifest_path.clone(),
                Arc::clone(&state.plugin_statuses),
                Arc::clone(&state.plugin_asset_metrics),
                Arc::clone(&state.plugin_ecs_history),
                Arc::clone(&state.plugin_watchdog_map),
                state.plugin_asset_requestable.clone(),
                Arc::clone(&state.animation_validation_log),
                state.animation_budget_sample,
                state.light_cluster_metrics_overlay,
                state.keyframe_editor_usage,
                Arc::clone(&state.keyframe_event_log),
            )
        };

        let (
            camera_bookmark_input_state,
            prefab_name_input_state,
            prefab_format_state,
            prefab_status_state,
            ui_scene_path_state,
            ui_scene_status_state,
            animation_group_input_state,
            animation_group_scale_input_state,
            inspector_status_state,
            ui_cell_size_state,
            ui_spatial_use_quadtree_state,
            ui_spatial_density_threshold_state,
            ui_spawn_per_press_state,
            ui_auto_spawn_rate_state,
            ui_environment_intensity_state,
            ui_root_spin_state,
            ui_emitter_rate_state,
            ui_emitter_spread_state,
            ui_emitter_speed_state,
            ui_emitter_lifetime_state,
            ui_emitter_start_size_state,
            ui_emitter_end_size_state,
            ui_emitter_start_color_state,
            ui_emitter_end_color_state,
            ui_particle_max_spawn_per_frame_state,
            ui_particle_max_total_state,
            ui_particle_max_emitter_backlog_state,
            ui_light_direction_state,
            ui_light_color_state,
            ui_light_ambient_state,
            ui_light_exposure_state,
            ui_shadow_distance_state,
            ui_shadow_bias_state,
            ui_shadow_strength_state,
            ui_shadow_cascade_count_state,
            ui_shadow_resolution_state,
            ui_shadow_split_lambda_state,
            ui_shadow_pcf_radius_state,
            ui_camera_zoom_min_state,
            ui_camera_zoom_max_state,
            ui_sprite_guard_pixels_state,
            ui_sprite_guard_mode_state,
            keyframe_panel_open_state,
            sprite_guardrail_status_state,
            gpu_metrics_status_state,
        ) = {
            let state = self.editor_ui_state();
            (
                state.camera_bookmark_input.clone(),
                state.prefab_name_input.clone(),
                state.prefab_format,
                state.prefab_status.clone(),
                state.ui_scene_path.clone(),
                state.ui_scene_status.clone(),
                state.animation_group_input.clone(),
                state.animation_group_scale_input,
                state.inspector_status.clone(),
                state.ui_cell_size,
                state.ui_spatial_use_quadtree,
                state.ui_spatial_density_threshold,
                state.ui_spawn_per_press,
                state.ui_auto_spawn_rate,
                state.ui_environment_intensity,
                state.ui_root_spin,
                state.ui_emitter_rate,
                state.ui_emitter_spread,
                state.ui_emitter_speed,
                state.ui_emitter_lifetime,
                state.ui_emitter_start_size,
                state.ui_emitter_end_size,
                state.ui_emitter_start_color,
                state.ui_emitter_end_color,
                state.ui_particle_max_spawn_per_frame,
                state.ui_particle_max_total,
                state.ui_particle_max_emitter_backlog,
                state.ui_light_direction,
                state.ui_light_color,
                state.ui_light_ambient,
                state.ui_light_exposure,
                state.ui_shadow_distance,
                state.ui_shadow_bias,
                state.ui_shadow_strength,
                state.ui_shadow_cascade_count,
                state.ui_shadow_resolution,
                state.ui_shadow_split_lambda,
                state.ui_shadow_pcf_radius,
                state.ui_camera_zoom_min,
                state.ui_camera_zoom_max,
                state.ui_sprite_guard_pixels,
                state.ui_sprite_guard_mode,
                state.animation_keyframe_panel.is_open(),
                state.sprite_guardrail_status.clone(),
                state.gpu_metrics_status.clone(),
            )
        };

        let (
            scene_dependencies_snapshot,
            gpu_timing_snapshot,
            gpu_history_empty,
            gpu_timing_averages,
            gizmo_mode_state,
        ) = {
            let state = self.editor_ui_state();
            let mut averages: BTreeMap<&'static str, (f32, usize)> = BTreeMap::new();
            for frame in &state.gpu_timing_history {
                for timing in &frame.timings {
                    let entry = averages.entry(timing.label).or_insert((0.0, 0));
                    entry.0 += timing.duration_ms;
                    entry.1 += 1;
                }
            }
            (
                state.scene_dependencies.clone(),
                Arc::clone(&state.gpu_timings),
                state.gpu_timing_history.is_empty(),
                averages,
                state.gizmo_mode,
            )
        };

        let (
            preview_mesh_key,
            mesh_control_mode_state,
            mesh_frustum_lock_state,
            mesh_orbit_radius,
            mesh_freefly_speed_state,
            mesh_status_message,
            persistent_materials,
            persistent_meshes,
        ) = if let Some(plugin) = self.mesh_preview_plugin() {
            (
                plugin.preview_mesh_key().to_string(),
                plugin.mesh_control_mode(),
                plugin.mesh_frustum_lock(),
                plugin.mesh_orbit().radius,
                plugin.mesh_freefly_speed(),
                plugin.mesh_status().map(|s| s.to_string()),
                plugin.persistent_materials().iter().cloned().collect(),
                plugin.persistent_meshes().iter().cloned().collect(),
            )
        } else {
            (String::new(), MeshControlMode::Disabled, false, 0.0, 0.0, None, HashSet::new(), HashSet::new())
        };

        let scene_dependency_data_available = scene_dependencies_snapshot.is_some();
        let atlas_dependencies_view = {
            let mut entries = Vec::with_capacity(atlas_snapshot.len());
            for atlas in atlas_snapshot.iter() {
                let path = scene_dependencies_snapshot.as_ref().and_then(|deps| {
                    deps.atlas_dependencies()
                        .find(|dep| dep.key() == atlas.as_str())
                        .and_then(|dep| dep.path().map(|p| p.to_string()))
                });
                entries.push(editor_ui::AtlasDependencyStatus {
                    key: atlas.clone(),
                    persistent: self.persistent_atlases.contains(atlas),
                    loaded: self.assets.has_atlas(atlas),
                    path,
                });
            }
            Arc::from(entries.into_boxed_slice())
        };
        let mesh_dependencies_view = {
            let mut entries = Vec::with_capacity(mesh_snapshot.len());
            for mesh_key in mesh_snapshot.iter() {
                let path = scene_dependencies_snapshot.as_ref().and_then(|deps| {
                    deps.mesh_dependencies()
                        .find(|dep| dep.key() == mesh_key.as_str())
                        .and_then(|dep| dep.path().map(|p| p.to_string()))
                });
                let ref_count = self.mesh_registry.mesh_ref_count(mesh_key).unwrap_or(0);
                entries.push(editor_ui::MeshDependencyStatus {
                    key: mesh_key.clone(),
                    persistent: persistent_meshes.contains(mesh_key),
                    loaded: self.mesh_registry.has(mesh_key),
                    ref_count,
                    path,
                });
            }
            Arc::from(entries.into_boxed_slice())
        };
        let clip_dependencies_view = {
            let mut entries = Vec::with_capacity(clip_snapshot.len());
            for clip_key in clip_snapshot.iter() {
                let path = scene_dependencies_snapshot.as_ref().and_then(|deps| {
                    deps.clip_dependencies()
                        .find(|dep| dep.key() == clip_key.as_str())
                        .and_then(|dep| dep.path().map(|p| p.to_string()))
                });
                entries.push(editor_ui::ClipDependencyStatus {
                    key: clip_key.clone(),
                    loaded: self.assets.clip(clip_key).is_some(),
                    path,
                });
            }
            Arc::from(entries.into_boxed_slice())
        };
        let environment_dependency_status = scene_dependencies_snapshot.as_ref().and_then(|deps| {
            deps.environment_dependency().map(|dep| editor_ui::EnvironmentDependencyStatus {
                key: dep.key().to_string(),
                persistent: self.persistent_environments.contains(dep.key()),
                loaded: self.environment_registry.definition(dep.key()).is_some(),
                path: dep.path().map(|p| p.to_string()),
            })
        });

        let editor_params = editor_ui::EditorUiParams {
            raw_input,
            base_pixels_per_point,
            hist_points,
            #[cfg(feature = "alloc_profiler")]
            allocation_delta,
            frame_timing_sample: latest_frame_timing,
            frame_budget_idle,
            frame_budget_panel,
            frame_budget_status,
            shadow_pass_metric,
            mesh_pass_metric,
            plugin_capability_metrics,
            plugin_capability_events,
            plugin_asset_readback_log,
            plugin_watchdog_log,
            plugin_manifest_error,
            plugin_manifest_entries,
            plugin_manifest_disabled_builtins,
            plugin_manifest_path,
            plugin_statuses,
            plugin_asset_metrics,
            plugin_ecs_history,
            plugin_watchdog_map,
            plugin_asset_requestable,
            animation_validation_log,
            animation_budget_sample,
            animation_time: self.ecs.world.resource::<AnimationTime>().clone(),
            light_cluster_metrics_overlay,
            light_cluster_metrics: light_cluster_snapshot,
            point_lights: self.renderer.lighting().point_lights.clone(),
            keyframe_editor_usage,
            keyframe_event_log,
            system_timings,
            entity_count,
            instances_drawn,
            vsync_enabled: self.renderer.vsync_enabled(),
            particle_budget: Some(particle_budget_snapshot),
            spatial_metrics,
            sprite_perf_sample,
            sprite_eval_ms,
            sprite_pack_ms,
            sprite_upload_ms,
            ui_scale,
            ui_cell_size: ui_cell_size_state,
            ui_spatial_use_quadtree: ui_spatial_use_quadtree_state,
            ui_spatial_density_threshold: ui_spatial_density_threshold_state,
            ui_spawn_per_press: ui_spawn_per_press_state,
            ui_auto_spawn_rate: ui_auto_spawn_rate_state,
            ui_environment_intensity: ui_environment_intensity_state,
            ui_root_spin: ui_root_spin_state,
            ui_emitter_rate: ui_emitter_rate_state,
            ui_emitter_spread: ui_emitter_spread_state,
            ui_emitter_speed: ui_emitter_speed_state,
            ui_emitter_lifetime: ui_emitter_lifetime_state,
            ui_emitter_start_size: ui_emitter_start_size_state,
            ui_emitter_end_size: ui_emitter_end_size_state,
            ui_emitter_start_color: ui_emitter_start_color_state,
            ui_emitter_end_color: ui_emitter_end_color_state,
            ui_particle_max_spawn_per_frame: ui_particle_max_spawn_per_frame_state,
            ui_particle_max_total: ui_particle_max_total_state,
            ui_particle_max_emitter_backlog: ui_particle_max_emitter_backlog_state,
            ui_light_direction: ui_light_direction_state,
            ui_light_color: ui_light_color_state,
            ui_light_ambient: ui_light_ambient_state,
            ui_light_exposure: ui_light_exposure_state,
            ui_shadow_distance: ui_shadow_distance_state,
            ui_shadow_bias: ui_shadow_bias_state,
            ui_shadow_strength: ui_shadow_strength_state,
            ui_shadow_cascade_count: ui_shadow_cascade_count_state,
            ui_shadow_resolution: ui_shadow_resolution_state,
            ui_shadow_split_lambda: ui_shadow_split_lambda_state,
            ui_shadow_pcf_radius: ui_shadow_pcf_radius_state,
            ui_camera_zoom_min: ui_camera_zoom_min_state,
            ui_camera_zoom_max: ui_camera_zoom_max_state,
            ui_sprite_guard_pixels: ui_sprite_guard_pixels_state,
            ui_sprite_guard_mode: ui_sprite_guard_mode_state,
            selected_entity: self.selected_entity(),
            selection_details: selected_info.clone(),
            prev_selection_details: prev_selection_details.clone(),
            prev_selected_entity,
            prev_gizmo_interaction,
            selection_bounds_2d,
            prev_selection_bounds_2d,
            gizmo_interaction: self.gizmo_interaction(),
            selection_changed,
            gizmo_changed,
            cursor_screen,
            cursor_world_2d,
            cursor_ray,
            hovered_scale_kind,
            viewport_camera_mode: self.viewport_camera_mode,
            camera_2d: self.camera.clone(),
            window_size,
            window_config_width: self.config.window.width,
            window_config_height: self.config.window.height,
            window_fullscreen: self.config.window.fullscreen,
            mesh_camera_for_ui,
            camera_position,
            camera_zoom,
            camera_bookmarks: self.camera_bookmarks(),
            active_camera_bookmark: self.active_camera_bookmark(),
            camera_follow_target: self.camera_follow_target.as_ref().map(|id| id.as_str().to_string()),
            preview_mesh_key,
            mesh_control_mode: mesh_control_mode_state,
            mesh_frustum_lock: mesh_frustum_lock_state,
            mesh_orbit_radius,
            mesh_freefly_speed: mesh_freefly_speed_state,
            mesh_status_message,
            camera_bookmark_input: camera_bookmark_input_state,
            mesh_keys,
            environment_options,
            active_environment,
            persistent_materials,
            debug_show_spatial_hash: debug_show_spatial_hash_state,
            debug_show_colliders: debug_show_colliders_state,
            spatial_hash_rects,
            collider_rects,

            scene_history_list,
            atlas_dependencies: atlas_dependencies_view,
            mesh_dependencies: mesh_dependencies_view,
            clip_dependencies: clip_dependencies_view,
            environment_dependency: environment_dependency_status,
            atlas_persistent_count: self.persistent_atlases.len(),
            mesh_persistent_count: persistent_meshes.len(),
            scene_dependency_data_available,
            recent_events,
            audio_triggers,
            audio_enabled,
            audio_health,
            audio_plugin_present,
            audio_spatial_config,
            binary_prefabs_enabled: BINARY_PREFABS_ENABLED,
            prefab_entries,
            prefab_name_input: prefab_name_input_state,
            prefab_format: prefab_format_state,
            prefab_status: prefab_status_state,
            clip_keys,
            clip_assets,
            skeleton_keys,
            skeleton_assets,
            atlas_keys,
            atlas_assets,
            skeleton_entities,
            material_options,
            mesh_subsets,
            input_modifiers,
            ui_scene_path: ui_scene_path_state,
            ui_scene_status: ui_scene_status_state,
            animation_group_input: animation_group_input_state,
            animation_group_scale_input: animation_group_scale_input_state,
            inspector_status: inspector_status_state,
            sprite_guardrail_status: sprite_guardrail_status_state,
            gpu_metrics_status: gpu_metrics_status_state,
            keyframe_panel_open: keyframe_panel_open_state,
            script_debugger: editor_ui::ScriptDebuggerParams {
                open: script_debugger_open,
                available: script_debugger_status.available,
                script_path: script_debugger_status.script_path.clone(),
                enabled: script_debugger_status.enabled,
                paused: script_debugger_status.paused,
                last_error: script_debugger_status.last_error.clone(),
                handles: script_debugger_status.handles.clone(),
                repl_input: script_repl_input,
                repl_history_index: script_repl_history_index,
                repl_history: script_repl_history,
                console_entries: script_console_entries,
                focus_repl: script_focus_repl,
            },
            id_lookup_input: id_lookup_input_state,
            id_lookup_active: id_lookup_active_state,
            gpu_timing_snapshot,
            gpu_history_empty,
            gpu_timing_averages,
            gpu_timing_supported: self.renderer.gpu_timing_supported(),
            gizmo_mode: gizmo_mode_state,
        };

        let ui_build_start = Instant::now();
        let editor_output = self.render_editor_ui(editor_params);
        ui_time_ms += ui_build_start.elapsed().as_secs_f32() * 1000.0;
        let editor_ui::EditorUiOutput {
            full_output,
            mut actions,
            pending_viewport,
            ui_scale: new_ui_scale,
            ui_cell_size,
            ui_spatial_use_quadtree,
            ui_spatial_density_threshold,
            ui_spawn_per_press,
            ui_auto_spawn_rate,
            ui_environment_intensity,
            ui_root_spin,
            ui_emitter_rate,
            ui_emitter_spread,
            ui_emitter_speed,
            ui_emitter_lifetime,
            ui_emitter_start_size,
            ui_emitter_end_size,
            ui_emitter_start_color,
            ui_emitter_end_color,
            ui_particle_max_spawn_per_frame,
            ui_particle_max_total,
            ui_particle_max_emitter_backlog,
            ui_light_direction,
            ui_light_color,
            ui_light_ambient,
            ui_light_exposure,
            ui_shadow_distance,
            ui_shadow_bias,
            ui_shadow_strength,
            ui_shadow_cascade_count,
            ui_shadow_resolution,
            ui_shadow_split_lambda,
            ui_shadow_pcf_radius,
            ui_camera_zoom_min,
            ui_camera_zoom_max,
            ui_sprite_guard_pixels,
            ui_sprite_guard_mode,
            mut selection,
            gizmo_mode,
            gizmo_interaction,
            viewport_mode_request,
            camera_bookmark_select,
            camera_bookmark_save,
            camera_bookmark_delete,
            mesh_control_request,
            mesh_frustum_request,
            mesh_frustum_snap,
            mesh_reset_request,
            mesh_selection_request,
            environment_selection_request,
            frame_selection_request,
            id_lookup_request,
            id_lookup_input,
            id_lookup_active,
            camera_bookmark_input,
            camera_follow_selection,
            camera_follow_clear,
            debug_show_spatial_hash,
            debug_show_colliders,
            vsync_request,
            script_debugger,
            prefab_name_input,
            prefab_format,
            prefab_status,
            ui_scene_path,
            ui_scene_status,
            animation_group_input,
            animation_group_scale_input,
            inspector_status,
            clear_scene_history,
            keyframe_panel_open,
            gpu_metrics_status,
            editor_settings_dirty,
        } = editor_output;

        let frame_budget_action = actions.frame_budget_action;
        self.handle_frame_budget_action(frame_budget_action);

        {
            let mut state = self.editor_ui_state_mut();
            state.ui_scale = new_ui_scale;
            state.camera_bookmark_input = camera_bookmark_input;
            state.prefab_name_input = prefab_name_input;
            state.prefab_format = prefab_format;
            state.prefab_status = prefab_status;
            state.ui_scene_path = ui_scene_path;
            state.ui_scene_status = ui_scene_status;
            state.animation_group_input = animation_group_input;
            state.animation_group_scale_input = animation_group_scale_input;
            state.inspector_status = inspector_status;
            if state.animation_keyframe_panel.is_open() != keyframe_panel_open {
                state.animation_keyframe_panel.toggle();
            }
            state.gpu_metrics_status = gpu_metrics_status;
            state.ui_cell_size = ui_cell_size;
            state.ui_spatial_use_quadtree = ui_spatial_use_quadtree;
            state.ui_spatial_density_threshold = ui_spatial_density_threshold;
            state.ui_spawn_per_press = ui_spawn_per_press;
            state.ui_auto_spawn_rate = ui_auto_spawn_rate;
            state.ui_environment_intensity = ui_environment_intensity;
            state.ui_root_spin = ui_root_spin;
            state.ui_emitter_rate = ui_emitter_rate;
            state.ui_emitter_spread = ui_emitter_spread;
            state.ui_emitter_speed = ui_emitter_speed;
            state.ui_emitter_lifetime = ui_emitter_lifetime;
            state.ui_emitter_start_size = ui_emitter_start_size;
            state.ui_emitter_end_size = ui_emitter_end_size;
            state.ui_emitter_start_color = ui_emitter_start_color;
            state.ui_emitter_end_color = ui_emitter_end_color;
            state.ui_particle_max_spawn_per_frame = ui_particle_max_spawn_per_frame;
            state.ui_particle_max_total = ui_particle_max_total;
            state.ui_particle_max_emitter_backlog = ui_particle_max_emitter_backlog;
            state.ui_light_direction = ui_light_direction;
            state.ui_light_color = ui_light_color;
            state.ui_light_ambient = ui_light_ambient;
            state.ui_light_exposure = ui_light_exposure;
            state.ui_shadow_distance = ui_shadow_distance;
            state.ui_shadow_bias = ui_shadow_bias;
            state.ui_shadow_strength = ui_shadow_strength;
            state.ui_shadow_cascade_count = ui_shadow_cascade_count;
            state.ui_shadow_resolution = ui_shadow_resolution;
            state.ui_shadow_split_lambda = ui_shadow_split_lambda;
            state.ui_shadow_pcf_radius = ui_shadow_pcf_radius;
            state.ui_camera_zoom_min = ui_camera_zoom_min;
            state.ui_camera_zoom_max = ui_camera_zoom_max;
            state.ui_sprite_guard_pixels = ui_sprite_guard_pixels;
            state.ui_sprite_guard_mode = ui_sprite_guard_mode;
            state.debug_show_spatial_hash = debug_show_spatial_hash;
            state.debug_show_colliders = debug_show_colliders;
            if clear_scene_history {
                state.scene_history.clear();
                state.scene_history_snapshot = None;
            }
            state.id_lookup_input = id_lookup_input;
            state.id_lookup_active = id_lookup_active;
        }
        if editor_settings_dirty {
            self.apply_editor_camera_settings();
            self.apply_editor_lighting_settings();
        }
        self.environment_intensity = ui_environment_intensity;
        self.renderer.set_environment_intensity(self.environment_intensity);

        for op in actions.inspector_actions.drain(..) {
            match op {
                editor_ui::InspectorAction::SetTranslation { entity, translation } => {
                    if self.ecs.set_translation(entity, translation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update position.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetRotation { entity, rotation } => {
                    if self.ecs.set_rotation(entity, rotation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update rotation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetScale { entity, scale } => {
                    if self.ecs.set_scale(entity, scale) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update scale.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetVelocity { entity, velocity } => {
                    if self.ecs.set_velocity(entity, velocity) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update velocity.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetEmitterTrail { entity, trail } => {
                    self.ecs.set_emitter_trail(entity, trail);
                    self.set_inspector_status(Some("Emitter trail updated.".to_string()));
                }
                editor_ui::InspectorAction::SetForceField { entity, field } => {
                    let field = field.map(|(kind, strength, radius, falloff, direction)| ForceField {
                        kind,
                        strength,
                        radius,
                        falloff,
                        direction,
                    });
                    self.ecs.set_force_field(entity, field);
                    self.set_inspector_status(Some("Force field updated.".to_string()));
                }
                editor_ui::InspectorAction::SetAttractor { entity, attractor } => {
                    let attractor = attractor.map(
                        |(strength, radius, min_distance, max_acceleration, falloff)| ParticleAttractor {
                            strength,
                            radius,
                            min_distance,
                            max_acceleration,
                            falloff,
                        },
                    );
                    self.ecs.set_attractor(entity, attractor);
                    self.set_inspector_status(Some("Attractor updated.".to_string()));
                }
                editor_ui::InspectorAction::ClearTransformClip { entity } => {
                    if self.ecs.clear_transform_clip(entity) {
                        self.set_inspector_status(Some("Transform clip cleared.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to clear transform clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClip { entity, clip_key } => {
                    if self.ecs.set_transform_clip(entity, &self.assets, &clip_key) {
                        self.set_inspector_status(Some(format!("Transform clip set to {}", clip_key)));
                    } else {
                        self.set_inspector_status(Some(format!(
                            "Transform clip '{}' not available",
                            clip_key
                        )));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipPlaying { entity, playing } => {
                    if self.ecs.set_transform_clip_playing(entity, playing) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update clip playback.".to_string()));
                    }
                }
                editor_ui::InspectorAction::ResetTransformClip { entity } => {
                    if self.ecs.reset_transform_clip(entity) {
                        self.set_inspector_status(Some("Transform clip reset.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to reset transform clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipSpeed { entity, speed } => {
                    if self.ecs.set_transform_clip_speed(entity, speed) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update clip speed.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipGroup { entity, group } => {
                    if self.ecs.set_transform_clip_group(entity, group.as_deref()) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update clip group.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipTime { entity, time } => {
                    if self.ecs.set_transform_clip_time(entity, time) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to scrub clip time.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformTrackMask { entity, mask } => {
                    if self.ecs.set_transform_track_mask(entity, mask) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update transform track mask.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetPropertyTrackMask { entity, mask } => {
                    if self.ecs.set_property_track_mask(entity, mask) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update property track mask.".to_string()));
                    }
                }
                editor_ui::InspectorAction::ClearSkeleton { entity } => {
                    if self.ecs.clear_skeleton(entity) {
                        self.set_inspector_status(Some("Skeleton detached.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to detach skeleton.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeleton { entity, skeleton_key } => {
                    if self.ecs.set_skeleton(entity, &self.assets, &skeleton_key) {
                        self.set_inspector_status(Some(format!("Skeleton set to {}", skeleton_key)));
                    } else {
                        self.set_inspector_status(Some(format!("Skeleton '{}' unavailable", skeleton_key)));
                    }
                }
                editor_ui::InspectorAction::ClearSkeletonClip { entity } => {
                    if self.ecs.clear_skeleton_clip(entity) {
                        self.set_inspector_status(Some("Skeletal clip cleared.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to clear skeletal clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClip { entity, clip_key } => {
                    if self.ecs.set_skeleton_clip(entity, &self.assets, &clip_key) {
                        self.set_inspector_status(Some(format!("Skeletal clip set to {}", clip_key)));
                    } else {
                        self.set_inspector_status(Some(format!("Skeletal clip '{}' unavailable", clip_key)));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipPlaying { entity, playing } => {
                    if self.ecs.set_skeleton_clip_playing(entity, playing) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some(
                            "Failed to update skeletal clip playback.".to_string(),
                        ));
                    }
                }
                editor_ui::InspectorAction::ResetSkeletonPose { entity } => {
                    if self.ecs.reset_skeleton_pose(entity) {
                        self.set_inspector_status(Some("Skeletal pose reset.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to reset skeletal pose.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipSpeed { entity, speed } => {
                    if self.ecs.set_skeleton_clip_speed(entity, speed) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update skeletal clip speed.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipGroup { entity, group } => {
                    if self.ecs.set_skeleton_clip_group(entity, group.as_deref()) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update skeletal clip group.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipTime { entity, time } => {
                    if self.ecs.set_skeleton_clip_time(entity, time) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to scrub skeletal clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAtlas { entity, atlas, cleared_timeline } => {
                    if self.ecs.set_sprite_atlas(entity, &self.assets, &atlas) {
                        if cleared_timeline {
                            self.set_inspector_status(Some(format!(
                                "Sprite atlas set to {} (timeline cleared)",
                                atlas
                            )));
                        } else {
                            self.set_inspector_status(Some(format!("Sprite atlas set to {}", atlas)));
                        }
                    } else {
                        self.set_inspector_status(Some(format!("Atlas '{}' unavailable", atlas)));
                    }
                }
                editor_ui::InspectorAction::SetSpriteRegion { entity, atlas, region } => {
                    if self.ecs.set_sprite_region(entity, &self.assets, &region) {
                        self.set_inspector_status(Some(format!("Sprite region set to {}", region)));
                    } else {
                        self.set_inspector_status(Some(format!(
                            "Region '{}' not found in atlas {}",
                            region, atlas
                        )));
                    }
                }
                editor_ui::InspectorAction::SetSpriteTimeline { entity, timeline } => {
                    if self.ecs.set_sprite_timeline(entity, &self.assets, timeline.as_deref()) {
                        self.set_inspector_status(
                            timeline
                                .as_ref()
                                .map(|name| format!("Sprite timeline set to {name}"))
                                .or_else(|| Some("Sprite timeline cleared".to_string())),
                        );
                    } else if let Some(name) = timeline {
                        self.set_inspector_status(Some(format!("Timeline '{name}' unavailable")));
                    } else {
                        self.set_inspector_status(Some("Failed to change sprite timeline.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationPlaying { entity, playing } => {
                    if self.ecs.set_sprite_animation_playing(entity, playing) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update animation playback.".to_string()));
                    }
                }
                editor_ui::InspectorAction::ResetSpriteAnimation { entity } => {
                    if self.ecs.reset_sprite_animation(entity) {
                        self.set_inspector_status(Some("Sprite animation reset.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to reset sprite animation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationLooped { entity, looped } => {
                    if self.ecs.set_sprite_animation_looped(entity, looped) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update loop flag.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationSpeed { entity, speed } => {
                    if self.ecs.set_sprite_animation_speed(entity, speed) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update animation speed.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationStartOffset { entity, start_offset } => {
                    if self.ecs.set_sprite_animation_start_offset(entity, start_offset) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update start offset.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationRandomStart { entity, random_start } => {
                    if self.ecs.set_sprite_animation_random_start(entity, random_start) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update random start.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationGroup { entity, group } => {
                    if self.ecs.set_sprite_animation_group(entity, group.as_deref()) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update animation group.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SeekSpriteAnimationFrame {
                    entity,
                    frame,
                    preview_events,
                    atlas,
                    timeline,
                } => {
                    if self.ecs.seek_sprite_animation_frame(entity, frame) {
                        if preview_events {
                            self.preview_sprite_events(&atlas, &timeline, frame);
                        } else {
                            self.set_inspector_status(None);
                        }
                    } else {
                        self.set_inspector_status(Some("Failed to seek animation frame.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshMaterial { entity, material } => {
                    let previous = self
                        .ecs
                        .entity_info(entity)
                        .and_then(|info| info.mesh.as_ref().and_then(|mesh| mesh.material.clone()));
                    let mut apply_change = true;
                    if let Some(ref key) = material {
                        if !self.material_registry.has(key) {
                            self.set_inspector_status(Some(format!("Material '{}' not registered", key)));
                            apply_change = false;
                        } else if let Err(err) = self.material_registry.retain(key) {
                            self.set_inspector_status(Some(format!(
                                "Failed to retain material '{}': {err}",
                                key
                            )));
                            apply_change = false;
                        }
                    }
                    if apply_change {
                        if self.ecs.set_mesh_material(entity, material.clone()) {
                            if let Some(prev) = previous {
                                if material.as_ref() != Some(&prev) {
                                    self.material_registry.release(&prev);
                                }
                            }
                            let persistent_materials: HashSet<String> = self
                                .mesh_preview_plugin()
                                .map(|plugin| plugin.persistent_materials().iter().cloned().collect())
                                .unwrap_or_default();
                            let mut refs = persistent_materials.clone();
                            for instance in self.ecs.collect_mesh_instances() {
                                if let Some(mat) = instance.material {
                                    refs.insert(mat);
                                }
                            }
                            self.scene_material_refs = refs;
                            self.set_inspector_status(None);
                        } else {
                            if let Some(ref key) = material {
                                self.material_registry.release(key);
                            }
                            self.set_inspector_status(Some("Failed to update mesh material.".to_string()));
                        }
                    } else if let Some(ref key) = material {
                        if material.as_ref() != previous.as_ref() {
                            self.material_registry.release(key);
                        }
                    }
                }
                editor_ui::InspectorAction::SetMeshShadowFlags { entity, cast, receive } => {
                    if self.ecs.set_mesh_shadow_flags(entity, cast, receive) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh shadow flags.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshMaterialParams {
                    entity,
                    base_color,
                    metallic,
                    roughness,
                    emissive,
                } => {
                    if self.ecs.set_mesh_material_params(entity, base_color, metallic, roughness, emissive) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some(
                            "Failed to update mesh material parameters.".to_string(),
                        ));
                    }
                }
                editor_ui::InspectorAction::SetMeshTranslation { entity, translation } => {
                    if self.ecs.set_mesh_translation(entity, translation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh translation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshRotationEuler { entity, rotation } => {
                    if self.ecs.set_mesh_rotation_euler(entity, rotation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh rotation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshScale3D { entity, scale } => {
                    if self.ecs.set_mesh_scale(entity, scale) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh scale.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshTint { entity, tint } => {
                    if self.ecs.set_tint(entity, tint) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update tint.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkinMeshJointCount { entity, joint_count } => {
                    if self.ecs.set_skin_mesh_joint_count(entity, joint_count) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some(
                            "Failed to update skin mesh joint count.".to_string(),
                        ));
                    }
                }
                editor_ui::InspectorAction::SetSkinMeshSkeleton { entity, skeleton } => {
                    if self.ecs.set_skin_mesh_skeleton(entity, skeleton) {
                        let status = skeleton
                            .map(|skel| format!("Skin mesh bound to skeleton #{:04}", skel.index()))
                            .unwrap_or_else(|| "Skin mesh skeleton cleared.".to_string());
                        self.set_inspector_status(Some(status));
                    } else {
                        self.set_inspector_status(Some("Failed to update skin mesh skeleton.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SyncSkinMeshJointCount { entity } => {
                    let skeleton = self
                        .ecs
                        .entity_info(entity)
                        .and_then(|info| info.skin_mesh.as_ref().and_then(|sm| sm.skeleton_entity));
                    match skeleton {
                        Some(skel_entity) => {
                            if let Some(skeleton_info) =
                                self.ecs.entity_info(skel_entity).and_then(|info| info.skeleton)
                            {
                                if self.ecs.set_skin_mesh_joint_count(entity, skeleton_info.joint_count) {
                                    self.set_inspector_status(Some(format!(
                                        "Skin mesh joints set to {}",
                                        skeleton_info.joint_count
                                    )));
                                } else {
                                    self.set_inspector_status(Some(
                                        "Failed to sync joint count from skeleton.".to_string(),
                                    ));
                                }
                            } else {
                                self.set_inspector_status(Some(
                                    "Selected skeleton is missing SkeletonInstance.".to_string(),
                                ));
                            }
                        }
                        None => {
                            self.set_inspector_status(Some(
                                "Assign a skeleton before syncing joints.".to_string(),
                            ));
                        }
                    }
                }
                editor_ui::InspectorAction::DetachSkinMesh { entity } => {
                    if self.ecs.detach_skin_mesh(entity) {
                        self.set_inspector_status(Some("Skin mesh component removed.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to remove skin mesh.".to_string()));
                    }
                }
                editor_ui::InspectorAction::AttachSkinMesh { entity } => {
                    if self.ecs.attach_skin_mesh(entity, 0) {
                        self.set_inspector_status(Some("Skin mesh component added.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to add skin mesh component.".to_string()));
                    }
                }
            }
        }

        if let Some(request) = id_lookup_request {
            let trimmed = request.trim();
            if trimmed.is_empty() {
                self.set_ui_scene_status("Enter an entity ID to select.".to_string());
            } else if let Some(entity) = self.ecs.find_entity_by_scene_id(trimmed) {
                selection.entity = Some(entity);
                selection.details = self.ecs.entity_info(entity);
                self.set_ui_scene_status(format!("Selected entity {}", trimmed));
            } else {
                self.set_ui_scene_status(format!("Entity {} not found", trimmed));
            }
        }

        self.set_selected_entity(selection.entity);
        self.set_gizmo_mode(gizmo_mode);
        self.set_gizmo_interaction(gizmo_interaction);
        if self.input.take_delete_selection() {
            if let Some(entity) = self.selected_entity() {
                if actions.delete_entity.is_none() {
                    actions.delete_entity = Some(entity);
                }
            }
        }
        self.apply_particle_caps();

        if let Some(request) = camera_bookmark_select {
            match request {
                Some(name) => {
                    if !self.apply_camera_bookmark_by_name(&name) {
                        self.set_ui_scene_status(format!("Bookmark '{}' not found.", name));
                    }
                }
                None => {
                    self.set_active_camera_bookmark(None);
                    self.camera_follow_target = None;
                    self.set_ui_scene_status("Camera set to free mode.".to_string());
                }
            }
        }
        if let Some(name) = camera_bookmark_save {
            if self.upsert_camera_bookmark(&name) {
                self.set_ui_scene_status(format!("Saved camera bookmark '{}'.", name.trim()));
            } else {
                self.set_ui_scene_status("Enter a bookmark name to save.".to_string());
            }
        }
        if let Some(name) = camera_bookmark_delete {
            if self.delete_camera_bookmark(&name) {
                self.set_ui_scene_status(format!("Deleted camera bookmark '{}'.", name.trim()));
            } else {
                self.set_ui_scene_status(format!("Bookmark '{}' not found.", name.trim()));
            }
        }
        if camera_follow_selection {
            if let Some(details) = selection.details.as_ref() {
                let scene_id = details.scene_id.clone();
                if self.set_camera_follow_scene_id(scene_id) {
                    self.set_ui_scene_status(format!("Following entity {}.", details.scene_id.as_str()));
                } else {
                    self.set_ui_scene_status("Unable to follow selected entity.".to_string());
                }
            } else {
                self.set_ui_scene_status("Select an entity to follow.".to_string());
            }
        }
        if camera_follow_clear && self.camera_follow_target.is_some() {
            self.clear_camera_follow();
            self.set_ui_scene_status("Camera follow cleared.".to_string());
        }

        if let Some(mode) = viewport_mode_request {
            self.set_viewport_camera_mode(mode);
        }
        if let Some(mode) = mesh_control_request {
            self.set_mesh_control_mode(mode);
        }
        if let Some(lock) = mesh_frustum_request {
            self.set_frustum_lock(lock);
        }
        if mesh_frustum_snap {
            if let Some(plugin) = self.mesh_preview_plugin_mut() {
                plugin.snap_frustum_to_selection(selection.details.as_ref(), orbit_target);
            }
        }
        if mesh_reset_request {
            self.reset_mesh_camera();
        }
        if let Some(key) = mesh_selection_request {
            self.set_preview_mesh(key);
        }
        if let Some(environment_key) = environment_selection_request {
            match self.set_active_environment(&environment_key, self.environment_intensity) {
                Ok(()) => {
                    self.set_ui_scene_status(format!("Environment set to {}", environment_key));
                }
                Err(err) => {
                    self.set_ui_scene_status(format!("Environment '{}' unavailable: {err}", environment_key));
                }
            }
        }
        if let Some(point_lights) = actions.point_light_update {
            self.renderer.lighting_mut().point_lights = point_lights;
        }

        let egui::FullOutput { platform_output, textures_delta, shapes, .. } = full_output;
        if let Some(window) = self.renderer.window() {
            self.editor_shell.egui_winit.as_mut().unwrap().handle_platform_output(window, platform_output);
        } else {
            return;
        }

        {
            let mut state = self.editor_ui_state_mut();
            state.script_debugger_open = script_debugger.open;
            state.script_repl_input = script_debugger.repl_input;
            state.script_repl_history_index = script_debugger.repl_history_index;
            state.script_focus_repl = script_debugger.focus_repl;
            if script_debugger.clear_console {
                state.script_console.clear();
                state.script_console_snapshot = None;
            }
        }
        if let Some(enabled) = script_debugger.set_enabled {
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.set_enabled(enabled);
            }
        }
        if let Some(paused) = script_debugger.set_paused {
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.set_paused(paused);
            }
        }
        if script_debugger.step_once {
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.step_once();
            }
        }
        if script_debugger.reload {
            if let Some(plugin) = self.script_plugin_mut() {
                if let Err(err) = plugin.force_reload() {
                    plugin.set_error_message(err.to_string());
                }
            }
        }
        if let Some(command) = script_debugger.submit_command {
            self.execute_repl_command(command);
        }

        if let Some((origin, size)) = pending_viewport {
            self.update_viewport(origin, size);
        }
        if frame_selection_request {
            if self.focus_selection() {
                self.set_inspector_status(Some("Viewport framed selection.".to_string()));
            } else {
                self.set_inspector_status(Some("Selection unavailable.".to_string()));
            }
        }

        for (key, path) in actions.retain_atlases {
            match self.assets.retain_atlas(&key, path.as_deref()) {
                Ok(()) => {
                    self.scene_atlas_refs.insert(key.clone());
                    self.invalidate_atlas_view(&key);
                    self.set_ui_scene_status(format!("Retained atlas {}", key));
                }
                Err(err) => {
                    self.set_ui_scene_status(format!("Atlas retain failed: {err}"));
                }
            }
        }
        for (key, path) in actions.retain_clips {
            match self.assets.retain_clip(&key, path.as_deref()) {
                Ok(()) => {
                    self.set_ui_scene_status(format!("Retained clip {}", key));
                    if let Some(source) = path.as_deref() {
                        self.queue_animation_watch_root(Path::new(source), AnimationAssetKind::Clip);
                    }
                }
                Err(err) => {
                    self.set_ui_scene_status(format!("Clip retain failed: {err}"));
                }
            }
        }
        for request in actions.sprite_atlas_requests {
            let entity = request.entity;
            let atlas = request.atlas;
            let path = request.path;
            if let Some(path) = path.as_deref() {
                if !self.assets.has_atlas(&atlas) {
                    match self.assets.retain_atlas(&atlas, Some(path)) {
                        Ok(()) => {
                            self.scene_atlas_refs.insert(atlas.clone());
                            self.invalidate_atlas_view(&atlas);
                        }
                        Err(err) => {
                            self.set_inspector_status(Some(format!(
                                "Failed to load atlas '{}': {err}",
                                atlas
                            )));
                            continue;
                        }
                    }
                }
            }
            if self.assets.has_atlas(&atlas) {
                let had_animation = self.ecs.world.get::<SpriteAnimation>(entity).is_some();
                if self.ecs.set_sprite_atlas(entity, &self.assets, &atlas) {
                    let status = if had_animation {
                        format!("Sprite atlas set to {} (timeline cleared)", atlas)
                    } else {
                        format!("Sprite atlas set to {}", atlas)
                    };
                    self.set_inspector_status(Some(status));
                    self.scene_atlas_refs.insert(atlas.clone());
                    self.invalidate_atlas_view(&atlas);
                } else {
                    self.set_inspector_status(Some(format!("Failed to assign atlas '{}' to sprite", atlas)));
                }
            } else {
                self.set_inspector_status(Some(format!("Atlas '{}' not loaded; unable to assign", atlas)));
            }
        }
        for (key, path) in actions.retain_meshes {
            match self.mesh_registry.retain_mesh(&key, path.as_deref(), &mut self.material_registry) {
                Ok(()) => {
                    self.scene_mesh_refs.insert(key.clone());
                    match self.mesh_registry.ensure_gpu(&key, &mut self.renderer) {
                        Ok(_) => {
                            self.set_ui_scene_status(format!("Retained mesh {}", key));
                        }
                        Err(err) => {
                            self.set_mesh_status(format!("Mesh upload failed: {err}"));
                        }
                    }
                }
                Err(err) => {
                    self.set_ui_scene_status(format!("Mesh retain failed: {err}"));
                }
            }
        }
        for (key, path) in actions.retain_environments {
            match self.environment_registry.retain(&key, path.as_deref()) {
                Ok(()) => {
                    let scene_requested = self.scene_environment_ref.as_deref() == Some(key.as_str());
                    let should_activate = scene_requested || self.active_environment_key == key;
                    if let Err(err) = self.environment_registry.ensure_gpu(&key, &mut self.renderer) {
                        self.set_ui_scene_status(format!("Environment upload failed: {err}"));
                        continue;
                    }
                    if should_activate {
                        match self.set_active_environment(&key, self.environment_intensity) {
                            Ok(()) => {
                                self.set_ui_scene_status(format!("Environment set to {}", key));
                            }
                            Err(err) => {
                                self.set_ui_scene_status(format!("Environment bind failed: {err}"));
                            }
                        }
                    } else {
                        self.set_ui_scene_status(format!("Retained environment {}", key));
                    }
                }
                Err(err) => {
                    self.set_ui_scene_status(format!("Environment retain failed: {err}"));
                }
            }
        }

        if actions.save_scene {
            let mesh_source_map: HashMap<String, String> = self
                .mesh_registry
                .keys()
                .filter_map(|key| {
                    self.mesh_registry
                        .mesh_source(key)
                        .map(|path| (key.to_string(), path.to_string_lossy().into_owned()))
                })
                .collect();
            let material_source_map: HashMap<String, String> = self
                .material_registry
                .keys()
                .filter_map(|key| {
                    self.material_registry
                        .material_source(key)
                        .map(|path| (key.to_string(), path.to_string()))
                })
                .collect();
            let mut scene = self.ecs.export_scene_with_sources(
                &self.assets,
                |key| mesh_source_map.get(key).cloned(),
                |key| material_source_map.get(key).cloned(),
            );
            let environment_dependency =
                self.environment_registry.definition(&self.active_environment_key).map(|def| {
                    EnvironmentDependency::new(
                        def.key().to_string(),
                        def.source().map(|path| path.to_string()),
                    )
                });
            scene.dependencies.set_environment_dependency(environment_dependency);
            scene.metadata = self.capture_scene_metadata();
            let scene_path = self.editor_ui_state().ui_scene_path.clone();
            match scene.save_to_path(&scene_path) {
                Ok(_) => {
                    self.set_ui_scene_status(format!("Saved {}", scene_path));
                    self.remember_scene_path(&scene_path);
                }
                Err(err) => self.set_ui_scene_status(format!("Save failed: {err}")),
            }
        }
        if actions.load_scene {
            let scene_path = self.editor_ui_state().ui_scene_path.clone();
            match Scene::load_from_path(&scene_path) {
                Ok(scene) => match self.update_scene_dependencies(&scene.dependencies) {
                    Ok(()) => {
                        if let Err(err) = self.ecs.load_scene_with_dependencies(
                            &scene,
                            &self.assets,
                            |_, _| Ok(()),
                            |_, _| Ok(()),
                            |_, _| Ok(()),
                        ) {
                            self.set_ui_scene_status(format!("Load failed: {err}"));
                        } else {
                            self.set_ui_scene_status(format!("Loaded {}", scene_path));
                            self.remember_scene_path(&scene_path);
                            self.apply_scene_metadata(&scene.metadata);
                            self.set_selected_entity(None);
                            self.set_gizmo_interaction(None);
                            if let Some(plugin) = self.script_plugin_mut() {
                                plugin.clear_handles();
                            }
                            if let Some(analytics) = self.analytics_plugin_mut() {
                                analytics.clear_frame_history();
                            }
                            self.sync_emitter_ui();
                            self.set_inspector_status(None);
                        }
                    }
                    Err(err) => {
                        self.set_ui_scene_status(format!("Load failed: {err}"));
                        self.ecs.clear_world();
                        self.clear_scene_atlases();
                        self.clear_scene_clips();
                        self.set_selected_entity(None);
                        self.set_gizmo_interaction(None);
                        if let Some(plugin) = self.script_plugin_mut() {
                            plugin.clear_handles();
                        }
                        self.sync_emitter_ui();
                        self.set_inspector_status(None);
                    }
                },
                Err(err) => {
                    self.set_ui_scene_status(format!("Load failed: {err}"));
                }
            }
        }
        if let Some(request) = actions.save_prefab {
            self.handle_save_prefab(request);
        }
        if let Some(request) = actions.instantiate_prefab {
            self.handle_instantiate_prefab(request);
        }
        if actions.spawn_now {
            let spawn_per_press = self.editor_ui_state().ui_spawn_per_press;
            self.ecs.spawn_burst(&self.assets, spawn_per_press as usize);
        }
        if let Some(mesh_key) = actions.spawn_mesh {
            self.spawn_mesh_entity(&mesh_key);
        }
        if let Some(entity) = actions.delete_entity {
            if self.ecs.despawn_entity(entity) {
                if let Some(plugin) = self.script_plugin_mut() {
                    plugin.forget_entity(entity);
                }
            }
            self.set_selected_entity(None);
            self.set_gizmo_interaction(None);
        }
        if actions.clear_particles {
            self.ecs.clear_particles();
            {
                let mut state = self.editor_ui_state_mut();
                state.ui_emitter_rate = 0.0;
                state.ui_emitter_spread = std::f32::consts::PI / 3.0;
                state.ui_emitter_speed = 0.8;
                state.ui_emitter_lifetime = 1.2;
                state.ui_emitter_start_size = 0.05;
                state.ui_emitter_end_size = 0.05;
                state.ui_emitter_start_color = [1.0, 1.0, 1.0, 1.0];
                state.ui_emitter_end_color = [1.0, 1.0, 1.0, 0.0];
            }
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.clear_handles();
            }
            self.set_gizmo_interaction(None);
            if let Some(emitter) = self.emitter_entity {
                let (
                    emitter_rate,
                    emitter_spread,
                    emitter_speed,
                    emitter_lifetime,
                    emitter_start_size,
                    emitter_end_size,
                    emitter_start_color,
                    emitter_end_color,
                ) = {
                    let state = self.editor_ui_state();
                    (
                        state.ui_emitter_rate,
                        state.ui_emitter_spread,
                        state.ui_emitter_speed,
                        state.ui_emitter_lifetime,
                        state.ui_emitter_start_size,
                        state.ui_emitter_end_size,
                        state.ui_emitter_start_color,
                        state.ui_emitter_end_color,
                    )
                };
                self.ecs.set_emitter_rate(emitter, emitter_rate);
                self.ecs.set_emitter_spread(emitter, emitter_spread);
                self.ecs.set_emitter_speed(emitter, emitter_speed);
                self.ecs.set_emitter_lifetime(emitter, emitter_lifetime);
                self.ecs.set_emitter_colors(
                    emitter,
                    Vec4::from_array(emitter_start_color),
                    Vec4::from_array(emitter_end_color),
                );
                self.ecs.set_emitter_sizes(emitter, emitter_start_size, emitter_end_size);
            }
        }
        if actions.reset_world {
            self.ecs.clear_world();
            self.clear_scene_atlases();
            self.clear_scene_clips();
            self.set_selected_entity(None);
            self.set_gizmo_interaction(None);
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.clear_handles();
            }
            self.sync_emitter_ui();
            self.set_inspector_status(None);
        }
        if !actions.plugin_watchdog_clear.is_empty() {
            let manager = self.plugin_runtime.manager_mut();
            for plugin in actions.plugin_watchdog_clear.drain(..) {
                manager.clear_watchdog_events(&plugin);
            }
        }
        if !actions.plugin_retry_asset_readback.is_empty() {
            let mut retry_results = Vec::new();
            {
                let manager = self.plugin_runtime.manager_mut();
                for plugin in actions.plugin_retry_asset_readback.drain(..) {
                    let result = manager.retry_last_asset_readback(&plugin);
                    retry_results.push((plugin, result));
                }
            }
            for (plugin, result) in retry_results {
                match result {
                    Ok(Some(response)) => {
                        let bytes = response.byte_length;
                        let content_type = response.content_type.clone();
                        self.set_ui_scene_status(format!(
                            "Retried asset readback for {plugin}: {bytes} bytes ({content_type})"
                        ));
                    }
                    Ok(None) => {
                        self.set_ui_scene_status(format!("No asset readbacks recorded for {plugin}"));
                    }
                    Err(err) => {
                        self.set_ui_scene_status(format!("Asset readback retry failed for {plugin}: {err}"));
                    }
                }
            }
        }
        if let Some(enabled) = actions.audio_set_enabled {
            match self.plugin_runtime.manager_mut().get_mut::<AudioPlugin>() {
                Some(audio) => audio.set_enabled(enabled),
                None => self.set_ui_scene_status("Audio plugin unavailable; cannot update audio state."),
            }
        }
        if actions.audio_spatial_enable.is_some()
            || actions.audio_spatial_min_distance.is_some()
            || actions.audio_spatial_max_distance.is_some()
            || actions.audio_spatial_pan_width.is_some()
        {
            match self.plugin_runtime.manager_mut().get_mut::<AudioPlugin>() {
                Some(audio) => {
                    let mut cfg = audio.spatial_config();
                    if let Some(en) = actions.audio_spatial_enable {
                        cfg.enabled = en;
                    }
                    if let Some(min) = actions.audio_spatial_min_distance {
                        cfg.min_distance = min.max(0.0);
                    }
                    if let Some(max) = actions.audio_spatial_max_distance {
                        cfg.max_distance = max.max(cfg.min_distance + 0.001);
                    }
                    if let Some(width) = actions.audio_spatial_pan_width {
                        cfg.pan_width = width.max(0.1);
                    }
                    audio.set_spatial_config(cfg);
                }
                None => self.set_ui_scene_status("Audio plugin unavailable; cannot update spatial audio."),
            }
        }
        if actions.audio_clear_log {
            match self.plugin_runtime.manager_mut().get_mut::<AudioPlugin>() {
                Some(audio) => audio.clear(),
                None => {
                    self.set_ui_scene_status("Audio plugin unavailable; cannot clear audio log.");
                }
            }
        }
        if !actions.plugin_toggles.is_empty() {
            self.apply_plugin_toggles(&actions.plugin_toggles);
        }
        if actions.reload_plugins {
            self.reload_dynamic_plugins();
        }
        if let (Some(ren), Some(screen)) =
            (self.editor_shell.egui_renderer.as_mut(), self.editor_shell.egui_screen.as_ref())
        {
            if let (Ok(device), Ok(queue)) = (self.renderer.device(), self.renderer.queue()) {
                for (id, delta) in &textures_delta.set {
                    ren.update_texture(device, queue, *id, delta);
                }
            }
            let ui_render_start = Instant::now();
            let meshes = self.editor_shell.egui_ctx.tessellate(shapes, screen.pixels_per_point);
            if let Err(err) = self.renderer.render_egui(ren, &meshes, screen, frame) {
                eprintln!("Egui render error: {err:?}");
            }
            ui_time_ms += ui_render_start.elapsed().as_secs_f32() * 1000.0;
            for id in &textures_delta.free {
                ren.free_texture(id);
            }
            let timings = self.renderer.take_gpu_timings();
            if !timings.is_empty() {
                if let Some(analytics) = self.analytics_plugin_mut() {
                    analytics.record_gpu_timings(&timings);
                }
                self.update_gpu_timing_snapshots(timings);
            }
        } else {
            frame.present();
            let timings = self.renderer.take_gpu_timings();
            if !timings.is_empty() {
                if let Some(analytics) = self.analytics_plugin_mut() {
                    analytics.record_gpu_timings(&timings);
                }
                self.update_gpu_timing_snapshots(timings);
            }
        }

        if let Some(enabled) = vsync_request {
            self.apply_vsync_toggle(enabled);
        }

        let ui_root_spin = self.editor_ui_state().ui_root_spin;
        self.ecs.set_root_spin(ui_root_spin);

        if let Some(w) = self.renderer.window() {
            w.request_redraw();
        }
        self.input.clear_frame();
        let frame_ms = frame_start.elapsed().as_secs_f32() * 1000.0;
        self.record_frame_timing_sample(FrameTimingSample {
            frame_ms,
            update_ms: update_time_ms,
            fixed_ms: fixed_time_ms,
            render_ms: render_time_ms,
            ui_ms: ui_time_ms,
        });
        #[cfg(feature = "alloc_profiler")]
        if let Some(mut capture) = self.frame_budget_capture.take() {
            capture.update(self);
            if capture.is_complete() {
                self.frame_budget_capture = None;
            } else {
                self.frame_budget_capture = Some(capture);
            }
        }
    }
}

#[cfg(feature = "alloc_profiler")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameBudgetCaptureStage {
    IdleWarmup,
    PanelWarmup,
    Done,
}

#[cfg(feature = "alloc_profiler")]
struct FrameBudgetCaptureScript {
    stage: FrameBudgetCaptureStage,
    idle_wait_frames: u32,
    panel_wait_frames: u32,
    panels_opened: bool,
}

#[cfg(feature = "alloc_profiler")]
impl FrameBudgetCaptureScript {
    fn from_env() -> Option<Self> {
        let value = env::var("KESTREL_FRAME_BUDGET_CAPTURE").ok()?;
        if value.eq_ignore_ascii_case("all_panels") {
            eprintln!("[frame_budget] auto-capture enabled (mode={value})");
            Some(Self {
                stage: FrameBudgetCaptureStage::IdleWarmup,
                idle_wait_frames: 180,
                panel_wait_frames: 240,
                panels_opened: false,
            })
        } else {
            eprintln!(
                "[frame_budget] unknown capture mode '{value}'; expected 'all_panels'. Ignoring request."
            );
            None
        }
    }

    fn update(&mut self, app: &mut App) {
        match self.stage {
            FrameBudgetCaptureStage::IdleWarmup => {
                if self.idle_wait_frames > 0 {
                    self.idle_wait_frames -= 1;
                    return;
                }
                eprintln!("[frame_budget] capturing idle baseline snapshot...");
                app.handle_frame_budget_action(Some(editor_ui::FrameBudgetAction::CaptureIdle));
                if let Some(snapshot) = app.editor_ui_state().frame_budget_idle_snapshot {
                    Self::log_snapshot("idle", snapshot);
                } else {
                    eprintln!("[frame_budget] idle snapshot unavailable after capture.");
                }
                self.stage = FrameBudgetCaptureStage::PanelWarmup;
            }
            FrameBudgetCaptureStage::PanelWarmup => {
                if !self.panels_opened {
                    eprintln!("[frame_budget] enabling optional editor panels for capture...");
                    app.with_editor_ui_state_mut(|state| {
                        if !state.animation_keyframe_panel.is_open() {
                            state.animation_keyframe_panel.toggle();
                        }
                        state.script_debugger_open = true;
                        state.id_lookup_active = true;
                    });
                    self.panels_opened = true;
                }
                if self.panel_wait_frames > 0 {
                    self.panel_wait_frames -= 1;
                    return;
                }
                eprintln!("[frame_budget] capturing all-panels snapshot...");
                app.handle_frame_budget_action(Some(editor_ui::FrameBudgetAction::CapturePanel));
                if let Some(snapshot) = app.editor_ui_state().frame_budget_panel_snapshot {
                    Self::log_snapshot("panels", snapshot);
                } else {
                    eprintln!("[frame_budget] panel snapshot unavailable after capture.");
                }
                if let Some(delta) = app.frame_budget_delta_message() {
                    eprintln!("[frame_budget] {delta}");
                }
                self.stage = FrameBudgetCaptureStage::Done;
                app.should_close = true;
            }
            FrameBudgetCaptureStage::Done => {}
        }
    }

    fn is_complete(&self) -> bool {
        matches!(self.stage, FrameBudgetCaptureStage::Done)
    }

    fn log_snapshot(label: &str, snapshot: FrameBudgetSnapshot) {
        if let Some(timing) = snapshot.timing {
            let mut message = format!(
                "[frame_budget] {label} snapshot: frame={:.2} ms update={:.2} ms fixed={:.2} ms render={:.2} ms ui={:.2} ms",
                timing.frame_ms, timing.update_ms, timing.fixed_ms, timing.render_ms, timing.ui_ms
            );
            if let Some(delta) = snapshot.alloc_delta {
                message.push_str(&format!(
                    " alloc=+{} B/-{} B net={:+} B",
                    delta.allocated_bytes,
                    delta.deallocated_bytes,
                    delta.net_bytes()
                ));
            }
            eprintln!("{message}");
        } else {
            eprintln!("[frame_budget] {label} snapshot missing timing data.");
        }
    }
}

struct SpriteGuardrailProjection {
    pixels_per_world: Vec2,
}

impl SpriteGuardrailProjection {
    fn new(camera: &Camera2D, viewport_size: PhysicalSize<u32>) -> Option<Self> {
        let (half_width, half_height) = camera.half_extents(viewport_size)?;
        if half_width <= f32::EPSILON || half_height <= f32::EPSILON {
            return None;
        }
        let pixels_per_world_x = viewport_size.width as f32 / (half_width * 2.0);
        let pixels_per_world_y = viewport_size.height as f32 / (half_height * 2.0);
        Some(Self { pixels_per_world: Vec2::new(pixels_per_world_x, pixels_per_world_y) })
    }

    fn extent(&self, half_extent: Vec2) -> f32 {
        let size = half_extent * 2.0;
        (size.x * self.pixels_per_world.x).max(size.y * self.pixels_per_world.y)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.with_plugins(|plugins, ctx| plugins.shutdown(ctx));
    }
}

impl App {
    fn apply_script_commands(&mut self, commands: Vec<ScriptCommand>) {
        for cmd in commands {
            match cmd {
                ScriptCommand::Spawn { handle, atlas, region, position, scale, velocity } => {
                    match self.ecs.spawn_scripted_sprite(
                        &self.assets,
                        &atlas,
                        &region,
                        position,
                        scale,
                        velocity,
                    ) {
                        Ok(entity) => {
                            self.register_script_spawn(handle, entity);
                        }
                        Err(err) => {
                            eprintln!("[script] spawn error for {atlas}:{region}: {err}");
                            self.forget_script_handle(handle);
                        }
                    }
                }
                ScriptCommand::SetVelocity { handle, velocity } => {
                    if let Some(entity) = self.resolve_script_handle(handle) {
                        if !self.ecs.set_velocity(entity, velocity) {
                            eprintln!("[script] set_velocity failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_velocity unknown handle {handle}");
                    }
                }
                ScriptCommand::SetPosition { handle, position } => {
                    if let Some(entity) = self.resolve_script_handle(handle) {
                        if !self.ecs.set_translation(entity, position) {
                            eprintln!("[script] set_position failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_position unknown handle {handle}");
                    }
                }
                ScriptCommand::SetRotation { handle, rotation } => {
                    if let Some(entity) = self.resolve_script_handle(handle) {
                        if !self.ecs.set_rotation(entity, rotation) {
                            eprintln!("[script] set_rotation failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_rotation unknown handle {handle}");
                    }
                }
                ScriptCommand::SetScale { handle, scale } => {
                    if let Some(entity) = self.resolve_script_handle(handle) {
                        if !self.ecs.set_scale(entity, scale) {
                            eprintln!("[script] set_scale failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_scale unknown handle {handle}");
                    }
                }
                ScriptCommand::SetTint { handle, tint } => {
                    if let Some(entity) = self.resolve_script_handle(handle) {
                        if !self.ecs.set_tint(entity, tint) {
                            eprintln!("[script] set_tint failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_tint unknown handle {handle}");
                    }
                }
                ScriptCommand::SetSpriteRegion { handle, region } => {
                    if let Some(entity) = self.resolve_script_handle(handle) {
                        if !self.ecs.set_sprite_region(entity, &self.assets, &region) {
                            eprintln!("[script] set_sprite_region failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_sprite_region unknown handle {handle}");
                    }
                }
                ScriptCommand::Despawn { handle } => {
                    if let Some(entity) = self.resolve_script_handle(handle) {
                        if self.ecs.despawn_entity(entity) {
                            self.forget_script_handle(handle);
                        } else {
                            eprintln!("[script] despawn failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] despawn unknown handle {handle}");
                    }
                }
                ScriptCommand::SetAutoSpawnRate { rate } => {
                    let clamped = rate.max(0.0);
                    self.editor_ui_state_mut().ui_auto_spawn_rate = clamped;
                }
                ScriptCommand::SetSpawnPerPress { count } => {
                    let clamped = count.max(0);
                    self.editor_ui_state_mut().ui_spawn_per_press = clamped;
                }
                ScriptCommand::SetEmitterRate { rate } => {
                    let clamped = rate.max(0.0);
                    self.editor_ui_state_mut().ui_emitter_rate = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_rate(emitter, clamped);
                    }
                }
                ScriptCommand::SetEmitterSpread { spread } => {
                    let clamped = spread.clamp(0.0, std::f32::consts::PI);
                    self.editor_ui_state_mut().ui_emitter_spread = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_spread(emitter, clamped);
                    }
                }
                ScriptCommand::SetEmitterSpeed { speed } => {
                    let clamped = speed.max(0.0);
                    self.editor_ui_state_mut().ui_emitter_speed = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_speed(emitter, clamped);
                    }
                }
                ScriptCommand::SetEmitterLifetime { lifetime } => {
                    let clamped = lifetime.max(0.05);
                    self.editor_ui_state_mut().ui_emitter_lifetime = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_lifetime(emitter, clamped);
                    }
                }
                ScriptCommand::SetEmitterStartColor { color } => {
                    self.editor_ui_state_mut().ui_emitter_start_color = color.to_array();
                    if let Some(emitter) = self.emitter_entity {
                        let end_color = self.editor_ui_state().ui_emitter_end_color;
                        self.ecs.set_emitter_colors(emitter, color, Vec4::from_array(end_color));
                    }
                }
                ScriptCommand::SetEmitterEndColor { color } => {
                    self.editor_ui_state_mut().ui_emitter_end_color = color.to_array();
                    if let Some(emitter) = self.emitter_entity {
                        let start_color = self.editor_ui_state().ui_emitter_start_color;
                        self.ecs.set_emitter_colors(emitter, Vec4::from_array(start_color), color);
                    }
                }
                ScriptCommand::SetEmitterStartSize { size } => {
                    let clamped = size.max(0.01);
                    self.editor_ui_state_mut().ui_emitter_start_size = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        let end_size = self.editor_ui_state().ui_emitter_end_size;
                        self.ecs.set_emitter_sizes(emitter, clamped, end_size);
                    }
                }
                ScriptCommand::SetEmitterEndSize { size } => {
                    let clamped = size.max(0.01);
                    self.editor_ui_state_mut().ui_emitter_end_size = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        let start_size = self.editor_ui_state().ui_emitter_start_size;
                        self.ecs.set_emitter_sizes(emitter, start_size, clamped);
                    }
                }
            }
        }
    }
}

struct AnimationReloadRequest {
    path: PathBuf,
    key: String,
    kind: AnimationAssetKind,
    skip_validation: bool,
}

struct AnimationReloadJob {
    request: AnimationReloadRequest,
}

struct AnimationReloadResult {
    request: AnimationReloadRequest,
    data: Result<AnimationReloadData>,
}

enum AnimationReloadData {
    Clip { clip: AnimationClip, bytes: Vec<u8> },
    Graph { graph: AnimationGraphAsset, bytes: Vec<u8> },
    Skeletal { import: skeletal::SkeletonImport },
}

struct AnimationReloadQueue {
    buckets: [VecDeque<AnimationReloadRequest>; AnimationAssetKind::COUNT],
    next_bucket: usize,
    max_len: usize,
}

impl AnimationReloadQueue {
    fn new(max_len: usize) -> Self {
        Self { buckets: [VecDeque::new(), VecDeque::new(), VecDeque::new()], next_bucket: 0, max_len }
    }

    fn enqueue(&mut self, request: AnimationReloadRequest) -> Option<AnimationReloadRequest> {
        let idx = request.kind.index();
        let bucket = &mut self.buckets[idx];
        let dropped = if bucket.len() >= self.max_len { bucket.pop_front() } else { None };
        bucket.push_back(request);
        dropped
    }

    fn push_front(&mut self, request: AnimationReloadRequest) -> Option<AnimationReloadRequest> {
        let idx = request.kind.index();
        let bucket = &mut self.buckets[idx];
        bucket.push_front(request);
        if bucket.len() > self.max_len {
            bucket.pop_back()
        } else {
            None
        }
    }

    fn pop_next(&mut self) -> Option<AnimationReloadRequest> {
        for _ in 0..self.buckets.len() {
            let idx = self.next_bucket % self.buckets.len();
            if let Some(request) = self.buckets[idx].pop_front() {
                self.next_bucket = (idx + 1) % self.buckets.len();
                return Some(request);
            }
            self.next_bucket = (idx + 1) % self.buckets.len();
        }
        None
    }
}

struct AnimationAssetReload {
    path: PathBuf,
    kind: AnimationAssetKind,
    bytes: Option<Vec<u8>>,
}

struct AnimationValidationJob {
    path: PathBuf,
    kind: AnimationAssetKind,
    bytes: Option<Vec<u8>>,
}

struct AnimationValidationResult {
    path: PathBuf,
    kind: AnimationAssetKind,
    events: Vec<AnimationValidationEvent>,
}

struct AnimationReloadWorker {
    senders: Vec<mpsc::SyncSender<AnimationReloadJob>>,
    next_sender: AtomicUsize,
    rx: mpsc::Receiver<AnimationReloadResult>,
}

impl AnimationReloadWorker {
    fn new() -> Option<Self> {
        let worker_count = thread::available_parallelism().map(|n| n.get().clamp(2, 4)).unwrap_or(2);
        let (result_tx, result_rx) = mpsc::channel();
        let mut senders = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (tx, rx) = mpsc::sync_channel(ANIMATION_RELOAD_WORKER_QUEUE_DEPTH);
            let thread_result_tx = result_tx.clone();
            let name = format!("animation-reload-{index}");
            if thread::Builder::new()
                .name(name)
                .spawn(move || {
                    while let Ok(job) = rx.recv() {
                        let result = run_animation_reload_job(job);
                        if thread_result_tx.send(result).is_err() {
                            break;
                        }
                    }
                })
                .is_err()
            {
                eprintln!("[animation] failed to spawn reload worker thread");
                return None;
            }
            senders.push(tx);
        }
        Some(Self { senders, next_sender: AtomicUsize::new(0), rx: result_rx })
    }

    fn submit(&self, job: AnimationReloadJob) -> Result<(), AnimationReloadJob> {
        if self.senders.is_empty() {
            return Err(job);
        }
        let len = self.senders.len();
        let mut job = job;
        let start = self.next_sender.fetch_add(1, AtomicOrdering::Relaxed) % len;
        for offset in 0..len {
            let idx = (start + offset) % len;
            match self.senders[idx].try_send(job) {
                Ok(()) => return Ok(()),
                Err(mpsc::TrySendError::Full(returned)) | Err(mpsc::TrySendError::Disconnected(returned)) => {
                    job = returned;
                }
            }
        }
        Err(job)
    }

    fn drain(&self) -> Vec<AnimationReloadResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            results.push(result);
        }
        results
    }
}

struct AnimationValidationWorker {
    tx: mpsc::Sender<AnimationValidationJob>,
    rx: mpsc::Receiver<AnimationValidationResult>,
}

impl AnimationValidationWorker {
    fn new() -> Option<Self> {
        let (tx, rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let builder = thread::Builder::new().name("animation-validation".to_string());
        match builder.spawn(move || {
            while let Ok(job) = rx.recv() {
                let result = run_animation_validation_job(job);
                if result_tx.send(result).is_err() {
                    break;
                }
            }
        }) {
            Ok(_) => Some(Self { tx, rx: result_rx }),
            Err(err) => {
                eprintln!("[animation] failed to spawn validation worker: {err:?}");
                None
            }
        }
    }

    fn submit(&self, job: AnimationValidationJob) -> Result<(), AnimationValidationJob> {
        self.tx.send(job).map_err(|err| err.0)
    }

    fn drain(&self) -> Vec<AnimationValidationResult> {
        let mut results = Vec::new();
        while let Ok(result) = self.rx.try_recv() {
            results.push(result);
        }
        results
    }
}

fn run_animation_validation_job(job: AnimationValidationJob) -> AnimationValidationResult {
    let AnimationValidationJob { path, kind, bytes } = job;
    let events = match kind {
        AnimationAssetKind::Clip => {
            if let Some(payload) = bytes.as_deref() {
                AnimationValidator::validate_clip_bytes(&path, payload)
            } else {
                AnimationValidator::validate_path(&path)
            }
        }
        AnimationAssetKind::Graph => {
            if let Some(payload) = bytes.as_deref() {
                AnimationValidator::validate_graph_bytes(&path, payload)
            } else {
                AnimationValidator::validate_path(&path)
            }
        }
        AnimationAssetKind::Skeletal => AnimationValidator::validate_path(&path),
    };
    AnimationValidationResult { path, kind, events }
}

fn run_animation_reload_job(job: AnimationReloadJob) -> AnimationReloadResult {
    let AnimationReloadJob { request } = job;
    let data = match request.kind {
        AnimationAssetKind::Clip => {
            let bytes = match fs::read(&request.path) {
                Ok(bytes) => bytes,
                Err(err) => return AnimationReloadResult { request, data: Err(err.into()) },
            };
            let label = request.path.to_string_lossy().to_string();
            match parse_animation_clip_bytes(&bytes, &request.key, &label) {
                Ok(clip) => Ok(AnimationReloadData::Clip { clip, bytes }),
                Err(err) => Err(err),
            }
        }
        AnimationAssetKind::Graph => {
            let bytes = match fs::read(&request.path) {
                Ok(bytes) => bytes,
                Err(err) => return AnimationReloadResult { request, data: Err(err.into()) },
            };
            let label = request.path.to_string_lossy().to_string();
            match parse_animation_graph_bytes(&bytes, &request.key, &label) {
                Ok(graph) => Ok(AnimationReloadData::Graph { graph, bytes }),
                Err(err) => Err(err),
            }
        }
        AnimationAssetKind::Skeletal => match skeletal::load_skeleton_from_gltf(&request.path) {
            Ok(import) => Ok(AnimationReloadData::Skeletal { import }),
            Err(err) => Err(err),
        },
    };
    AnimationReloadResult { request, data }
}
