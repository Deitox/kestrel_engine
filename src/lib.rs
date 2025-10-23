pub mod assets;
pub mod audio;
pub mod camera;
pub mod camera3d;
pub mod config;
pub mod ecs;
pub mod events;
pub mod input;
pub mod mesh;
mod mesh_preview;
pub mod mesh_registry;
pub mod renderer;
pub mod scene;
pub mod scripts;
pub mod time;

use crate::assets::AssetManager;
use crate::audio::AudioManager;
use crate::camera::Camera2D;
use crate::camera3d::{Camera3D, OrbitCamera};
use crate::config::AppConfig;
use crate::ecs::{EcsWorld, InstanceData, MeshLightingInfo, SpriteInfo};
use crate::events::GameEvent;
use crate::input::{Input, InputEvent};
use crate::mesh_preview::{
    FreeflyController, MeshControlMode, GIZMO_3D_AXIS_LENGTH_SCALE, GIZMO_3D_AXIS_MAX, GIZMO_3D_AXIS_MIN,
    MESH_CAMERA_FAR, MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR,
};
use crate::mesh_registry::MeshRegistry;
use crate::renderer::{MeshDraw, RenderViewport, Renderer, SpriteBatch};
use crate::scene::{SceneCamera2D, SceneDependencies, SceneMetadata, SceneViewportMode, Vec2Data};
use crate::scripts::{ScriptCommand, ScriptHost};
use crate::time::Time;

use bevy_ecs::prelude::Entity;
use glam::{EulerRot, Mat4, Quat, Vec2, Vec3, Vec4};

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
use egui_plot as eplot;
use egui_wgpu::{Renderer as EguiRenderer, RendererOptions, ScreenDescriptor};
use egui_winit::State as EguiWinit;

const CAMERA_BASE_HALF_HEIGHT: f32 = 1.2;
const GIZMO_TRANSLATE_RADIUS_PX: f32 = 18.0;
const GIZMO_SCALE_INNER_RADIUS_PX: f32 = 20.0;
const GIZMO_SCALE_OUTER_RADIUS_PX: f32 = 32.0;
const GIZMO_SCALE_AXIS_LENGTH_PX: f32 = 44.0;
const GIZMO_SCALE_AXIS_THICKNESS_PX: f32 = 8.0;
const GIZMO_SCALE_AXIS_DEADZONE_PX: f32 = 10.0;
const GIZMO_SCALE_HANDLE_SIZE_PX: f32 = 12.0;
const GIZMO_ROTATE_INNER_RADIUS_PX: f32 = 38.0;
const GIZMO_ROTATE_OUTER_RADIUS_PX: f32 = 52.0;
const SCALE_MIN_RATIO: f32 = 0.05;
const SCALE_MAX_RATIO: f32 = 20.0;
const SCALE_SNAP_STEP: f32 = 0.1;
const TRANSLATE_SNAP_STEP: f32 = 0.05;
const ROTATE_SNAP_STEP_RADIANS: f32 = 15.0_f32.to_radians();

#[derive(Clone, Copy, PartialEq, Eq)]
enum GizmoMode {
    Translate,
    Scale,
    Rotate,
}

impl Default for GizmoMode {
    fn default() -> Self {
        GizmoMode::Translate
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewportCameraMode {
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

#[derive(Clone, Copy, PartialEq)]
enum GizmoInteraction {
    Translate {
        entity: Entity,
        offset: Vec2,
        start_translation: Vec2,
        start_pointer: Vec2,
        axis_lock: Option<Axis2>,
    },
    Translate3D {
        entity: Entity,
        offset: Vec3,
        plane_origin: Vec3,
        plane_normal: Vec3,
    },
    Rotate {
        entity: Entity,
        start_rotation: f32,
        start_angle: f32,
    },
    Rotate3D {
        entity: Entity,
        axis: Vec3,
        start_rotation: Quat,
        start_vector: Vec3,
    },
    Scale {
        entity: Entity,
        start_scale: Vec2,
        handle: ScaleHandle,
    },
    Scale3D {
        entity: Entity,
        start_scale: Vec3,
        start_distance: f32,
        plane_normal: Vec3,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis2 {
    X,
    Y,
}

impl Axis2 {
    fn label(self) -> &'static str {
        match self {
            Axis2::X => "X axis",
            Axis2::Y => "Y axis",
        }
    }

    fn vector(self) -> Vec2 {
        match self {
            Axis2::X => Vec2::X,
            Axis2::Y => Vec2::Y,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum ScaleHandle {
    Uniform { start_distance: f32 },
    Axis { axis: Axis2, start_extent: f32 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScaleHandleKind {
    Uniform,
    Axis(Axis2),
}

impl ScaleHandle {
    fn kind(self) -> ScaleHandleKind {
        match self {
            ScaleHandle::Uniform { .. } => ScaleHandleKind::Uniform,
            ScaleHandle::Axis { axis, .. } => ScaleHandleKind::Axis(axis),
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

pub(crate) fn wrap_angle(mut radians: f32) -> f32 {
    let two_pi = 2.0 * std::f32::consts::PI;
    while radians > std::f32::consts::PI {
        radians -= two_pi;
    }
    while radians < -std::f32::consts::PI {
        radians += two_pi;
    }
    radians
}

fn apply_scale_ratio(ratio: f32, snap: bool) -> f32 {
    let clamped = ratio.clamp(SCALE_MIN_RATIO, SCALE_MAX_RATIO);
    if snap {
        let snapped = (clamped / SCALE_SNAP_STEP).round() * SCALE_SNAP_STEP;
        snapped.clamp(SCALE_MIN_RATIO, SCALE_MAX_RATIO)
    } else {
        clamped
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
    renderer: Renderer,
    ecs: EcsWorld,
    time: Time,
    input: Input,
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
    camera: Camera2D,
    viewport_camera_mode: ViewportCameraMode,
    selected_entity: Option<Entity>,
    gizmo_mode: GizmoMode,
    gizmo_interaction: Option<GizmoInteraction>,

    // Configuration
    config: AppConfig,

    scene_atlas_refs: HashSet<String>,
    persistent_atlases: HashSet<String>,
    persistent_meshes: HashSet<String>,
    scene_mesh_refs: HashSet<String>,

    mesh_registry: MeshRegistry,
    preview_mesh_key: String,
    mesh_orbit: OrbitCamera,
    mesh_camera: Camera3D,
    mesh_model: Mat4,
    mesh_angle: f32,
    mesh_control_mode: MeshControlMode,
    mesh_freefly: FreeflyController,
    mesh_freefly_speed: f32,
    mesh_freefly_velocity: Vec3,
    mesh_freefly_rot_velocity: Vec3,
    mesh_frustum_lock: bool,
    mesh_frustum_focus: Vec3,
    mesh_frustum_distance: f32,
    mesh_status: Option<String>,

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

        let mut mesh_registry = MeshRegistry::new();
        let preview_mesh_key = mesh_registry.default_key().to_string();
        if let Err(err) = mesh_registry.retain_mesh(&preview_mesh_key, None) {
            eprintln!("[mesh] failed to retain preview mesh '{preview_mesh_key}': {err:?}");
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

    fn detect_scale_handle(
        &self,
        pointer_world: Vec2,
        pointer_viewport: Vec2,
        center_world: Vec2,
        center_viewport: Vec2,
        shift: bool,
    ) -> Option<(ScaleHandleKind, ScaleHandle)> {
        let rel_view = pointer_viewport - center_viewport;
        let dist = pointer_viewport.distance(center_viewport);
        let axis_half = GIZMO_SCALE_AXIS_THICKNESS_PX * 0.5;
        let axis_length = GIZMO_SCALE_AXIS_LENGTH_PX;
        let deadzone = GIZMO_SCALE_AXIS_DEADZONE_PX;
        let mut kind = None;
        if rel_view.x.abs() >= deadzone && rel_view.x.abs() <= axis_length && rel_view.y.abs() <= axis_half {
            kind = Some(if shift { ScaleHandleKind::Uniform } else { ScaleHandleKind::Axis(Axis2::X) });
        } else if rel_view.y.abs() >= deadzone
            && rel_view.y.abs() <= axis_length
            && rel_view.x.abs() <= axis_half
        {
            kind = Some(if shift { ScaleHandleKind::Uniform } else { ScaleHandleKind::Axis(Axis2::Y) });
        } else if dist >= GIZMO_SCALE_INNER_RADIUS_PX && dist <= GIZMO_SCALE_OUTER_RADIUS_PX {
            kind = Some(ScaleHandleKind::Uniform);
        }
        let kind = kind?;
        let delta_world = pointer_world - center_world;
        match kind {
            ScaleHandleKind::Uniform => {
                let distance = delta_world.length();
                if distance > f32::EPSILON {
                    Some((kind, ScaleHandle::Uniform { start_distance: distance }))
                } else {
                    None
                }
            }
            ScaleHandleKind::Axis(axis) => {
                let extent = delta_world.dot(axis.vector()).abs();
                if extent > f32::EPSILON {
                    Some((kind, ScaleHandle::Axis { axis, start_extent: extent }))
                } else {
                    None
                }
            }
        }
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
                    .ensure_mesh(dep.key(), dep.path())
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
                .retain_mesh(key, None)
                .with_context(|| format!("Failed to retain mesh '{key}'"))?;
        }
        self.scene_mesh_refs = next_mesh;
        self.scene_dependencies = Some(deps.clone());
        Ok(())
    }

    fn capture_scene_metadata(&self) -> SceneMetadata {
        let mut metadata = SceneMetadata::default();
        metadata.viewport = SceneViewportMode::from(self.viewport_camera_mode);
        metadata.camera2d =
            Some(SceneCamera2D { position: Vec2Data::from(self.camera.position), zoom: self.camera.zoom });
        metadata.preview_camera = Some(mesh_preview::capture_preview_camera(self));
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
        for key in std::mem::take(&mut self.scene_mesh_refs) {
            self.mesh_registry.release_mesh(&key);
        }
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

        let view_proj = self.camera.view_projection(viewport_size);
        let shift_held = self.input.shift_held();
        let hovered_scale_kind = if self.viewport_camera_mode == ViewportCameraMode::Ortho2D
            && self.gizmo_mode == GizmoMode::Scale
        {
            if let (Some(info), Some(center_viewport), Some(pointer_viewport), Some(pointer_world)) =
                (selected_info.as_ref(), gizmo_center_viewport, cursor_viewport, cursor_world_2d)
            {
                self.detect_scale_handle(
                    pointer_world,
                    pointer_viewport,
                    info.translation,
                    center_viewport,
                    shift_held,
                )
                .map(|(kind, _)| kind)
            } else {
                None
            }
        } else {
            None
        };

        let mut gizmo_click_consumed = false;
        if self.input.take_left_click() {
            if let Some(entity) = self.selected_entity {
                match self.viewport_camera_mode {
                    ViewportCameraMode::Perspective3D => {
                        if let Some(center_world) = mesh_center_world {
                            match self.gizmo_mode {
                                GizmoMode::Translate => {
                                    if let Some((ray_origin, ray_dir)) = cursor_ray {
                                        let plane_normal = self.mesh_camera_forward();
                                        if plane_normal.length_squared() > f32::EPSILON {
                                            if let Some(hit) = App::intersect_ray_plane(
                                                ray_origin,
                                                ray_dir,
                                                center_world,
                                                plane_normal,
                                            ) {
                                                let offset = center_world - hit;
                                                self.gizmo_interaction =
                                                    Some(GizmoInteraction::Translate3D {
                                                        entity,
                                                        offset,
                                                        plane_origin: center_world,
                                                        plane_normal,
                                                    });
                                                gizmo_click_consumed = true;
                                                self.inspector_status = None;
                                            }
                                        }
                                    }
                                }
                                GizmoMode::Scale => {
                                    if let (
                                        Some(center_viewport),
                                        Some(pointer_viewport),
                                        Some((ray_origin, ray_dir)),
                                    ) = (gizmo_center_viewport, cursor_viewport, cursor_ray)
                                    {
                                        let dist = pointer_viewport.distance(center_viewport);
                                        if dist <= GIZMO_SCALE_OUTER_RADIUS_PX {
                                            let plane_normal = self.mesh_camera_forward();
                                            if let Some(hit) = App::intersect_ray_plane(
                                                ray_origin,
                                                ray_dir,
                                                center_world,
                                                plane_normal,
                                            ) {
                                                let start_vec = hit - center_world;
                                                let start_distance = start_vec.length();
                                                if start_distance > f32::EPSILON {
                                                    let start_scale = selected_info
                                                        .as_ref()
                                                        .and_then(|info| {
                                                            info.mesh_transform.as_ref().map(|tx| tx.scale)
                                                        })
                                                        .unwrap_or(Vec3::splat(1.0));
                                                    self.gizmo_interaction =
                                                        Some(GizmoInteraction::Scale3D {
                                                            entity,
                                                            start_scale,
                                                            start_distance,
                                                            plane_normal,
                                                        });
                                                    gizmo_click_consumed = true;
                                                    self.inspector_status = None;
                                                }
                                            }
                                        }
                                    }
                                }
                                GizmoMode::Rotate => {
                                    if let (
                                        Some(center_viewport),
                                        Some(pointer_viewport),
                                        Some((ray_origin, ray_dir)),
                                    ) = (gizmo_center_viewport, cursor_viewport, cursor_ray)
                                    {
                                        let dist = pointer_viewport.distance(center_viewport);
                                        if dist >= GIZMO_ROTATE_INNER_RADIUS_PX
                                            && dist <= GIZMO_ROTATE_OUTER_RADIUS_PX
                                        {
                                            let plane_normal = self.mesh_camera_forward();
                                            if let Some(hit) = App::intersect_ray_plane(
                                                ray_origin,
                                                ray_dir,
                                                center_world,
                                                plane_normal,
                                            ) {
                                                let start_vec = hit - center_world;
                                                if start_vec.length_squared() > f32::EPSILON {
                                                    let start_rotation = selected_info
                                                        .as_ref()
                                                        .and_then(|info| {
                                                            info.mesh_transform.as_ref().map(|tx| tx.rotation)
                                                        })
                                                        .unwrap_or(Quat::IDENTITY);
                                                    self.gizmo_interaction =
                                                        Some(GizmoInteraction::Rotate3D {
                                                            entity,
                                                            axis: plane_normal.normalize_or_zero(),
                                                            start_rotation,
                                                            start_vector: start_vec,
                                                        });
                                                    gizmo_click_consumed = true;
                                                    self.inspector_status = None;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    ViewportCameraMode::Ortho2D => match self.gizmo_mode {
                        GizmoMode::Translate => {
                            if let (Some(center_viewport), Some(pointer_viewport)) =
                                (gizmo_center_viewport, cursor_viewport)
                            {
                                let dist = pointer_viewport.distance(center_viewport);
                                if dist <= GIZMO_TRANSLATE_RADIUS_PX {
                                    if let Some(pointer_world) = cursor_world_2d {
                                        let offset = selected_info
                                            .as_ref()
                                            .map(|info| info.translation - pointer_world)
                                            .unwrap_or(Vec2::ZERO);
                                        if let Some(info) = selected_info.as_ref() {
                                            self.gizmo_interaction = Some(GizmoInteraction::Translate {
                                                entity,
                                                offset,
                                                start_translation: info.translation,
                                                start_pointer: pointer_world,
                                                axis_lock: None,
                                            });
                                        }
                                        gizmo_click_consumed = true;
                                        self.inspector_status = None;
                                    }
                                }
                            }
                        }
                        GizmoMode::Scale => {
                            if let (
                                Some(pointer_world),
                                Some(pointer_viewport),
                                Some(info),
                                Some(center_viewport),
                            ) = (
                                cursor_world_2d,
                                cursor_viewport,
                                selected_info.as_ref(),
                                gizmo_center_viewport,
                            ) {
                                if let Some((_kind, handle)) = self.detect_scale_handle(
                                    pointer_world,
                                    pointer_viewport,
                                    info.translation,
                                    center_viewport,
                                    shift_held,
                                ) {
                                    self.gizmo_interaction = Some(GizmoInteraction::Scale {
                                        entity,
                                        start_scale: info.scale,
                                        handle,
                                    });
                                    gizmo_click_consumed = true;
                                    self.inspector_status = None;
                                }
                            }
                        }
                        GizmoMode::Rotate => {
                            if let (Some(center_viewport), Some(pointer_viewport)) =
                                (gizmo_center_viewport, cursor_viewport)
                            {
                                let dist = pointer_viewport.distance(center_viewport);
                                if dist >= GIZMO_ROTATE_INNER_RADIUS_PX
                                    && dist <= GIZMO_ROTATE_OUTER_RADIUS_PX
                                {
                                    if let (Some(pointer_world), Some(info)) =
                                        (cursor_world_2d, selected_info.as_ref())
                                    {
                                        let center = info.translation;
                                        let vec = pointer_world - center;
                                        if vec.length_squared() > f32::EPSILON {
                                            let start_angle = vec.y.atan2(vec.x);
                                            self.gizmo_interaction = Some(GizmoInteraction::Rotate {
                                                entity,
                                                start_rotation: info.rotation,
                                                start_angle,
                                            });
                                            gizmo_click_consumed = true;
                                            self.inspector_status = None;
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }

            if !gizmo_click_consumed {
                match self.viewport_camera_mode {
                    ViewportCameraMode::Perspective3D => {
                        if let Some((ray_origin, ray_dir)) = cursor_ray {
                            let mut picked =
                                self.ecs.pick_entity_3d(ray_origin, ray_dir, &self.mesh_registry);
                            if picked.is_none() {
                                if let Some(hit) =
                                    App::intersect_ray_plane(ray_origin, ray_dir, Vec3::ZERO, Vec3::Z)
                                {
                                    picked = self.ecs.pick_entity(hit.truncate());
                                }
                            }
                            self.selected_entity = picked;
                            if self.selected_entity.is_some() {
                                self.inspector_status = None;
                            }
                        } else if cursor_in_viewport {
                            self.selected_entity = None;
                            self.inspector_status = None;
                        }
                    }
                    ViewportCameraMode::Ortho2D => {
                        if let Some(world) = cursor_world_2d {
                            self.selected_entity = self.ecs.pick_entity(world);
                            self.inspector_status = None;
                        } else if cursor_in_viewport {
                            self.selected_entity = None;
                            self.inspector_status = None;
                        }
                    }
                }
                if cursor_in_viewport {
                    self.gizmo_interaction = None;
                }
            }
        }

        if self.selected_entity.is_none() {
            self.gizmo_interaction = None;
        }

        if let Some(interaction) = self.gizmo_interaction.as_mut() {
            let mut keep_active = true;
            match interaction {
                GizmoInteraction::Translate {
                    entity,
                    offset,
                    start_translation,
                    start_pointer,
                    axis_lock,
                } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some(pointer_world) = cursor_world_2d {
                        if self.ecs.entity_exists(*entity) {
                            let mut current_axis = None;
                            if self.input.shift_held() {
                                let delta = pointer_world - *start_pointer;
                                if delta.length_squared() > f32::EPSILON {
                                    current_axis = Some(if delta.x.abs() >= delta.y.abs() {
                                        Axis2::X
                                    } else {
                                        Axis2::Y
                                    });
                                }
                            }
                            *axis_lock = current_axis;
                            let mut translation = if let Some(axis) = current_axis {
                                let delta = pointer_world - *start_pointer;
                                let mut result = *start_translation;
                                match axis {
                                    Axis2::X => result.x += delta.x,
                                    Axis2::Y => result.y += delta.y,
                                }
                                result
                            } else {
                                pointer_world + *offset
                            };
                            if self.input.ctrl_held() {
                                match current_axis {
                                    Some(Axis2::X) => {
                                        translation.x = (translation.x / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                    }
                                    Some(Axis2::Y) => {
                                        translation.y = (translation.y / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                    }
                                    None => {
                                        translation.x = (translation.x / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                        translation.y = (translation.y / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                    }
                                }
                            }
                            self.ecs.set_translation(*entity, translation);
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Translate3D { entity, offset, plane_origin, plane_normal } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some((ray_origin, ray_dir)) = cursor_ray {
                        if let Some(hit) =
                            App::intersect_ray_plane(ray_origin, ray_dir, *plane_origin, *plane_normal)
                        {
                            if self.ecs.entity_exists(*entity) {
                                let mut translation = hit + *offset;
                                if self.input.ctrl_held() {
                                    translation.x =
                                        (translation.x / TRANSLATE_SNAP_STEP).round() * TRANSLATE_SNAP_STEP;
                                    translation.y =
                                        (translation.y / TRANSLATE_SNAP_STEP).round() * TRANSLATE_SNAP_STEP;
                                    translation.z =
                                        (translation.z / TRANSLATE_SNAP_STEP).round() * TRANSLATE_SNAP_STEP;
                                }
                                self.ecs.set_mesh_translation(*entity, translation);
                                self.ecs.set_translation(*entity, translation.truncate());
                            } else {
                                keep_active = false;
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Rotate { entity, start_rotation, start_angle } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some(pointer_world) = cursor_world_2d {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let vec = pointer_world - info.translation;
                            if vec.length_squared() > f32::EPSILON {
                                let current_angle = vec.y.atan2(vec.x);
                                let mut delta = wrap_angle(current_angle - *start_angle);
                                if self.input.ctrl_held() {
                                    delta =
                                        (delta / ROTATE_SNAP_STEP_RADIANS).round() * ROTATE_SNAP_STEP_RADIANS;
                                }
                                self.ecs.set_rotation(*entity, *start_rotation + delta);
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Rotate3D { entity, axis, start_rotation, start_vector } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some((ray_origin, ray_dir)) = cursor_ray {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let center = info
                                .mesh_transform
                                .as_ref()
                                .map(|tx| tx.translation)
                                .unwrap_or(Vec3::new(info.translation.x, info.translation.y, 0.0));
                            if let Some(hit) = App::intersect_ray_plane(ray_origin, ray_dir, center, *axis) {
                                let start_vec = start_vector.normalize_or_zero();
                                let current_vec = (hit - center).normalize_or_zero();
                                if start_vec.length_squared() > f32::EPSILON
                                    && current_vec.length_squared() > f32::EPSILON
                                {
                                    let axis_norm = axis.normalize_or_zero();
                                    if axis_norm.length_squared() > f32::EPSILON {
                                        let dot = start_vec.dot(current_vec).clamp(-1.0, 1.0);
                                        let cross = start_vec.cross(current_vec);
                                        let sin = cross.dot(axis_norm);
                                        let mut delta = sin.atan2(dot);
                                        if self.input.ctrl_held() {
                                            delta = (delta / ROTATE_SNAP_STEP_RADIANS).round()
                                                * ROTATE_SNAP_STEP_RADIANS;
                                        }
                                        if delta.abs() > f32::EPSILON {
                                            let quat =
                                                Quat::from_axis_angle(axis_norm, delta) * *start_rotation;
                                            let (x, y, z) = quat.to_euler(EulerRot::XYZ);
                                            self.ecs.set_mesh_rotation_euler(*entity, Vec3::new(x, y, z));
                                        }
                                    }
                                }
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Scale { entity, start_scale, handle } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some(pointer_world) = cursor_world_2d {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let center = info.translation;
                            let mut new_scale = *start_scale;
                            let snap = self.input.ctrl_held();
                            match handle {
                                ScaleHandle::Uniform { start_distance } => {
                                    let delta = pointer_world - center;
                                    let len_sq = delta.length_squared();
                                    if len_sq > f32::EPSILON && *start_distance > f32::EPSILON {
                                        let distance = len_sq.sqrt();
                                        let ratio = apply_scale_ratio(distance / *start_distance, snap);
                                        new_scale = Vec2::new(
                                            (start_scale.x * ratio).max(0.01),
                                            (start_scale.y * ratio).max(0.01),
                                        );
                                    }
                                }
                                ScaleHandle::Axis { axis, start_extent } => {
                                    let axis_vec = axis.vector();
                                    let extent = (pointer_world - center).dot(axis_vec).abs();
                                    if extent > f32::EPSILON && *start_extent > f32::EPSILON {
                                        let ratio = apply_scale_ratio(extent / *start_extent, snap);
                                        match axis {
                                            Axis2::X => {
                                                new_scale.x = (start_scale.x * ratio).max(0.01);
                                                if self.input.shift_held() {
                                                    new_scale.y = (start_scale.y * ratio).max(0.01);
                                                }
                                            }
                                            Axis2::Y => {
                                                new_scale.y = (start_scale.y * ratio).max(0.01);
                                                if self.input.shift_held() {
                                                    new_scale.x = (start_scale.x * ratio).max(0.01);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if new_scale != *start_scale {
                                self.ecs.set_scale(*entity, new_scale);
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Scale3D { entity, start_scale, start_distance, plane_normal } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some((ray_origin, ray_dir)) = cursor_ray {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let center = info
                                .mesh_transform
                                .as_ref()
                                .map(|tx| tx.translation)
                                .unwrap_or(Vec3::new(info.translation.x, info.translation.y, 0.0));
                            if let Some(hit) =
                                App::intersect_ray_plane(ray_origin, ray_dir, center, *plane_normal)
                            {
                                let distance = (hit - center).length();
                                if distance > f32::EPSILON && *start_distance > f32::EPSILON {
                                    let ratio =
                                        apply_scale_ratio(distance / *start_distance, self.input.ctrl_held());
                                    let mut new_scale = Vec3::new(
                                        (start_scale.x * ratio).max(0.01),
                                        (start_scale.y * ratio).max(0.01),
                                        (start_scale.z * ratio).max(0.01),
                                    );
                                    if self.input.shift_held() {
                                        let uniform = new_scale.x.max(new_scale.y).max(new_scale.z);
                                        new_scale = Vec3::splat(uniform);
                                    }
                                    if (new_scale - *start_scale).length_squared() > f32::EPSILON {
                                        self.ecs.set_mesh_scale(*entity, new_scale);
                                        self.ecs.set_scale(*entity, Vec2::new(new_scale.x, new_scale.y));
                                    }
                                }
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
            }
            if !keep_active {
                self.gizmo_interaction = None;
            }
        }

        let mut selection_changed = self.selected_entity != prev_selected_entity;
        let mut gizmo_changed = self.gizmo_interaction != prev_gizmo_interaction;
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
        let mut mesh_draw_infos: Vec<(String, Mat4, MeshLightingInfo)> = Vec::new();
        match self.mesh_registry.ensure_gpu(&self.preview_mesh_key, &mut self.renderer) {
            Ok(_) => mesh_draw_infos.push((
                self.preview_mesh_key.clone(),
                self.mesh_model,
                MeshLightingInfo::default(),
            )),
            Err(err) => {
                self.mesh_status = Some(format!("Mesh upload failed: {err}"));
            }
        }
        let scene_meshes = self.ecs.collect_mesh_instances();
        for instance in scene_meshes {
            match self.mesh_registry.ensure_gpu(&instance.key, &mut self.renderer) {
                Ok(_) => {
                    mesh_draw_infos.push((instance.key.clone(), instance.model, instance.lighting.clone()))
                }
                Err(err) => {
                    eprintln!("[mesh] Unable to prepare '{}': {err:?}", instance.key);
                }
            }
        }
        let mut mesh_draws: Vec<MeshDraw> = Vec::new();
        for (key, model, lighting) in mesh_draw_infos {
            if let Some(mesh) = self.mesh_registry.gpu_mesh(&key) {
                mesh_draws.push(MeshDraw { mesh, model, lighting });
            }
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
        let mut ui_pixels_per_point = self.egui_ctx.pixels_per_point();
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
        let mut ui_cell_size = self.ui_cell_size;
        let mut ui_spawn_per_press = self.ui_spawn_per_press;
        let orbit_target = self.mesh_orbit.target;
        let mut ui_auto_spawn_rate = self.ui_auto_spawn_rate;
        let mut ui_root_spin = self.ui_root_spin;
        let mut ui_emitter_rate = self.ui_emitter_rate;
        let mut ui_emitter_spread = self.ui_emitter_spread;
        let mut ui_emitter_speed = self.ui_emitter_speed;
        let mut ui_emitter_lifetime = self.ui_emitter_lifetime;
        let mut ui_emitter_start_size = self.ui_emitter_start_size;
        let mut ui_emitter_end_size = self.ui_emitter_end_size;
        let mut ui_emitter_start_color = self.ui_emitter_start_color;
        let mut ui_emitter_end_color = self.ui_emitter_end_color;
        let mut selected_entity = self.selected_entity;
        let mut selection_details = selected_info.clone();
        let camera_position = self.camera.position;
        let camera_zoom = self.camera.zoom;
        let mesh_camera_for_ui = self.mesh_camera.clone();
        let recent_events: Vec<GameEvent> = self.recent_events.iter().cloned().collect();
        let audio_triggers: Vec<String> = self.audio.recent_triggers().cloned().collect();
        let mut audio_enabled = self.audio.enabled();

        #[derive(Default)]
        struct UiActions {
            spawn_now: bool,
            delete_entity: Option<Entity>,
            clear_particles: bool,
            reset_world: bool,
            save_scene: bool,
            load_scene: bool,
            spawn_mesh: Option<String>,
            retain_atlases: Vec<(String, Option<String>)>,
            retain_meshes: Vec<(String, Option<String>)>,
        }
        let mut actions = UiActions::default();
        let mut viewport_mode_request: Option<ViewportCameraMode> = None;
        let mut mesh_control_request: Option<MeshControlMode> = None;
        let mut mesh_frustum_request: Option<bool> = None;
        let mut mesh_reset_request = false;
        let mut mesh_selection_request: Option<String> = None;
        let mut frame_selection_request = false;
        let mut mesh_keys: Vec<String> = self.mesh_registry.keys().map(|k| k.to_string()).collect();
        mesh_keys.sort();
        let scene_history_list: Vec<String> = self.scene_history.iter().cloned().collect();
        let atlas_snapshot: Vec<String> = self.scene_atlas_refs.iter().cloned().collect();
        let mesh_snapshot: Vec<String> = self.scene_mesh_refs.iter().cloned().collect();
        let mut pending_viewport: Option<(Vec2, Vec2)> = None;
        let mut left_panel_width_px = 0.0;
        let mut right_panel_width_px = 0.0;

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            let left_panel =
                egui::SidePanel::left("kestrel_left_panel").default_width(340.0).show(ctx, |ui| {
                    egui::CollapsingHeader::new("Stats").default_open(true).show(ui, |ui| {
                        ui.label(format!("Entities: {}", entity_count));
                        ui.label(format!("Instances drawn: {}", instances_drawn));
                        ui.separator();
                        ui.label("Frame time (ms)");
                        let hist = eplot::Plot::new("fps_plot").height(120.0).include_y(0.0).include_y(40.0);
                        hist.show(ui, |plot_ui| {
                            plot_ui.line(eplot::Line::new(
                                "ms/frame",
                                eplot::PlotPoints::from(hist_points.clone()),
                            ));
                        });
                        ui.label("Target: 16.7ms for 60 FPS");
                    });

                    egui::CollapsingHeader::new("UI & Camera").default_open(true).show(ui, |ui| {
                        if ui.add(egui::Slider::new(&mut self.ui_scale, 0.5..=2.0).text("UI scale")).changed()
                        {
                            self.ui_scale = self.ui_scale.clamp(0.5, 2.0);
                            self.egui_ctx.set_pixels_per_point(base_pixels_per_point * self.ui_scale);
                            if let Some(screen) = self.egui_screen.as_mut() {
                                screen.pixels_per_point = self.egui_ctx.pixels_per_point();
                            }
                            ui_pixels_per_point = self.egui_ctx.pixels_per_point();
                        }
                        let mut viewport_mode = self.viewport_camera_mode;
                        egui::ComboBox::from_id_salt("viewport_mode")
                            .selected_text(viewport_mode.label())
                            .show_ui(ui, |ui| {
                                for mode in [ViewportCameraMode::Ortho2D, ViewportCameraMode::Perspective3D] {
                                    if ui.selectable_label(viewport_mode == mode, mode.label()).clicked() {
                                        viewport_mode = mode;
                                    }
                                }
                            });
                        if viewport_mode != self.viewport_camera_mode {
                            viewport_mode_request = Some(viewport_mode);
                        }
                        ui.label(format!(
                            "Camera: pos({:.2}, {:.2}) zoom {:.2}",
                            camera_position.x, camera_position.y, camera_zoom
                        ));
                        if self.viewport_camera_mode == ViewportCameraMode::Perspective3D {
                            let pos = mesh_camera_for_ui.position;
                            ui.label(format!("3D camera pos: ({:.2}, {:.2}, {:.2})", pos.x, pos.y, pos.z));
                        }
                        let display_mode =
                            if self.config.window.fullscreen { "Fullscreen" } else { "Windowed" };
                        ui.label(format!(
                            "Display: {}x{} {}",
                            self.config.window.width, self.config.window.height, display_mode
                        ));
                        ui.label(format!("VSync: {}", if self.config.window.vsync { "On" } else { "Off" }));
                        if let Some(cursor) = cursor_world_2d {
                            ui.label(format!("Cursor world: ({:.2}, {:.2})", cursor.x, cursor.y));
                        } else {
                            ui.label("Cursor world: n/a");
                        }
                    });

                    egui::CollapsingHeader::new("Spawn & Emitters").default_open(true).show(ui, |ui| {
                        ui.add(egui::Slider::new(&mut ui_cell_size, 0.05..=0.8).text("Spatial cell size"));
                        ui.add(egui::Slider::new(&mut ui_spawn_per_press, 1..=5000).text("Spawn per press"));
                        ui.add(
                            egui::Slider::new(&mut ui_auto_spawn_rate, 0.0..=5000.0)
                                .text("Auto-spawn per second"),
                        );
                        ui.add(
                            egui::Slider::new(&mut ui_emitter_rate, 0.0..=200.0)
                                .text("Emitter rate (particles/s)"),
                        );
                        ui.add(
                            egui::Slider::new(&mut ui_emitter_spread, 0.0..=std::f32::consts::PI)
                                .text("Emitter spread (rad)"),
                        );
                        ui.add(egui::Slider::new(&mut ui_emitter_speed, 0.0..=3.0).text("Emitter speed"));
                        ui.add(
                            egui::Slider::new(&mut ui_emitter_lifetime, 0.1..=5.0)
                                .text("Particle lifetime (s)"),
                        );
                        ui.add(
                            egui::Slider::new(&mut ui_emitter_start_size, 0.01..=0.5)
                                .text("Particle start size"),
                        );
                        ui.add(
                            egui::Slider::new(&mut ui_emitter_end_size, 0.01..=0.5).text("Particle end size"),
                        );
                        ui.horizontal(|ui| {
                            ui.label("Start color");
                            ui.color_edit_button_rgba_unmultiplied(&mut ui_emitter_start_color);
                        });
                        ui.horizontal(|ui| {
                            ui.label("End color");
                            ui.color_edit_button_rgba_unmultiplied(&mut ui_emitter_end_color);
                        });
                        ui.add(egui::Slider::new(&mut ui_root_spin, -5.0..=5.0).text("Root spin speed"));
                        ui.horizontal(|ui| {
                            if ui.button("Spawn now").clicked() {
                                actions.spawn_now = true;
                            }
                            if ui.button("Clear particles").clicked() {
                                actions.clear_particles = true;
                            }
                            if ui.button("Reset world").clicked() {
                                actions.reset_world = true;
                            }
                        });
                    });

                    egui::CollapsingHeader::new("Scripts").default_open(false).show(ui, |ui| {
                        ui.label(format!("Path: {}", self.scripts.script_path().display()));
                        let mut scripts_enabled = self.scripts.enabled();
                        if ui.checkbox(&mut scripts_enabled, "Enable scripts").changed() {
                            self.scripts.set_enabled(scripts_enabled);
                        }
                        if ui.button("Reload script").clicked() {
                            if let Err(err) = self.scripts.force_reload() {
                                self.scripts.set_error_message(err.to_string());
                            }
                        }
                        if let Some(err) = self.scripts.last_error() {
                            ui.colored_label(egui::Color32::RED, format!("Error: {err}"));
                        } else if self.scripts.enabled() {
                            ui.label("Script running");
                        } else {
                            ui.label("Scripts disabled");
                        }
                    });
                });

            let right_panel =
                egui::SidePanel::right("kestrel_right_panel").default_width(360.0).show(ctx, |ui| {
                    ui.heading("3D Preview");
                    egui::ComboBox::from_label("Mesh asset").selected_text(&self.preview_mesh_key).show_ui(
                        ui,
                        |ui| {
                            for key in &mesh_keys {
                                let selected = self.preview_mesh_key == *key;
                                if ui.selectable_label(selected, key).clicked() && !selected {
                                    mesh_selection_request = Some(key.clone());
                                }
                            }
                        },
                    );
                    let mut mesh_control_mode = self.mesh_control_mode;
                    egui::ComboBox::from_id_salt("mesh_control_mode")
                        .selected_text(mesh_control_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in
                                [MeshControlMode::Disabled, MeshControlMode::Orbit, MeshControlMode::Freefly]
                            {
                                if ui.selectable_label(mesh_control_mode == mode, mode.label()).clicked() {
                                    mesh_control_mode = mode;
                                }
                            }
                        });
                    if mesh_control_mode != self.mesh_control_mode {
                        mesh_control_request = Some(mesh_control_mode);
                    }
                    let mut frustum_lock = self.mesh_frustum_lock;
                    if ui.checkbox(&mut frustum_lock, "Frustum lock (L)").changed() {
                        mesh_frustum_request = Some(frustum_lock);
                    }
                    if frustum_lock && ui.button("Snap to selection").clicked() {
                        let focus = selection_details
                            .as_ref()
                            .and_then(|info| info.mesh_transform.as_ref().map(|t| t.translation))
                            .or_else(|| {
                                selection_details
                                    .as_ref()
                                    .map(|info| Vec3::new(info.translation.x, info.translation.y, 0.0))
                            })
                            .unwrap_or(orbit_target);
                        self.mesh_frustum_focus = focus;
                        self.mesh_frustum_distance =
                            (self.mesh_camera.position - self.mesh_frustum_focus).length().max(0.1);
                        self.mesh_status = Some("Frustum focus updated.".to_string());
                    }
                    if ui.button("Reset camera").clicked() {
                        mesh_reset_request = true;
                    }
                    if ui.button("Spawn mesh entity").clicked() {
                        actions.spawn_mesh = Some(self.preview_mesh_key.clone());
                    }
                    match self.mesh_control_mode {
                        MeshControlMode::Orbit => {
                            ui.label(format!("Orbit radius: {:.2}", self.mesh_orbit.radius));
                        }
                        MeshControlMode::Freefly => {
                            ui.label(format!("Free-fly speed: {:.2}", self.mesh_freefly_speed));
                        }
                        MeshControlMode::Disabled => {
                            ui.label(format!("Orbit radius: {:.2}", self.mesh_orbit.radius));
                        }
                    }
                    if let Some(status) = &self.mesh_status {
                        ui.label(status);
                    } else {
                        match self.mesh_control_mode {
                            MeshControlMode::Disabled => {
                                ui.label("Scripted orbit animates the camera.");
                            }
                            MeshControlMode::Orbit => {
                                ui.label("Right drag to orbit, scroll to zoom.");
                            }
                            MeshControlMode::Freefly => {
                                ui.label("Hold RMB to look, use WASD/QE and Shift for boost.");
                            }
                        }
                    }

                    ui.separator();
                    if let Some(entity) = selected_entity {
                        ui.heading("Entity Inspector");
                        ui.label(format!("Entity: {:?}", entity));
                        ui.horizontal(|ui| {
                            ui.label("Gizmo");
                            ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Translate, "Translate");
                            ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Rotate, "Rotate");
                            ui.selectable_value(&mut self.gizmo_mode, GizmoMode::Scale, "Scale");
                        });
                        match self.gizmo_mode {
                            GizmoMode::Scale => {
                                ui.small("Shift = uniform scale, Ctrl = snap steps");
                            }
                            GizmoMode::Translate => {
                                ui.small("Shift = lock axis, Ctrl = snap to grid");
                            }
                            GizmoMode::Rotate => {
                                ui.small("Ctrl = snap to 15° increments");
                            }
                        }
                        if let Some(interaction) = &self.gizmo_interaction {
                            match interaction {
                                GizmoInteraction::Translate { axis_lock, .. } => {
                                    let mut msg = String::from("Translate gizmo active");
                                    if let Some(axis) = axis_lock {
                                        msg.push_str(&format!(" ({} axis)", axis.label()));
                                    }
                                    if self.input.ctrl_held() {
                                        msg.push_str(" [snap]");
                                    }
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                                }
                                GizmoInteraction::Translate3D { .. } => {
                                    let msg = if self.input.ctrl_held() {
                                        "3D translate gizmo active [snap]"
                                    } else {
                                        "3D translate gizmo active"
                                    };
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                                }
                                GizmoInteraction::Rotate { .. } => {
                                    let msg = if self.input.ctrl_held() {
                                        "Rotate gizmo active [snap]"
                                    } else {
                                        "Rotate gizmo active"
                                    };
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                                }
                                GizmoInteraction::Scale { handle, .. } => {
                                    match handle {
                                        ScaleHandle::Uniform { .. } => ui.colored_label(
                                            egui::Color32::LIGHT_GREEN,
                                            if self.input.ctrl_held() {
                                                "Scale gizmo active (uniform) [snap]"
                                            } else {
                                                "Scale gizmo active (uniform)"
                                            },
                                        ),
                                        ScaleHandle::Axis { axis, .. } => ui.colored_label(
                                            egui::Color32::LIGHT_GREEN,
                                            if self.input.ctrl_held() {
                                                format!("Scale gizmo active ({}) [snap]", axis.label())
                                            } else {
                                                format!("Scale gizmo active ({})", axis.label())
                                            },
                                        ),
                                    };
                                }
                                GizmoInteraction::Rotate3D { .. } => {
                                    let msg = if self.input.ctrl_held() {
                                        "3D rotate gizmo active [snap]"
                                    } else {
                                        "3D rotate gizmo active"
                                    };
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                                }
                                GizmoInteraction::Scale3D { .. } => {
                                    let mut msg = String::from("3D scale gizmo active");
                                    if self.input.shift_held() {
                                        msg.push_str(" (uniform)");
                                    }
                                    if self.input.ctrl_held() {
                                        msg.push_str(" [snap]");
                                    }
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                                }
                            }
                        }
                        let mut inspector_refresh = false;
                        let mut inspector_info = selection_details.clone();
                        if let Some(mut info) = inspector_info {
                            let mut translation = info.translation;
                            ui.horizontal(|ui| {
                                ui.label("Position");
                                if ui.add(egui::DragValue::new(&mut translation.x).speed(0.01)).changed()
                                    | ui.add(egui::DragValue::new(&mut translation.y).speed(0.01)).changed()
                                {
                                    if self.ecs.set_translation(entity, translation) {
                                        info.translation = translation;
                                        inspector_refresh = true;
                                        self.inspector_status = None;
                                    }
                                }
                            });

                            let mut rotation_deg = info.rotation.to_degrees();
                            if ui
                                .add(egui::DragValue::new(&mut rotation_deg).speed(1.0).suffix(" deg"))
                                .changed()
                            {
                                let rotation_rad = rotation_deg.to_radians();
                                if self.ecs.set_rotation(entity, rotation_rad) {
                                    info.rotation = rotation_rad;
                                    inspector_refresh = true;
                                    self.inspector_status = None;
                                }
                            }

                            let mut scale = info.scale;
                            ui.horizontal(|ui| {
                                ui.label("Scale");
                                if ui.add(egui::DragValue::new(&mut scale.x).speed(0.01)).changed()
                                    | ui.add(egui::DragValue::new(&mut scale.y).speed(0.01)).changed()
                                {
                                    let clamped = Vec2::new(scale.x.max(0.01), scale.y.max(0.01));
                                    if self.ecs.set_scale(entity, clamped) {
                                        info.scale = clamped;
                                        inspector_refresh = true;
                                        self.inspector_status = None;
                                    }
                                }
                            });

                            if let Some(mut velocity) = info.velocity {
                                ui.horizontal(|ui| {
                                    ui.label("Velocity");
                                    if ui.add(egui::DragValue::new(&mut velocity.x).speed(0.01)).changed()
                                        | ui.add(egui::DragValue::new(&mut velocity.y).speed(0.01)).changed()
                                    {
                                        if self.ecs.set_velocity(entity, velocity) {
                                            info.velocity = Some(velocity);
                                            inspector_refresh = true;
                                            self.inspector_status = None;
                                        }
                                    }
                                });
                            } else {
                                ui.label("Velocity: n/a");
                            }

                            if let Some(sprite) = info.sprite.clone() {
                                ui.separator();
                                ui.label(format!("Atlas: {}", sprite.atlas));
                                let mut region = sprite.region.clone();
                                if ui.text_edit_singleline(&mut region).changed() {
                                    if self.ecs.set_sprite_region(entity, &self.assets, &region) {
                                        info.sprite = Some(SpriteInfo {
                                            atlas: sprite.atlas.clone(),
                                            region: region.clone(),
                                        });
                                        inspector_refresh = true;
                                        self.inspector_status =
                                            Some(format!("Sprite region set to {}", region));
                                    } else {
                                        self.inspector_status = Some(format!(
                                            "Region '{}' not found in atlas {}",
                                            region, sprite.atlas
                                        ));
                                    }
                                }
                            } else {
                                ui.label("Sprite: n/a");
                            }

                            if let Some(mesh) = info.mesh.clone() {
                                ui.separator();
                                ui.label(format!("Mesh: {}", mesh.key));
                                if let Some(material) = mesh.material.as_ref() {
                                    ui.label(format!("Material: {}", material));
                                } else {
                                    ui.label("Material: default");
                                }
                                ui.label(format!(
                                    "Shadows: cast={} receive={}",
                                    mesh.lighting.cast_shadows, mesh.lighting.receive_shadows
                                ));
                                if let Some(subsets) = self.mesh_registry.mesh_subsets(&mesh.key) {
                                    ui.collapsing("Submeshes", |ui| {
                                        for (index, subset) in subsets.iter().enumerate() {
                                            let subset_name = subset.name.as_deref().unwrap_or("unnamed");
                                            let material_label =
                                                subset.material.as_deref().unwrap_or("default");
                                            ui.label(format!(
                                                "#{index}: {} | indices {}-{} | material: {}",
                                                subset_name,
                                                subset.index_offset,
                                                subset.index_offset + subset.index_count,
                                                material_label
                                            ));
                                        }
                                    });
                                }
                                let mut base_color_arr = mesh.lighting.base_color.to_array();
                                let mut metallic = mesh.lighting.metallic;
                                let mut roughness = mesh.lighting.roughness;
                                let mut emissive_enabled = mesh.lighting.emissive.is_some();
                                let mut emissive_arr =
                                    mesh.lighting.emissive.unwrap_or(Vec3::ZERO).to_array();

                                let base_color_changed = ui
                                    .horizontal(|ui| {
                                        ui.label("Base Color");
                                        ui.color_edit_button_rgb(&mut base_color_arr).changed()
                                    })
                                    .inner;
                                let metallic_changed = ui
                                    .add(egui::Slider::new(&mut metallic, 0.0..=1.0).text("Metallic"))
                                    .changed();
                                let roughness_changed = ui
                                    .add(egui::Slider::new(&mut roughness, 0.04..=1.0).text("Roughness"))
                                    .changed();
                                let mut emissive_changed = false;
                                ui.horizontal(|ui| {
                                    if ui.checkbox(&mut emissive_enabled, "Emissive").changed() {
                                        emissive_changed = true;
                                    }
                                    if emissive_enabled {
                                        if ui.color_edit_button_rgb(&mut emissive_arr).changed() {
                                            emissive_changed = true;
                                        }
                                    }
                                });

                                let material_changed = base_color_changed
                                    || metallic_changed
                                    || roughness_changed
                                    || emissive_changed;
                                if material_changed {
                                    let base_color_vec = Vec3::from_array(base_color_arr);
                                    let emissive_opt = if emissive_enabled {
                                        Some(Vec3::from_array(emissive_arr))
                                    } else {
                                        None
                                    };
                                    if self.ecs.set_mesh_material_params(
                                        entity,
                                        base_color_vec,
                                        metallic,
                                        roughness,
                                        emissive_opt,
                                    ) {
                                        inspector_refresh = true;
                                        self.inspector_status = None;
                                    } else {
                                        self.inspector_status =
                                            Some("Failed to update mesh material".to_string());
                                    }
                                }
                                if let Some(mut mesh_tx) = info.mesh_transform.clone() {
                                    let mut translation3 = mesh_tx.translation;
                                    ui.horizontal(|ui| {
                                        ui.label("Position (X/Y/Z)");
                                        let mut changed = false;
                                        changed |= ui
                                            .add(egui::DragValue::new(&mut translation3.x).speed(0.01))
                                            .changed();
                                        changed |= ui
                                            .add(egui::DragValue::new(&mut translation3.y).speed(0.01))
                                            .changed();
                                        changed |= ui
                                            .add(egui::DragValue::new(&mut translation3.z).speed(0.01))
                                            .changed();
                                        if changed {
                                            if self.ecs.set_mesh_translation(entity, translation3) {
                                                mesh_tx.translation = translation3;
                                                inspector_refresh = true;
                                                self.inspector_status = None;
                                            }
                                        }
                                    });

                                    let rotation_euler = mesh_tx.rotation.to_euler(EulerRot::XYZ);
                                    let mut rotation_deg = Vec3::new(
                                        rotation_euler.0.to_degrees(),
                                        rotation_euler.1.to_degrees(),
                                        rotation_euler.2.to_degrees(),
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label("Rotation (deg)");
                                        let mut changed = false;
                                        changed |= ui
                                            .add(egui::DragValue::new(&mut rotation_deg.x).speed(0.5))
                                            .changed();
                                        changed |= ui
                                            .add(egui::DragValue::new(&mut rotation_deg.y).speed(0.5))
                                            .changed();
                                        changed |= ui
                                            .add(egui::DragValue::new(&mut rotation_deg.z).speed(0.5))
                                            .changed();
                                        if changed {
                                            let radians = Vec3::new(
                                                rotation_deg.x.to_radians(),
                                                rotation_deg.y.to_radians(),
                                                rotation_deg.z.to_radians(),
                                            );
                                            if self.ecs.set_mesh_rotation_euler(entity, radians) {
                                                mesh_tx.rotation = Quat::from_euler(
                                                    EulerRot::XYZ,
                                                    radians.x,
                                                    radians.y,
                                                    radians.z,
                                                );
                                                inspector_refresh = true;
                                                self.inspector_status = None;
                                            }
                                        }
                                    });

                                    let mut scale3 = mesh_tx.scale;
                                    ui.horizontal(|ui| {
                                        ui.label("Scale (XYZ)");
                                        let mut changed = false;
                                        changed |=
                                            ui.add(egui::DragValue::new(&mut scale3.x).speed(0.01)).changed();
                                        changed |=
                                            ui.add(egui::DragValue::new(&mut scale3.y).speed(0.01)).changed();
                                        changed |=
                                            ui.add(egui::DragValue::new(&mut scale3.z).speed(0.01)).changed();
                                        if changed {
                                            let clamped = Vec3::new(
                                                scale3.x.max(0.01),
                                                scale3.y.max(0.01),
                                                scale3.z.max(0.01),
                                            );
                                            if self.ecs.set_mesh_scale(entity, clamped) {
                                                mesh_tx.scale = clamped;
                                                inspector_refresh = true;
                                                self.inspector_status = None;
                                            }
                                        }
                                    });

                                    info.mesh_transform = Some(mesh_tx);
                                } else {
                                    ui.label("Mesh transform: n/a");
                                }
                            }

                            ui.separator();
                            let mut tinted = info.tint.is_some();
                            if ui.checkbox(&mut tinted, "Tint override").changed() {
                                if tinted {
                                    let color = Vec4::splat(1.0);
                                    if self.ecs.set_tint(entity, Some(color)) {
                                        info.tint = Some(color);
                                        inspector_refresh = true;
                                        self.inspector_status = None;
                                    }
                                } else if self.ecs.set_tint(entity, None) {
                                    info.tint = None;
                                    inspector_refresh = true;
                                    self.inspector_status = None;
                                }
                            }
                            if let Some(color) = info.tint {
                                let mut color_arr = color.to_array();
                                if ui.color_edit_button_rgba_unmultiplied(&mut color_arr).changed() {
                                    let vec = Vec4::from_array(color_arr);
                                    if self.ecs.set_tint(entity, Some(vec)) {
                                        info.tint = Some(vec);
                                        inspector_refresh = true;
                                        self.inspector_status = None;
                                    }
                                }
                            }

                            inspector_info = Some(info);
                        } else {
                            ui.label("Selection data unavailable");
                        }

                        ui.horizontal(|ui| {
                            if ui.button("Frame selection").clicked() {
                                frame_selection_request = true;
                            }
                        });

                        if inspector_refresh {
                            selection_details =
                                selected_entity.and_then(|entity| self.ecs.entity_info(entity));
                        } else {
                            selection_details = inspector_info;
                        }
                        if let Some(status) = &self.inspector_status {
                            ui.colored_label(egui::Color32::YELLOW, status);
                        }
                        if ui.button("Delete selected").clicked() {
                            actions.delete_entity = Some(entity);
                            selected_entity = None;
                            selection_details = None;
                            self.inspector_status = None;
                        }
                    } else {
                        ui.label("No entity selected");
                    }

                    ui.separator();
                    ui.heading("Scene");
                    ui.horizontal(|ui| {
                        ui.label("Path");
                        ui.text_edit_singleline(&mut self.ui_scene_path);
                        ui.menu_button("Recent", |menu| {
                            if scene_history_list.is_empty() {
                                menu.label("No saved paths yet");
                            } else {
                                for entry in &scene_history_list {
                                    if menu.button(entry).clicked() {
                                        self.ui_scene_path = entry.clone();
                                        menu.close();
                                    }
                                }
                                menu.separator();
                                if menu.button("Clear history").clicked() {
                                    self.scene_history.clear();
                                    menu.close();
                                }
                            }
                        });
                        if ui.button("Save").clicked() {
                            actions.save_scene = true;
                        }
                        if ui.button("Load").clicked() {
                            actions.load_scene = true;
                        }
                    });
                    if let Some(status) = &self.ui_scene_status {
                        ui.label(status);
                    }
                    ui.collapsing("Dependency Summary", |ui| {
                        if atlas_snapshot.is_empty() {
                            ui.small("Atlases: none retained");
                        } else {
                            ui.label(format!(
                                "Atlases retained: {} (persistent: {})",
                                atlas_snapshot.len(),
                                self.persistent_atlases.len()
                            ));
                            for atlas in &atlas_snapshot {
                                let scope = if self.persistent_atlases.contains(atlas) {
                                    "persistent"
                                } else {
                                    "scene"
                                };
                                let loaded = self.assets.has_atlas(atlas);
                                let color = if loaded {
                                    egui::Color32::LIGHT_GREEN
                                } else {
                                    egui::Color32::from_rgb(220, 120, 120)
                                };
                                let status_label = if loaded { "loaded" } else { "missing" };
                                let path_opt = self.scene_dependencies.as_ref().and_then(|deps| {
                                    deps.atlas_dependencies()
                                        .find(|dep| dep.key() == atlas.as_str())
                                        .and_then(|dep| dep.path().map(|p| p.to_string()))
                                });
                                let path_display = path_opt.as_deref().unwrap_or("n/a");
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        color,
                                        format!(
                                            "- {} ({}, {}, path={})",
                                            atlas, scope, status_label, path_display
                                        ),
                                    );
                                    if !loaded {
                                        if ui.button("Retain").clicked() {
                                            actions.retain_atlases.push((atlas.clone(), path_opt.clone()));
                                        }
                                        if path_opt.is_none() {
                                            ui.small("no recorded path");
                                        }
                                    }
                                });
                            }
                        }
                        if mesh_snapshot.is_empty() {
                            ui.small("Meshes: none retained");
                        } else {
                            ui.separator();
                            ui.label(format!(
                                "Meshes retained: {} (persistent: {})",
                                mesh_snapshot.len(),
                                self.persistent_meshes.len()
                            ));
                            for mesh_key in &mesh_snapshot {
                                let scope = if self.persistent_meshes.contains(mesh_key) {
                                    "persistent"
                                } else {
                                    "scene"
                                };
                                let ref_count = self.mesh_registry.mesh_ref_count(mesh_key).unwrap_or(0);
                                let loaded = self.mesh_registry.has(mesh_key);
                                let color = if loaded {
                                    egui::Color32::LIGHT_GREEN
                                } else {
                                    egui::Color32::from_rgb(220, 120, 120)
                                };
                                let status_label = if loaded { "loaded" } else { "missing" };
                                let path_opt = self.scene_dependencies.as_ref().and_then(|deps| {
                                    deps.mesh_dependencies()
                                        .find(|dep| dep.key() == mesh_key.as_str())
                                        .and_then(|dep| dep.path().map(|p| p.to_string()))
                                });
                                let path_display = path_opt.as_deref().unwrap_or("n/a");
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        color,
                                        format!(
                                            "- {} ({}, refs={}, {}, path={})",
                                            mesh_key, scope, ref_count, status_label, path_display
                                        ),
                                    );
                                    if !loaded {
                                        if ui.button("Retain").clicked() {
                                            actions.retain_meshes.push((mesh_key.clone(), path_opt.clone()));
                                        }
                                        if path_opt.is_none() {
                                            ui.small("no recorded path");
                                        }
                                    }
                                });
                            }
                        }
                        if self.scene_dependencies.is_none() {
                            ui.small("Load or save a scene to populate dependency details.");
                        }
                    });

                    ui.separator();
                    ui.heading("Recent Events");
                    if recent_events.is_empty() {
                        ui.label("No events recorded");
                    } else {
                        for event in recent_events.iter().rev().take(10) {
                            ui.label(event.to_string());
                        }
                    }

                    ui.separator();
                    ui.heading("Audio Debug");
                    if ui.checkbox(&mut audio_enabled, "Enable audio triggers").changed() {
                        self.audio.set_enabled(audio_enabled);
                    }
                    if !self.audio.available() {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 80, 80),
                            "Audio device unavailable; triggers will be silent.",
                        );
                    }
                    if ui.button("Clear audio log").clicked() {
                        self.audio.clear();
                    }
                    if audio_triggers.is_empty() {
                        ui.label("No audio triggers");
                    } else {
                        for trigger in audio_triggers.iter().rev() {
                            ui.label(trigger);
                        }
                    }
                });
            left_panel_width_px = left_panel.response.rect.width() * ui_pixels_per_point;
            right_panel_width_px = right_panel.response.rect.width() * ui_pixels_per_point;
            let window_width_px = window_size.width as f32;
            let window_height_px = window_size.height as f32;
            let viewport_width_px = (window_width_px - left_panel_width_px - right_panel_width_px).max(1.0);
            let viewport_origin_vec2 = Vec2::new(left_panel_width_px, 0.0);
            let viewport_size_vec2 = Vec2::new(viewport_width_px, window_height_px);
            let viewport_size_physical = PhysicalSize::new(
                viewport_size_vec2.x.max(1.0).round() as u32,
                viewport_size_vec2.y.max(1.0).round() as u32,
            );
            pending_viewport = Some((viewport_origin_vec2, viewport_size_vec2));

            let cursor_in_new_viewport = cursor_screen
                .map(|pos| {
                    pos.x >= viewport_origin_vec2.x
                        && pos.x <= viewport_origin_vec2.x + viewport_size_vec2.x
                        && pos.y >= viewport_origin_vec2.y
                        && pos.y <= viewport_origin_vec2.y + viewport_size_vec2.y
                })
                .unwrap_or(false);
            if !cursor_in_new_viewport {
                if selection_changed {
                    self.selected_entity = prev_selected_entity;
                    selection_details = self.selected_entity.and_then(|entity| self.ecs.entity_info(entity));
                    selected_entity = self.selected_entity;
                    selection_changed = false;
                }
                if gizmo_changed {
                    self.gizmo_interaction = prev_gizmo_interaction;
                    gizmo_changed = false;
                }
            }

            let mut highlight_rect = None;
            let mut gizmo_center_px = None;
            let mut gizmo_center_world3d = None;
            if let Some(entity) = self.selected_entity {
                if self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
                    if let Some((min, max)) = self.ecs.entity_bounds(entity) {
                        if let Some((min_px_view, max_px_view)) =
                            self.camera.world_rect_to_screen_bounds(min, max, viewport_size_physical)
                        {
                            let min_screen = min_px_view + viewport_origin_vec2;
                            let max_screen = max_px_view + viewport_origin_vec2;
                            highlight_rect = Some(egui::Rect::from_two_pos(
                                egui::pos2(
                                    min_screen.x / ui_pixels_per_point,
                                    min_screen.y / ui_pixels_per_point,
                                ),
                                egui::pos2(
                                    max_screen.x / ui_pixels_per_point,
                                    max_screen.y / ui_pixels_per_point,
                                ),
                            ));
                            gizmo_center_px = Some((min_screen + max_screen) * 0.5);
                        }
                    }
                } else if let Some(info) = self.ecs.entity_info(entity) {
                    if let Some(mesh_tx) = info.mesh_transform {
                        if let Some(center_view) =
                            mesh_camera_for_ui.project_point(mesh_tx.translation, viewport_size_physical)
                        {
                            let center_screen = center_view + viewport_origin_vec2;
                            gizmo_center_px = Some(center_screen);
                            gizmo_center_world3d = Some(mesh_tx.translation);
                        }
                    }
                }
            }

            let painter = ctx.debug_painter();
            let viewport_outline = egui::Rect::from_min_size(
                egui::pos2(
                    viewport_origin_vec2.x / ui_pixels_per_point,
                    viewport_origin_vec2.y / ui_pixels_per_point,
                ),
                egui::vec2(
                    viewport_size_vec2.x / ui_pixels_per_point,
                    viewport_size_vec2.y / ui_pixels_per_point,
                ),
            );
            painter.rect_stroke(
                viewport_outline,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(220, 220, 240, 80)),
                egui::StrokeKind::Outside,
            );
            if let Some(rect) = highlight_rect {
                painter.rect_stroke(
                    rect,
                    0.0,
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                    egui::StrokeKind::Inside,
                );
            }
            let active_scale_handle_kind =
                self.gizmo_interaction.as_ref().and_then(|interaction| match interaction {
                    GizmoInteraction::Scale { handle, .. } => Some(handle.kind()),
                    _ => None,
                });
            let scale_highlight_kind = active_scale_handle_kind.or(hovered_scale_kind);
            if let Some(center_px) = gizmo_center_px {
                let center = egui::pos2(center_px.x / ui_pixels_per_point, center_px.y / ui_pixels_per_point);
                let draw_translate_axes = self.viewport_camera_mode == ViewportCameraMode::Perspective3D
                    && self.gizmo_mode == GizmoMode::Translate;
                if draw_translate_axes {
                    if let Some(center_world) = gizmo_center_world3d {
                        let distance = (mesh_camera_for_ui.position - center_world).length().max(0.001);
                        let axis_length = (distance * GIZMO_3D_AXIS_LENGTH_SCALE)
                            .clamp(GIZMO_3D_AXIS_MIN, GIZMO_3D_AXIS_MAX);
                        let axes = [
                            (Vec3::X, egui::Color32::from_rgb(240, 100, 100)),
                            (Vec3::Y, egui::Color32::from_rgb(100, 220, 100)),
                            (Vec3::Z, egui::Color32::from_rgb(120, 150, 255)),
                        ];
                        for (axis, color) in axes {
                            let end_world = center_world + axis * axis_length;
                            if let Some(end_view) =
                                mesh_camera_for_ui.project_point(end_world, viewport_size_physical)
                            {
                                let end_screen = end_view + viewport_origin_vec2;
                                let end_pos = egui::pos2(
                                    end_screen.x / ui_pixels_per_point,
                                    end_screen.y / ui_pixels_per_point,
                                );
                                painter.line_segment([center, end_pos], egui::Stroke::new(2.0, color));
                                painter.circle_filled(end_pos, 3.0 / ui_pixels_per_point, color);
                            }
                        }
                    }
                } else {
                    match self.gizmo_mode {
                        GizmoMode::Translate => {
                            let extent = 8.0 / ui_pixels_per_point;
                            painter.line_segment(
                                [
                                    egui::pos2(center.x - extent, center.y),
                                    egui::pos2(center.x + extent, center.y),
                                ],
                                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                            );
                            painter.line_segment(
                                [
                                    egui::pos2(center.x, center.y - extent),
                                    egui::pos2(center.x, center.y + extent),
                                ],
                                egui::Stroke::new(2.0, egui::Color32::YELLOW),
                            );
                        }
                        GizmoMode::Scale => {
                            let inner = GIZMO_SCALE_INNER_RADIUS_PX / ui_pixels_per_point;
                            let outer = GIZMO_SCALE_OUTER_RADIUS_PX / ui_pixels_per_point;
                            let axis_length = GIZMO_SCALE_AXIS_LENGTH_PX / ui_pixels_per_point;
                            let axis_half = (GIZMO_SCALE_AXIS_THICKNESS_PX * 0.5) / ui_pixels_per_point;
                            let handle_half = (GIZMO_SCALE_HANDLE_SIZE_PX * 0.5) / ui_pixels_per_point;

                            let active_uniform =
                                matches!(scale_highlight_kind, Some(ScaleHandleKind::Uniform));
                            let active_axis = match scale_highlight_kind {
                                Some(ScaleHandleKind::Axis(axis)) => Some(axis),
                                _ => None,
                            };

                            let base_x = if matches!(active_axis, Some(Axis2::X)) {
                                egui::Color32::from_rgb(255, 185, 185)
                            } else {
                                egui::Color32::from_rgb(240, 120, 120)
                            };
                            let base_y = if matches!(active_axis, Some(Axis2::Y)) {
                                egui::Color32::from_rgb(185, 225, 255)
                            } else {
                                egui::Color32::from_rgb(120, 180, 255)
                            };

                            let horiz_pos = egui::Rect::from_min_max(
                                egui::pos2(center.x, center.y - axis_half),
                                egui::pos2(center.x + axis_length, center.y + axis_half),
                            );
                            let horiz_neg = egui::Rect::from_min_max(
                                egui::pos2(center.x - axis_length, center.y - axis_half),
                                egui::pos2(center.x, center.y + axis_half),
                            );
                            painter.rect_filled(horiz_pos, 0.0, base_x);
                            painter.rect_filled(horiz_neg, 0.0, base_x);

                            let vert_pos = egui::Rect::from_min_max(
                                egui::pos2(center.x - axis_half, center.y - axis_length),
                                egui::pos2(center.x + axis_half, center.y),
                            );
                            let vert_neg = egui::Rect::from_min_max(
                                egui::pos2(center.x - axis_half, center.y),
                                egui::pos2(center.x + axis_half, center.y + axis_length),
                            );
                            painter.rect_filled(vert_pos, 0.0, base_y);
                            painter.rect_filled(vert_neg, 0.0, base_y);

                            let handle_size = egui::vec2(handle_half * 2.0, handle_half * 2.0);
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    egui::pos2(center.x + axis_length, center.y),
                                    handle_size,
                                ),
                                0.0,
                                base_x,
                            );
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    egui::pos2(center.x - axis_length, center.y),
                                    handle_size,
                                ),
                                0.0,
                                base_x,
                            );
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    egui::pos2(center.x, center.y - axis_length),
                                    handle_size,
                                ),
                                0.0,
                                base_y,
                            );
                            painter.rect_filled(
                                egui::Rect::from_center_size(
                                    egui::pos2(center.x, center.y + axis_length),
                                    handle_size,
                                ),
                                0.0,
                                base_y,
                            );

                            let outer_color = if active_uniform {
                                egui::Color32::from_rgb(255, 235, 150)
                            } else {
                                egui::Color32::from_rgb(255, 210, 90)
                            };
                            let inner_color = if active_uniform {
                                egui::Color32::from_rgb(220, 200, 110)
                            } else {
                                egui::Color32::from_rgb(180, 160, 60)
                            };
                            painter.circle_stroke(center, outer, egui::Stroke::new(2.0, outer_color));
                            painter.circle_stroke(center, inner, egui::Stroke::new(1.0, inner_color));
                        }
                        GizmoMode::Rotate => {
                            let inner = GIZMO_ROTATE_INNER_RADIUS_PX / ui_pixels_per_point;
                            let outer = GIZMO_ROTATE_OUTER_RADIUS_PX / ui_pixels_per_point;
                            painter.circle_stroke(
                                center,
                                outer,
                                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 210, 40)),
                            );
                            painter.circle_stroke(
                                center,
                                inner,
                                egui::Stroke::new(1.0, egui::Color32::from_rgb(180, 160, 40)),
                            );
                        }
                    }
                }
                painter.circle_stroke(
                    center,
                    3.0 / ui_pixels_per_point,
                    egui::Stroke::new(2.0, egui::Color32::YELLOW),
                );
            }
        });

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
        self.selected_entity = selected_entity;

        if let Some(mode) = viewport_mode_request {
            self.set_viewport_camera_mode(mode);
        }
        if let Some(mode) = mesh_control_request {
            self.set_mesh_control_mode(mode);
        }
        if let Some(lock) = mesh_frustum_request {
            self.set_frustum_lock(lock);
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
            match self.mesh_registry.retain_mesh(&key, path.as_deref()) {
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
            let mut scene =
                self.ecs.export_scene_with_mesh_source(&self.assets, |key| mesh_source_map.get(key).cloned());
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
                |key, path| self.mesh_registry.ensure_mesh(key, path),
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
