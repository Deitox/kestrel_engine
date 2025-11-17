use super::animation_keyframe_panel::AnimationKeyframePanel;
use super::{ClipEditRecord, FrameBudgetSnapshot, ScriptConsoleEntry};
use crate::animation_validation::AnimationValidationEvent;
use crate::assets::AnimationClip;
use crate::config::{EditorConfig, ParticleConfig, SpriteGuardrailMode};
use crate::prefab::{PrefabFormat, PrefabStatusMessage};
use crate::renderer::SceneLightingState;
use crate::scene::{SceneDependencies, SceneDependencyFingerprints};
use egui::Context as EguiCtx;
use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};
use egui_winit::State as EguiWinit;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use super::SCRIPT_CONSOLE_CAPACITY;

pub(crate) struct EditorShell {
    pub egui_ctx: EguiCtx,
    pub egui_winit: Option<EguiWinit>,
    pub egui_renderer: Option<EguiRenderer>,
    pub egui_screen: Option<ScreenDescriptor>,
    #[allow(dead_code)]
    pub ui_state: Option<RefCell<EditorUiState>>,
}

impl EditorShell {
    pub fn new() -> Self {
        Self {
            egui_ctx: EguiCtx::default(),
            egui_winit: None,
            egui_renderer: None,
            egui_screen: None,
            ui_state: None,
        }
    }

    #[allow(dead_code)]
    pub fn install_ui_state(&mut self, state: EditorUiState) {
        self.ui_state = Some(RefCell::new(state));
    }

    #[allow(dead_code)]
    pub fn ui_state(&self) -> Option<Ref<'_, EditorUiState>> {
        self.ui_state.as_ref().map(|cell| cell.borrow())
    }

    #[allow(dead_code)]
    pub fn ui_state_mut(&mut self) -> Option<RefMut<'_, EditorUiState>> {
        self.ui_state.as_ref().map(|cell| cell.borrow_mut())
    }
}

#[allow(dead_code)]
pub(crate) struct EditorUiState {
    pub ui_spawn_per_press: i32,
    pub ui_auto_spawn_rate: f32,
    pub ui_cell_size: f32,
    pub ui_spatial_use_quadtree: bool,
    pub ui_spatial_density_threshold: f32,
    pub ui_root_spin: f32,
    pub ui_emitter_rate: f32,
    pub ui_emitter_spread: f32,
    pub ui_emitter_speed: f32,
    pub ui_emitter_lifetime: f32,
    pub ui_emitter_start_size: f32,
    pub ui_emitter_end_size: f32,
    pub ui_emitter_start_color: [f32; 4],
    pub ui_emitter_end_color: [f32; 4],
    pub ui_particle_max_spawn_per_frame: u32,
    pub ui_particle_max_total: u32,
    pub ui_particle_max_emitter_backlog: f32,
    pub ui_light_direction: glam::Vec3,
    pub ui_light_color: glam::Vec3,
    pub ui_light_ambient: glam::Vec3,
    pub ui_light_exposure: f32,
    pub ui_environment_intensity: f32,
    pub ui_shadow_distance: f32,
    pub ui_shadow_bias: f32,
    pub ui_shadow_strength: f32,
    pub ui_shadow_cascade_count: u32,
    pub ui_shadow_resolution: u32,
    pub ui_shadow_split_lambda: f32,
    pub ui_shadow_pcf_radius: f32,
    pub ui_camera_zoom_min: f32,
    pub ui_camera_zoom_max: f32,
    pub ui_sprite_guard_pixels: f32,
    pub ui_sprite_guard_mode: SpriteGuardrailMode,
    pub ui_scale: f32,
    pub ui_scene_path: String,
    pub ui_scene_status: Option<String>,
    pub prefab_name_input: String,
    pub prefab_format: PrefabFormat,
    pub prefab_status: Option<PrefabStatusMessage>,
    pub animation_group_input: String,
    pub animation_group_scale_input: f32,
    pub camera_bookmark_input: String,
    pub scene_dependencies: Option<SceneDependencies>,
    pub scene_dependency_fingerprints: Option<SceneDependencyFingerprints>,
    pub scene_history: VecDeque<String>,
    pub scene_history_snapshot: Option<Arc<[String]>>,
    pub scene_atlas_snapshot: Option<Arc<[String]>>,
    pub scene_mesh_snapshot: Option<Arc<[String]>>,
    pub scene_clip_snapshot: Option<Arc<[String]>>,
    pub inspector_status: Option<String>,
    pub debug_show_spatial_hash: bool,
    pub debug_show_colliders: bool,
    pub sprite_guardrail_status: Option<String>,
    pub gpu_metrics_status: Option<String>,
    pub frame_budget_idle_snapshot: Option<FrameBudgetSnapshot>,
    pub frame_budget_panel_snapshot: Option<FrameBudgetSnapshot>,
    pub frame_budget_status: Option<String>,
    pub script_debugger_open: bool,
    pub script_focus_repl: bool,
    pub script_repl_input: String,
    pub script_repl_history: VecDeque<String>,
    pub script_repl_history_index: Option<usize>,
    pub script_repl_history_snapshot: Option<Arc<[String]>>,
    pub script_console: VecDeque<ScriptConsoleEntry>,
    pub script_console_snapshot: Option<Arc<[ScriptConsoleEntry]>>,
    pub last_reported_script_error: Option<String>,
    pub animation_keyframe_panel: AnimationKeyframePanel,
    pub clip_dirty: HashSet<String>,
    pub clip_edit_history: Vec<ClipEditRecord>,
    pub clip_edit_redo: Vec<ClipEditRecord>,
    pub animation_clip_status: Option<String>,
    pub clip_edit_overrides: HashMap<String, Arc<AnimationClip>>,
    pub pending_animation_validation_events: Vec<AnimationValidationEvent>,
    pub suppressed_validation_paths: HashSet<PathBuf>,
}

#[allow(dead_code)]
pub(crate) struct EditorUiStateParams {
    pub scene_path: String,
    pub scene_history: VecDeque<String>,
    pub emitter_defaults: EmitterUiDefaults,
    pub particle_config: ParticleConfig,
    pub lighting_state: SceneLightingState,
    pub environment_intensity: f32,
    pub editor_config: EditorConfig,
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct EmitterUiDefaults {
    pub rate: f32,
    pub spread: f32,
    pub speed: f32,
    pub lifetime: f32,
    pub start_size: f32,
    pub end_size: f32,
    pub start_color: [f32; 4],
    pub end_color: [f32; 4],
}

#[allow(dead_code)]
impl EditorUiState {
    pub fn new(params: EditorUiStateParams) -> Self {
        Self {
            ui_spawn_per_press: 200,
            ui_auto_spawn_rate: 0.0,
            ui_cell_size: 0.25,
            ui_spatial_use_quadtree: false,
            ui_spatial_density_threshold: 6.0,
            ui_root_spin: 1.2,
            ui_emitter_rate: params.emitter_defaults.rate,
            ui_emitter_spread: params.emitter_defaults.spread,
            ui_emitter_speed: params.emitter_defaults.speed,
            ui_emitter_lifetime: params.emitter_defaults.lifetime,
            ui_emitter_start_size: params.emitter_defaults.start_size,
            ui_emitter_end_size: params.emitter_defaults.end_size,
            ui_emitter_start_color: params.emitter_defaults.start_color,
            ui_emitter_end_color: params.emitter_defaults.end_color,
            ui_particle_max_spawn_per_frame: params.particle_config.max_spawn_per_frame,
            ui_particle_max_total: params.particle_config.max_total,
            ui_particle_max_emitter_backlog: params.particle_config.max_emitter_backlog,
            ui_light_direction: params.lighting_state.direction,
            ui_light_color: params.lighting_state.color,
            ui_light_ambient: params.lighting_state.ambient,
            ui_light_exposure: params.lighting_state.exposure,
            ui_environment_intensity: params.environment_intensity,
            ui_shadow_distance: params.lighting_state.shadow_distance,
            ui_shadow_bias: params.lighting_state.shadow_bias,
            ui_shadow_strength: params.lighting_state.shadow_strength,
            ui_shadow_cascade_count: params.lighting_state.shadow_cascade_count,
            ui_shadow_resolution: params.lighting_state.shadow_resolution,
            ui_shadow_split_lambda: params.lighting_state.shadow_split_lambda,
            ui_shadow_pcf_radius: params.lighting_state.shadow_pcf_radius,
            ui_camera_zoom_min: params.editor_config.camera_zoom_min,
            ui_camera_zoom_max: params.editor_config.camera_zoom_max,
            ui_sprite_guard_pixels: params.editor_config.sprite_guard_max_pixels,
            ui_sprite_guard_mode: params.editor_config.sprite_guardrail_mode,
            ui_scale: 1.0,
            ui_scene_path: params.scene_path,
            ui_scene_status: None,
            prefab_name_input: String::new(),
            prefab_format: PrefabFormat::Json,
            prefab_status: None,
            animation_group_input: String::new(),
            animation_group_scale_input: 1.0,
            camera_bookmark_input: String::new(),
            scene_dependencies: None,
            scene_dependency_fingerprints: None,
            scene_history: params.scene_history,
            scene_history_snapshot: None,
            scene_atlas_snapshot: None,
            scene_mesh_snapshot: None,
            scene_clip_snapshot: None,
            inspector_status: None,
            debug_show_spatial_hash: false,
            debug_show_colliders: false,
            sprite_guardrail_status: None,
            gpu_metrics_status: None,
            frame_budget_idle_snapshot: None,
            frame_budget_panel_snapshot: None,
            frame_budget_status: None,
            script_debugger_open: false,
            script_focus_repl: false,
            script_repl_input: String::new(),
            script_repl_history: VecDeque::new(),
            script_repl_history_index: None,
            script_repl_history_snapshot: None,
            script_console: VecDeque::with_capacity(SCRIPT_CONSOLE_CAPACITY),
            script_console_snapshot: None,
            last_reported_script_error: None,
            animation_keyframe_panel: AnimationKeyframePanel::default(),
            clip_dirty: HashSet::new(),
            clip_edit_history: Vec::new(),
            clip_edit_redo: Vec::new(),
            animation_clip_status: None,
            clip_edit_overrides: HashMap::new(),
            pending_animation_validation_events: Vec::new(),
            suppressed_validation_paths: HashSet::new(),
        }
    }
}
