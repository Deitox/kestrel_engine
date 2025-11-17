mod animation_keyframe_panel;
mod animation_watch;
mod atlas_watch;
mod editor_shell;
mod editor_ui;
mod gizmo_interaction;
mod plugin_host;
mod plugin_runtime;
mod runtime_loop;

use self::animation_keyframe_panel::{
    AnimationKeyframePanelState, AnimationPanelCommand, AnimationTrackBinding, AnimationTrackId, AnimationTrackKind,
    AnimationTrackSummary, KeyframeDetail, KeyframeId, KeyframeValue,
};
use self::animation_watch::{AnimationAssetKind, AnimationAssetWatcher};
use self::atlas_watch::{normalize_path_for_watch, AtlasHotReload};
use self::editor_shell::{EditorShell, EditorUiState, EditorUiStateParams, EmitterUiDefaults};
use self::plugin_host::{BuiltinPluginFactory, PluginHost};
use self::plugin_runtime::{PluginContextInputs, PluginRuntime};
use self::runtime_loop::{RuntimeLoop, RuntimeTick};
#[cfg(feature = "alloc_profiler")]
use crate::alloc_profiler;
use crate::analytics::{
    AnalyticsPlugin, AnimationBudgetSample, KeyframeEditorEventKind, KeyframeEditorTrackKind,
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
use crate::audio::{AudioHealthSnapshot, AudioPlugin};
use crate::camera::Camera2D;
use crate::camera3d::Camera3D;
use crate::config::{AppConfig, AppConfigOverrides, SpriteGuardrailMode};
use crate::ecs::{
    AnimationTime, ClipInstance, EcsWorld, EntityInfo, InstanceData, MeshLightingInfo, ParticleCaps,
    SkeletonInstance, SpriteAnimation, SpriteAnimationInfo, SpriteInstance,
};
use crate::environment::EnvironmentRegistry;
use crate::events::GameEvent;
use crate::gizmo::{GizmoInteraction, GizmoMode};
use crate::input::{Input, InputEvent};
use crate::material_registry::{MaterialGpu, MaterialRegistry};
use crate::mesh_preview::{MeshControlMode, MeshPreviewPlugin};
use crate::mesh_registry::MeshRegistry;
use crate::plugins::{ManifestBuiltinToggle, ManifestDynamicToggle, PluginContext, PluginManager};
use crate::prefab::{PrefabFormat, PrefabLibrary, PrefabStatusKind, PrefabStatusMessage};
use crate::renderer::{
    GpuPassTiming, MeshDraw, RenderViewport, Renderer, ScenePointLight, SpriteBatch, MAX_SHADOW_CASCADES,
};
use crate::scene::{
    EnvironmentDependency, Scene, SceneCamera2D, SceneCameraBookmark, SceneDependencies,
    SceneDependencyFingerprints, SceneEntityId, SceneEnvironment, SceneLightingData, SceneMetadata,
    ScenePointLightData, SceneShadowData, SceneViewportMode, Vec2Data,
};
use crate::scripts::{ScriptCommand, ScriptHandle, ScriptPlugin};
use crate::time::Time;
use bevy_ecs::prelude::Entity;
use glam::{Mat4, Vec2, Vec3, Vec4};

use anyhow::{anyhow, Context, Result};
use std::cell::{Ref, RefMut};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet, VecDeque};
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
use egui_plot as eplot;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{SpriteAnimationFrame, SpriteAnimationLoopMode, SpriteFrameHotData, TransformClipInfo};
    use glam::{Vec2, Vec4};
    use std::sync::Arc;

    #[test]
    fn sprite_key_details_capture_active_frame() {
        let animation = SpriteAnimationInfo {
            timeline: "walk".to_string(),
            playing: true,
            looped: true,
            loop_mode: "Loop".to_string(),
            speed: 1.0,
            frame_index: 1,
            frame_count: 3,
            frame_elapsed: 0.25,
            frame_duration: 0.5,
            frame_region: Some("walk_01".to_string()),
            frame_region_id: Some(42),
            frame_uv: Some([0.0, 0.0, 0.5, 0.5]),
            frame_events: vec!["footstep".to_string()],
            start_offset: 0.0,
            random_start: false,
            group: Some("default".to_string()),
        };
        let track_id = AnimationTrackId::for_entity_slot(Entity::from_raw(1), 0);
        let details = App::sprite_key_details(track_id, &animation, None);
        assert_eq!(details.len(), animation.frame_count);
        assert_eq!(details[1].time, Some(animation.frame_elapsed));
        assert_eq!(details[0].value_preview.as_deref(), Some("walk_01"));
    }

    #[test]
    fn transform_clip_details_reflect_channels() {
        let clip = TransformClipInfo {
            clip_key: "transform_clip".to_string(),
            playing: true,
            looped: false,
            speed: 1.0,
            time: 0.5,
            duration: 2.0,
            group: None,
            has_translation: true,
            has_rotation: true,
            has_scale: false,
            has_tint: true,
            sample_translation: Some(Vec2::new(1.0, 2.0)),
            sample_rotation: Some(45.0),
            sample_scale: None,
            sample_tint: Some(Vec4::new(0.1, 0.2, 0.3, 0.9)),
        };
        let track_id = AnimationTrackId::for_entity_slot(Entity::from_raw(1), 1);
        let details = App::transform_channel_details(
            track_id,
            clip.time,
            clip.sample_tint.map(|value| {
                format!("Tint ({:.2}, {:.2}, {:.2}, {:.2})", value.x, value.y, value.z, value.w)
            }),
        );
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].time, Some(clip.time));
        assert!(details[0].value_preview.as_ref().unwrap().contains("Tint"));
    }

    #[test]
    fn sprite_key_details_use_timeline_offsets() {
        let animation = SpriteAnimationInfo {
            timeline: "run".to_string(),
            playing: true,
            looped: true,
            loop_mode: "Loop".to_string(),
            speed: 1.0,
            frame_index: 1,
            frame_count: 2,
            frame_elapsed: 0.2,
            frame_duration: 0.4,
            frame_region: Some("run_01".to_string()),
            frame_region_id: Some(1),
            frame_uv: Some([0.0, 0.0, 1.0, 1.0]),
            frame_events: Vec::new(),
            start_offset: 0.0,
            random_start: false,
            group: None,
        };
        let frames = vec![
            SpriteAnimationFrame {
                name: Arc::from("run_00"),
                region: Arc::from("run_00"),
                region_id: 0,
                duration: 0.5,
                uv: [0.0; 4],
                events: Arc::from(Vec::new()),
            },
            SpriteAnimationFrame {
                name: Arc::from("run_01"),
                region: Arc::from("run_01"),
                region_id: 1,
                duration: 0.75,
                uv: [0.0; 4],
                events: Arc::from(Vec::new()),
            },
        ];
        let hot_frames = vec![
            SpriteFrameHotData { region_id: 0, uv: [0.0; 4] },
            SpriteFrameHotData { region_id: 1, uv: [0.0; 4] },
        ];
        let timeline = SpriteTimeline {
            name: Arc::from("run"),
            looped: true,
            loop_mode: SpriteAnimationLoopMode::Loop,
            frames: Arc::from(frames),
            hot_frames: Arc::from(hot_frames),
            durations: Arc::from(vec![0.5, 0.75].into_boxed_slice()),
            frame_offsets: Arc::from(vec![0.0, 0.5].into_boxed_slice()),
            total_duration: 1.25,
            total_duration_inv: 0.8,
        };
        let track_id = AnimationTrackId::for_entity_slot(Entity::from_raw(2), 0);
        let details = App::sprite_key_details(track_id, &animation, Some(&timeline));
        assert_eq!(details.len(), 2);
        assert_eq!(details[0].time, Some(0.0));
        assert_eq!(details[1].time, Some(0.5));
        assert!(details[1].value_preview.as_ref().unwrap().contains("0.75"));
    }
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

enum TrackEditOperation {
    Insert { time: f32, value: Option<KeyframeValue> },
    Delete { indices: Vec<usize> },
    Update { index: usize, new_time: Option<f32>, new_value: Option<KeyframeValue> },
    Adjust { indices: Vec<usize>, time_delta: Option<f32>, value_delta: Option<KeyframeValue> },
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

struct FrameProfiler {
    history: VecDeque<FrameTimingSample>,
    capacity: usize,
}

impl FrameProfiler {
    fn new(capacity: usize) -> Self {
        Self { history: VecDeque::with_capacity(capacity), capacity: capacity.max(1) }
    }

    fn push(&mut self, sample: FrameTimingSample) {
        if self.history.len() == self.capacity {
            self.history.pop_front();
        }
        self.history.push_back(sample);
    }

    fn latest(&self) -> Option<FrameTimingSample> {
        self.history.back().copied()
    }
}

#[derive(Default)]
struct TelemetryCache {
    mesh_keys: VersionedTelemetry<String>,
    environment_options: VersionedTelemetry<(String, String)>,
    prefab_entries: VersionedTelemetry<editor_ui::PrefabShelfEntry>,
}

impl TelemetryCache {
    fn mesh_keys(&mut self, registry: &MeshRegistry) -> Arc<[String]> {
        self.mesh_keys.get_or_update(registry.version(), || {
            let mut keys = registry.keys().map(|k| k.to_string()).collect::<Vec<_>>();
            keys.sort();
            keys
        })
    }

    fn environment_options(&mut self, registry: &EnvironmentRegistry) -> Arc<[(String, String)]> {
        self.environment_options.get_or_update(registry.version(), || {
            let mut options = registry
                .keys()
                .filter_map(|key| {
                    registry.definition(key).map(|definition| (key.clone(), definition.label().to_string()))
                })
                .collect::<Vec<_>>();
            options.sort_by(|a, b| a.1.cmp(&b.1));
            options
        })
    }

    fn prefab_entries(&mut self, library: &PrefabLibrary) -> Arc<[editor_ui::PrefabShelfEntry]> {
        self.prefab_entries.get_or_update(library.version(), || {
            library
                .entries()
                .iter()
                .map(|entry| {
                    let relative = entry
                        .path
                        .strip_prefix(library.root())
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| entry.path.display().to_string());
                    editor_ui::PrefabShelfEntry {
                        name: entry.name.clone(),
                        format: entry.format,
                        path_display: relative,
                    }
                })
                .collect()
        })
    }
}

struct VersionedTelemetry<T> {
    version: Option<u64>,
    data: Option<Arc<[T]>>,
}

impl<T> Default for VersionedTelemetry<T> {
    fn default() -> Self {
        Self { version: None, data: None }
    }
}

impl<T> VersionedTelemetry<T> {
    fn get_or_update<F>(&mut self, version: u64, rebuild: F) -> Arc<[T]>
    where
        F: FnOnce() -> Vec<T>,
    {
        if let (Some(current_version), Some(data)) = (&self.version, &self.data) {
            if *current_version == version {
                return Arc::clone(data);
            }
        }
        let values = rebuild();
        let arc: Arc<[T]> = Arc::from(values.into_boxed_slice());
        self.version = Some(version);
        self.data = Some(Arc::clone(&arc));
        arc
    }
}

#[derive(Clone, Copy, Default)]
struct FrameBudgetSnapshot {
    timing: Option<FrameTimingSample>,
    #[cfg(feature = "alloc_profiler")]
    alloc_delta: Option<alloc_profiler::AllocationDelta>,
}

#[derive(Clone)]
struct GpuTimingFrame {
    frame_index: u64,
    timings: Vec<GpuPassTiming>,
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

    // UI State
    scene_dependencies: Option<SceneDependencies>,
    scene_dependency_fingerprints: Option<SceneDependencyFingerprints>,

    // Plugins
    plugin_runtime: PluginRuntime,

    // Camera / selection
    pub(crate) camera: Camera2D,
    pub(crate) viewport_camera_mode: ViewportCameraMode,
    camera_bookmarks: Vec<CameraBookmark>,
    active_camera_bookmark: Option<String>,
    camera_follow_target: Option<SceneEntityId>,
    pub(crate) selected_entity: Option<Entity>,
    gizmo_mode: GizmoMode,
    gizmo_interaction: Option<GizmoInteraction>,

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
    id_lookup_input: String,
    id_lookup_active: bool,
    frame_profiler: FrameProfiler,
    #[cfg(feature = "alloc_profiler")]
    last_alloc_snapshot: alloc_profiler::AllocationSnapshot,
    telemetry_cache: TelemetryCache,
    frame_plot_points: Arc<[eplot::PlotPoint]>,
    frame_plot_revision: u64,
    gpu_timings: Arc<[GpuPassTiming]>,
    gpu_timing_history: VecDeque<GpuTimingFrame>,
    gpu_timing_history_capacity: usize,
    gpu_frame_counter: u64,

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
        self.editor_shell.ui_state.as_ref().expect("editor UI state not initialized").borrow()
    }

    fn editor_ui_state_mut(&self) -> RefMut<'_, EditorUiState> {
        self.editor_shell.ui_state.as_ref().expect("editor UI state not initialized").borrow_mut()
    }

    fn with_editor_ui_state_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut EditorUiState) -> R,
    {
        let mut state = self.editor_ui_state_mut();
        f(&mut state)
    }

    fn set_ui_scene_status(&self, message: impl Into<String>) {
        self.editor_ui_state_mut().ui_scene_status = Some(message.into());
    }

    fn set_inspector_status(&self, status: Option<String>) {
        self.editor_ui_state_mut().inspector_status = status;
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

    fn sync_atlas_hot_reload(&mut self) {
        let Some(watcher) = self.atlas_hot_reload.as_mut() else {
            return;
        };
        let mut desired = Vec::new();
        for (key, path) in self.assets.atlas_sources() {
            let path_buf = PathBuf::from(path);
            if let Some((original, normalized)) = normalize_path_for_watch(&path_buf) {
                desired.push((original, normalized, key));
            } else {
                eprintln!("[assets] skipping atlas '{key}' ΓÇô unable to resolve path for watching");
            }
        }
        if let Err(err) = watcher.sync(&desired) {
            eprintln!("[assets] failed to sync atlas hot-reload watchers: {err}");
        }
    }

    fn sync_animation_asset_watch_roots(&mut self) {
        let Some(watcher) = self.animation_asset_watcher.as_mut() else {
            self.animation_watch_roots_queue.clear();
            self.animation_watch_roots_pending.clear();
            self.animation_watch_roots_registered.clear();
            return;
        };
        while let Some((path, kind)) = self.animation_watch_roots_queue.pop() {
            let key = (path.clone(), kind);
            self.animation_watch_roots_pending.remove(&key);
            if !path.exists() {
                continue;
            }
            match watcher.watch_root(&path, kind) {
                Ok(()) => {
                    self.animation_watch_roots_registered.insert(key);
                }
                Err(err) => {
                    eprintln!(
                        "[animation] failed to watch {} directory {}: {err:?}",
                        kind.label(),
                        path.display()
                    );
                }
            }
        }
    }

    fn seed_animation_watch_roots(&mut self) {
        for (_, source) in self.assets.clip_sources() {
            self.queue_animation_watch_root(Path::new(&source), AnimationAssetKind::Clip);
        }
        for (_, source) in self.assets.skeleton_sources() {
            self.queue_animation_watch_root(Path::new(&source), AnimationAssetKind::Skeletal);
        }
        for (_, source) in self.assets.animation_graph_sources() {
            self.queue_animation_watch_root(Path::new(&source), AnimationAssetKind::Graph);
        }
    }

    fn queue_animation_watch_root(&mut self, path: &Path, kind: AnimationAssetKind) {
        let Some(root) = Self::watch_root_for_source(path) else {
            return;
        };
        if !root.exists() {
            return;
        }
        let normalized = Self::normalize_validation_path(&root);
        let key = (normalized, kind);
        if self.animation_watch_roots_registered.contains(&key)
            || self.animation_watch_roots_pending.contains(&key)
        {
            return;
        }
        self.animation_watch_roots_pending.insert(key.clone());
        self.animation_watch_roots_queue.push(key);
    }

    fn watch_root_for_source(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            Some(path.to_path_buf())
        } else if let Some(parent) = path.parent() {
            Some(parent.to_path_buf())
        } else {
            Some(path.to_path_buf())
        }
    }

    fn init_animation_asset_watcher() -> Option<AnimationAssetWatcher> {
        let mut watcher = match AnimationAssetWatcher::new() {
            Ok(watcher) => watcher,
            Err(err) => {
                eprintln!("[animation] asset watcher disabled: {err:?}");
                return None;
            }
        };
        let watch_roots = [
            ("assets/animations/clips", AnimationAssetKind::Clip),
            ("assets/animations/graphs", AnimationAssetKind::Graph),
            ("assets/animations/skeletal", AnimationAssetKind::Skeletal),
        ];
        for (root, kind) in watch_roots {
            let path = Path::new(root);
            if !path.exists() {
                continue;
            }
            if let Err(err) = watcher.watch_root(path, kind) {
                eprintln!("[animation] failed to watch {} ({}): {err:?}", path.display(), kind.label())
            }
        }
        Some(watcher)
    }

    fn drain_animation_validation_results(&mut self) {
        let Some(worker) = self.animation_validation_worker.as_ref() else {
            return;
        };
        for result in worker.drain() {
            self.handle_validation_events(result.kind.label(), result.path.as_path(), result.events);
        }
    }

    fn process_animation_asset_watchers(&mut self) {
        self.dispatch_animation_reload_queue();
        self.drain_animation_reload_results();
        self.drain_animation_validation_results();
        self.sync_animation_asset_watch_roots();
        let Some(watcher) = self.animation_asset_watcher.as_mut() else {
            return;
        };
        let changes = watcher.drain_changes();
        if changes.is_empty() {
            return;
        }
        let mut dedup: HashSet<(PathBuf, AnimationAssetKind)> = HashSet::new();
        for change in changes {
            let normalized = Self::normalize_validation_path(&change.path);
            if !dedup.insert((normalized.clone(), change.kind)) {
                continue;
            }
            if let Some(mut request) = self.prepare_animation_reload_request(normalized, change.kind) {
                request.skip_validation = self.consume_validation_suppression(&request.path);
                self.enqueue_animation_reload(request);
            }
        }
        self.dispatch_animation_reload_queue();
        self.drain_animation_reload_results();
        self.drain_animation_validation_results();
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
            self.with_editor_ui_state_mut(|state| state.pending_animation_validation_events.push(event.clone()));
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
            self.with_editor_ui_state_mut(|state| state.pending_animation_validation_events.push(event.clone()));
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

    fn show_animation_keyframe_panel(&mut self, ctx: &egui::Context, animation_time: &AnimationTime) {
        let panel_open = {
            let state = self.editor_ui_state();
            state.animation_keyframe_panel.is_open()
        };
        if !panel_open {
            return;
        }
        let panel_state = {
            let state = self.editor_ui_state();
            AnimationKeyframePanelState {
                animation_time,
                selected_entity: self.selected_entity,
                track_summaries: self.collect_animation_track_summaries(),
                can_undo: !state.clip_edit_history.is_empty(),
                can_redo: !state.clip_edit_redo.is_empty(),
                status_message: state.animation_clip_status.clone(),
            }
        };
        self.with_editor_ui_state_mut(|state| {
            state.animation_keyframe_panel.render_window(ctx, panel_state);
        });
        self.process_animation_panel_commands();
    }

    fn collect_animation_track_summaries(&self) -> Vec<AnimationTrackSummary> {
        let mut summaries = Vec::new();
        if let Some(entity) = self.selected_entity {
            if let Some(info) = self.ecs.entity_info(entity) {
                let mut slot_index = 0_u32;
                self.collect_sprite_track_summaries(entity, &info, &mut slot_index, &mut summaries);
                self.collect_transform_clip_summaries(entity, &info, &mut slot_index, &mut summaries);
            }
        }
        summaries
    }

    fn process_animation_panel_commands(&mut self) {
        let commands = self.with_editor_ui_state_mut(|state| state.animation_keyframe_panel.drain_commands());
        for command in commands {
            match command {
                AnimationPanelCommand::ScrubTrack { binding, time } => {
                    let track_kind = Self::analytics_track_kind(&binding);
                    self.handle_scrub_command(binding, time);
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::Scrub { track: track_kind });
                }
                AnimationPanelCommand::InsertKey { binding, time, value } => {
                    let track_kind = Self::analytics_track_kind(&binding);
                    self.apply_track_edit(binding, TrackEditOperation::Insert { time, value });
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::InsertKey { track: track_kind });
                }
                AnimationPanelCommand::DeleteKeys { binding, indices } => {
                    if !indices.is_empty() {
                        let track_kind = Self::analytics_track_kind(&binding);
                        let count = indices.len();
                        self.apply_track_edit(binding, TrackEditOperation::Delete { indices });
                        self.log_keyframe_editor_event(KeyframeEditorEventKind::DeleteKeys {
                            track: track_kind,
                            count,
                        });
                    }
                }
                AnimationPanelCommand::UpdateKey { binding, index, new_time, new_value } => {
                    let track_kind = Self::analytics_track_kind(&binding);
                    let changed_time = new_time.is_some();
                    let changed_value = new_value.is_some();
                    self.apply_track_edit(binding, TrackEditOperation::Update { index, new_time, new_value });
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::UpdateKey {
                        track: track_kind,
                        changed_time,
                        changed_value,
                    });
                }
                AnimationPanelCommand::AdjustKeys { binding, indices, time_delta, value_delta } => {
                    if !indices.is_empty() {
                        let track_kind = Self::analytics_track_kind(&binding);
                        let count = indices.len();
                        let time_changed = time_delta.is_some();
                        let value_changed = value_delta.is_some();
                        self.apply_track_edit(
                            binding,
                            TrackEditOperation::Adjust { indices, time_delta, value_delta },
                        );
                        self.log_keyframe_editor_event(KeyframeEditorEventKind::AdjustKeys {
                            track: track_kind,
                            count,
                            time_delta: time_changed,
                            value_delta: value_changed,
                        });
                    }
                }
                AnimationPanelCommand::Undo => {
                    self.undo_clip_edit();
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::Undo);
                }
                AnimationPanelCommand::Redo => {
                    self.redo_clip_edit();
                    self.log_keyframe_editor_event(KeyframeEditorEventKind::Redo);
                }
            }
        }
    }

    fn analytics_track_kind(binding: &AnimationTrackBinding) -> KeyframeEditorTrackKind {
        match binding {
            AnimationTrackBinding::SpriteTimeline { .. } => KeyframeEditorTrackKind::SpriteTimeline,
            AnimationTrackBinding::TransformChannel { channel, .. } => {
                Self::analytics_track_kind_from_channel(*channel)
            }
        }
    }

    fn analytics_track_kind_from_channel(channel: AnimationTrackKind) -> KeyframeEditorTrackKind {
        match channel {
            AnimationTrackKind::SpriteTimeline => KeyframeEditorTrackKind::SpriteTimeline,
            AnimationTrackKind::Translation => KeyframeEditorTrackKind::Translation,
            AnimationTrackKind::Rotation => KeyframeEditorTrackKind::Rotation,
            AnimationTrackKind::Scale => KeyframeEditorTrackKind::Scale,
            AnimationTrackKind::Tint => KeyframeEditorTrackKind::Tint,
        }
    }

    fn log_keyframe_editor_event(&mut self, event: KeyframeEditorEventKind) {
        if let Some(analytics) = self.analytics_plugin_mut() {
            analytics.record_keyframe_editor_event(event);
        }
    }

    fn collect_sprite_track_summaries(
        &self,
        entity: Entity,
        info: &EntityInfo,
        slot_index: &mut u32,
        summaries: &mut Vec<AnimationTrackSummary>,
    ) {
        if let Some(sprite) = info.sprite.as_ref() {
            if let Some(animation) = sprite.animation.as_ref() {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let timeline = self.assets.atlas_timeline(sprite.atlas.as_str(), animation.timeline.as_str());
                let duration = timeline
                    .map(|timeline| timeline.total_duration)
                    .unwrap_or(animation.frame_duration * animation.frame_count as f32);
                let key_count =
                    timeline.map(|timeline| timeline.frames.len()).unwrap_or(animation.frame_count);
                let playhead = timeline
                    .and_then(|timeline| {
                        if timeline.frames.is_empty() {
                            None
                        } else {
                            let clamped_index =
                                animation.frame_index.min(timeline.frames.len().saturating_sub(1));
                            let offset = timeline.frame_offsets.get(clamped_index).copied().unwrap_or(0.0);
                            Some((offset + animation.frame_elapsed).min(timeline.total_duration))
                        }
                    })
                    .or(Some(animation.frame_elapsed));
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Sprite Timeline ({})", animation.timeline),
                    kind: AnimationTrackKind::SpriteTimeline,
                    binding: AnimationTrackBinding::SpriteTimeline { entity },
                    duration,
                    key_count,
                    interpolation: None,
                    playhead,
                    dirty: false,
                    key_details: Self::sprite_key_details(track_id, animation, timeline),
                });
            }
        }
    }

    fn collect_transform_clip_summaries(
        &self,
        entity: Entity,
        info: &EntityInfo,
        slot_index: &mut u32,
        summaries: &mut Vec<AnimationTrackSummary>,
    ) {
        if let Some(clip) = info.transform_clip.as_ref() {
            let clip_asset = self.clip_resource(&clip.clip_key);
            let clip_dirty = {
                let state = self.editor_ui_state();
                state.clip_dirty.contains(&clip.clip_key)
            };
            if clip.has_translation {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.translation.as_ref())
                {
                    let details = Self::vec2_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_translation
                            .map(|value| format!("Translation ({:.2}, {:.2})", value.x, value.y)),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Translation ({})", clip.clip_key),
                    kind: AnimationTrackKind::Translation,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Translation,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
            if clip.has_rotation {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.rotation.as_ref())
                {
                    let details = Self::scalar_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_rotation.map(|value| format!("Rotation {:.2}", value)),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Rotation ({})", clip.clip_key),
                    kind: AnimationTrackKind::Rotation,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Rotation,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
            if clip.has_scale {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.scale.as_ref())
                {
                    let details = Self::vec2_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_scale.map(|value| format!("Scale ({:.2}, {:.2})", value.x, value.y)),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Scale ({})", clip.clip_key),
                    kind: AnimationTrackKind::Scale,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Scale,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
            if clip.has_tint {
                let track_id = AnimationTrackId::for_entity_slot(entity, *slot_index);
                *slot_index += 1;
                let (key_details, key_count, interpolation, duration) = if let Some(track_data) =
                    clip_asset.as_ref().and_then(|clip_asset| clip_asset.tint.as_ref())
                {
                    let details = Self::vec4_track_details(track_id, track_data);
                    (details, track_data.keyframes.len(), Some(track_data.interpolation), track_data.duration)
                } else {
                    let details = Self::transform_channel_details(
                        track_id,
                        clip.time,
                        clip.sample_tint.map(|value| {
                            format!("Tint ({:.2}, {:.2}, {:.2}, {:.2})", value.x, value.y, value.z, value.w)
                        }),
                    );
                    let detail_count = details.len();
                    (details, detail_count, None, clip.duration)
                };
                summaries.push(AnimationTrackSummary {
                    id: track_id,
                    label: format!("Tint ({})", clip.clip_key),
                    kind: AnimationTrackKind::Tint,
                    binding: AnimationTrackBinding::TransformChannel {
                        entity,
                        channel: AnimationTrackKind::Tint,
                    },
                    duration,
                    key_count,
                    interpolation,
                    playhead: Some(clip.time),
                    dirty: clip_dirty,
                    key_details,
                });
            }
        }
    }

    fn handle_scrub_command(&mut self, binding: AnimationTrackBinding, time: f32) {
        match binding {
            AnimationTrackBinding::SpriteTimeline { entity } => self.scrub_sprite_track(entity, time),
            AnimationTrackBinding::TransformChannel { entity, .. } => {
                self.scrub_transform_track(entity, time)
            }
        }
    }

    fn scrub_sprite_track(&mut self, entity: Entity, time: f32) {
        let Some(info) = self.ecs.entity_info(entity) else {
            return;
        };
        let Some(sprite) = info.sprite.as_ref() else {
            return;
        };
        let Some(animation) = sprite.animation.as_ref() else {
            return;
        };
        let Some(timeline) = self.assets.atlas_timeline(&sprite.atlas, animation.timeline.as_str()) else {
            return;
        };
        if timeline.frames.is_empty() {
            return;
        }
        let duration = timeline.total_duration.max(0.0);
        let wrapped = if timeline.looped && duration > 0.0 {
            let mut t = time % duration;
            if t < 0.0 {
                t += duration;
            }
            t
        } else {
            time.clamp(0.0, duration)
        };
        let mut target_index = timeline.frames.len() - 1;
        for (index, offset) in timeline.frame_offsets.iter().enumerate() {
            let span = timeline.durations.get(index).copied().unwrap_or(0.0).max(std::f32::EPSILON);
            if wrapped <= offset + span || index == timeline.frames.len() - 1 {
                target_index = index;
                break;
            }
        }
        let _ = self.ecs.set_sprite_animation_playing(entity, false);
        let _ = self.ecs.seek_sprite_animation_frame(entity, target_index);
    }

    fn scrub_transform_track(&mut self, entity: Entity, time: f32) {
        let Some(info) = self.ecs.entity_info(entity) else {
            return;
        };
        let Some(clip) = info.transform_clip.as_ref() else {
            return;
        };
        let clamped = time.clamp(0.0, clip.duration.max(0.0));
        let _ = self.ecs.set_transform_clip_playing(entity, false);
        let _ = self.ecs.set_transform_clip_time(entity, clamped);
    }

    fn apply_track_edit(&mut self, binding: AnimationTrackBinding, edit: TrackEditOperation) {
        match binding {
            AnimationTrackBinding::TransformChannel { entity, channel } => {
                self.edit_transform_channel(entity, channel, edit)
            }
            AnimationTrackBinding::SpriteTimeline { .. } => {}
        }
    }

    fn edit_transform_channel(
        &mut self,
        entity: Entity,
        channel: AnimationTrackKind,
        edit: TrackEditOperation,
    ) {
        let Some(info) = self.ecs.entity_info(entity) else {
            return;
        };
        let Some(clip_info) = info.transform_clip.as_ref() else {
            return;
        };
        let Some(source_clip) = self.clip_resource(&clip_info.clip_key) else {
            return;
        };
        let before_arc = Arc::clone(&source_clip);
        let mut clip = (*source_clip).clone();
        let mut dirty = false;
        match channel {
            AnimationTrackKind::Translation => {
                dirty = self.edit_vec2_track(
                    &mut clip.translation,
                    edit,
                    clip_info.sample_translation.or(Some(info.translation)),
                    Vec2::ZERO,
                );
            }
            AnimationTrackKind::Rotation => {
                dirty = self.edit_scalar_track(
                    &mut clip.rotation,
                    edit,
                    clip_info.sample_rotation.or(Some(info.rotation)),
                    0.0,
                );
            }
            AnimationTrackKind::Scale => {
                dirty = self.edit_vec2_track(
                    &mut clip.scale,
                    edit,
                    clip_info.sample_scale.or(Some(info.scale)),
                    Vec2::ONE,
                );
            }
            AnimationTrackKind::Tint => {
                dirty = self.edit_vec4_track(
                    &mut clip.tint,
                    edit,
                    clip_info.sample_tint.or(info.tint),
                    Vec4::ONE,
                );
            }
            AnimationTrackKind::SpriteTimeline => {}
        }
        if !dirty {
            return;
        }
        self.recompute_clip_duration(&mut clip);
        let clip_arc = Arc::new(clip);
        self.with_editor_ui_state_mut(|state| {
            state.clip_edit_overrides.insert(clip_info.clip_key.clone(), Arc::clone(&clip_arc));
        });
        self.apply_clip_override_to_instances(&clip_info.clip_key, Arc::clone(&clip_arc));
        self.record_clip_edit(&clip_info.clip_key, before_arc, Arc::clone(&clip_arc));
        self.persist_clip_edit(&clip_info.clip_key, clip_arc);
    }

    fn sprite_key_details(
        track_id: AnimationTrackId,
        animation: &SpriteAnimationInfo,
        timeline: Option<&SpriteTimeline>,
    ) -> Vec<KeyframeDetail> {
        if let Some(timeline) = timeline {
            timeline
                .frames
                .iter()
                .enumerate()
                .map(|(index, frame)| {
                    let time = timeline.frame_offsets.get(index).copied();
                    let duration = timeline.durations.get(index).copied().unwrap_or(0.0);
                    let mut preview = frame.name.as_ref().to_string();
                    if duration > 0.0 {
                        preview = format!("{preview} ({duration:.2}s)");
                    }
                    if !frame.events.is_empty() {
                        let events: Vec<String> =
                            frame.events.iter().map(|event| event.as_ref().to_string()).collect();
                        preview = format!("{preview} [{}]", events.join(", "));
                    }
                    KeyframeDetail {
                        id: KeyframeId::new(track_id, index),
                        index,
                        time,
                        value_preview: Some(preview),
                        value: KeyframeValue::None,
                    }
                })
                .collect()
        } else {
            (0..animation.frame_count)
                .map(|index| KeyframeDetail {
                    id: KeyframeId::new(track_id, index),
                    index,
                    time: if index == animation.frame_index { Some(animation.frame_elapsed) } else { None },
                    value_preview: animation.frame_region.clone(),
                    value: KeyframeValue::None,
                })
                .collect()
        }
    }

    fn vec2_track_details(track_id: AnimationTrackId, track: &ClipVec2Track) -> Vec<KeyframeDetail> {
        track
            .keyframes
            .iter()
            .enumerate()
            .map(|(index, keyframe)| KeyframeDetail {
                id: KeyframeId::new(track_id, index),
                index,
                time: Some(keyframe.time),
                value_preview: Some(format!("({:.2}, {:.2})", keyframe.value.x, keyframe.value.y)),
                value: KeyframeValue::Vec2([keyframe.value.x, keyframe.value.y]),
            })
            .collect()
    }

    fn scalar_track_details(track_id: AnimationTrackId, track: &ClipScalarTrack) -> Vec<KeyframeDetail> {
        track
            .keyframes
            .iter()
            .enumerate()
            .map(|(index, keyframe)| KeyframeDetail {
                id: KeyframeId::new(track_id, index),
                index,
                time: Some(keyframe.time),
                value_preview: Some(format!("{:.2}", keyframe.value)),
                value: KeyframeValue::Scalar(keyframe.value),
            })
            .collect()
    }

    fn vec4_track_details(track_id: AnimationTrackId, track: &ClipVec4Track) -> Vec<KeyframeDetail> {
        track
            .keyframes
            .iter()
            .enumerate()
            .map(|(index, keyframe)| {
                let value = keyframe.value;
                KeyframeDetail {
                    id: KeyframeId::new(track_id, index),
                    index,
                    time: Some(keyframe.time),
                    value_preview: Some(format!(
                        "({:.2}, {:.2}, {:.2}, {:.2})",
                        value.x, value.y, value.z, value.w
                    )),
                    value: KeyframeValue::Vec4([value.x, value.y, value.z, value.w]),
                }
            })
            .collect()
    }

    fn transform_channel_details(
        track_id: AnimationTrackId,
        time: f32,
        value: Option<String>,
    ) -> Vec<KeyframeDetail> {
        value
            .map(|preview| {
                vec![KeyframeDetail {
                    id: KeyframeId::new(track_id, 0),
                    index: 0,
                    time: Some(time),
                    value_preview: Some(preview),
                    value: KeyframeValue::None,
                }]
            })
            .unwrap_or_else(Vec::new)
    }

    fn clip_resource(&self, key: &str) -> Option<Arc<AnimationClip>> {
        if let Some(override_clip) = {
            let state = self.editor_ui_state();
            state.clip_edit_overrides.get(key).cloned()
        } {
            return Some(override_clip);
        }
        self.assets.clip(key).map(|clip| Arc::new(clip.clone()))
    }

    fn apply_clip_override_to_instances(&mut self, clip_key: &str, clip: Arc<AnimationClip>) {
        let clip_key_arc: Arc<str> = Arc::from(clip_key.to_string());
        let mut query = self.ecs.world.query::<&mut ClipInstance>();
        for mut instance in query.iter_mut(&mut self.ecs.world) {
            if instance.clip_key.as_ref() == clip_key {
                instance.replace_clip(Arc::clone(&clip_key_arc), Arc::clone(&clip));
            }
        }
    }

    fn record_clip_edit(&mut self, clip_key: &str, before: Arc<AnimationClip>, after: Arc<AnimationClip>) {
        self.with_editor_ui_state_mut(|state| {
            state.clip_edit_history.push(ClipEditRecord { clip_key: clip_key.to_string(), before, after });
            state.clip_edit_redo.clear();
        });
    }

    fn undo_clip_edit(&mut self) {
        if let Some(record) = self.with_editor_ui_state_mut(|state| state.clip_edit_history.pop()) {
            self.with_editor_ui_state_mut(|state| state.clip_edit_redo.push(record.clone()));
            self.apply_clip_history_state(&record.clip_key, Arc::clone(&record.before));
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status = Some(format!("Undid edit on '{}'", record.clip_key));
            });
        }
    }

    fn redo_clip_edit(&mut self) {
        if let Some(record) = self.with_editor_ui_state_mut(|state| state.clip_edit_redo.pop()) {
            let clip_key = record.clip_key.clone();
            self.with_editor_ui_state_mut(|state| state.clip_edit_history.push(record.clone()));
            self.apply_clip_history_state(&clip_key, Arc::clone(&record.after));
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status = Some(format!("Redid edit on '{}'", clip_key));
            });
        }
    }

    fn apply_clip_history_state(&mut self, clip_key: &str, clip: Arc<AnimationClip>) {
        self.with_editor_ui_state_mut(|state| {
            state.clip_edit_overrides.insert(clip_key.to_string(), Arc::clone(&clip));
        });
        self.apply_clip_override_to_instances(clip_key, Arc::clone(&clip));
        self.persist_clip_edit(clip_key, clip);
    }

    fn persist_clip_edit(&mut self, clip_key: &str, clip: Arc<AnimationClip>) {
        self.with_editor_ui_state_mut(|state| {
            state.clip_dirty.insert(clip_key.to_string());
        });
        let clip_source_path = self.assets.clip_source(clip_key).map(|p| p.to_string());
        if let Some(path) = clip_source_path.as_deref() {
            self.suppress_validation_for_path(Path::new(path));
        }
        if let Err(err) = self.assets.save_clip(clip_key, clip.as_ref()) {
            eprintln!("[animation] failed to save clip '{clip_key}': {err:?}");
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status = Some(format!("Failed to save '{clip_key}': {err}"));
            });
            return;
        }
        let mut status_note = format!("Saved clip '{clip_key}'");
        if let Some(path) = clip_source_path.as_deref() {
            if let Err(err) = self.assets.load_clip(clip_key, path) {
                eprintln!("[animation] failed to reload clip '{clip_key}' after save: {err:?}");
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status = Some(format!("Reload failed for '{clip_key}': {err}"));
                });
                return;
            }
        } else {
            status_note = format!("Saved clip '{clip_key}' (no source metadata available)");
        }
        if let Some(updated) = self.assets.clip(clip_key) {
            let canonical = Arc::new(updated.clone());
            self.apply_clip_override_to_instances(clip_key, Arc::clone(&canonical));
            self.with_editor_ui_state_mut(|state| {
                state.clip_edit_overrides.remove(clip_key);
            });
        }
        self.with_editor_ui_state_mut(|state| {
            state.clip_dirty.remove(clip_key);
        });
        if let Some(path) = clip_source_path {
            let path_buf = PathBuf::from(&path);
            let events = AnimationValidator::validate_path(path_buf.as_path());
            self.handle_validation_events("clip edit", path_buf.as_path(), events);
        }
        self.with_editor_ui_state_mut(|state| {
            state.animation_clip_status = Some(status_note);
        });
    }

    fn edit_vec2_track(
        &self,
        target: &mut Option<ClipVec2Track>,
        edit: TrackEditOperation,
        sample: Option<Vec2>,
        fallback: Vec2,
    ) -> bool {
        let interpolation =
            target.as_ref().map(|track| track.interpolation).unwrap_or(ClipInterpolation::Linear);
        let mut frames: Vec<ClipKeyframe<Vec2>> =
            target.as_ref().map(|track| track.keyframes.iter().copied().collect()).unwrap_or_else(Vec::new);
        match edit {
            TrackEditOperation::Insert { time, value } => {
                let insert_value = value
                    .and_then(|v| v.as_vec2())
                    .map(|arr| Vec2::new(arr[0], arr[1]))
                    .or(sample)
                    .unwrap_or(fallback);
                frames.push(ClipKeyframe { time, value: insert_value });
            }
            TrackEditOperation::Delete { indices } => Self::remove_key_indices(&mut frames, &indices),
            TrackEditOperation::Update { index, new_time, new_value } => {
                if frames.is_empty() || index >= frames.len() {
                    return false;
                }
                let mut changed = false;
                if let Some(time) = new_time {
                    let clamped = time.max(0.0);
                    if (frames[index].time - clamped).abs() > f32::EPSILON {
                        frames[index].time = clamped;
                        changed = true;
                    }
                }
                if let Some(KeyframeValue::Vec2(value)) = new_value {
                    let new_vec = Vec2::new(value[0], value[1]);
                    if frames[index].value != new_vec {
                        frames[index].value = new_vec;
                        changed = true;
                    }
                }
                if !changed {
                    return false;
                }
            }
            TrackEditOperation::Adjust { indices, time_delta, value_delta } => {
                if frames.is_empty() {
                    return false;
                }
                let mut changed = false;
                for index in indices {
                    if index >= frames.len() {
                        continue;
                    }
                    if let Some(delta) = time_delta {
                        let clamped = (frames[index].time + delta).max(0.0);
                        if (frames[index].time - clamped).abs() > f32::EPSILON {
                            frames[index].time = clamped;
                            changed = true;
                        }
                    }
                    if let Some(KeyframeValue::Vec2(offset)) = value_delta {
                        let offset_vec = Vec2::new(offset[0], offset[1]);
                        let new_value = frames[index].value + offset_vec;
                        if frames[index].value != new_value {
                            frames[index].value = new_value;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    return false;
                }
            }
        }
        Self::apply_vec2_frames(target, frames, interpolation)
    }

    fn edit_scalar_track(
        &self,
        target: &mut Option<ClipScalarTrack>,
        edit: TrackEditOperation,
        sample: Option<f32>,
        fallback: f32,
    ) -> bool {
        let interpolation =
            target.as_ref().map(|track| track.interpolation).unwrap_or(ClipInterpolation::Linear);
        let mut frames: Vec<ClipKeyframe<f32>> =
            target.as_ref().map(|track| track.keyframes.iter().copied().collect()).unwrap_or_else(Vec::new);
        match edit {
            TrackEditOperation::Insert { time, value } => {
                let insert_value = value.and_then(|v| v.as_scalar()).or(sample).unwrap_or(fallback);
                frames.push(ClipKeyframe { time, value: insert_value });
            }
            TrackEditOperation::Delete { indices } => Self::remove_key_indices(&mut frames, &indices),
            TrackEditOperation::Update { index, new_time, new_value } => {
                if frames.is_empty() || index >= frames.len() {
                    return false;
                }
                let mut changed = false;
                if let Some(time) = new_time {
                    let clamped = time.max(0.0);
                    if (frames[index].time - clamped).abs() > f32::EPSILON {
                        frames[index].time = clamped;
                        changed = true;
                    }
                }
                if let Some(KeyframeValue::Scalar(value)) = new_value {
                    if (frames[index].value - value).abs() > f32::EPSILON {
                        frames[index].value = value;
                        changed = true;
                    }
                }
                if !changed {
                    return false;
                }
            }
            TrackEditOperation::Adjust { indices, time_delta, value_delta } => {
                if frames.is_empty() {
                    return false;
                }
                let mut changed = false;
                for index in indices {
                    if index >= frames.len() {
                        continue;
                    }
                    if let Some(delta) = time_delta {
                        let clamped = (frames[index].time + delta).max(0.0);
                        if (frames[index].time - clamped).abs() > f32::EPSILON {
                            frames[index].time = clamped;
                            changed = true;
                        }
                    }
                    if let Some(KeyframeValue::Scalar(offset)) = value_delta {
                        let new_value = frames[index].value + offset;
                        if (frames[index].value - new_value).abs() > f32::EPSILON {
                            frames[index].value = new_value;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    return false;
                }
            }
        }
        Self::apply_scalar_frames(target, frames, interpolation)
    }

    fn edit_vec4_track(
        &self,
        target: &mut Option<ClipVec4Track>,
        edit: TrackEditOperation,
        sample: Option<Vec4>,
        fallback: Vec4,
    ) -> bool {
        let interpolation =
            target.as_ref().map(|track| track.interpolation).unwrap_or(ClipInterpolation::Linear);
        let mut frames: Vec<ClipKeyframe<Vec4>> =
            target.as_ref().map(|track| track.keyframes.iter().copied().collect()).unwrap_or_else(Vec::new);
        match edit {
            TrackEditOperation::Insert { time, value } => {
                let insert_value = value
                    .and_then(|v| v.as_vec4())
                    .map(|arr| Vec4::new(arr[0], arr[1], arr[2], arr[3]))
                    .or(sample)
                    .unwrap_or(fallback);
                frames.push(ClipKeyframe { time, value: insert_value });
            }
            TrackEditOperation::Delete { indices } => Self::remove_key_indices(&mut frames, &indices),
            TrackEditOperation::Update { index, new_time, new_value } => {
                if frames.is_empty() || index >= frames.len() {
                    return false;
                }
                let mut changed = false;
                if let Some(time) = new_time {
                    let clamped = time.max(0.0);
                    if (frames[index].time - clamped).abs() > f32::EPSILON {
                        frames[index].time = clamped;
                        changed = true;
                    }
                }
                if let Some(KeyframeValue::Vec4(value)) = new_value {
                    let new_vec = Vec4::new(value[0], value[1], value[2], value[3]);
                    if frames[index].value != new_vec {
                        frames[index].value = new_vec;
                        changed = true;
                    }
                }
                if !changed {
                    return false;
                }
            }
            TrackEditOperation::Adjust { indices, time_delta, value_delta } => {
                if frames.is_empty() {
                    return false;
                }
                let mut changed = false;
                for index in indices {
                    if index >= frames.len() {
                        continue;
                    }
                    if let Some(delta) = time_delta {
                        let clamped = (frames[index].time + delta).max(0.0);
                        if (frames[index].time - clamped).abs() > f32::EPSILON {
                            frames[index].time = clamped;
                            changed = true;
                        }
                    }
                    if let Some(KeyframeValue::Vec4(offset)) = value_delta {
                        let offset_vec = Vec4::new(offset[0], offset[1], offset[2], offset[3]);
                        let new_value = frames[index].value + offset_vec;
                        if frames[index].value != new_value {
                            frames[index].value = new_value;
                            changed = true;
                        }
                    }
                }
                if !changed {
                    return false;
                }
            }
        }
        Self::apply_vec4_frames(target, frames, interpolation)
    }

    fn remove_key_indices<T>(frames: &mut Vec<ClipKeyframe<T>>, indices: &[usize]) {
        if frames.is_empty() || indices.is_empty() {
            return;
        }
        let mut sorted = indices.to_vec();
        sorted.sort_unstable_by(|a, b| b.cmp(a));
        for index in sorted {
            if index < frames.len() {
                frames.remove(index);
            }
        }
    }

    fn apply_vec2_frames(
        target: &mut Option<ClipVec2Track>,
        frames: Vec<ClipKeyframe<Vec2>>,
        interpolation: ClipInterpolation,
    ) -> bool {
        if frames.is_empty() {
            let had_track = target.is_some();
            *target = None;
            return had_track;
        }
        let normalized = Self::normalize_keyframes(frames);
        let track = Self::build_vec2_track_from_frames(interpolation, normalized);
        let changed = target
            .as_ref()
            .map(|existing| {
                existing.keyframes.len() != track.keyframes.len() || existing.duration != track.duration
            })
            .unwrap_or(true);
        *target = Some(track);
        changed
    }

    fn apply_scalar_frames(
        target: &mut Option<ClipScalarTrack>,
        frames: Vec<ClipKeyframe<f32>>,
        interpolation: ClipInterpolation,
    ) -> bool {
        if frames.is_empty() {
            let had_track = target.is_some();
            *target = None;
            return had_track;
        }
        let normalized = Self::normalize_keyframes(frames);
        let track = Self::build_scalar_track_from_frames(interpolation, normalized);
        let changed = target
            .as_ref()
            .map(|existing| {
                existing.keyframes.len() != track.keyframes.len() || existing.duration != track.duration
            })
            .unwrap_or(true);
        *target = Some(track);
        changed
    }

    fn apply_vec4_frames(
        target: &mut Option<ClipVec4Track>,
        frames: Vec<ClipKeyframe<Vec4>>,
        interpolation: ClipInterpolation,
    ) -> bool {
        if frames.is_empty() {
            let had_track = target.is_some();
            *target = None;
            return had_track;
        }
        let normalized = Self::normalize_keyframes(frames);
        let track = Self::build_vec4_track_from_frames(interpolation, normalized);
        let changed = target
            .as_ref()
            .map(|existing| {
                existing.keyframes.len() != track.keyframes.len() || existing.duration != track.duration
            })
            .unwrap_or(true);
        *target = Some(track);
        changed
    }

    fn normalize_keyframes<T: Copy>(mut frames: Vec<ClipKeyframe<T>>) -> Vec<ClipKeyframe<T>> {
        if frames.is_empty() {
            return frames;
        }
        frames.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(Ordering::Equal));
        let mut normalized: Vec<ClipKeyframe<T>> = Vec::with_capacity(frames.len());
        for mut frame in frames {
            frame.time = frame.time.max(0.0);
            if let Some(last) = normalized.last_mut() {
                if (last.time - frame.time).abs() < 1e-4 {
                    *last = frame;
                    continue;
                }
            }
            normalized.push(frame);
        }
        normalized
    }

    fn build_vec2_track_from_frames(
        interpolation: ClipInterpolation,
        frames: Vec<ClipKeyframe<Vec2>>,
    ) -> ClipVec2Track {
        let duration = frames.last().map(|frame| frame.time).unwrap_or(0.0);
        let duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
        let (segment_deltas, segments, segment_offsets) = Self::build_segment_cache_vec2(&frames);
        ClipVec2Track {
            interpolation,
            keyframes: Arc::from(frames.into_boxed_slice()),
            duration,
            duration_inv,
            segment_deltas,
            segments,
            segment_offsets,
        }
    }

    fn build_scalar_track_from_frames(
        interpolation: ClipInterpolation,
        frames: Vec<ClipKeyframe<f32>>,
    ) -> ClipScalarTrack {
        let duration = frames.last().map(|frame| frame.time).unwrap_or(0.0);
        let duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
        let (segment_deltas, segments, segment_offsets) = Self::build_segment_cache_scalar(&frames);
        ClipScalarTrack {
            interpolation,
            keyframes: Arc::from(frames.into_boxed_slice()),
            duration,
            duration_inv,
            segment_deltas,
            segments,
            segment_offsets,
        }
    }

    fn build_vec4_track_from_frames(
        interpolation: ClipInterpolation,
        frames: Vec<ClipKeyframe<Vec4>>,
    ) -> ClipVec4Track {
        let duration = frames.last().map(|frame| frame.time).unwrap_or(0.0);
        let duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
        let (segment_deltas, segments, segment_offsets) = Self::build_segment_cache_vec4(&frames);
        ClipVec4Track {
            interpolation,
            keyframes: Arc::from(frames.into_boxed_slice()),
            duration,
            duration_inv,
            segment_deltas,
            segments,
            segment_offsets,
        }
    }

    fn build_segment_cache_vec2(
        frames: &[ClipKeyframe<Vec2>],
    ) -> (Arc<[Vec2]>, Arc<[ClipSegment<Vec2>]>, Arc<[f32]>) {
        if frames.len() < 2 {
            return (Arc::from([]), Arc::from([]), Arc::from([]));
        }
        let mut deltas = Vec::with_capacity(frames.len() - 1);
        let mut segments = Vec::with_capacity(frames.len() - 1);
        let mut offsets = Vec::with_capacity(frames.len() - 1);
        for window in frames.windows(2) {
            let start = &window[0];
            let end = &window[1];
            let span = (end.time - start.time).max(std::f32::EPSILON);
            let inv_span = 1.0 / span;
            offsets.push(start.time);
            let delta = end.value - start.value;
            deltas.push(delta);
            segments.push(ClipSegment { slope: delta * inv_span, span, inv_span });
        }
        (
            Arc::from(deltas.into_boxed_slice()),
            Arc::from(segments.into_boxed_slice()),
            Arc::from(offsets.into_boxed_slice()),
        )
    }

    fn build_segment_cache_scalar(
        frames: &[ClipKeyframe<f32>],
    ) -> (Arc<[f32]>, Arc<[ClipSegment<f32>]>, Arc<[f32]>) {
        if frames.len() < 2 {
            return (Arc::from([]), Arc::from([]), Arc::from([]));
        }
        let mut deltas = Vec::with_capacity(frames.len() - 1);
        let mut segments = Vec::with_capacity(frames.len() - 1);
        let mut offsets = Vec::with_capacity(frames.len() - 1);
        for window in frames.windows(2) {
            let start = &window[0];
            let end = &window[1];
            let span = (end.time - start.time).max(std::f32::EPSILON);
            let inv_span = 1.0 / span;
            offsets.push(start.time);
            let delta = end.value - start.value;
            deltas.push(delta);
            segments.push(ClipSegment { slope: delta * inv_span, span, inv_span });
        }
        (
            Arc::from(deltas.into_boxed_slice()),
            Arc::from(segments.into_boxed_slice()),
            Arc::from(offsets.into_boxed_slice()),
        )
    }

    fn build_segment_cache_vec4(
        frames: &[ClipKeyframe<Vec4>],
    ) -> (Arc<[Vec4]>, Arc<[ClipSegment<Vec4>]>, Arc<[f32]>) {
        if frames.len() < 2 {
            return (Arc::from([]), Arc::from([]), Arc::from([]));
        }
        let mut deltas = Vec::with_capacity(frames.len() - 1);
        let mut segments = Vec::with_capacity(frames.len() - 1);
        let mut offsets = Vec::with_capacity(frames.len() - 1);
        for window in frames.windows(2) {
            let start = &window[0];
            let end = &window[1];
            let span = (end.time - start.time).max(std::f32::EPSILON);
            let inv_span = 1.0 / span;
            offsets.push(start.time);
            let delta = end.value - start.value;
            deltas.push(delta);
            segments.push(ClipSegment { slope: delta * inv_span, span, inv_span });
        }
        (
            Arc::from(deltas.into_boxed_slice()),
            Arc::from(segments.into_boxed_slice()),
            Arc::from(offsets.into_boxed_slice()),
        )
    }

    fn recompute_clip_duration(&self, clip: &mut AnimationClip) {
        let mut duration = 0.0_f32;
        if let Some(track) = clip.translation.as_ref() {
            duration = duration.max(track.duration);
        }
        if let Some(track) = clip.rotation.as_ref() {
            duration = duration.max(track.duration);
        }
        if let Some(track) = clip.scale.as_ref() {
            duration = duration.max(track.duration);
        }
        if let Some(track) = clip.tint.as_ref() {
            duration = duration.max(track.duration);
        }
        clip.duration = duration;
        clip.duration_inv = if duration > 0.0 { 1.0 / duration } else { 0.0 };
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
        if let Some(bookmark) = self.camera_bookmarks.iter().find(|b| b.name == name) {
            self.camera.position = bookmark.position;
            self.camera.set_zoom(bookmark.zoom);
            self.active_camera_bookmark = Some(bookmark.name.clone());
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
        if let Some(existing) = self.camera_bookmarks.iter_mut().find(|b| b.name == trimmed) {
            existing.position = self.camera.position;
            existing.zoom = self.camera.zoom;
        } else {
            self.camera_bookmarks.push(CameraBookmark {
                name: trimmed.to_string(),
                position: self.camera.position,
                zoom: self.camera.zoom,
            });
            self.camera_bookmarks.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        }
        self.active_camera_bookmark = Some(trimmed.to_string());
        self.camera_follow_target = None;
        true
    }

    fn delete_camera_bookmark(&mut self, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        let before = self.camera_bookmarks.len();
        self.camera_bookmarks.retain(|bookmark| bookmark.name != trimmed);
        if self.camera_bookmarks.len() != before {
            if self.active_camera_bookmark.as_deref() == Some(trimmed) {
                self.active_camera_bookmark = None;
            }
            true
        } else {
            false
        }
    }

    fn set_camera_follow_scene_id(&mut self, scene_id: SceneEntityId) -> bool {
        self.camera_follow_target = Some(scene_id);
        if self.refresh_camera_follow() {
            self.active_camera_bookmark = None;
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
        let scene_path = String::from("assets/scenes/quick_save.json");
        let mut scene_history = VecDeque::with_capacity(8);
        scene_history.push_back(scene_path.clone());
        let scene_path_for_ui = scene_path.clone();
        let scene_history_for_ui = scene_history.clone();
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
            editor_shell: EditorShell::new(),
            scene_dependencies: None,
            scene_dependency_fingerprints: None,
            plugin_runtime,
            camera,
            viewport_camera_mode: ViewportCameraMode::default(),
            camera_bookmarks: Vec::new(),
            active_camera_bookmark: None,
            camera_follow_target: None,
            selected_entity: None,
            gizmo_mode: GizmoMode::default(),
            gizmo_interaction: None,
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
            id_lookup_input: String::new(),
            id_lookup_active: false,
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
            frame_profiler: FrameProfiler::new(240),
            #[cfg(feature = "alloc_profiler")]
            last_alloc_snapshot: alloc_profiler::allocation_snapshot(),
            telemetry_cache: TelemetryCache::default(),
            frame_plot_points: Arc::from(Vec::<eplot::PlotPoint>::new().into_boxed_slice()),
            frame_plot_revision: 0,
            gpu_timings: Arc::from(Vec::<GpuPassTiming>::new().into_boxed_slice()),
            gpu_timing_history: VecDeque::with_capacity(240),
            gpu_timing_history_capacity: 240,
            gpu_frame_counter: 0,
            sprite_batch_map: HashMap::new(),
            sprite_batch_pool: Vec::new(),
            sprite_batch_order: Vec::new(),
        };
        let ui_state = EditorUiState::new(EditorUiStateParams {
            scene_path: scene_path_for_ui,
            scene_history: scene_history_for_ui,
            emitter_defaults,
            particle_config: particle_config.clone(),
            lighting_state: editor_lighting_state,
            environment_intensity,
            editor_config: editor_cfg.clone(),
        });
        app.editor_shell.install_ui_state(ui_state);
        app.seed_animation_watch_roots();
        app.sync_animation_asset_watch_roots();
        app.apply_particle_caps();
        app.apply_editor_camera_settings();
        app.report_audio_startup_status();
        app
    }

    fn record_events(&mut self) {
        let events = self.ecs.drain_events();
        if events.is_empty() {
            return;
        }
        self.with_plugins(|plugins, ctx| plugins.handle_events(ctx, &events));
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
                        self.active_camera_bookmark = None;
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

    fn set_prefab_status(&mut self, kind: PrefabStatusKind, message: impl Into<String>) {
        self.editor_ui_state_mut().prefab_status =
            Some(PrefabStatusMessage { kind, message: message.into() });
    }

    fn handle_save_prefab(&mut self, request: editor_ui::PrefabSaveRequest) {
        let trimmed = request.name.trim();
        if trimmed.is_empty() {
            self.set_prefab_status(PrefabStatusKind::Warning, "Prefab name cannot be empty.");
            return;
        }
        if request.format == PrefabFormat::Binary && !BINARY_PREFABS_ENABLED {
            self.set_prefab_status(
                PrefabStatusKind::Error,
                "Binary prefab format requires building with the 'binary_scene' feature.",
            );
            return;
        }
        if !self.ecs.entity_exists(request.entity) {
            self.set_prefab_status(PrefabStatusKind::Error, "Selected entity is no longer available.");
            return;
        }
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
                self.material_registry.material_source(key).map(|path| (key.to_string(), path.to_string()))
            })
            .collect();
        let Some(scene) = self.ecs.export_prefab_with_sources(
            request.entity,
            &self.assets,
            |key| mesh_source_map.get(key).cloned(),
            |key| material_source_map.get(key).cloned(),
        ) else {
            self.set_prefab_status(PrefabStatusKind::Error, "Failed to export selection to prefab.");
            return;
        };
        let path = self.prefab_library.path_for(trimmed, request.format);
        let existed = path.exists();
        let sanitized_name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(trimmed).to_string();
        match scene.save_to_path(&path) {
            Ok(()) => {
                self.editor_ui_state_mut().prefab_name_input = sanitized_name.clone();
                if let Err(err) = self.prefab_library.refresh() {
                    self.set_prefab_status(
                        PrefabStatusKind::Warning,
                        format!("Prefab '{}' saved but refresh failed: {err}", sanitized_name),
                    );
                } else {
                    self.set_prefab_status(
                        if existed { PrefabStatusKind::Info } else { PrefabStatusKind::Success },
                        if existed {
                            format!(
                                "Overwrote prefab '{}' ({})",
                                sanitized_name,
                                request.format.short_label()
                            )
                        } else {
                            format!("Saved prefab '{}' ({})", sanitized_name, request.format.short_label())
                        },
                    );
                }
            }
            Err(err) => {
                self.set_prefab_status(PrefabStatusKind::Error, format!("Saving prefab failed: {err}"));
            }
        }
    }

    fn handle_instantiate_prefab(&mut self, request: editor_ui::PrefabInstantiateRequest) {
        let entry_path = self
            .prefab_library
            .entries()
            .iter()
            .find(|entry| entry.name == request.name && entry.format == request.format)
            .map(|entry| entry.path.clone());
        let Some(path) = entry_path else {
            self.set_prefab_status(
                PrefabStatusKind::Error,
                format!("Prefab '{}' ({}) not found.", request.name, request.format.short_label()),
            );
            return;
        };
        let mut scene = match Scene::load_from_path(&path) {
            Ok(scene) => scene,
            Err(err) => {
                self.set_prefab_status(
                    PrefabStatusKind::Error,
                    format!("Failed to load prefab '{}': {err}", request.name),
                );
                return;
            }
        };
        if scene.entities.is_empty() {
            self.set_prefab_status(
                PrefabStatusKind::Warning,
                format!("Prefab '{}' contains no entities.", request.name),
            );
            return;
        }
        scene = scene.with_fresh_entity_ids();
        if let Some(target) = request.drop_target {
            match target {
                editor_ui::PrefabDropTarget::World2D(target_2d) => {
                    let current: Vec2 = scene.entities.first().unwrap().transform.translation.clone().into();
                    scene.offset_entities_2d(target_2d - current);
                }
                editor_ui::PrefabDropTarget::World3D(target_3d) => {
                    if let Some(root) = scene.entities.first() {
                        let current = root
                            .transform3d
                            .as_ref()
                            .map(|tx| Vec3::from(tx.translation.clone()))
                            .unwrap_or_else(|| {
                                let base: Vec2 = root.transform.translation.clone().into();
                                Vec3::new(base.x, base.y, 0.0)
                            });
                        scene.offset_entities_3d(target_3d - current);
                    }
                }
            }
        }
        match self.ecs.instantiate_prefab_with_mesh(&scene, &mut self.assets, |key, path| {
            self.mesh_registry.ensure_mesh(key, path, &mut self.material_registry)
        }) {
            Ok(spawned) => {
                if let Some(&root) = spawned.first() {
                    self.selected_entity = Some(root);
                }
                self.gizmo_interaction = None;
                self.set_prefab_status(
                    PrefabStatusKind::Success,
                    format!("Instantiated prefab '{}' ({})", request.name, request.format.short_label()),
                );
            }
            Err(err) => {
                self.set_prefab_status(PrefabStatusKind::Error, format!("Prefab instantiate failed: {err}"));
            }
        }
    }

    fn export_gpu_timings_csv<P: AsRef<std::path::Path>>(&self, path: P) -> Result<PathBuf> {
        if self.gpu_timing_history.is_empty() {
            return Err(anyhow!("No GPU timing samples available to export."));
        }
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Creating GPU timing export directory {}", parent.display()))?;
            }
        }
        let mut rows = String::from("frame,label,duration_ms\n");
        for frame in &self.gpu_timing_history {
            for timing in &frame.timings {
                rows.push_str(&format!("{},{},{:.4}\n", frame.frame_index, timing.label, timing.duration_ms));
            }
        }
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

    fn frame_plot_points_arc(&mut self) -> Arc<[eplot::PlotPoint]> {
        let revision = self.analytics_plugin().map(|plugin| plugin.frame_history_revision()).unwrap_or(0);
        if self.frame_plot_revision != revision {
            let new_arc = if let Some(plugin) = self.analytics_plugin() {
                let history = plugin.frame_history();
                let mut data = Vec::with_capacity(history.len());
                for (idx, value) in history.iter().enumerate() {
                    data.push(eplot::PlotPoint::new(idx as f64, *value as f64));
                }
                Arc::from(data.into_boxed_slice())
            } else {
                Arc::from(Vec::<eplot::PlotPoint>::new().into_boxed_slice())
            };
            self.frame_plot_revision = revision;
            self.frame_plot_points = Arc::clone(&new_arc);
            return new_arc;
        }
        Arc::clone(&self.frame_plot_points)
    }

    fn capture_frame_budget_snapshot(&self) -> FrameBudgetSnapshot {
        FrameBudgetSnapshot {
            timing: self.frame_profiler.latest(),
            #[cfg(feature = "alloc_profiler")]
            alloc_delta: self.analytics_plugin().and_then(|plugin| plugin.allocation_delta()),
        }
    }

    fn frame_budget_snapshot_view(snapshot: &FrameBudgetSnapshot) -> editor_ui::FrameBudgetSnapshotView {
        editor_ui::FrameBudgetSnapshotView {
            timing: snapshot.timing,
            #[cfg(feature = "alloc_profiler")]
            alloc_delta: snapshot.alloc_delta,
        }
    }

    fn frame_budget_delta_message(&self) -> Option<String> {
        let (baseline_snapshot, comparison_snapshot) = {
            let state = self.editor_ui_state();
            (state.frame_budget_idle_snapshot, state.frame_budget_panel_snapshot)
        };
        let baseline = baseline_snapshot?;
        let comparison = comparison_snapshot?;
        let idle = baseline.timing?;
        let panel = comparison.timing?;
        let update_delta = panel.update_ms - idle.update_ms;
        let ui_delta = panel.ui_ms - idle.ui_ms;
        #[cfg(feature = "alloc_profiler")]
        let alloc_note = if let (Some(idle_alloc), Some(panel_alloc)) =
            (baseline.alloc_delta, comparison.alloc_delta)
        {
            let diff = panel_alloc.net_bytes() - idle_alloc.net_bytes();
            format!(", delta_alloc={:+} B", diff)
        } else {
            String::new()
        };
        #[cfg(not(feature = "alloc_profiler"))]
        let alloc_note = String::new();
        Some(format!(
            "Frame budget delta: delta_update={:+.2} ms, delta_ui={:+.2} ms{alloc_note}",
            update_delta, ui_delta
        ))
    }

    fn handle_frame_budget_action(&mut self, action: Option<editor_ui::FrameBudgetAction>) {
        use editor_ui::FrameBudgetAction;
        let Some(action) = action else {
            return;
        };
        match action {
            FrameBudgetAction::CaptureIdle => {
                let snapshot = self.capture_frame_budget_snapshot();
                self.with_editor_ui_state_mut(|state| {
                    state.frame_budget_idle_snapshot = Some(snapshot);
                    state.frame_budget_status = Some(
                        "Idle baseline captured. Toggle panels, then capture the panel snapshot.".to_string(),
                    );
                });
            }
            FrameBudgetAction::CapturePanel => {
                let snapshot = self.capture_frame_budget_snapshot();
                self.with_editor_ui_state_mut(|state| {
                    state.frame_budget_panel_snapshot = Some(snapshot);
                });
                let status = self.frame_budget_delta_message().or_else(|| {
                    Some(
                        "Panel snapshot captured. Capture an idle baseline first for delta comparisons."
                            .to_string(),
                    )
                });
                self.with_editor_ui_state_mut(|state| state.frame_budget_status = status);
            }
            FrameBudgetAction::Clear => {
                self.with_editor_ui_state_mut(|state| {
                    state.frame_budget_idle_snapshot = None;
                    state.frame_budget_panel_snapshot = None;
                    state.frame_budget_status = Some("Cleared frame budget snapshots.".to_string());
                });
            }
        }
    }

    fn with_plugin_runtime<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut PluginHost, &mut PluginManager, &mut PluginContext<'_>) -> R,
    {
        let selected_entity = self.selected_entity;
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

    fn set_mesh_status<S: Into<String>>(&mut self, message: S) {
        if let Some(plugin) = self.mesh_preview_plugin_mut() {
            plugin.set_status(message);
        }
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

    fn remember_scene_path(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut state = self.editor_ui_state_mut();
        if let Some(pos) = state.scene_history.iter().position(|entry| entry == trimmed) {
            state.scene_history.remove(pos);
        }
        state.scene_history.push_front(trimmed.to_string());
        while state.scene_history.len() > 8 {
            state.scene_history.pop_back();
        }
        state.scene_history_snapshot = None;
    }

    fn push_script_console(&mut self, kind: ScriptConsoleKind, text: impl Into<String>) {
        let mut state = self.editor_ui_state_mut();
        state.script_console.push_back(ScriptConsoleEntry { kind, text: text.into() });
        while state.script_console.len() > SCRIPT_CONSOLE_CAPACITY {
            state.script_console.pop_front();
        }
        state.script_console_snapshot = None;
    }

    fn script_console_entries(&mut self) -> Arc<[ScriptConsoleEntry]> {
        let mut state = self.editor_ui_state_mut();
        if let Some(cache) = &state.script_console_snapshot {
            return Arc::clone(cache);
        }
        let data = state.script_console.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        state.script_console_snapshot = Some(Arc::clone(&arc));
        arc
    }

    fn script_repl_history_arc(&mut self) -> Arc<[String]> {
        let mut state = self.editor_ui_state_mut();
        if let Some(cache) = &state.script_repl_history_snapshot {
            return Arc::clone(cache);
        }
        let data = state.script_repl_history.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        state.script_repl_history_snapshot = Some(Arc::clone(&arc));
        arc
    }

    fn scene_history_arc(&mut self) -> Arc<[String]> {
        let mut state = self.editor_ui_state_mut();
        if let Some(cache) = &state.scene_history_snapshot {
            return Arc::clone(cache);
        }
        let data = state.scene_history.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        state.scene_history_snapshot = Some(Arc::clone(&arc));
        arc
    }

    fn scene_atlas_refs_arc(&mut self) -> Arc<[String]> {
        {
            let state = self.editor_ui_state();
            if let Some(cache) = &state.scene_atlas_snapshot {
                return Arc::clone(cache);
            }
        }
        let mut data = self.scene_atlas_refs.iter().cloned().collect::<Vec<_>>();
        data.sort();
        let arc = Arc::from(data.into_boxed_slice());
        self.editor_ui_state_mut().scene_atlas_snapshot = Some(Arc::clone(&arc));
        arc
    }

    fn scene_mesh_refs_arc(&mut self) -> Arc<[String]> {
        {
            let state = self.editor_ui_state();
            if let Some(cache) = &state.scene_mesh_snapshot {
                return Arc::clone(cache);
            }
        }
        let mut data = self.scene_mesh_refs.iter().cloned().collect::<Vec<_>>();
        data.sort();
        let arc = Arc::from(data.into_boxed_slice());
        self.editor_ui_state_mut().scene_mesh_snapshot = Some(Arc::clone(&arc));
        arc
    }

    fn scene_clip_refs_arc(&mut self) -> Arc<[String]> {
        {
            let state = self.editor_ui_state();
            if let Some(cache) = &state.scene_clip_snapshot {
                return Arc::clone(cache);
            }
        }
        let mut data = self.scene_clip_refs.keys().cloned().collect::<Vec<_>>();
        data.sort();
        let arc = Arc::from(data.into_boxed_slice());
        self.editor_ui_state_mut().scene_clip_snapshot = Some(Arc::clone(&arc));
        arc
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

    fn append_script_history(&mut self, command: &str) {
        if command.is_empty() {
            return;
        }
        let mut state = self.editor_ui_state_mut();
        state.script_repl_history.push_back(command.to_string());
        while state.script_repl_history.len() > SCRIPT_HISTORY_CAPACITY {
            state.script_repl_history.pop_front();
        }
        state.script_repl_history_index = None;
        state.script_repl_history_snapshot = None;
    }

    fn execute_repl_command(&mut self, command: String) {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return;
        }
        self.append_script_history(trimmed);
        self.push_script_console(ScriptConsoleKind::Input, format!("> {trimmed}"));
        {
            let mut state = self.editor_ui_state_mut();
            state.script_repl_input.clear();
            state.script_focus_repl = true;
        }
        let result: Result<Option<String>, String> = if let Some(plugin) = self.script_plugin_mut() {
            match plugin.eval_repl(trimmed) {
                Ok(value) => Ok(value),
                Err(err) => {
                    let message = err.to_string();
                    plugin.set_error_message(message.clone());
                    Err(message)
                }
            }
        } else {
            Err("Script plugin unavailable; cannot evaluate command.".to_string())
        };
        match result {
            Ok(Some(value)) => self.push_script_console(ScriptConsoleKind::Output, value),
            Ok(None) => {}
            Err(message) => {
                self.push_script_console(ScriptConsoleKind::Error, message);
                let mut state = self.editor_ui_state_mut();
                state.script_debugger_open = true;
                state.script_focus_repl = true;
            }
        }
    }

    fn sync_script_error_state(&mut self) {
        let current_error =
            self.script_plugin().and_then(|plugin| plugin.last_error().map(|err| err.to_string()));
        {
            let mut state = self.editor_ui_state_mut();
            if current_error == state.last_reported_script_error {
                return;
            }
            state.last_reported_script_error = current_error.clone();
        }
        if let Some(err) = current_error {
            self.push_script_console(ScriptConsoleKind::Error, format!("Runtime error: {err}"));
            let mut state = self.editor_ui_state_mut();
            state.script_debugger_open = true;
            state.script_focus_repl = true;
        }
    }

    fn focus_selection(&mut self) -> bool {
        let Some(entity) = self.selected_entity else {
            return false;
        };
        let Some(info) = self.ecs.entity_info(entity) else {
            return false;
        };
        self.camera_follow_target = None;
        self.active_camera_bookmark = None;
        self.camera.position = info.translation;
        if let Some(plugin) = self.mesh_preview_plugin_mut() {
            plugin.focus_selection_with_info(&info)
        } else {
            true
        }
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

    fn set_mesh_control_mode(&mut self, mode: MeshControlMode) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.set_mesh_control_mode(ctx, mode) {
                    eprintln!("[mesh_preview] set_mesh_control_mode failed: {err:?}");
                }
            }
        });
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

    fn set_frustum_lock(&mut self, enabled: bool) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.set_frustum_lock(ctx, enabled) {
                    eprintln!("[mesh_preview] set_frustum_lock failed: {err:?}");
                }
            }
        });
    }

    fn reset_mesh_camera(&mut self) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.reset_mesh_camera(ctx) {
                    eprintln!("[mesh_preview] reset_mesh_camera failed: {err:?}");
                }
            }
        });
    }

    fn set_preview_mesh(&mut self, new_key: String) {
        let scene_refs = self.scene_material_refs.clone();
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.set_preview_mesh(ctx, &scene_refs, new_key.clone()) {
                    eprintln!("[mesh_preview] set_preview_mesh failed: {err:?}");
                }
            }
        });
    }

    fn spawn_mesh_entity(&mut self, mesh_key: &str) {
        let key = mesh_key.to_string();
        let mut spawned = None;
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                match plugin.spawn_mesh_entity(ctx, &key) {
                    Ok(entity) => spawned = entity,
                    Err(err) => eprintln!("[mesh_preview] spawn_mesh_entity failed: {err:?}"),
                }
            }
        });
        if let Some(entity) = spawned {
            self.selected_entity = Some(entity);
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
        if self.scene_dependency_fingerprints == Some(fingerprint) {
            self.scene_dependencies = Some(deps.clone());
            return Ok(());
        }
        let atlas_dirty =
            self.scene_dependency_fingerprints.map_or(true, |fp| fp.atlases != fingerprint.atlases);
        let clip_dirty = self.scene_dependency_fingerprints.map_or(true, |fp| fp.clips != fingerprint.clips);
        let mesh_dirty =
            self.scene_dependency_fingerprints.map_or(true, |fp| fp.meshes != fingerprint.meshes);
        let material_dirty =
            self.scene_dependency_fingerprints.map_or(true, |fp| fp.materials != fingerprint.materials);
        let environment_dirty =
            self.scene_dependency_fingerprints.map_or(true, |fp| fp.environments != fingerprint.environments);

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

        self.scene_dependencies = Some(deps.clone());
        self.scene_dependency_fingerprints = Some(fingerprint);
        Ok(())
    }

    fn capture_scene_metadata(&self) -> SceneMetadata {
        let mut metadata = SceneMetadata::default();
        metadata.viewport = SceneViewportMode::from(self.viewport_camera_mode);
        metadata.camera2d =
            Some(SceneCamera2D { position: Vec2Data::from(self.camera.position), zoom: self.camera.zoom });
        metadata.camera_bookmarks = self.camera_bookmarks.iter().map(CameraBookmark::to_scene).collect();
        metadata.active_camera_bookmark =
            if self.camera_follow_target.is_none() { self.active_camera_bookmark.clone() } else { None };
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
        self.camera_bookmarks = metadata.camera_bookmarks.iter().map(CameraBookmark::from_scene).collect();
        self.camera_bookmarks.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        self.camera_follow_target = metadata.camera_follow_entity.clone();
        if self.camera_follow_target.is_some() && !self.refresh_camera_follow() {
            self.camera_follow_target = None;
        }
        if self.camera_follow_target.is_none() {
            if let Some(active) = metadata.active_camera_bookmark.as_deref() {
                if !self.apply_camera_bookmark_by_name(active) {
                    self.active_camera_bookmark = None;
                }
            } else {
                self.active_camera_bookmark = None;
            }
        } else {
            self.active_camera_bookmark = None;
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

        if let Some(entity) = self.selected_entity {
            if !self.ecs.entity_exists(entity) {
                self.selected_entity = None;
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
            self.ecs
                .spawn_burst(&self.assets, (ui_spawn_per_press * 5).max(1000) as usize);
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
                Self::log_allocation_delta(alloc_delta);
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
        let mut selected_info = self.selected_entity.and_then(|entity| self.ecs.entity_info(entity));
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
        let prev_selected_entity = self.selected_entity;
        let prev_gizmo_interaction = self.gizmo_interaction;

        if self.viewport_camera_mode == ViewportCameraMode::Ortho2D
            && mesh_control_mode == MeshControlMode::Disabled
        {
            if let Some(delta) = self.input.consume_wheel_delta() {
                self.camera.apply_scroll_zoom(delta);
                self.active_camera_bookmark = None;
            }

            if self.input.right_held() {
                let (dx, dy) = self.input.mouse_delta;
                if dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON {
                    self.camera.pan_screen_delta(Vec2::new(dx, dy), viewport_size);
                    self.active_camera_bookmark = None;
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
        let selection_changed = self.selected_entity != prev_selected_entity;
        let gizmo_changed = self.gizmo_interaction != prev_gizmo_interaction;
        selected_info = self.selected_entity.and_then(|entity| self.ecs.entity_info(entity));

        let (cell_size, use_quadtree, density_threshold) = {
            let state = self.editor_ui_state();
            (
                state.ui_cell_size.max(0.05),
                state.ui_spatial_use_quadtree,
                state.ui_spatial_density_threshold,
            )
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
            self.frame_profiler.push(FrameTimingSample {
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
        let sprite_upload_ms = self
            .gpu_timings
            .iter()
            .find(|timing| timing.label == "Sprite pass")
            .map(|timing| timing.duration_ms);
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
        let (audio_triggers, audio_enabled, audio_health) = if let Some(audio) = self.audio_plugin() {
            (audio.recent_triggers().cloned().collect(), audio.enabled(), audio.health_snapshot())
        } else {
            (Vec::new(), false, AudioHealthSnapshot::default())
        };
        let (script_plugin_available, script_path, scripts_enabled, scripts_paused, script_last_error) =
            if let Some(plugin) = self.script_plugin() {
                (
                    true,
                    Some(plugin.script_path().display().to_string()),
                    plugin.enabled(),
                    plugin.paused(),
                    plugin.last_error().map(|err| err.to_string()),
                )
            } else {
                (false, None, false, false, None)
            };
        let mesh_keys = self.telemetry_cache.mesh_keys(&self.mesh_registry);
        let scene_history_list = self.scene_history_arc();
        let atlas_snapshot = self.scene_atlas_refs_arc();
        let mesh_snapshot = self.scene_mesh_refs_arc();
        let clip_snapshot = self.scene_clip_refs_arc();
        let environment_options = self.telemetry_cache.environment_options(&self.environment_registry);
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
        let prefab_entries = self.telemetry_cache.prefab_entries(&self.prefab_library);
        let latest_frame_timing = self.frame_profiler.latest();
        let (frame_budget_idle, frame_budget_panel, frame_budget_status) = {
            let state = self.editor_ui_state();
            (
                state.frame_budget_idle_snapshot.as_ref().map(Self::frame_budget_snapshot_view),
                state.frame_budget_panel_snapshot.as_ref().map(Self::frame_budget_snapshot_view),
                state.frame_budget_status.clone(),
            )
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
            selected_entity: self.selected_entity,
            selection_details: selected_info.clone(),
            prev_selected_entity,
            prev_gizmo_interaction,
            selection_changed,
            gizmo_changed,
            cursor_screen,
            cursor_world_2d,
            cursor_ray,
            hovered_scale_kind,
            window_size,
            mesh_camera_for_ui,
            camera_position,
            camera_zoom,
            camera_bookmarks: self.camera_bookmarks.clone(),
            active_camera_bookmark: self.active_camera_bookmark.clone(),
            camera_follow_target: self.camera_follow_target.as_ref().map(|id| id.as_str().to_string()),
            camera_bookmark_input: camera_bookmark_input_state,
            mesh_keys,
            environment_options,
            active_environment,
            debug_show_spatial_hash: debug_show_spatial_hash_state,
            debug_show_colliders: debug_show_colliders_state,
            spatial_hash_rects,
            collider_rects,

            scene_history_list,
            atlas_snapshot,
            mesh_snapshot,
            clip_snapshot,
            recent_events,
            audio_triggers,
            audio_enabled,
            audio_health,
            binary_prefabs_enabled: BINARY_PREFABS_ENABLED,
            prefab_entries,
            prefab_name_input: prefab_name_input_state,
            prefab_format: prefab_format_state,
            prefab_status: prefab_status_state,
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
                available: script_plugin_available,
                script_path,
                enabled: scripts_enabled,
                paused: scripts_paused,
                last_error: script_last_error,
                repl_input: script_repl_input,
                repl_history_index: script_repl_history_index,
                repl_history: script_repl_history,
                console_entries: script_console_entries,
                focus_repl: script_focus_repl,
            },
            id_lookup_input: self.id_lookup_input.clone(),
            id_lookup_active: self.id_lookup_active,
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
        }
        if editor_settings_dirty {
            self.apply_editor_camera_settings();
        }
        self.environment_intensity = ui_environment_intensity;
        self.renderer.set_environment_intensity(self.environment_intensity);
        self.id_lookup_input = id_lookup_input;
        self.id_lookup_active = id_lookup_active;

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

        self.selected_entity = selection.entity;
        if self.input.take_delete_selection() {
            if let Some(entity) = self.selected_entity {
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
                    self.active_camera_bookmark = None;
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
                            self.selected_entity = None;
                            self.gizmo_interaction = None;
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
                        self.selected_entity = None;
                        self.gizmo_interaction = None;
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
            self.selected_entity = None;
            self.gizmo_interaction = None;
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
            self.gizmo_interaction = None;
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
            self.selected_entity = None;
            self.gizmo_interaction = None;
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.clear_handles();
            }
            self.sync_emitter_ui();
            self.set_inspector_status(None);
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
                self.gpu_frame_counter = self.gpu_frame_counter.saturating_add(1);
                if let Some(analytics) = self.analytics_plugin_mut() {
                    analytics.record_gpu_timings(&timings);
                }
                let arc_timings = Arc::from(timings.clone().into_boxed_slice());
                self.gpu_timings = Arc::clone(&arc_timings);
                self.gpu_timing_history
                    .push_back(GpuTimingFrame { frame_index: self.gpu_frame_counter, timings });
                while self.gpu_timing_history.len() > self.gpu_timing_history_capacity {
                    self.gpu_timing_history.pop_front();
                }
            }
        } else {
            frame.present();
            let timings = self.renderer.take_gpu_timings();
            if !timings.is_empty() {
                self.gpu_frame_counter = self.gpu_frame_counter.saturating_add(1);
                if let Some(analytics) = self.analytics_plugin_mut() {
                    analytics.record_gpu_timings(&timings);
                }
                let arc_timings = Arc::from(timings.clone().into_boxed_slice());
                self.gpu_timings = Arc::clone(&arc_timings);
                self.gpu_timing_history
                    .push_back(GpuTimingFrame { frame_index: self.gpu_frame_counter, timings });
                while self.gpu_timing_history.len() > self.gpu_timing_history_capacity {
                    self.gpu_timing_history.pop_front();
                }
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
        self.frame_profiler.push(FrameTimingSample {
            frame_ms,
            update_ms: update_time_ms,
            fixed_ms: fixed_time_ms,
            render_ms: render_time_ms,
            ui_ms: ui_time_ms,
        });
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
                        self.ecs.set_emitter_colors(
                            emitter,
                            color,
                            Vec4::from_array(end_color),
                        );
                    }
                }
                ScriptCommand::SetEmitterEndColor { color } => {
                    self.editor_ui_state_mut().ui_emitter_end_color = color.to_array();
                    if let Some(emitter) = self.emitter_entity {
                        let start_color = self.editor_ui_state().ui_emitter_start_color;
                        self.ecs.set_emitter_colors(
                            emitter,
                            Vec4::from_array(start_color),
                            color,
                        );
                    }
                }
                ScriptCommand::SetEmitterStartSize { size } => {
                    let clamped = size.max(0.01);
                    self.editor_ui_state_mut().ui_emitter_start_size = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        let end_size = self.editor_ui_state().ui_emitter_end_size;
                        self.ecs.set_emitter_sizes(
                            emitter,
                            clamped,
                            end_size,
                        );
                    }
                }
                ScriptCommand::SetEmitterEndSize { size } => {
                    let clamped = size.max(0.01);
                    self.editor_ui_state_mut().ui_emitter_end_size = clamped;
                    if let Some(emitter) = self.emitter_entity {
                        let start_size = self.editor_ui_state().ui_emitter_start_size;
                        self.ecs.set_emitter_sizes(
                            emitter,
                            start_size,
                            clamped,
                        );
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
