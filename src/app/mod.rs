use crate::assets::AssetManager;
use crate::audio::AudioManager;
use crate::camera::Camera2D;
use crate::camera3d::{Camera3D, OrbitCamera};
use crate::config::AppConfig;
use crate::ecs::{EcsWorld, InstanceData, MeshLightingInfo};
use crate::events::GameEvent;
use crate::gizmo::{GizmoInteraction, GizmoMode};
use crate::input::{Input, InputEvent};
use crate::material_registry::{MaterialGpu, MaterialRegistry};
use crate::mesh_preview;
use crate::mesh_preview::{
    FreeflyController, MeshControlMode, MESH_CAMERA_FAR, MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR,
};
use crate::mesh_registry::MeshRegistry;
use crate::renderer::{MeshDraw, RenderViewport, Renderer, SpriteBatch};
use crate::scene::{
    SceneCamera2D, SceneDependencies, SceneLightingData, SceneMetadata, SceneViewportMode, Vec2Data,
};
use crate::scripts::{ScriptCommand, ScriptHost};
use crate::time::Time;
mod editor_ui;
mod gizmo_interaction;

use bevy_ecs::prelude::Entity;
use glam::{Mat4, Vec2, Vec3, Vec4};

use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;
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
pub async fn run() -> Result<()> {
    let config = AppConfig::load_or_default("config/app.json");
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
    ui_hist: Vec<f32>,
    ui_root_spin: f32,
    ui_emitter_rate: f32,
    ui_emitter_spread: f32,
    ui_emitter_speed: f32,
    ui_emitter_lifetime: f32,
    ui_emitter_start_size: f32,
    ui_emitter_end_size: f32,
    ui_emitter_start_color: [f32; 4],
    ui_emitter_end_color: [f32; 4],
    ui_light_direction: Vec3,
    ui_light_color: Vec3,
    ui_light_ambient: Vec3,
    ui_light_exposure: f32,
    ui_scale: f32,
    ui_scene_path: String,
    ui_scene_status: Option<String>,
    scene_dependencies: Option<SceneDependencies>,
    scene_history: VecDeque<String>,
    inspector_status: Option<String>,

    // Audio
    audio: AudioManager,

    // Events
    recent_events: VecDeque<GameEvent>,
    event_log_limit: usize,

    // Camera / selection
    pub(crate) camera: Camera2D,
    pub(crate) viewport_camera_mode: ViewportCameraMode,
    pub(crate) selected_entity: Option<Entity>,
    gizmo_mode: GizmoMode,
    gizmo_interaction: Option<GizmoInteraction>,

    // Configuration
    config: AppConfig,

    scene_atlas_refs: HashSet<String>,
    persistent_atlases: HashSet<String>,
    pub(crate) persistent_meshes: HashSet<String>,
    scene_mesh_refs: HashSet<String>,
    pub(crate) persistent_materials: HashSet<String>,
    pub(crate) scene_material_refs: HashSet<String>,

    pub(crate) material_registry: MaterialRegistry,
    pub(crate) mesh_registry: MeshRegistry,
    pub(crate) preview_mesh_key: String,
    pub(crate) mesh_orbit: OrbitCamera,
    pub(crate) mesh_camera: Camera3D,
    mesh_model: Mat4,
    mesh_angle: f32,
    pub(crate) mesh_control_mode: MeshControlMode,
    pub(crate) mesh_freefly: FreeflyController,
    pub(crate) mesh_freefly_speed: f32,
    pub(crate) mesh_freefly_velocity: Vec3,
    pub(crate) mesh_freefly_rot_velocity: Vec3,
    pub(crate) mesh_frustum_lock: bool,
    pub(crate) mesh_frustum_focus: Vec3,
    pub(crate) mesh_frustum_distance: f32,
    pub(crate) mesh_status: Option<String>,

    viewport: Viewport,

    // Particles
    emitter_entity: Option<Entity>,

    // Scripting
    scripts: ScriptHost,

    sprite_atlas_views: HashMap<String, Arc<wgpu::TextureView>>,
}

impl App {
    pub async fn new(config: AppConfig) -> Self {
        let renderer = Renderer::new(&config.window).await;
        let lighting_state = renderer.lighting().clone();
        let mut ecs = EcsWorld::new();
        let emitter = ecs.spawn_demo_scene();
        let mut audio = AudioManager::new(16);
        let event_log_limit = 32;
        let mut recent_events = VecDeque::with_capacity(event_log_limit);
        for event in ecs.drain_events() {
            if recent_events.len() == event_log_limit {
                recent_events.pop_front();
            }
            audio.handle_event(&event);
            recent_events.push_back(event);
        }
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
        let input = Input::new();
        let assets = AssetManager::new();
        let mut persistent_materials = HashSet::new();

        let mut material_registry = MaterialRegistry::new();
        let mut mesh_registry = MeshRegistry::new(&mut material_registry);
        let preview_mesh_key = mesh_registry.default_key().to_string();
        if let Err(err) = mesh_registry.retain_mesh(&preview_mesh_key, None, &mut material_registry) {
            eprintln!("[mesh] failed to retain preview mesh '{preview_mesh_key}': {err:?}");
        }
        if let Some(subsets) = mesh_registry.mesh_subsets(&preview_mesh_key) {
            for subset in subsets {
                if let Some(material_key) = subset.material.as_ref() {
                    match material_registry.retain(material_key) {
                        Ok(()) => {
                            persistent_materials.insert(material_key.clone());
                        }
                        Err(err) => {
                            eprintln!("[material] failed to retain '{material_key}': {err:?}");
                        }
                    }
                }
            }
        }
        let mesh_status_initial =
            Some(format!("Preview mesh: {} - press M to cycle camera control", preview_mesh_key));
        let mut persistent_meshes = HashSet::new();
        persistent_meshes.insert(preview_mesh_key.clone());
        let mesh_orbit = OrbitCamera::new(Vec3::ZERO, 5.0);
        let mesh_camera = mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
        let mesh_frustum_focus = mesh_orbit.target;
        let mesh_frustum_distance = mesh_orbit.radius;
        let mesh_freefly = FreeflyController::from_camera(&mesh_camera);
        let mesh_model = Mat4::IDENTITY;
        let scene_material_refs = HashSet::new();

        // egui context and state
        let egui_ctx = EguiCtx::default();
        let egui_winit = None;
        let scripts = ScriptHost::new("assets/scripts/main.rhai");

        Self {
            renderer,
            ecs,
            time,
            input,
            assets,
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
            ui_hist: Vec::with_capacity(240),
            ui_root_spin: 1.2,
            ui_emitter_rate,
            ui_emitter_spread,
            ui_emitter_speed,
            ui_emitter_lifetime,
            ui_emitter_start_size,
            ui_emitter_end_size,
            ui_emitter_start_color,
            ui_emitter_end_color,
            ui_light_direction: lighting_state.direction,
            ui_light_color: lighting_state.color,
            ui_light_ambient: lighting_state.ambient,
            ui_light_exposure: lighting_state.exposure,
            ui_scale: 1.0,
            ui_scene_path: scene_path,
            ui_scene_status: None,
            scene_dependencies: None,
            scene_history,
            inspector_status: None,
            audio,
            recent_events,
            event_log_limit,
            camera: Camera2D::new(CAMERA_BASE_HALF_HEIGHT),
            viewport_camera_mode: ViewportCameraMode::default(),
            selected_entity: None,
            gizmo_mode: GizmoMode::default(),
            gizmo_interaction: None,
            scene_atlas_refs: HashSet::new(),
            persistent_atlases: HashSet::new(),
            persistent_meshes,
            scene_mesh_refs: HashSet::new(),
            persistent_materials,
            scene_material_refs,
            material_registry,
            mesh_registry,
            preview_mesh_key,
            mesh_orbit,
            mesh_camera,
            mesh_freefly,
            mesh_model,
            mesh_angle: 0.0,
            mesh_control_mode: MeshControlMode::Disabled,
            mesh_freefly_speed: 4.0,
            mesh_freefly_velocity: Vec3::ZERO,
            mesh_freefly_rot_velocity: Vec3::ZERO,
            mesh_frustum_lock: false,
            mesh_frustum_focus,
            mesh_frustum_distance,
            mesh_status: mesh_status_initial,
            viewport: Viewport::new(
                Vec2::ZERO,
                Vec2::new(config.window.width as f32, config.window.height as f32),
            ),
            config,
            emitter_entity: Some(emitter),
            scripts,
            sprite_atlas_views: HashMap::new(),
        }
    }

    fn record_events(&mut self) {
        let events = self.ecs.drain_events();
        if events.is_empty() {
            return;
        }
        for event in events {
            self.audio.handle_event(&event);
            if self.recent_events.len() == self.event_log_limit {
                self.recent_events.pop_front();
            }
            self.recent_events.push_back(event);
        }
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

    fn focus_selection(&mut self) -> bool {
        mesh_preview::focus_selection(self)
    }

    fn update_mesh_camera(&mut self, dt: f32) {
        mesh_preview::update_mesh_camera(self, dt);
    }

    fn set_mesh_control_mode(&mut self, mode: MeshControlMode) {
        mesh_preview::set_mesh_control_mode(self, mode);
    }

    fn set_viewport_camera_mode(&mut self, mode: ViewportCameraMode) {
        mesh_preview::set_viewport_camera_mode(self, mode);
    }

    fn set_frustum_lock(&mut self, enabled: bool) {
        mesh_preview::set_frustum_lock(self, enabled);
    }

    fn handle_mesh_control_input(&mut self) {
        mesh_preview::handle_mesh_control_input(self);
    }

    fn reset_mesh_camera(&mut self) {
        mesh_preview::reset_mesh_camera(self);
    }

    fn set_preview_mesh(&mut self, new_key: String) {
        mesh_preview::set_preview_mesh(self, new_key);
    }

    fn spawn_mesh_entity(&mut self, mesh_key: &str) {
        mesh_preview::spawn_mesh_entity(self, mesh_key);
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
        let previous_materials = self.scene_material_refs.clone();
        let mut next_materials = self.persistent_materials.clone();
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
            if !next_materials.contains(&key) && !self.persistent_materials.contains(&key) {
                self.material_registry.release(&key);
            }
        }
        self.scene_material_refs = next_materials;
        self.scene_dependencies = Some(deps.clone());
        Ok(())
    }

    fn capture_scene_metadata(&self) -> SceneMetadata {
        let mut metadata = SceneMetadata::default();
        metadata.viewport = SceneViewportMode::from(self.viewport_camera_mode);
        metadata.camera2d =
            Some(SceneCamera2D { position: Vec2Data::from(self.camera.position), zoom: self.camera.zoom });
        metadata.preview_camera = Some(mesh_preview::capture_preview_camera(self));
        let lighting = self.renderer.lighting();
        metadata.lighting = Some(SceneLightingData {
            direction: lighting.direction.into(),
            color: lighting.color.into(),
            ambient: lighting.ambient.into(),
            exposure: lighting.exposure,
        });
        metadata
    }

    fn apply_scene_metadata(&mut self, metadata: &SceneMetadata) {
        self.set_viewport_camera_mode(ViewportCameraMode::from(metadata.viewport));
        if let Some(cam2d) = metadata.camera2d.as_ref() {
            self.camera.position = Vec2::from(cam2d.position.clone());
            self.camera.set_zoom(cam2d.zoom);
        }
        if let Some(preview) = metadata.preview_camera.as_ref() {
            mesh_preview::apply_preview_camera(self, preview);
        }
        if let Some(lighting) = metadata.lighting.as_ref() {
            let (mut direction, color, ambient, exposure) = lighting.components();
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
            }
            let renderer_lighting = self.renderer.lighting();
            self.ui_light_direction = renderer_lighting.direction;
            self.ui_light_color = renderer_lighting.color;
            self.ui_light_ambient = renderer_lighting.ambient;
            self.ui_light_exposure = renderer_lighting.exposure;
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
        let mesh_to_release: Vec<String> = self
            .scene_mesh_refs
            .iter()
            .filter(|key| !self.persistent_meshes.contains(*key))
            .cloned()
            .collect();
        for key in &mesh_to_release {
            self.mesh_registry.release_mesh(key);
        }
        self.scene_mesh_refs = self.persistent_meshes.clone();

        let material_to_release: Vec<String> = self
            .scene_material_refs
            .iter()
            .filter(|key| !self.persistent_materials.contains(*key))
            .cloned()
            .collect();
        for key in &material_to_release {
            self.material_registry.release(key);
        }
        self.scene_material_refs = self.persistent_materials.clone();
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
        mesh_preview::mesh_camera_forward(self)
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

        if let Err(err) = self.mesh_registry.ensure_gpu(&self.preview_mesh_key, &mut self.renderer) {
            eprintln!("Failed to upload preview mesh '{}': {err:?}", self.preview_mesh_key);
            self.mesh_status = Some(format!("Mesh upload failed: {err}"));
        }
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

        self.mesh_angle = (self.mesh_angle + dt * 0.5) % (std::f32::consts::TAU);
        self.mesh_model = Mat4::from_rotation_y(self.mesh_angle);
        self.handle_mesh_control_input();
        self.update_mesh_camera(dt);

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

        let window_size = self.renderer.size();
        let viewport_size = self.viewport_physical_size();
        let cursor_screen = self.input.cursor_position().map(|(sx, sy)| Vec2::new(sx, sy));
        let cursor_viewport = cursor_screen.and_then(|pos| self.screen_to_viewport(pos));
        let cursor_world_2d = if self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
            cursor_viewport.and_then(|pos| self.camera.screen_to_world(pos, viewport_size))
        } else {
            None
        };
        let cursor_ray = if self.viewport_camera_mode == ViewportCameraMode::Perspective3D {
            cursor_viewport.and_then(|pos| self.mesh_camera.screen_ray(pos, viewport_size))
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
                mesh_center_world.and_then(|center| self.mesh_camera.project_point(center, viewport_size))
            }
        };
        let prev_selected_entity = self.selected_entity;
        let prev_gizmo_interaction = self.gizmo_interaction;

        if self.viewport_camera_mode == ViewportCameraMode::Ortho2D
            && self.mesh_control_mode == MeshControlMode::Disabled
        {
            if let Some(delta) = self.input.consume_wheel_delta() {
                self.camera.apply_scroll_zoom(delta);
            }

            if self.input.right_held() {
                let (dx, dy) = self.input.mouse_delta;
                if dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON {
                    self.camera.pan_screen_delta(Vec2::new(dx, dy), viewport_size);
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
        let view_proj = self.camera.view_projection(viewport_size);
        let selection_changed = self.selected_entity != prev_selected_entity;
        let gizmo_changed = self.gizmo_interaction != prev_gizmo_interaction;
        selected_info = self.selected_entity.and_then(|entity| self.ecs.entity_info(entity));

        self.ecs.set_spatial_cell(self.ui_cell_size.max(0.05));
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
        self.scripts.update(dt);
        let commands = self.scripts.drain_commands();
        self.apply_script_commands(commands);
        for message in self.scripts.drain_logs() {
            self.ecs.push_event(GameEvent::ScriptMessage { message });
        }

        while self.accumulator >= self.fixed_dt {
            self.ecs.fixed_step(self.fixed_dt);
            self.accumulator -= self.fixed_dt;
        }
        self.ecs.update(dt);
        self.record_events();

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
        let default_material_key = self.material_registry.default_key().to_string();
        let mut mesh_draw_infos: Vec<(String, Mat4, MeshLightingInfo, String)> = Vec::new();
        match self.mesh_registry.ensure_gpu(&self.preview_mesh_key, &mut self.renderer) {
            Ok(_) => {
                let material_key = self.resolve_material_for_mesh(&self.preview_mesh_key, None);
                mesh_draw_infos.push((
                    self.preview_mesh_key.clone(),
                    self.mesh_model,
                    MeshLightingInfo::default(),
                    material_key,
                ));
            }
            Err(err) => {
                self.mesh_status = Some(format!("Mesh upload failed: {err}"));
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
            mesh_draws.push(MeshDraw { mesh, model, lighting, material: material_gpu });
        }
        let mesh_camera_opt = if mesh_draws.is_empty() { None } else { Some(&self.mesh_camera) };
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

        if self.egui_winit.is_none() {
            frame.present();
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
        let dt_ms = dt * 1000.0;
        self.ui_hist.push(dt_ms);
        if self.ui_hist.len() > 240 {
            self.ui_hist.remove(0);
        }

        let hist_points: Vec<[f64; 2]> =
            self.ui_hist.iter().enumerate().map(|(i, v)| [i as f64, *v as f64]).collect();
        let entity_count = self.ecs.entity_count();
        let instances_drawn = instances.len();
        let orbit_target = self.mesh_orbit.target;
        let mesh_camera_for_ui = self.mesh_camera.clone();
        let camera_position = self.camera.position;
        let camera_zoom = self.camera.zoom;
        let recent_events: Vec<GameEvent> = self.recent_events.iter().cloned().collect();
        let audio_triggers: Vec<String> = self.audio.recent_triggers().cloned().collect();
        let audio_enabled = self.audio.enabled();
        let mut mesh_keys: Vec<String> = self.mesh_registry.keys().map(|k| k.to_string()).collect();
        mesh_keys.sort();
        let scene_history_list: Vec<String> = self.scene_history.iter().cloned().collect();
        let atlas_snapshot: Vec<String> = self.scene_atlas_refs.iter().cloned().collect();
        let mesh_snapshot: Vec<String> = self.scene_mesh_refs.iter().cloned().collect();

        let editor_params = editor_ui::EditorUiParams {
            raw_input,
            base_pixels_per_point,
            hist_points,
            entity_count,
            instances_drawn,
            ui_scale: self.ui_scale,
            ui_cell_size: self.ui_cell_size,
            ui_spawn_per_press: self.ui_spawn_per_press,
            ui_auto_spawn_rate: self.ui_auto_spawn_rate,
            ui_root_spin: self.ui_root_spin,
            ui_emitter_rate: self.ui_emitter_rate,
            ui_emitter_spread: self.ui_emitter_spread,
            ui_emitter_speed: self.ui_emitter_speed,
            ui_emitter_lifetime: self.ui_emitter_lifetime,
            ui_emitter_start_size: self.ui_emitter_start_size,
            ui_emitter_end_size: self.ui_emitter_end_size,
            ui_emitter_start_color: self.ui_emitter_start_color,
            ui_emitter_end_color: self.ui_emitter_end_color,
            selected_entity: self.selected_entity,
            selection_details: selected_info.clone(),
            prev_selected_entity,
            prev_gizmo_interaction,
            selection_changed,
            gizmo_changed,
            cursor_screen,
            cursor_world_2d,
            hovered_scale_kind,
            window_size,
            mesh_camera_for_ui,
            camera_position,
            camera_zoom,
            mesh_keys,
            scene_history_list,
            atlas_snapshot,
            mesh_snapshot,
            recent_events,
            audio_triggers,
            audio_enabled,
        };

        let editor_output = self.render_editor_ui(editor_params);
        let editor_ui::EditorUiOutput {
            full_output,
            actions,
            pending_viewport,
            ui_scale,
            ui_cell_size,
            ui_spawn_per_press,
            ui_auto_spawn_rate,
            ui_root_spin,
            ui_emitter_rate,
            ui_emitter_spread,
            ui_emitter_speed,
            ui_emitter_lifetime,
            ui_emitter_start_size,
            ui_emitter_end_size,
            ui_emitter_start_color,
            ui_emitter_end_color,
            selection,
            viewport_mode_request,
            mesh_control_request,
            mesh_frustum_request,
            mesh_frustum_snap,
            mesh_reset_request,
            mesh_selection_request,
            frame_selection_request,
        } = editor_output;

        self.ui_scale = ui_scale;
        self.ui_cell_size = ui_cell_size;
        self.ui_spawn_per_press = ui_spawn_per_press;
        self.ui_auto_spawn_rate = ui_auto_spawn_rate;
        self.ui_root_spin = ui_root_spin;
        self.ui_emitter_rate = ui_emitter_rate;
        self.ui_emitter_spread = ui_emitter_spread;
        self.ui_emitter_speed = ui_emitter_speed;
        self.ui_emitter_lifetime = ui_emitter_lifetime;
        self.ui_emitter_start_size = ui_emitter_start_size;
        self.ui_emitter_end_size = ui_emitter_end_size;
        self.ui_emitter_start_color = ui_emitter_start_color;
        self.ui_emitter_end_color = ui_emitter_end_color;
        self.selected_entity = selection.entity;

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
            mesh_preview::snap_frustum_to_selection(self, selection.details.as_ref(), orbit_target);
        }
        if mesh_reset_request {
            self.reset_mesh_camera();
        }
        if let Some(key) = mesh_selection_request {
            self.set_preview_mesh(key);
        }

        let egui::FullOutput { platform_output, textures_delta, shapes, .. } = full_output;
        if let Some(window) = self.renderer.window() {
            self.egui_winit.as_mut().unwrap().handle_platform_output(window, platform_output);
        } else {
            return;
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
                            self.mesh_status = Some(format!("Mesh upload failed: {err}"));
                        }
                    }
                }
                Err(err) => {
                    self.ui_scene_status = Some(format!("Mesh retain failed: {err}"));
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
                        self.scripts.clear_handles();
                        self.ui_hist.clear();
                        self.sync_emitter_ui();
                        self.inspector_status = None;
                    }
                    Err(err) => {
                        self.ui_scene_status = Some(format!("Load failed: {err}"));
                        self.ecs.clear_world();
                        self.clear_scene_atlases();
                        self.selected_entity = None;
                        self.gizmo_interaction = None;
                        self.scripts.clear_handles();
                        self.sync_emitter_ui();
                        self.inspector_status = None;
                    }
                },
                Err(err) => {
                    self.ui_scene_status = Some(format!("Load failed: {err}"));
                }
            }
        }
        if actions.spawn_now {
            self.ecs.spawn_burst(&self.assets, self.ui_spawn_per_press as usize);
        }
        if let Some(mesh_key) = actions.spawn_mesh {
            self.spawn_mesh_entity(&mesh_key);
        }
        if let Some(entity) = actions.delete_entity {
            if self.ecs.despawn_entity(entity) {
                self.scripts.forget_entity(entity);
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
            self.scripts.clear_handles();
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
            self.scripts.clear_handles();
            self.sync_emitter_ui();
            self.inspector_status = None;
        }
        if let (Some(ren), Some(screen)) = (self.egui_renderer.as_mut(), self.egui_screen.as_ref()) {
            if let (Ok(device), Ok(queue)) = (self.renderer.device(), self.renderer.queue()) {
                for (id, delta) in &textures_delta.set {
                    ren.update_texture(device, queue, *id, delta);
                }
            }
            let meshes = self.egui_ctx.tessellate(shapes, screen.pixels_per_point);
            if let Err(err) = self.renderer.render_egui(ren, &meshes, screen, frame) {
                eprintln!("Egui render error: {err:?}");
            }
            for id in &textures_delta.free {
                ren.free_texture(id);
            }
        } else {
            frame.present();
        }

        self.ecs.set_root_spin(self.ui_root_spin);

        if let Some(w) = self.renderer.window() {
            w.request_redraw();
        }
        self.input.clear_frame();
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
                            self.scripts.register_spawn_result(handle, entity);
                        }
                        Err(err) => {
                            eprintln!("[script] spawn error for {atlas}:{region}: {err}");
                            self.scripts.forget_handle(handle);
                        }
                    }
                }
                ScriptCommand::SetVelocity { handle, velocity } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_velocity(entity, velocity) {
                            eprintln!("[script] set_velocity failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_velocity unknown handle {handle}");
                    }
                }
                ScriptCommand::SetPosition { handle, position } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_translation(entity, position) {
                            eprintln!("[script] set_position failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_position unknown handle {handle}");
                    }
                }
                ScriptCommand::SetRotation { handle, rotation } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_rotation(entity, rotation) {
                            eprintln!("[script] set_rotation failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_rotation unknown handle {handle}");
                    }
                }
                ScriptCommand::SetScale { handle, scale } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_scale(entity, scale) {
                            eprintln!("[script] set_scale failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_scale unknown handle {handle}");
                    }
                }
                ScriptCommand::SetTint { handle, tint } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_tint(entity, tint) {
                            eprintln!("[script] set_tint failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_tint unknown handle {handle}");
                    }
                }
                ScriptCommand::SetSpriteRegion { handle, region } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if !self.ecs.set_sprite_region(entity, &self.assets, &region) {
                            eprintln!("[script] set_sprite_region failed for handle {handle}");
                        }
                    } else {
                        eprintln!("[script] set_sprite_region unknown handle {handle}");
                    }
                }
                ScriptCommand::Despawn { handle } => {
                    if let Some(entity) = self.scripts.resolve_handle(handle) {
                        if self.ecs.despawn_entity(entity) {
                            self.scripts.forget_handle(handle);
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
