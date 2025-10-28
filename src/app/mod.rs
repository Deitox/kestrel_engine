use crate::analytics::AnalyticsPlugin;
use crate::assets::AssetManager;
use crate::audio::{AudioHealthSnapshot, AudioPlugin};
use crate::camera::Camera2D;
use crate::camera3d::Camera3D;
use crate::config::{AppConfig, AppConfigOverrides};
use crate::ecs::{EcsWorld, InstanceData, MeshLightingInfo, ParticleCaps};
use crate::environment::EnvironmentRegistry;
use crate::events::GameEvent;
use crate::gizmo::{GizmoInteraction, GizmoMode};
use crate::input::{Input, InputEvent};
use crate::material_registry::{MaterialGpu, MaterialRegistry};
use crate::mesh_preview::{MeshControlMode, MeshPreviewPlugin};
use crate::mesh_registry::MeshRegistry;
use crate::plugins::{FeatureRegistryHandle, PluginContext, PluginManager};
use crate::prefab::{PrefabFormat, PrefabLibrary, PrefabStatusKind, PrefabStatusMessage};
use crate::renderer::{GpuPassTiming, MeshDraw, RenderViewport, Renderer, SpriteBatch};
use crate::scene::{
    EnvironmentDependency, Scene, SceneCamera2D, SceneCameraBookmark, SceneDependencies, SceneEntityId,
    SceneEnvironment, SceneLightingData, SceneMetadata, SceneShadowData, SceneViewportMode, Vec2Data,
};
use crate::scripts::{ScriptCommand, ScriptHandle, ScriptPlugin};
use crate::time::Time;
mod editor_ui;
mod gizmo_interaction;

use bevy_ecs::prelude::Entity;
use glam::{Mat4, Vec2, Vec3, Vec4};

use anyhow::{anyhow, Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};

// egui
use egui::Context as EguiCtx;
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use egui_winit::State as EguiWinit;

const CAMERA_BASE_HALF_HEIGHT: f32 = 1.2;
const PLUGIN_MANIFEST_PATH: &str = "config/plugins.json";
const INPUT_CONFIG_PATH: &str = "config/input.json";
const SCRIPT_CONSOLE_CAPACITY: usize = 200;
const SCRIPT_HISTORY_CAPACITY: usize = 64;
const BINARY_PREFABS_ENABLED: bool = cfg!(feature = "binary_scene");

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

    fn samples(&self) -> Vec<FrameTimingSample> {
        self.history.iter().copied().collect()
    }
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
    time: Time,
    pub(crate) input: Input,
    assets: AssetManager,
    prefab_library: PrefabLibrary,
    environment_registry: EnvironmentRegistry,
    persistent_environments: HashSet<String>,
    scene_environment_ref: Option<String>,
    active_environment_key: String,
    environment_intensity: f32,
    should_close: bool,
    accumulator: f32,
    fixed_dt: f32,

    // egui
    egui_ctx: EguiCtx,
    egui_winit: Option<EguiWinit>,
    egui_renderer: Option<EguiRenderer>,
    egui_screen: Option<ScreenDescriptor>,

    // UI State
    ui_spawn_per_press: i32,
    ui_auto_spawn_rate: f32, // per second
    ui_cell_size: f32,
    ui_spatial_use_quadtree: bool,
    ui_spatial_density_threshold: f32,
    ui_root_spin: f32,
    ui_emitter_rate: f32,
    ui_emitter_spread: f32,
    ui_emitter_speed: f32,
    ui_emitter_lifetime: f32,
    ui_emitter_start_size: f32,
    ui_emitter_end_size: f32,
    ui_emitter_start_color: [f32; 4],
    ui_emitter_end_color: [f32; 4],
    ui_particle_max_spawn_per_frame: u32,
    ui_particle_max_total: u32,
    ui_particle_max_emitter_backlog: f32,
    ui_light_direction: Vec3,
    ui_light_color: Vec3,
    ui_light_ambient: Vec3,
    ui_light_exposure: f32,
    ui_environment_intensity: f32,
    ui_shadow_distance: f32,
    ui_shadow_bias: f32,
    ui_shadow_strength: f32,
    ui_scale: f32,
    ui_scene_path: String,
    ui_scene_status: Option<String>,
    prefab_name_input: String,
    prefab_format: PrefabFormat,
    prefab_status: Option<PrefabStatusMessage>,
    camera_bookmark_input: String,
    scene_dependencies: Option<SceneDependencies>,
    scene_history: VecDeque<String>,
    inspector_status: Option<String>,
    debug_show_spatial_hash: bool,
    debug_show_colliders: bool,
    script_debugger_open: bool,
    script_focus_repl: bool,
    script_repl_input: String,
    script_repl_history: VecDeque<String>,
    script_repl_history_index: Option<usize>,
    script_console: VecDeque<ScriptConsoleEntry>,
    last_reported_script_error: Option<String>,

    // Plugins
    plugins: PluginManager,

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
    scene_mesh_refs: HashSet<String>,
    pub(crate) scene_material_refs: HashSet<String>,

    pub(crate) material_registry: MaterialRegistry,
    pub(crate) mesh_registry: MeshRegistry,

    viewport: Viewport,
    id_lookup_input: String,
    id_lookup_active: bool,
    frame_profiler: FrameProfiler,
    gpu_timings: Vec<GpuPassTiming>,
    gpu_timing_history: VecDeque<GpuTimingFrame>,
    gpu_timing_history_capacity: usize,
    gpu_frame_counter: u64,
    gpu_metrics_status: Option<String>,

    // Particles
    emitter_entity: Option<Entity>,

    sprite_atlas_views: HashMap<String, Arc<wgpu::TextureView>>,
}

impl App {
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
        let manifest = match PluginManager::load_manifest(PLUGIN_MANIFEST_PATH) {
            Ok(data) => data,
            Err(err) => {
                self.ui_scene_status = Some(format!("Plugin manifest parse failed: {err}"));
                return;
            }
        };
        let Some(manifest) = manifest else {
            self.ui_scene_status = Some("Plugin manifest not found".to_string());
            return;
        };
        let mut reload_error = None;
        let mut newly_loaded = Vec::new();
        self.with_plugins(|plugins, ctx| {
            plugins.clear_dynamic_statuses();
            match plugins.load_dynamic_from_manifest(&manifest, ctx) {
                Ok(mut names) => newly_loaded.append(&mut names),
                Err(err) => reload_error = Some(err),
            }
        });
        if let Some(err) = reload_error {
            self.ui_scene_status = Some(format!("Plugin reload failed: {err}"));
        } else if newly_loaded.is_empty() {
            self.ui_scene_status = Some("Plugin manifest reloaded".to_string());
        } else {
            self.ui_scene_status = Some(format!("Loaded plugins: {}", newly_loaded.join(", ")));
        }
    }
    pub async fn new(config: AppConfig) -> Self {
        let mut renderer = Renderer::new(&config.window).await;
        let lighting_state = renderer.lighting().clone();
        let particle_config = config.particles.clone();
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
        let scene_path = String::from("assets/scenes/quick_save.json");
        let mut scene_history = VecDeque::with_capacity(8);
        scene_history.push_back(scene_path.clone());
        let time = Time::new();
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

        // egui context and state
        let egui_ctx = EguiCtx::default();
        let egui_winit = None;
        let plugin_manifest = match PluginManager::load_manifest(PLUGIN_MANIFEST_PATH) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("[plugin] failed to parse manifest: {err:?}");
                None
            }
        };
        let disabled_builtins: HashSet<String> = plugin_manifest
            .as_ref()
            .map(|manifest| manifest.disabled_builtins().map(|name| name.to_string()).collect())
            .unwrap_or_default();
        let mut plugins = PluginManager::default();
        {
            let mut ctx = PluginContext::new(
                &mut renderer,
                &mut ecs,
                &mut assets,
                &mut input,
                &mut material_registry,
                &mut mesh_registry,
                &mut environment_registry,
                &time,
                Self::emit_event_for_plugin,
                plugins.feature_handle(),
                None,
            );
            if disabled_builtins.contains("mesh_preview") {
                plugins.record_builtin_disabled("mesh_preview", "disabled via config/plugins.json");
            } else if let Err(err) = plugins.register(Box::new(MeshPreviewPlugin::new()), &mut ctx) {
                eprintln!("[plugin] failed to register mesh preview plugin: {err:?}");
            }
            if disabled_builtins.contains("analytics") {
                plugins.record_builtin_disabled("analytics", "disabled via config/plugins.json");
            } else if let Err(err) = plugins.register(Box::new(AnalyticsPlugin::default()), &mut ctx) {
                eprintln!("[plugin] failed to register analytics plugin: {err:?}");
            }
            if disabled_builtins.contains("scripts") {
                plugins.record_builtin_disabled("scripts", "disabled via config/plugins.json");
            } else if let Err(err) =
                plugins.register(Box::new(ScriptPlugin::new("assets/scripts/main.rhai")), &mut ctx)
            {
                eprintln!("[plugin] failed to register script plugin: {err:?}");
            }
            if disabled_builtins.contains("audio") {
                plugins.record_builtin_disabled("audio", "disabled via config/plugins.json");
            } else if let Err(err) = plugins.register(Box::new(AudioPlugin::new(16)), &mut ctx) {
                eprintln!("[plugin] failed to register audio plugin: {err:?}");
            }
            if let Some(manifest) = plugin_manifest.as_ref() {
                match plugins.load_dynamic_from_manifest(manifest, &mut ctx) {
                    Ok(loaded) => {
                        if !loaded.is_empty() {
                            println!("[plugin] loaded dynamic plugins: {}", loaded.join(", "));
                        }
                    }
                    Err(err) => eprintln!("[plugin] failed to load dynamic plugins: {err:?}"),
                }
            }
        }
        if !initial_events.is_empty() {
            let mut ctx = PluginContext::new(
                &mut renderer,
                &mut ecs,
                &mut assets,
                &mut input,
                &mut material_registry,
                &mut mesh_registry,
                &mut environment_registry,
                &time,
                Self::emit_event_for_plugin,
                plugins.feature_handle(),
                None,
            );
            plugins.handle_events(&mut ctx, &initial_events);
        }

        let mut app = Self {
            renderer,
            ecs,
            time,
            input,
            assets,
            prefab_library,
            environment_registry,
            persistent_environments,
            scene_environment_ref: None,
            active_environment_key: default_environment_key.clone(),
            environment_intensity,
            should_close: false,
            accumulator: 0.0,
            fixed_dt: 1.0 / 60.0,
            egui_ctx,
            egui_winit,
            egui_renderer: None,
            egui_screen: None,
            ui_spawn_per_press: 200,
            ui_auto_spawn_rate: 0.0,
            ui_cell_size: 0.25,
            ui_spatial_use_quadtree: false,
            ui_spatial_density_threshold: 6.0,
            ui_root_spin: 1.2,
            ui_emitter_rate,
            ui_emitter_spread,
            ui_emitter_speed,
            ui_emitter_lifetime,
            ui_emitter_start_size,
            ui_emitter_end_size,
            ui_emitter_start_color,
            ui_emitter_end_color,
            ui_particle_max_spawn_per_frame: particle_config.max_spawn_per_frame,
            ui_particle_max_total: particle_config.max_total,
            ui_particle_max_emitter_backlog: particle_config.max_emitter_backlog,
            ui_light_direction: lighting_state.direction,
            ui_light_color: lighting_state.color,
            ui_light_ambient: lighting_state.ambient,
            ui_light_exposure: lighting_state.exposure,
            ui_environment_intensity: environment_intensity,
            ui_shadow_distance: lighting_state.shadow_distance,
            ui_shadow_bias: lighting_state.shadow_bias,
            ui_shadow_strength: lighting_state.shadow_strength,
            ui_scale: 1.0,
            ui_scene_path: scene_path,
            ui_scene_status: None,
            prefab_name_input: String::new(),
            prefab_format: PrefabFormat::Json,
            prefab_status: None,
            camera_bookmark_input: String::new(),
            scene_dependencies: None,
            scene_history,
            inspector_status: None,
            debug_show_spatial_hash: false,
            debug_show_colliders: false,
            script_debugger_open: false,
            script_focus_repl: false,
            script_repl_input: String::new(),
            script_repl_history: VecDeque::new(),
            script_repl_history_index: None,
            script_console: VecDeque::with_capacity(SCRIPT_CONSOLE_CAPACITY),
            last_reported_script_error: None,
            plugins,
            camera: Camera2D::new(CAMERA_BASE_HALF_HEIGHT),
            viewport_camera_mode: ViewportCameraMode::default(),
            camera_bookmarks: Vec::new(),
            active_camera_bookmark: None,
            camera_follow_target: None,
            selected_entity: None,
            gizmo_mode: GizmoMode::default(),
            gizmo_interaction: None,
            scene_atlas_refs: HashSet::new(),
            persistent_atlases: HashSet::new(),
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
            frame_profiler: FrameProfiler::new(240),
            gpu_timings: Vec::new(),
            gpu_timing_history: VecDeque::with_capacity(240),
            gpu_timing_history_capacity: 240,
            gpu_frame_counter: 0,
            gpu_metrics_status: None,
        };
        app.apply_particle_caps();
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

    fn set_prefab_status(&mut self, kind: PrefabStatusKind, message: impl Into<String>) {
        self.prefab_status = Some(PrefabStatusMessage { kind, message: message.into() });
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
        let Some(scene) = self.ecs.export_prefab(request.entity, &self.assets) else {
            self.set_prefab_status(PrefabStatusKind::Error, "Failed to export selection to prefab.");
            return;
        };
        let path = self.prefab_library.path_for(trimmed, request.format);
        let existed = path.exists();
        let sanitized_name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(trimmed).to_string();
        match scene.save_to_path(&path) {
            Ok(()) => {
                self.prefab_name_input = sanitized_name.clone();
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

    fn plugin_context(&mut self, feature_handle: FeatureRegistryHandle) -> PluginContext<'_> {
        PluginContext::new(
            &mut self.renderer,
            &mut self.ecs,
            &mut self.assets,
            &mut self.input,
            &mut self.material_registry,
            &mut self.mesh_registry,
            &mut self.environment_registry,
            &self.time,
            Self::emit_event_for_plugin,
            feature_handle,
            self.selected_entity,
        )
    }

    fn emit_event_for_plugin(ecs: &mut EcsWorld, event: GameEvent) {
        ecs.push_event(event);
    }

    fn with_plugins<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut PluginManager, &mut PluginContext<'_>),
    {
        let mut plugins = std::mem::take(&mut self.plugins);
        {
            let handle = plugins.feature_handle();
            let mut ctx = self.plugin_context(handle);
            f(&mut plugins, &mut ctx);
        }
        self.plugins = plugins;
    }

    fn audio_plugin(&self) -> Option<&AudioPlugin> {
        self.plugins.get::<AudioPlugin>()
    }

    fn analytics_plugin(&self) -> Option<&AnalyticsPlugin> {
        self.plugins.get::<AnalyticsPlugin>()
    }

    fn analytics_plugin_mut(&mut self) -> Option<&mut AnalyticsPlugin> {
        self.plugins.get_mut::<AnalyticsPlugin>()
    }

    fn mesh_preview_plugin(&self) -> Option<&MeshPreviewPlugin> {
        self.plugins.get::<MeshPreviewPlugin>()
    }

    fn mesh_preview_plugin_mut(&mut self) -> Option<&mut MeshPreviewPlugin> {
        self.plugins.get_mut::<MeshPreviewPlugin>()
    }

    fn script_plugin(&self) -> Option<&ScriptPlugin> {
        self.plugins.get::<ScriptPlugin>()
    }

    fn script_plugin_mut(&mut self) -> Option<&mut ScriptPlugin> {
        self.plugins.get_mut::<ScriptPlugin>()
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
        if let Some(pos) = self.scene_history.iter().position(|entry| entry == trimmed) {
            self.scene_history.remove(pos);
        }
        self.scene_history.push_front(trimmed.to_string());
        while self.scene_history.len() > 8 {
            self.scene_history.pop_back();
        }
    }

    fn push_script_console(&mut self, kind: ScriptConsoleKind, text: impl Into<String>) {
        self.script_console.push_back(ScriptConsoleEntry { kind, text: text.into() });
        while self.script_console.len() > SCRIPT_CONSOLE_CAPACITY {
            self.script_console.pop_front();
        }
    }

    fn append_script_history(&mut self, command: &str) {
        if command.is_empty() {
            return;
        }
        self.script_repl_history.push_back(command.to_string());
        while self.script_repl_history.len() > SCRIPT_HISTORY_CAPACITY {
            self.script_repl_history.pop_front();
        }
        self.script_repl_history_index = None;
    }

    fn execute_repl_command(&mut self, command: String) {
        let trimmed = command.trim();
        if trimmed.is_empty() {
            return;
        }
        self.append_script_history(trimmed);
        self.push_script_console(ScriptConsoleKind::Input, format!("> {trimmed}"));
        self.script_repl_input.clear();
        self.script_focus_repl = true;
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
                self.script_debugger_open = true;
            }
        }
    }

    fn sync_script_error_state(&mut self) {
        let current_error =
            self.script_plugin().and_then(|plugin| plugin.last_error().map(|err| err.to_string()));
        if current_error == self.last_reported_script_error {
            return;
        }
        self.last_reported_script_error = current_error.clone();
        if let Some(err) = current_error {
            self.push_script_console(ScriptConsoleKind::Error, format!("Runtime error: {err}"));
            self.script_debugger_open = true;
            self.script_focus_repl = true;
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
            self.ui_environment_intensity = intensity;
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
        self.ui_environment_intensity = intensity;
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
                plugin.set_mesh_control_mode(ctx, mode);
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
                        plugin.set_mesh_control_mode(ctx, MeshControlMode::Orbit);
                    }
                }
            });
        }
    }

    fn set_frustum_lock(&mut self, enabled: bool) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                plugin.set_frustum_lock(ctx, enabled);
            }
        });
    }

    fn reset_mesh_camera(&mut self) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                plugin.reset_mesh_camera(ctx);
            }
        });
    }

    fn set_preview_mesh(&mut self, new_key: String) {
        let scene_refs = self.scene_material_refs.clone();
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                plugin.set_preview_mesh(ctx, &scene_refs, new_key.clone());
            }
        });
    }

    fn spawn_mesh_entity(&mut self, mesh_key: &str) {
        let key = mesh_key.to_string();
        let mut spawned = None;
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                spawned = plugin.spawn_mesh_entity(ctx, &key);
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
                self.ui_scene_status =
                    Some(format!("VSync {}", if enabled { "enabled" } else { "disabled" }));
            }
            Err(err) => {
                eprintln!("Failed to update VSync: {err:?}");
                self.ui_scene_status = Some(format!("Failed to update VSync: {err}"));
            }
        }
    }

    fn apply_particle_caps(&mut self) {
        if self.ui_particle_max_spawn_per_frame > self.ui_particle_max_total {
            self.ui_particle_max_spawn_per_frame = self.ui_particle_max_total;
        }
        let caps = ParticleCaps::new(
            self.ui_particle_max_spawn_per_frame,
            self.ui_particle_max_total,
            self.ui_particle_max_emitter_backlog,
        );
        self.ecs.set_particle_caps(caps);
    }

    fn sync_emitter_ui(&mut self) {
        if let Some(entity) = self.ecs.first_emitter() {
            self.emitter_entity = Some(entity);
            if let Some(snapshot) = self.ecs.emitter_snapshot(entity) {
                self.ui_emitter_rate = snapshot.rate;
                self.ui_emitter_spread = snapshot.spread;
                self.ui_emitter_speed = snapshot.speed;
                self.ui_emitter_lifetime = snapshot.lifetime;
                self.ui_emitter_start_size = snapshot.start_size;
                self.ui_emitter_end_size = snapshot.end_size;
                self.ui_emitter_start_color = snapshot.start_color.to_array();
                self.ui_emitter_end_color = snapshot.end_color.to_array();
            }
        } else {
            self.emitter_entity = None;
        }
    }

    fn update_scene_dependencies(&mut self, deps: &SceneDependencies) -> Result<()> {
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
        self.scene_dependencies = Some(deps.clone());
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
            },
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
            let (mut direction, color, ambient, exposure, shadow) = lighting.components();
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
            }
            let renderer_lighting = self.renderer.lighting();
            self.ui_light_direction = renderer_lighting.direction;
            self.ui_light_color = renderer_lighting.color;
            self.ui_light_ambient = renderer_lighting.ambient;
            self.ui_light_exposure = renderer_lighting.exposure;
            self.ui_shadow_distance = renderer_lighting.shadow_distance;
            self.ui_shadow_bias = renderer_lighting.shadow_bias;
            self.ui_shadow_strength = renderer_lighting.shadow_strength;
            self.renderer.mark_shadow_settings_dirty();
        }
        if let Some(environment) = metadata.environment.as_ref() {
            let intensity = environment.intensity.max(0.0);
            if let Err(err) = self.set_active_environment(&environment.key, intensity) {
                self.ui_scene_status = Some(format!("Environment '{}' unavailable: {err}", environment.key));
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

        if self.egui_winit.is_none() {
            if let Some(window) = self.renderer.window() {
                let state = EguiWinit::new(
                    self.egui_ctx.clone(),
                    egui::ViewportId::ROOT,
                    window,
                    Some(self.renderer.pixels_per_point()),
                    window.theme(),
                    None,
                );
                self.egui_winit = Some(state);
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
        self.egui_renderer = Some(egui_renderer);
        let size = self.renderer.size();
        self.egui_screen = Some(ScreenDescriptor {
            size_in_pixels: [size.width, size.height],
            pixels_per_point: self.renderer.pixels_per_point() * self.ui_scale,
        });

        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                plugin.ensure_preview_gpu(ctx);
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
        if let (Some(window), Some(state)) = (self.renderer.window(), self.egui_winit.as_mut()) {
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
                if let Some(sd) = &mut self.egui_screen {
                    sd.size_in_pixels = [size.width, size.height];
                    sd.pixels_per_point = self.renderer.pixels_per_point() * self.ui_scale;
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
        self.time.tick();
        let dt = self.time.delta_seconds();
        self.accumulator += dt;
        self.ecs.profiler_begin_frame();
        let frame_start = Instant::now();
        let mut fixed_time_ms = 0.0;
        #[allow(unused_assignments)]
        let mut update_time_ms = 0.0;
        #[allow(unused_assignments)]
        let mut render_time_ms = 0.0;
        let mut ui_time_ms = 0.0;

        if let Some(entity) = self.selected_entity {
            if !self.ecs.entity_exists(entity) {
                self.selected_entity = None;
            }
        }

        if self.ui_auto_spawn_rate > 0.0 {
            let to_spawn = (self.ui_auto_spawn_rate * dt) as i32;
            if to_spawn > 0 {
                self.ecs.spawn_burst(&self.assets, to_spawn as usize);
            }
        }

        if self.input.take_space_pressed() {
            self.ecs.spawn_burst(&self.assets, self.ui_spawn_per_press as usize);
        }
        if self.input.take_b_pressed() {
            self.ecs.spawn_burst(&self.assets, (self.ui_spawn_per_press * 5).max(1000) as usize);
        }

        self.with_plugins(|plugins, ctx| plugins.update(ctx, dt));

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

        self.ecs.set_spatial_cell(self.ui_cell_size.max(0.05));
        self.ecs.set_spatial_quadtree_enabled(self.ui_spatial_use_quadtree);
        self.ecs.set_spatial_density_threshold(self.ui_spatial_density_threshold);
        if let Some(emitter) = self.emitter_entity {
            self.ecs.set_emitter_rate(emitter, self.ui_emitter_rate);
            self.ecs.set_emitter_spread(emitter, self.ui_emitter_spread);
            self.ecs.set_emitter_speed(emitter, self.ui_emitter_speed);
            self.ecs.set_emitter_lifetime(emitter, self.ui_emitter_lifetime);
            self.ecs.set_emitter_colors(
                emitter,
                Vec4::from_array(self.ui_emitter_start_color),
                Vec4::from_array(self.ui_emitter_end_color),
            );
            self.ecs.set_emitter_sizes(emitter, self.ui_emitter_start_size, self.ui_emitter_end_size);
        }
        let commands = self.drain_script_commands();
        self.apply_script_commands(commands);
        for message in self.drain_script_logs() {
            self.push_script_console(ScriptConsoleKind::Log, format!("[log] {message}"));
            self.ecs.push_event(GameEvent::ScriptMessage { message });
        }

        while self.accumulator >= self.fixed_dt {
            let fixed_dt = self.fixed_dt;
            let fixed_start = Instant::now();
            self.ecs.fixed_step(fixed_dt);
            fixed_time_ms += fixed_start.elapsed().as_secs_f32() * 1000.0;
            let plugin_fixed_start = Instant::now();
            self.with_plugins(|plugins, ctx| plugins.fixed_update(ctx, fixed_dt));
            fixed_time_ms += plugin_fixed_start.elapsed().as_secs_f32() * 1000.0;
            self.accumulator -= fixed_dt;
        }
        let update_start = Instant::now();
        self.ecs.update(dt);
        update_time_ms = update_start.elapsed().as_secs_f32() * 1000.0;
        if self.camera_follow_target.is_some() && !self.refresh_camera_follow() {
            self.camera_follow_target = None;
        }
        self.record_events();
        let particle_budget_snapshot = self.ecs.particle_budget_metrics();
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
        let mut grouped_instances: BTreeMap<String, Vec<InstanceData>> = BTreeMap::new();
        for instance in sprite_instances {
            grouped_instances.entry(instance.atlas).or_default().push(instance.data);
        }
        let mut instances: Vec<InstanceData> = Vec::new();
        let mut sprite_batches: Vec<SpriteBatch> = Vec::new();
        for (atlas, batch_instances) in grouped_instances {
            if batch_instances.is_empty() {
                continue;
            }
            let start_len = instances.len();
            instances.extend(batch_instances.into_iter());
            if instances.len() > u32::MAX as usize {
                eprintln!("Too many sprite instances to render ({}).", instances.len());
                instances.truncate(start_len);
                break;
            }
            let start = start_len as u32;
            let end = instances.len() as u32;
            match self.atlas_view(&atlas) {
                Ok(view) => {
                    sprite_batches.push(SpriteBatch { atlas, range: start..end, view });
                }
                Err(err) => {
                    eprintln!("Atlas '{}' unavailable for rendering: {err:?}", atlas);
                    instances.truncate(start_len);
                    self.invalidate_atlas_view(&atlas);
                }
            }
        }
        let render_viewport = RenderViewport {
            origin: (self.viewport.origin.x, self.viewport.origin.y),
            size: (self.viewport.size.x, self.viewport.size.y),
        };
        let view_proj = self.camera.view_projection(viewport_size);
        let default_material_key = self.material_registry.default_key().to_string();
        let mut mesh_draw_infos: Vec<(String, Mat4, MeshLightingInfo, String)> = Vec::new();
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
                    mesh_draw_infos.push((
                        instance.key.clone(),
                        instance.model,
                        instance.lighting,
                        material_key,
                    ));
                }
                Err(err) => {
                    eprintln!("[mesh] Unable to prepare '{}': {err:?}", instance.key);
                }
            }
        }
        let mut mesh_draws: Vec<MeshDraw> = Vec::new();
        let mut material_cache: HashMap<String, Arc<MaterialGpu>> = HashMap::new();
        for (key, model, lighting, material_key) in mesh_draw_infos {
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
            mesh_draws.push(MeshDraw { mesh, model, lighting, material: material_gpu, casts_shadows });
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

        if self.egui_winit.is_none() {
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
            self.egui_winit.as_mut().unwrap().take_egui_input(window)
        };
        let base_pixels_per_point = self.renderer.pixels_per_point();
        self.egui_ctx.set_pixels_per_point(base_pixels_per_point * self.ui_scale);
        let ui_pixels_per_point = self.egui_ctx.pixels_per_point();
        if let Some(screen) = self.egui_screen.as_mut() {
            screen.pixels_per_point = ui_pixels_per_point;
        };
        let hist_points =
            self.analytics_plugin().map(|plugin| plugin.frame_plot_points()).unwrap_or_else(Vec::new);
        let spatial_metrics = self.analytics_plugin().and_then(|plugin| plugin.spatial_metrics());
        let frame_timings = self.frame_profiler.samples();
        let system_timings = self.ecs.system_timings();
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
        let recent_events: Vec<GameEvent> = self
            .analytics_plugin()
            .map(|plugin| plugin.recent_events().cloned().collect())
            .unwrap_or_default();
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
        let mut mesh_keys: Vec<String> = self.mesh_registry.keys().map(|k| k.to_string()).collect();
        mesh_keys.sort();
        let scene_history_list: Vec<String> = self.scene_history.iter().cloned().collect();
        let atlas_snapshot: Vec<String> = self.scene_atlas_refs.iter().cloned().collect();
        let mesh_snapshot: Vec<String> = self.scene_mesh_refs.iter().cloned().collect();
        let environment_options: Vec<(String, String)> = self
            .environment_registry
            .keys()
            .map(|key| {
                let label = self
                    .environment_registry
                    .definition(key)
                    .map(|def| def.label().to_string())
                    .unwrap_or_else(|| key.clone());
                (key.clone(), label)
            })
            .collect();
        let active_environment = self.active_environment_key.clone();
        let collider_rects =
            if self.debug_show_colliders && self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
                self.ecs.collider_rects()
            } else {
                Vec::new()
            };
        let spatial_hash_rects =
            if self.debug_show_spatial_hash && self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
                self.ecs.spatial_hash_rects()
            } else {
                Vec::new()
            };
        if !BINARY_PREFABS_ENABLED && self.prefab_format == PrefabFormat::Binary {
            self.prefab_format = PrefabFormat::Json;
        }
        let prefab_entries: Vec<editor_ui::PrefabShelfEntry> = self
            .prefab_library
            .entries()
            .iter()
            .map(|entry| {
                let relative = entry
                    .path
                    .strip_prefix(self.prefab_library.root())
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| entry.path.display().to_string());
                editor_ui::PrefabShelfEntry {
                    name: entry.name.clone(),
                    format: entry.format,
                    path_display: relative,
                }
            })
            .collect();

        let editor_params = editor_ui::EditorUiParams {
            raw_input,
            base_pixels_per_point,
            hist_points,
            frame_timings,
            system_timings,
            entity_count,
            instances_drawn,
            vsync_enabled: self.renderer.vsync_enabled(),
            particle_budget: Some(particle_budget_snapshot),
            spatial_metrics,
            ui_scale: self.ui_scale,
            ui_cell_size: self.ui_cell_size,
            ui_spatial_use_quadtree: self.ui_spatial_use_quadtree,
            ui_spatial_density_threshold: self.ui_spatial_density_threshold,
            ui_spawn_per_press: self.ui_spawn_per_press,
            ui_auto_spawn_rate: self.ui_auto_spawn_rate,
            ui_environment_intensity: self.ui_environment_intensity,
            ui_root_spin: self.ui_root_spin,
            ui_emitter_rate: self.ui_emitter_rate,
            ui_emitter_spread: self.ui_emitter_spread,
            ui_emitter_speed: self.ui_emitter_speed,
            ui_emitter_lifetime: self.ui_emitter_lifetime,
            ui_emitter_start_size: self.ui_emitter_start_size,
            ui_emitter_end_size: self.ui_emitter_end_size,
            ui_emitter_start_color: self.ui_emitter_start_color,
            ui_emitter_end_color: self.ui_emitter_end_color,
            ui_particle_max_spawn_per_frame: self.ui_particle_max_spawn_per_frame,
            ui_particle_max_total: self.ui_particle_max_total,
            ui_particle_max_emitter_backlog: self.ui_particle_max_emitter_backlog,
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
            camera_bookmark_input: self.camera_bookmark_input.clone(),
            mesh_keys,
            environment_options,
            active_environment,
            debug_show_spatial_hash: self.debug_show_spatial_hash,
            debug_show_colliders: self.debug_show_colliders,
            spatial_hash_rects,
            collider_rects,

            scene_history_list,
            atlas_snapshot,
            mesh_snapshot,
            recent_events,
            audio_triggers,
            audio_enabled,
            audio_health,
            binary_prefabs_enabled: BINARY_PREFABS_ENABLED,
            prefab_entries,
            prefab_name_input: self.prefab_name_input.clone(),
            prefab_format: self.prefab_format,
            prefab_status: self.prefab_status.clone(),
            script_debugger: editor_ui::ScriptDebuggerParams {
                open: self.script_debugger_open,
                available: script_plugin_available,
                script_path,
                enabled: scripts_enabled,
                paused: scripts_paused,
                last_error: script_last_error,
                repl_input: self.script_repl_input.clone(),
                repl_history_index: self.script_repl_history_index,
                repl_history: self.script_repl_history.iter().cloned().collect(),
                console_entries: self.script_console.iter().cloned().collect(),
                focus_repl: self.script_focus_repl,
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
            ui_scale,
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
        } = editor_output;

        self.ui_scale = ui_scale;
        self.ui_cell_size = ui_cell_size;
        self.ui_spatial_use_quadtree = ui_spatial_use_quadtree;
        self.ui_spatial_density_threshold = ui_spatial_density_threshold;
        self.ui_spawn_per_press = ui_spawn_per_press;
        self.ui_auto_spawn_rate = ui_auto_spawn_rate;
        self.ui_environment_intensity = ui_environment_intensity;
        self.environment_intensity = ui_environment_intensity;
        self.renderer.set_environment_intensity(self.environment_intensity);
        self.ui_root_spin = ui_root_spin;
        self.ui_emitter_rate = ui_emitter_rate;
        self.ui_emitter_spread = ui_emitter_spread;
        self.ui_emitter_speed = ui_emitter_speed;
        self.ui_emitter_lifetime = ui_emitter_lifetime;
        self.ui_emitter_start_size = ui_emitter_start_size;
        self.ui_emitter_end_size = ui_emitter_end_size;
        self.ui_emitter_start_color = ui_emitter_start_color;
        self.ui_emitter_end_color = ui_emitter_end_color;
        self.ui_particle_max_spawn_per_frame = ui_particle_max_spawn_per_frame;
        self.ui_particle_max_total = ui_particle_max_total;
        self.ui_particle_max_emitter_backlog = ui_particle_max_emitter_backlog;
        self.id_lookup_input = id_lookup_input;
        self.id_lookup_active = id_lookup_active;
        self.camera_bookmark_input = camera_bookmark_input;
        self.debug_show_spatial_hash = debug_show_spatial_hash;
        self.debug_show_colliders = debug_show_colliders;
        self.prefab_name_input = prefab_name_input;
        self.prefab_format = prefab_format;
        self.prefab_status = prefab_status;

        if let Some(request) = id_lookup_request {
            let trimmed = request.trim();
            if trimmed.is_empty() {
                self.ui_scene_status = Some("Enter an entity ID to select.".to_string());
            } else if let Some(entity) = self.ecs.find_entity_by_scene_id(trimmed) {
                selection.entity = Some(entity);
                selection.details = self.ecs.entity_info(entity);
                self.ui_scene_status = Some(format!("Selected entity {}", trimmed));
            } else {
                self.ui_scene_status = Some(format!("Entity {} not found", trimmed));
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
                        self.ui_scene_status = Some(format!("Bookmark '{}' not found.", name));
                    }
                }
                None => {
                    self.active_camera_bookmark = None;
                    self.camera_follow_target = None;
                    self.ui_scene_status = Some("Camera set to free mode.".to_string());
                }
            }
        }
        if let Some(name) = camera_bookmark_save {
            if self.upsert_camera_bookmark(&name) {
                self.ui_scene_status = Some(format!("Saved camera bookmark '{}'.", name.trim()));
            } else {
                self.ui_scene_status = Some("Enter a bookmark name to save.".to_string());
            }
        }
        if let Some(name) = camera_bookmark_delete {
            if self.delete_camera_bookmark(&name) {
                self.ui_scene_status = Some(format!("Deleted camera bookmark '{}'.", name.trim()));
            } else {
                self.ui_scene_status = Some(format!("Bookmark '{}' not found.", name.trim()));
            }
        }
        if camera_follow_selection {
            if let Some(details) = selection.details.as_ref() {
                let scene_id = details.scene_id.clone();
                if self.set_camera_follow_scene_id(scene_id) {
                    self.ui_scene_status = Some(format!("Following entity {}.", details.scene_id.as_str()));
                } else {
                    self.ui_scene_status = Some("Unable to follow selected entity.".to_string());
                }
            } else {
                self.ui_scene_status = Some("Select an entity to follow.".to_string());
            }
        }
        if camera_follow_clear && self.camera_follow_target.is_some() {
            self.clear_camera_follow();
            self.ui_scene_status = Some("Camera follow cleared.".to_string());
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
                    self.ui_scene_status = Some(format!("Environment set to {}", environment_key));
                }
                Err(err) => {
                    self.ui_scene_status =
                        Some(format!("Environment '{}' unavailable: {err}", environment_key));
                }
            }
        }

        let egui::FullOutput { platform_output, textures_delta, shapes, .. } = full_output;
        if let Some(window) = self.renderer.window() {
            self.egui_winit.as_mut().unwrap().handle_platform_output(window, platform_output);
        } else {
            return;
        }

        self.script_debugger_open = script_debugger.open;
        self.script_repl_input = script_debugger.repl_input;
        self.script_repl_history_index = script_debugger.repl_history_index;
        self.script_focus_repl = script_debugger.focus_repl;
        if script_debugger.clear_console {
            self.script_console.clear();
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
                self.inspector_status = Some("Viewport framed selection.".to_string());
            } else {
                self.inspector_status = Some("Selection unavailable.".to_string());
            }
        }

        for (key, path) in actions.retain_atlases {
            match self.assets.retain_atlas(&key, path.as_deref()) {
                Ok(()) => {
                    self.scene_atlas_refs.insert(key.clone());
                    self.invalidate_atlas_view(&key);
                    self.ui_scene_status = Some(format!("Retained atlas {}", key));
                }
                Err(err) => {
                    self.ui_scene_status = Some(format!("Atlas retain failed: {err}"));
                }
            }
        }
        for (key, path) in actions.retain_meshes {
            match self.mesh_registry.retain_mesh(&key, path.as_deref(), &mut self.material_registry) {
                Ok(()) => {
                    self.scene_mesh_refs.insert(key.clone());
                    match self.mesh_registry.ensure_gpu(&key, &mut self.renderer) {
                        Ok(_) => {
                            self.ui_scene_status = Some(format!("Retained mesh {}", key));
                        }
                        Err(err) => {
                            self.set_mesh_status(format!("Mesh upload failed: {err}"));
                        }
                    }
                }
                Err(err) => {
                    self.ui_scene_status = Some(format!("Mesh retain failed: {err}"));
                }
            }
        }
        for (key, path) in actions.retain_environments {
            match self.environment_registry.retain(&key, path.as_deref()) {
                Ok(()) => {
                    let scene_requested = self.scene_environment_ref.as_deref() == Some(key.as_str());
                    let should_activate = scene_requested || self.active_environment_key == key;
                    if let Err(err) = self.environment_registry.ensure_gpu(&key, &mut self.renderer) {
                        self.ui_scene_status = Some(format!("Environment upload failed: {err}"));
                        continue;
                    }
                    if should_activate {
                        match self.set_active_environment(&key, self.environment_intensity) {
                            Ok(()) => {
                                self.ui_scene_status = Some(format!("Environment set to {}", key));
                            }
                            Err(err) => {
                                self.ui_scene_status = Some(format!("Environment bind failed: {err}"));
                            }
                        }
                    } else {
                        self.ui_scene_status = Some(format!("Retained environment {}", key));
                    }
                }
                Err(err) => {
                    self.ui_scene_status = Some(format!("Environment retain failed: {err}"));
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
            match scene.save_to_path(&self.ui_scene_path) {
                Ok(_) => {
                    let path = self.ui_scene_path.clone();
                    self.ui_scene_status = Some(format!("Saved {}", path));
                    self.remember_scene_path(&path);
                }
                Err(err) => self.ui_scene_status = Some(format!("Save failed: {err}")),
            }
        }
        if actions.load_scene {
            match self.ecs.load_scene_from_path_with_mesh(
                &self.ui_scene_path,
                &mut self.assets,
                |key, path| self.mesh_registry.ensure_mesh(key, path, &mut self.material_registry),
            ) {
                Ok(scene) => match self.update_scene_dependencies(&scene.dependencies) {
                    Ok(()) => {
                        let path = self.ui_scene_path.clone();
                        self.ui_scene_status = Some(format!("Loaded {}", path));
                        self.remember_scene_path(&path);
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
                        self.inspector_status = None;
                    }
                    Err(err) => {
                        self.ui_scene_status = Some(format!("Load failed: {err}"));
                        self.ecs.clear_world();
                        self.clear_scene_atlases();
                        self.selected_entity = None;
                        self.gizmo_interaction = None;
                        if let Some(plugin) = self.script_plugin_mut() {
                            plugin.clear_handles();
                        }
                        self.sync_emitter_ui();
                        self.inspector_status = None;
                    }
                },
                Err(err) => {
                    self.ui_scene_status = Some(format!("Load failed: {err}"));
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
            self.ecs.spawn_burst(&self.assets, self.ui_spawn_per_press as usize);
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
            self.ui_emitter_rate = 0.0;
            self.ui_emitter_spread = std::f32::consts::PI / 3.0;
            self.ui_emitter_speed = 0.8;
            self.ui_emitter_lifetime = 1.2;
            self.ui_emitter_start_size = 0.05;
            self.ui_emitter_end_size = 0.05;
            self.ui_emitter_start_color = [1.0, 1.0, 1.0, 1.0];
            self.ui_emitter_end_color = [1.0, 1.0, 1.0, 0.0];
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.clear_handles();
            }
            self.gizmo_interaction = None;
            if let Some(emitter) = self.emitter_entity {
                self.ecs.set_emitter_rate(emitter, self.ui_emitter_rate);
                self.ecs.set_emitter_spread(emitter, self.ui_emitter_spread);
                self.ecs.set_emitter_speed(emitter, self.ui_emitter_speed);
                self.ecs.set_emitter_lifetime(emitter, self.ui_emitter_lifetime);
                self.ecs.set_emitter_colors(
                    emitter,
                    Vec4::from_array(self.ui_emitter_start_color),
                    Vec4::from_array(self.ui_emitter_end_color),
                );
                self.ecs.set_emitter_sizes(emitter, self.ui_emitter_start_size, self.ui_emitter_end_size);
            }
        }
        if actions.reset_world {
            self.ecs.clear_world();
            self.clear_scene_atlases();
            self.selected_entity = None;
            self.gizmo_interaction = None;
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.clear_handles();
            }
            self.sync_emitter_ui();
            self.inspector_status = None;
        }
        if actions.reload_plugins {
            self.reload_dynamic_plugins();
        }
        if let (Some(ren), Some(screen)) = (self.egui_renderer.as_mut(), self.egui_screen.as_ref()) {
            if let (Ok(device), Ok(queue)) = (self.renderer.device(), self.renderer.queue()) {
                for (id, delta) in &textures_delta.set {
                    ren.update_texture(device, queue, *id, delta);
                }
            }
            let ui_render_start = Instant::now();
            let meshes = self.egui_ctx.tessellate(shapes, screen.pixels_per_point);
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
                self.gpu_timings = timings.clone();
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
                self.gpu_timings = timings.clone();
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

        self.ecs.set_root_spin(self.ui_root_spin);

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

impl Drop for App {
    fn drop(&mut self) {
        let mut plugins = std::mem::take(&mut self.plugins);
        {
            let handle = plugins.feature_handle();
            let mut ctx = PluginContext::new(
                &mut self.renderer,
                &mut self.ecs,
                &mut self.assets,
                &mut self.input,
                &mut self.material_registry,
                &mut self.mesh_registry,
                &mut self.environment_registry,
                &self.time,
                Self::emit_event_for_plugin,
                handle,
                self.selected_entity,
            );
            plugins.shutdown(&mut ctx);
        }
        // plugins dropped here
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
                    self.ui_auto_spawn_rate = rate.max(0.0);
                }
                ScriptCommand::SetSpawnPerPress { count } => {
                    self.ui_spawn_per_press = count.max(0);
                }
                ScriptCommand::SetEmitterRate { rate } => {
                    self.ui_emitter_rate = rate.max(0.0);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_rate(emitter, self.ui_emitter_rate);
                    }
                }
                ScriptCommand::SetEmitterSpread { spread } => {
                    self.ui_emitter_spread = spread.clamp(0.0, std::f32::consts::PI);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_spread(emitter, self.ui_emitter_spread);
                    }
                }
                ScriptCommand::SetEmitterSpeed { speed } => {
                    self.ui_emitter_speed = speed.max(0.0);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_speed(emitter, self.ui_emitter_speed);
                    }
                }
                ScriptCommand::SetEmitterLifetime { lifetime } => {
                    self.ui_emitter_lifetime = lifetime.max(0.05);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_lifetime(emitter, self.ui_emitter_lifetime);
                    }
                }
                ScriptCommand::SetEmitterStartColor { color } => {
                    self.ui_emitter_start_color = color.to_array();
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_colors(
                            emitter,
                            color,
                            Vec4::from_array(self.ui_emitter_end_color),
                        );
                    }
                }
                ScriptCommand::SetEmitterEndColor { color } => {
                    self.ui_emitter_end_color = color.to_array();
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_colors(
                            emitter,
                            Vec4::from_array(self.ui_emitter_start_color),
                            color,
                        );
                    }
                }
                ScriptCommand::SetEmitterStartSize { size } => {
                    self.ui_emitter_start_size = size.max(0.01);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_sizes(
                            emitter,
                            self.ui_emitter_start_size,
                            self.ui_emitter_end_size,
                        );
                    }
                }
                ScriptCommand::SetEmitterEndSize { size } => {
                    self.ui_emitter_end_size = size.max(0.01);
                    if let Some(emitter) = self.emitter_entity {
                        self.ecs.set_emitter_sizes(
                            emitter,
                            self.ui_emitter_start_size,
                            self.ui_emitter_end_size,
                        );
                    }
                }
            }
        }
    }
}
