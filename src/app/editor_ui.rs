use super::{
    App, CameraBookmark, FrameTimingSample, MeshControlMode, ScriptConsoleEntry, ScriptConsoleKind,
    ViewportCameraMode,
};
#[cfg(feature = "alloc_profiler")]
use crate::alloc_profiler::AllocationDelta;
use crate::analytics::{
    AnimationBudgetSample, KeyframeEditorEvent, KeyframeEditorEventKind, KeyframeEditorTrackKind,
    KeyframeEditorUsageSnapshot,
};
use crate::animation_validation::{AnimationValidationEvent, AnimationValidationSeverity};
use crate::audio::{AudioHealthSnapshot, AudioPlugin};
use crate::camera3d::Camera3D;
use crate::ecs::{
    AnimationTime, EntityInfo, ParticleBudgetMetrics, SpatialMetrics, SpatialMode, SpriteAnimPerfSample,
    SystemTimingSummary,
};
use crate::events::GameEvent;
use crate::gizmo::{
    Axis2, GizmoInteraction, GizmoMode, ScaleHandleKind, GIZMO_ROTATE_INNER_RADIUS_PX,
    GIZMO_ROTATE_OUTER_RADIUS_PX, GIZMO_SCALE_AXIS_LENGTH_PX, GIZMO_SCALE_AXIS_THICKNESS_PX,
    GIZMO_SCALE_HANDLE_SIZE_PX, GIZMO_SCALE_INNER_RADIUS_PX, GIZMO_SCALE_OUTER_RADIUS_PX,
};
use crate::mesh_preview::{GIZMO_3D_AXIS_LENGTH_SCALE, GIZMO_3D_AXIS_MAX, GIZMO_3D_AXIS_MIN};
use crate::plugins::{
    AssetReadbackStats, CapabilityViolationLog, PluginCapability, PluginManager, PluginState, PluginStatus,
    PluginTrust, PluginWatchdogEvent,
};
use crate::prefab::{PrefabFormat, PrefabStatusKind, PrefabStatusMessage};
use crate::renderer::{LightClusterMetrics, ScenePointLight, LIGHT_CLUSTER_MAX_LIGHTS, MAX_SHADOW_CASCADES};
use crate::scene::SceneShadowData;

use crate::config::SpriteGuardrailMode;
use bevy_ecs::prelude::Entity;
use egui::{Checkbox, DragAndDrop, Key, SliderClamping};
use egui_plot as eplot;
use glam::{Vec2, Vec3};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use winit::dpi::PhysicalSize;

mod entity_inspector;
const SPRITE_EVAL_BUDGET_MS: f32 = 0.30;
const SPRITE_PACK_BUDGET_MS: f32 = 0.05;
const SPRITE_UPLOAD_BUDGET_MS: f32 = 0.10;
const TRANSFORM_CLIP_BUDGET_MS: f32 = 0.40;
const SKELETAL_EVAL_BUDGET_MS: f32 = 1.20;
const GPU_PALETTE_UPLOAD_BUDGET_MS: f32 = 0.50;
#[derive(Clone, Copy)]
pub(super) struct PrefabDragPayload {
    pub entity: Entity,
}

#[derive(Clone)]
pub(super) struct PrefabSpawnPayload {
    pub name: String,
    pub format: PrefabFormat,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum PrefabDropTarget {
    World2D(Vec2),
    World3D(Vec3),
}

#[derive(Clone)]
pub(super) struct PrefabShelfEntry {
    pub name: String,
    pub format: PrefabFormat,
    pub path_display: String,
}

#[derive(Debug, Clone)]
pub(super) struct PrefabSaveRequest {
    pub entity: Entity,
    pub name: String,
    pub format: PrefabFormat,
}

#[derive(Debug, Clone)]
pub(super) struct PrefabInstantiateRequest {
    pub name: String,
    pub format: PrefabFormat,
    pub drop_target: Option<PrefabDropTarget>,
}

#[derive(Debug, Clone)]
pub(super) enum PluginToggleKind {
    Dynamic { new_enabled: bool },
    Builtin { disable: bool },
}

#[derive(Debug, Clone)]
pub(super) struct PluginToggleRequest {
    pub name: String,
    pub kind: PluginToggleKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum AudioTriggerKind {
    Spawn,
    Despawn,
    CollisionStart,
    CollisionEnd,
    CollisionForce,
    Other,
}

#[derive(Debug, Clone)]
struct ParsedAudioTrigger {
    kind: AudioTriggerKind,
    summary: String,
    color: egui::Color32,
    force: Option<f32>,
}

fn summarize_game_event(event: &GameEvent) -> (String, egui::Color32) {
    match event {
        GameEvent::SpriteSpawned { entity, atlas, region } => (
            format!("Sprite #{:04} spawned - {atlas}/{region}", entity.index()),
            egui::Color32::from_rgb(120, 200, 120),
        ),
        GameEvent::EntityDespawned { entity } => {
            (format!("Entity #{:04} despawned", entity.index()), egui::Color32::from_rgb(210, 130, 130))
        }
        GameEvent::CollisionStarted { a, b } => (
            format!("Collision started between #{:04} and #{:04}", a.index(), b.index()),
            egui::Color32::from_rgb(220, 180, 90),
        ),
        GameEvent::CollisionEnded { a, b } => (
            format!("Collision resolved between #{:04} and #{:04}", a.index(), b.index()),
            egui::Color32::from_rgb(130, 170, 220),
        ),
        GameEvent::CollisionForce { a, b, force } => (
            format!("Impact #{:04}/{:04} - force {:.1}", a.index(), b.index(), force),
            egui::Color32::from_rgb(200, 150, 240),
        ),
        GameEvent::SpriteAnimationEvent { entity, timeline, event } => (
            format!("Anim event #{:04} {}::{}", entity.index(), timeline, event),
            egui::Color32::from_rgb(180, 200, 255),
        ),
        GameEvent::ScriptMessage { message } => {
            (format!("Script: {message}"), egui::Color32::from_rgb(170, 170, 170))
        }
    }
}

fn plugin_status_summary(status: &PluginStatus) -> (egui::Color32, String) {
    match &status.state {
        PluginState::Loaded => (egui::Color32::LIGHT_GREEN, "Loaded".to_string()),
        PluginState::Disabled(reason) => {
            (egui::Color32::from_rgb(220, 180, 80), format!("Disabled: {reason}"))
        }
        PluginState::Failed(reason) => (egui::Color32::from_rgb(220, 120, 120), format!("Failed: {reason}")),
    }
}

fn format_capability_list(list: &[PluginCapability]) -> String {
    if list.is_empty() {
        "none".to_string()
    } else {
        list.iter().map(|cap| cap.label()).collect::<Vec<_>>().join(", ")
    }
}

fn capability_violation_summary(log: Option<&CapabilityViolationLog>) -> (egui::Color32, String) {
    if let Some(log) = log {
        if log.count > 0 {
            let last = log.last_capability.map(|cap| cap.label()).unwrap_or("unknown");
            return (
                egui::Color32::from_rgb(220, 120, 80),
                format!("Capability violations: {} (last: {last})", log.count),
            );
        }
    }
    (egui::Color32::from_rgb(120, 200, 120), "Capability violations: 0".to_string())
}

fn animation_validation_color(severity: AnimationValidationSeverity) -> egui::Color32 {
    match severity {
        AnimationValidationSeverity::Info => egui::Color32::from_rgb(140, 200, 255),
        AnimationValidationSeverity::Warning => egui::Color32::from_rgb(230, 200, 120),
        AnimationValidationSeverity::Error => egui::Color32::from_rgb(240, 120, 120),
    }
}

fn render_keyframe_editor_usage(
    ui: &mut egui::Ui,
    usage: KeyframeEditorUsageSnapshot,
    events: &[KeyframeEditorEvent],
) {
    ui.label("Keyframe Editor Usage");
    ui.label(format!("Opened {} | Closed {}", usage.panel_open_count, usage.panel_close_count));
    ui.label(format!(
        "Scrubs {} | Inserts {} | Deletes {} ({} keys)",
        usage.scrub_count, usage.insert_count, usage.delete_count, usage.delete_key_total
    ));
    ui.label(format!(
        "Updates {} (time {} | value {})",
        usage.update_count, usage.update_time_edits, usage.update_value_edits
    ));
    ui.label(format!(
        "Adjustments {} (time {} | value {})",
        usage.adjust_count, usage.adjust_time_edits, usage.adjust_value_edits
    ));
    ui.label(format!("Undo {} | Redo {}", usage.undo_count, usage.redo_count));
    if events.is_empty() {
        ui.small("No recent keyframe events.");
    } else {
        ui.label("Recent events:");
        for event in events.iter().take(5) {
            let ago = event.timestamp.elapsed().as_secs_f32();
            ui.small(format!("[{ago:>4.1}s ago] {}", format_keyframe_event(&event.kind)));
        }
    }
}

fn format_keyframe_event(event: &KeyframeEditorEventKind) -> String {
    match event {
        KeyframeEditorEventKind::PanelOpened => "Panel opened".to_string(),
        KeyframeEditorEventKind::PanelClosed => "Panel closed".to_string(),
        KeyframeEditorEventKind::Scrub { track } => {
            format!("Scrubbed {}", keyframe_track_label(*track))
        }
        KeyframeEditorEventKind::InsertKey { track } => {
            format!("Inserted key on {}", keyframe_track_label(*track))
        }
        KeyframeEditorEventKind::DeleteKeys { track, count } => {
            format!("Deleted {count} key(s) from {}", keyframe_track_label(*track))
        }
        KeyframeEditorEventKind::UpdateKey { track, changed_time, changed_value } => {
            let mut details = Vec::new();
            if *changed_time {
                details.push("time");
            }
            if *changed_value {
                details.push("value");
            }
            if details.is_empty() {
                format!("Updated {}", keyframe_track_label(*track))
            } else {
                format!("Updated {} ({})", keyframe_track_label(*track), details.join(" + "))
            }
        }
        KeyframeEditorEventKind::AdjustKeys { track, count, time_delta, value_delta } => {
            let mut details = Vec::new();
            if *time_delta {
                details.push("time offset");
            }
            if *value_delta {
                details.push("value offset");
            }
            let descriptor = if details.is_empty() { "offset".to_string() } else { details.join(" & ") };
            format!("Adjusted {} key(s) on {} ({descriptor})", count, keyframe_track_label(*track))
        }
        KeyframeEditorEventKind::Undo => "Undo edit".to_string(),
        KeyframeEditorEventKind::Redo => "Redo edit".to_string(),
    }
}

fn keyframe_track_label(track: KeyframeEditorTrackKind) -> &'static str {
    match track {
        KeyframeEditorTrackKind::SpriteTimeline => "Sprite Timeline",
        KeyframeEditorTrackKind::Translation => "Translation",
        KeyframeEditorTrackKind::Rotation => "Rotation",
        KeyframeEditorTrackKind::Scale => "Scale",
        KeyframeEditorTrackKind::Tint => "Tint",
        KeyframeEditorTrackKind::Unknown => "Unknown track",
    }
}

fn show_capability_badges(ui: &mut egui::Ui, caps: &[PluginCapability]) {
    fn has_cap(caps: &[PluginCapability], target: PluginCapability) -> bool {
        caps.iter().any(|cap| matches!(cap, PluginCapability::All) || *cap == target)
    }
    let mut badges = Vec::new();
    if has_cap(caps, PluginCapability::Ecs) {
        badges.push(("ECS", egui::Color32::from_rgb(120, 170, 250)));
    }
    if has_cap(caps, PluginCapability::Assets) {
        badges.push(("ASSETS", egui::Color32::from_rgb(200, 170, 120)));
    }
    if badges.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for (label, color) in badges {
            ui.colored_label(color, egui::RichText::new(label).monospace());
        }
    });
}

fn show_capability_info(
    ui: &mut egui::Ui,
    caps: &[PluginCapability],
    trust: PluginTrust,
    log: Option<&CapabilityViolationLog>,
) {
    ui.small(format!("Capabilities: {} (trust: {})", format_capability_list(caps), trust.label()));
    show_capability_badges(ui, caps);
    let (color, text) = capability_violation_summary(log);
    ui.colored_label(color, text);
}

fn format_ecs_entity(bits: u64) -> String {
    let entity = Entity::from_bits(bits);
    format!("Entity #{:05} (bits {} v{})", entity.index(), bits, entity.generation())
}

fn format_data_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    if bytes == 0 {
        return "0 B".to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn plugin_debug_ui(
    ui: &mut egui::Ui,
    plugin_name: &str,
    asset_metrics: &HashMap<String, AssetReadbackStats>,
    ecs_history: &HashMap<String, Vec<u64>>,
    watchdog_events: &HashMap<String, Vec<PluginWatchdogEvent>>,
    plugin_manager: &mut PluginManager,
    scene_status: &mut Option<String>,
) {
    if let Some(events) = watchdog_events.get(plugin_name).filter(|entries| !entries.is_empty()) {
        ui.horizontal(|ui| {
            ui.colored_label(
                egui::Color32::from_rgb(230, 190, 90),
                format!("Watchdog events: {}", events.len()),
            );
            if ui.button("Clear").clicked() {
                plugin_manager.clear_watchdog_events(plugin_name);
            }
        });
        egui::CollapsingHeader::new("Watchdog history").default_open(false).show(ui, |ui| {
            for event in events {
                let ago = event
                    .timestamp
                    .elapsed()
                    .map(|duration| format!("{:.1}s ago", duration.as_secs_f32()))
                    .unwrap_or_else(|_| "just now".to_string());
                ui.label(format!(
                    "{} | {} ({} ms) | last request: {}",
                    ago, event.reason, event.elapsed_ms, event.last_request
                ));
            }
        });
    }
    if let Some(history) = ecs_history.get(plugin_name).filter(|entries| !entries.is_empty()) {
        egui::CollapsingHeader::new("Read-only ECS").default_open(false).show(ui, |ui| {
            let max_rows = 8;
            for bits in history.iter().take(max_rows) {
                ui.small(format_ecs_entity(*bits));
            }
            if history.len() > max_rows {
                ui.small(format!("... {} older queries hidden", history.len() - max_rows));
            }
        });
    }
    if let Some(stats) = asset_metrics.get(plugin_name) {
        ui.small(format!(
            "Asset readbacks: {} req / {} cache hits / {} throttled – {} transferred",
            stats.requests,
            stats.cache_hits,
            stats.throttled,
            format_data_size(stats.bytes),
        ));
    }
    let retry_enabled = plugin_manager.has_asset_readback_request(plugin_name);
    let retry_button = ui.add_enabled(retry_enabled, egui::Button::new("Retry asset readback"));
    if retry_button.clicked() {
        match plugin_manager.retry_last_asset_readback(plugin_name) {
            Ok(Some(response)) => {
                let bytes = response.byte_length;
                let content_type = response.content_type.clone();
                *scene_status = Some(format!(
                    "Retried asset readback for {plugin_name}: {} ({content_type})",
                    format_data_size(bytes)
                ));
            }
            Ok(None) => {
                *scene_status = Some(format!("No asset readbacks recorded for {plugin_name}"));
            }
            Err(err) => {
                *scene_status = Some(format!("Asset readback retry failed for {plugin_name}: {err}"));
            }
        }
    } else if !retry_enabled {
        ui.small("No asset readbacks recorded yet.");
    }
}

fn ellipsize(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    if max_len <= 3 {
        return "...".to_string();
    }
    let mut truncated = String::new();
    let target = max_len - 3;
    for (idx, ch) in text.chars().enumerate() {
        if idx >= target {
            break;
        }
        truncated.push(ch);
    }
    truncated.push_str("...");
    truncated
}

fn parse_audio_trigger(label: &str) -> ParsedAudioTrigger {
    if let Some(rest) = label.strip_prefix("spawn:") {
        let mut parts = rest.splitn(2, ':');
        let atlas = parts.next().unwrap_or_default();
        let region = parts.next().unwrap_or_default();
        let summary = if region.is_empty() {
            format!("Spawn trigger for {atlas}")
        } else {
            format!("Spawn trigger for {atlas}/{region}")
        };
        return ParsedAudioTrigger {
            kind: AudioTriggerKind::Spawn,
            summary,
            color: egui::Color32::from_rgb(120, 200, 120),
            force: None,
        };
    }
    if label == "despawn" {
        return ParsedAudioTrigger {
            kind: AudioTriggerKind::Despawn,
            summary: "Despawn trigger".to_string(),
            color: egui::Color32::from_rgb(210, 130, 130),
            force: None,
        };
    }
    if label == "collision" {
        return ParsedAudioTrigger {
            kind: AudioTriggerKind::CollisionStart,
            summary: "Collision trigger".to_string(),
            color: egui::Color32::from_rgb(220, 180, 90),
            force: None,
        };
    }
    if label == "collision_end" {
        return ParsedAudioTrigger {
            kind: AudioTriggerKind::CollisionEnd,
            summary: "Collision resolved trigger".to_string(),
            color: egui::Color32::from_rgb(130, 170, 220),
            force: None,
        };
    }
    if let Some(force_str) = label.strip_prefix("collision_force:") {
        let parsed_force = force_str.parse::<f32>().ok();
        let summary = if let Some(force) = parsed_force {
            format!("Collision impact trigger ({force:.1})")
        } else {
            "Collision impact trigger".to_string()
        };
        return ParsedAudioTrigger {
            kind: AudioTriggerKind::CollisionForce,
            summary,
            color: egui::Color32::from_rgb(200, 150, 240),
            force: parsed_force,
        };
    }
    ParsedAudioTrigger {
        kind: AudioTriggerKind::Other,
        summary: format!("Trigger: {label}"),
        color: egui::Color32::from_rgb(180, 180, 180),
        force: None,
    }
}

fn audio_trigger_kind_label(kind: AudioTriggerKind) -> &'static str {
    match kind {
        AudioTriggerKind::Spawn => "sprite spawns",
        AudioTriggerKind::Despawn => "despawns",
        AudioTriggerKind::CollisionStart => "collisions",
        AudioTriggerKind::CollisionEnd => "collision ends",
        AudioTriggerKind::CollisionForce => "collision impacts",
        AudioTriggerKind::Other => "other",
    }
}

#[derive(Default)]
pub(super) struct UiActions {
    pub spawn_now: bool,
    pub delete_entity: Option<Entity>,
    pub clear_particles: bool,
    pub reset_world: bool,
    pub save_scene: bool,
    pub load_scene: bool,
    pub spawn_mesh: Option<String>,
    pub retain_atlases: Vec<(String, Option<String>)>,
    pub retain_clips: Vec<(String, Option<String>)>,
    pub retain_meshes: Vec<(String, Option<String>)>,
    pub retain_environments: Vec<(String, Option<String>)>,
    pub sprite_atlas_requests: Vec<SpriteAtlasRequest>,
    pub plugin_toggles: Vec<PluginToggleRequest>,
    pub reload_plugins: bool,
    pub save_prefab: Option<PrefabSaveRequest>,
    pub instantiate_prefab: Option<PrefabInstantiateRequest>,
}

pub(super) struct SpriteAtlasRequest {
    pub entity: Entity,
    pub atlas: String,
    pub path: Option<String>,
}

pub(super) struct SelectionResult {
    pub entity: Option<Entity>,
    pub details: Option<EntityInfo>,
}

pub(super) struct ScriptDebuggerParams {
    pub open: bool,
    pub available: bool,
    pub script_path: Option<String>,
    pub enabled: bool,
    pub paused: bool,
    pub last_error: Option<String>,
    pub repl_input: String,
    pub repl_history_index: Option<usize>,
    pub repl_history: Arc<[String]>,
    pub console_entries: Arc<[ScriptConsoleEntry]>,
    pub focus_repl: bool,
}

pub(super) struct ScriptDebuggerOutput {
    pub open: bool,
    pub repl_input: String,
    pub repl_history_index: Option<usize>,
    pub focus_repl: bool,
    pub submit_command: Option<String>,
    pub clear_console: bool,
    pub set_enabled: Option<bool>,
    pub set_paused: Option<bool>,
    pub step_once: bool,
    pub reload: bool,
}

pub(super) struct EditorUiParams {
    pub raw_input: egui::RawInput,
    pub base_pixels_per_point: f32,
    pub hist_points: Arc<[eplot::PlotPoint]>,
    #[cfg(feature = "alloc_profiler")]
    pub allocation_delta: Option<AllocationDelta>,
    pub frame_timing_sample: Option<FrameTimingSample>,
    pub system_timings: Vec<SystemTimingSummary>,
    pub entity_count: usize,
    pub instances_drawn: usize,
    pub vsync_enabled: bool,
    pub particle_budget: Option<ParticleBudgetMetrics>,
    pub spatial_metrics: Option<SpatialMetrics>,
    pub sprite_perf_sample: Option<SpriteAnimPerfSample>,
    pub sprite_eval_ms: Option<f32>,
    pub sprite_pack_ms: Option<f32>,
    pub sprite_upload_ms: Option<f32>,
    pub ui_scale: f32,
    pub ui_cell_size: f32,
    pub ui_spatial_use_quadtree: bool,
    pub ui_spatial_density_threshold: f32,
    pub ui_spawn_per_press: i32,
    pub ui_auto_spawn_rate: f32,
    pub ui_environment_intensity: f32,
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
    pub selected_entity: Option<Entity>,
    pub selection_details: Option<EntityInfo>,
    pub prev_selected_entity: Option<Entity>,
    pub prev_gizmo_interaction: Option<GizmoInteraction>,
    pub selection_changed: bool,
    pub gizmo_changed: bool,
    pub cursor_screen: Option<Vec2>,
    pub cursor_world_2d: Option<Vec2>,
    pub cursor_ray: Option<(Vec3, Vec3)>,
    pub hovered_scale_kind: Option<ScaleHandleKind>,
    pub window_size: PhysicalSize<u32>,
    pub mesh_camera_for_ui: Camera3D,
    pub camera_position: Vec2,
    pub camera_zoom: f32,
    pub camera_bookmarks: Vec<CameraBookmark>,
    pub active_camera_bookmark: Option<String>,
    pub camera_follow_target: Option<String>,
    pub camera_bookmark_input: String,
    pub mesh_keys: Arc<[String]>,
    pub environment_options: Arc<[(String, String)]>,
    pub active_environment: String,
    pub debug_show_spatial_hash: bool,
    pub debug_show_colliders: bool,
    pub spatial_hash_rects: Vec<(Vec2, Vec2)>,
    pub collider_rects: Vec<(Vec2, Vec2)>,
    pub scene_history_list: Arc<[String]>,
    pub atlas_snapshot: Arc<[String]>,
    pub mesh_snapshot: Arc<[String]>,
    pub clip_snapshot: Arc<[String]>,
    pub recent_events: Arc<[GameEvent]>,
    pub audio_triggers: Vec<String>,
    pub audio_enabled: bool,
    pub audio_health: AudioHealthSnapshot,
    pub binary_prefabs_enabled: bool,
    pub prefab_entries: Arc<[PrefabShelfEntry]>,
    pub prefab_name_input: String,
    pub prefab_format: PrefabFormat,
    pub prefab_status: Option<PrefabStatusMessage>,
    pub script_debugger: ScriptDebuggerParams,
    pub id_lookup_input: String,
    pub id_lookup_active: bool,
}

pub(super) struct EditorUiOutput {
    pub full_output: egui::FullOutput,
    pub actions: UiActions,
    pub pending_viewport: Option<(Vec2, Vec2)>,
    pub ui_scale: f32,
    pub ui_cell_size: f32,
    pub ui_spatial_use_quadtree: bool,
    pub ui_spatial_density_threshold: f32,
    pub ui_spawn_per_press: i32,
    pub ui_auto_spawn_rate: f32,
    pub ui_environment_intensity: f32,
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
    pub selection: SelectionResult,
    pub viewport_mode_request: Option<ViewportCameraMode>,
    pub camera_bookmark_select: Option<Option<String>>,
    pub camera_bookmark_save: Option<String>,
    pub camera_bookmark_delete: Option<String>,
    pub mesh_control_request: Option<MeshControlMode>,
    pub mesh_frustum_request: Option<bool>,
    pub mesh_frustum_snap: bool,
    pub mesh_reset_request: bool,
    pub mesh_selection_request: Option<String>,
    pub environment_selection_request: Option<String>,
    pub frame_selection_request: bool,
    pub id_lookup_request: Option<String>,
    pub id_lookup_input: String,
    pub id_lookup_active: bool,
    pub camera_bookmark_input: String,
    pub camera_follow_selection: bool,
    pub camera_follow_clear: bool,
    pub debug_show_spatial_hash: bool,
    pub debug_show_colliders: bool,
    pub vsync_request: Option<bool>,
    pub script_debugger: ScriptDebuggerOutput,
    pub prefab_name_input: String,
    pub prefab_format: PrefabFormat,
    pub prefab_status: Option<PrefabStatusMessage>,
}

impl App {
    pub(super) fn render_editor_ui(&mut self, params: EditorUiParams) -> EditorUiOutput {
        let EditorUiParams {
            raw_input,
            base_pixels_per_point,
            hist_points,
            #[cfg(feature = "alloc_profiler")]
            allocation_delta,
            frame_timing_sample,
            system_timings,
            entity_count,
            instances_drawn,
            mut vsync_enabled,
            mut ui_scale,
            mut ui_cell_size,
            mut ui_spatial_use_quadtree,
            mut ui_spatial_density_threshold,
            mut ui_spawn_per_press,
            mut ui_auto_spawn_rate,
            mut ui_environment_intensity,
            mut ui_root_spin,
            mut ui_emitter_rate,
            mut ui_emitter_spread,
            mut ui_emitter_speed,
            mut ui_emitter_lifetime,
            mut ui_emitter_start_size,
            mut ui_emitter_end_size,
            mut ui_emitter_start_color,
            mut ui_emitter_end_color,
            mut ui_particle_max_spawn_per_frame,
            mut ui_particle_max_total,
            mut ui_particle_max_emitter_backlog,
            mut selected_entity,
            mut selection_details,
            prev_selected_entity,
            prev_gizmo_interaction,
            mut selection_changed,
            mut gizmo_changed,
            cursor_screen,
            cursor_world_2d,
            cursor_ray,
            hovered_scale_kind,
            window_size,
            mesh_camera_for_ui,
            camera_position,
            camera_zoom,
            camera_bookmarks,
            active_camera_bookmark,
            camera_follow_target,
            mut camera_bookmark_input,
            mesh_keys,
            environment_options,
            active_environment,
            mut debug_show_spatial_hash,
            mut debug_show_colliders,
            spatial_hash_rects,
            collider_rects,
            scene_history_list,
            atlas_snapshot,
            mesh_snapshot,
            clip_snapshot,
            recent_events,
            audio_triggers,
            mut audio_enabled,
            audio_health,
            particle_budget,
            spatial_metrics,
            sprite_perf_sample,
            sprite_eval_ms,
            sprite_pack_ms,
            sprite_upload_ms,
            mut id_lookup_input,
            mut id_lookup_active,
            binary_prefabs_enabled,
            prefab_entries,
            mut prefab_name_input,
            mut prefab_format,
            prefab_status,
            mut script_debugger,
        } = params;

        let mut camera_bookmark_select: Option<Option<String>> = None;
        let mut camera_bookmark_save: Option<String> = None;
        let mut camera_bookmark_delete: Option<String> = None;
        let mut camera_follow_selection = false;
        let mut camera_follow_clear = false;
        let (
            preview_mesh_key,
            mesh_control_mode_state,
            mesh_frustum_lock_state,
            mesh_orbit_radius,
            mesh_freefly_speed_state,
            mesh_status_message,
        ) = if let Some(plugin) = self.mesh_preview_plugin() {
            (
                plugin.preview_mesh_key().to_string(),
                plugin.mesh_control_mode(),
                plugin.mesh_frustum_lock(),
                plugin.mesh_orbit().radius,
                plugin.mesh_freefly_speed(),
                plugin.mesh_status().map(|s| s.to_string()),
            )
        } else {
            (String::new(), MeshControlMode::Disabled, false, 0.0, 0.0, None)
        };
        let mut actions = UiActions::default();
        let mut viewport_mode_request: Option<ViewportCameraMode> = None;
        let mut mesh_control_request: Option<MeshControlMode> = None;
        let mut gpu_export_requested = false;
        let persistent_materials: HashSet<String> = self
            .mesh_preview_plugin()
            .map(|plugin| plugin.persistent_materials().iter().cloned().collect())
            .unwrap_or_default();
        let persistent_meshes: HashSet<String> = self
            .mesh_preview_plugin()
            .map(|plugin| plugin.persistent_meshes().iter().cloned().collect())
            .unwrap_or_default();
        let mut mesh_frustum_request: Option<bool> = None;
        let mut mesh_frustum_snap = false;
        let mut mesh_reset_request = false;
        let mut mesh_selection_request: Option<String> = None;
        let mut environment_selection_request: Option<String> = None;
        let mut frame_selection_request = false;
        let mut id_lookup_request: Option<String> = None;
        let mut pending_viewport: Option<(Vec2, Vec2)> = None;
        let mut left_panel_width_px = 0.0;
        let mut right_panel_width_px = 0.0;

        let animation_snapshot = self.ecs.world.resource::<AnimationTime>().clone();
        let mut animation_scale = animation_snapshot.scale;
        let mut animation_paused = animation_snapshot.paused;
        let mut animation_fixed_enabled = animation_snapshot.fixed_step.is_some();
        let mut animation_fixed_step = animation_snapshot.fixed_step.unwrap_or(1.0 / 60.0);
        let animation_remainder = animation_snapshot.remainder;
        let mut animation_group_entries: Vec<(String, f32)> =
            animation_snapshot.group_scales.iter().map(|(name, value)| (name.clone(), *value)).collect();
        animation_group_entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut ui_pixels_per_point = self.egui_ctx.pixels_per_point();
        if let Some(screen) = self.egui_screen.as_mut() {
            screen.pixels_per_point = ui_pixels_per_point;
        }

        let mut vsync_toggle_request: Option<bool> = None;

        let mut script_debugger_output = ScriptDebuggerOutput {
            open: script_debugger.open,
            repl_input: script_debugger.repl_input.clone(),
            repl_history_index: script_debugger.repl_history_index,
            focus_repl: script_debugger.focus_repl,
            submit_command: None,
            clear_console: false,
            set_enabled: None,
            set_paused: None,
            step_once: false,
            reload: false,
        };

        let shadow_pass_metric =
            self.analytics_plugin().and_then(|analytics| analytics.gpu_pass_metric("Shadow pass"));
        let mesh_pass_metric =
            self.analytics_plugin().and_then(|analytics| analytics.gpu_pass_metric("Mesh pass"));
        let plugin_capability_metrics_snapshot = self
            .analytics_plugin()
            .map(|analytics| analytics.plugin_capability_metrics())
            .unwrap_or_else(|| Arc::new(HashMap::new()));
        let plugin_capability_events_snapshot =
            self.analytics_plugin().map(|analytics| analytics.plugin_capability_events()).unwrap_or_default();
        let plugin_asset_readback_log =
            self.analytics_plugin().map(|analytics| analytics.plugin_asset_readbacks()).unwrap_or_default();
        let plugin_watchdog_log =
            self.analytics_plugin().map(|analytics| analytics.plugin_watchdog_events()).unwrap_or_default();
        let animation_validation_log: Arc<[AnimationValidationEvent]> =
            if let Some(analytics) = self.analytics_plugin_mut() {
                analytics.animation_validation_events_arc()
            } else {
                Arc::from([])
            };
        let animation_budget_sample =
            self.analytics_plugin().and_then(|analytics| analytics.animation_budget_sample());
        let light_cluster_metrics_overlay =
            self.analytics_plugin().and_then(|analytics| analytics.light_cluster_metrics());
        let keyframe_editor_usage =
            self.analytics_plugin().map(|analytics| analytics.keyframe_editor_usage());
        let keyframe_event_log: Arc<[KeyframeEditorEvent]> =
            if let Some(analytics) = self.analytics_plugin_mut() {
                analytics.keyframe_editor_events_arc()
            } else {
                Arc::from([])
            };

        let mut keyframe_panel_toggle_event: Option<KeyframeEditorEventKind> = None;
        let mut editor_settings_dirty = false;
        let keyframe_panel_ctx = self.egui_ctx.clone();
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            let left_panel =
                egui::SidePanel::left("kestrel_left_panel").default_width(340.0).show(ctx, |ui| {
                    egui::CollapsingHeader::new("Stats").default_open(true).show(ui, |ui| {
                        ui.label(format!("Entities: {}", entity_count));
                        ui.label(format!("Instances drawn: {}", instances_drawn));
                        let mut checkbox_state = vsync_enabled;
                        if ui.checkbox(&mut checkbox_state, "Enable VSync").changed() {
                            vsync_enabled = checkbox_state;
                            vsync_toggle_request = Some(checkbox_state);
                        }
                        ui.separator();
                        ui.label("Frame time (ms)");
                        let hist = eplot::Plot::new("fps_plot").height(120.0).include_y(0.0).include_y(40.0);
                        hist.show(ui, |plot_ui| {
                            plot_ui.line(eplot::Line::new(
                                "ms/frame",
                                eplot::PlotPoints::from(hist_points.as_ref()),
                            ));
                        });
                        ui.label("Target: 16.7ms for 60 FPS");
                        #[cfg(feature = "alloc_profiler")]
                        if let Some(delta) = allocation_delta {
                            let allocated_kb = delta.allocated_bytes as f64 / 1024.0;
                            let deallocated_kb = delta.deallocated_bytes as f64 / 1024.0;
                            let net_kb = delta.net_bytes() as f64 / 1024.0;
                            ui.label(format!(
                                "Alloc Δ: +{:.2} KB / -{:.2} KB (net {:+.2} KB)",
                                allocated_kb, deallocated_kb, net_kb
                            ));
                        }
                        ui.separator();
                        if shadow_pass_metric.is_some() || mesh_pass_metric.is_some() {
                            egui::CollapsingHeader::new("GPU Pass Baselines").default_open(false).show(
                                ui,
                                |ui| {
                                    for metric in [shadow_pass_metric, mesh_pass_metric].into_iter().flatten()
                                    {
                                        ui.label(format!(
                                            "{:<12} {:>5.2} ms (avg {:>5.2} ms over {} frames)",
                                            metric.label,
                                            metric.latest_ms,
                                            metric.average_ms,
                                            metric.sample_count
                                        ));
                                    }
                                },
                            );
                            ui.separator();
                        }
                        let metrics = self.renderer.light_cluster_metrics();
                        egui::CollapsingHeader::new("Light Culling").default_open(false).show(ui, |ui| {
                            ui.label(format!(
                                "Lights: {} visible / {} total (culled {})",
                                metrics.visible_lights,
                                metrics.total_lights,
                                metrics.culled_lights()
                            ));
                            ui.label(format!(
                                "Grid: {}x{}x{} (active {} / {})",
                                metrics.grid_dims[0],
                                metrics.grid_dims[1],
                                metrics.grid_dims[2],
                                metrics.active_clusters,
                                metrics.total_clusters
                            ));
                            ui.label(format!(
                                "Avg lights/cluster: {:.2} (max {})",
                                metrics.average_lights_per_cluster, metrics.max_lights_per_cluster
                            ));
                            if metrics.overflow_clusters > 0 {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 140, 0),
                                    format!("Cluster overflow events: {}", metrics.overflow_clusters),
                                );
                            } else {
                                ui.label("Cluster overflow events: 0");
                            }
                            if metrics.truncated_lights > 0 {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 90, 90),
                                    format!(
                                        "Lights over budget: {} (max {})",
                                        metrics.truncated_lights, LIGHT_CLUSTER_MAX_LIGHTS
                                    ),
                                );
                            } else {
                                ui.label("Lights over budget: 0");
                            }
                        });
                        ui.separator();
                        if let Some(metrics) = particle_budget {
                            egui::CollapsingHeader::new("Particle Budget").default_open(false).show(
                                ui,
                                |ui| {
                                    let utilization = metrics.cap_utilization() * 100.0;
                                    ui.label(format!(
                                        "Active: {} / {} ({utilization:.1}%)",
                                        metrics.active_particles, metrics.max_total
                                    ));
                                    ui.label(format!(
                                        "Spawn budget: {} / {} available",
                                        metrics.available_spawn_this_frame, metrics.max_spawn_per_frame
                                    ));
                                    if metrics.total_emitters > 0 {
                                        ui.label(format!(
                                            "Emitters: {} (avg backlog {:.1} / {:.0}, max {:.1})",
                                            metrics.total_emitters,
                                            metrics.average_backlog(),
                                            metrics.emitter_backlog_limit,
                                            metrics.emitter_backlog_max_observed
                                        ));
                                    } else {
                                        ui.label("Emitters: none active");
                                    }
                                },
                            );
                            ui.separator();
                        }
                        egui::CollapsingHeader::new("Sprite Animation Perf").default_open(false).show(
                            ui,
                            |ui| {
                                let warn_color = egui::Color32::from_rgb(255, 140, 0);
                                if let Some(perf) = sprite_perf_sample {
                                    if perf.total_animators() == 0 {
                                        ui.label("No sprite animators updated last frame.");
                                    } else {
                                        let slow_pct = perf.slow_ratio() * 100.0;
                                        let slow_text =
                                            format!("Slow bucket: {} ({slow_pct:.2}%)", perf.slow_animators);
                                        if perf.slow_ratio_streak >= 60 && slow_pct > 1.0 {
                                            ui.colored_label(warn_color, slow_text);
                                        } else {
                                            ui.label(slow_text);
                                        }
                                        ui.label(format!("Fast bucket: {}", perf.fast_animators));
                                        ui.label(format!(
                                            "Δt mix – variable: {} | fixed: {}",
                                            perf.var_dt_animators, perf.const_dt_animators
                                        ));
                                        ui.label(format!(
                                            "Ping-pong: {} | Event-heavy: {}",
                                            perf.ping_pong_animators, perf.events_heavy_animators
                                        ));
                                        ui.label(format!(
                                            "Events emitted: {} (coalesced {})",
                                            perf.events_emitted, perf.events_coalesced
                                        ));
                                        ui.label(format!("Modulo fallbacks: {}", perf.mod_or_div_calls));
                                        if perf.simd_supported && perf.fast_animators > 0 {
                                            let tail_pct = perf.tail_scalar_ratio() * 100.0;
                                            let lanes_text = format!(
                                                "SIMD lanes 8/4/tail: {}/{}/{} (tail {:.1}%)",
                                                perf.simd_lanes_8,
                                                perf.simd_lanes_4,
                                                perf.simd_tail_scalar,
                                                tail_pct
                                            );
                                            if perf.tail_scalar_streak >= 60 && tail_pct > 5.0 {
                                                ui.colored_label(warn_color, lanes_text);
                                            } else {
                                                ui.label(lanes_text);
                                            }
                                        } else if perf.simd_supported {
                                            ui.label("SIMD lanes: no fast animators recorded");
                                        } else {
                                            ui.label("SIMD lanes: scalar path (feature disabled)");
                                        }
                                    }
                                } else {
                                    ui.label("No sprite perf samples recorded yet.");
                                }
                            },
                        );
                        ui.separator();
                        egui::CollapsingHeader::new("Sprite Stage Timings").default_open(false).show(
                            ui,
                            |ui| {
                                sprite_stage_bar(
                                    ui,
                                    "Eval (sys_drive_sprite_animations)",
                                    sprite_eval_ms,
                                    0.205,
                                );
                                sprite_stage_bar(
                                    ui,
                                    "Pack (sys_apply_sprite_frame_states)",
                                    sprite_pack_ms,
                                    0.050,
                                );
                                sprite_stage_bar(ui, "Upload (Sprite GPU pass)", sprite_upload_ms, 0.100);
                            },
                        );
                        ui.separator();
                        egui::CollapsingHeader::new("Spatial Index").default_open(false).show(ui, |ui| {
                            if let Some(metrics) = spatial_metrics {
                                ui.label(format!(
                                    "Mode: {:?} | Cells: {} | Avg occ {:.2} | Max {}",
                                    metrics.mode,
                                    metrics.occupied_cells,
                                    metrics.average_occupancy,
                                    metrics.max_cell_occupancy
                                ));
                                if metrics.mode == SpatialMode::Quadtree {
                                    ui.label(format!("Quadtree nodes: {}", metrics.quadtree_nodes));
                                }
                            } else {
                                ui.label("Metrics unavailable.");
                            }
                            if ui.checkbox(&mut ui_spatial_use_quadtree, "Enable quadtree fallback").changed()
                            {
                                self.inspector_status = Some(if ui_spatial_use_quadtree {
                                    "Quadtree fallback enabled.".to_string()
                                } else {
                                    "Quadtree fallback disabled.".to_string()
                                });
                            }
                            let mut threshold = ui_spatial_density_threshold;
                            if ui
                                .add(
                                    egui::DragValue::new(&mut threshold)
                                        .speed(0.1)
                                        .range(1.0..=64.0)
                                        .prefix("Density threshold "),
                                )
                                .changed()
                            {
                                ui_spatial_density_threshold = threshold.max(1.0);
                            }
                            if ui.button("Find entity by ID...").clicked() {
                                id_lookup_active = true;
                            }
                        });
                        if !plugin_capability_metrics_snapshot.is_empty() {
                            ui.separator();
                            ui.label("Plugin Capability Metrics");
                            let mut rows = plugin_capability_metrics_snapshot.iter().collect::<Vec<_>>();
                            rows.sort_by(|a, b| a.0.cmp(b.0));
                            for (plugin, log) in rows {
                                let (color, summary) = capability_violation_summary(Some(log));
                                ui.horizontal(|ui| {
                                    ui.label(format!("{plugin}:"));
                                    ui.colored_label(color, summary);
                                    if let Some(last) = log.last_capability {
                                        ui.small(format!("last missing: {}", last.label()));
                                    }
                                });
                            }
                        }
                        if !plugin_capability_events_snapshot.is_empty() {
                            ui.separator();
                            ui.label("Capability Violations");
                            for event in plugin_capability_events_snapshot.iter().take(6) {
                                let ago = event
                                    .timestamp
                                    .elapsed()
                                    .map(|duration| format!("{:.1}s ago", duration.as_secs_f32()))
                                    .unwrap_or_else(|_| "just now".to_string());
                                ui.small(format!(
                                    "[{}] {} attempted {}",
                                    ago,
                                    event.plugin,
                                    event.capability.label()
                                ));
                            }
                        }
                        if !animation_validation_log.is_empty() {
                            ui.separator();
                            ui.label("Animation Validation Alerts");
                            for event in animation_validation_log.iter().take(6) {
                                let color = animation_validation_color(event.severity);
                                ui.colored_label(
                                    color,
                                    format!(
                                        "[{}] {} - {}",
                                        event.severity,
                                        event.path.display(),
                                        event.message
                                    ),
                                );
                            }
                        }
                        ui.separator();
                        let panel_open = self.animation_keyframe_panel.is_open();
                        let button_label =
                            if panel_open { "Hide Keyframe Editor" } else { "Open Keyframe Editor" };
                        if ui.button(button_label).clicked() {
                            self.animation_keyframe_panel.toggle();
                            let event = if panel_open {
                                KeyframeEditorEventKind::PanelClosed
                            } else {
                                KeyframeEditorEventKind::PanelOpened
                            };
                            keyframe_panel_toggle_event = Some(event);
                        }
                        if let Some(usage) = keyframe_editor_usage {
                            render_keyframe_editor_usage(ui, usage, keyframe_event_log.as_ref());
                        }
                        ui.separator();
                        egui::CollapsingHeader::new("Animation Time").default_open(false).show(ui, |ui| {
                            ui.checkbox(&mut animation_paused, "Pause playback");
                            ui.add(egui::Slider::new(&mut animation_scale, 0.0..=4.0).text("Global scale"));
                            ui.horizontal(|ui| {
                                let mut enabled = animation_fixed_enabled;
                                if ui.checkbox(&mut enabled, "Fixed step (s)").changed() {
                                    animation_fixed_enabled = enabled;
                                }
                                let response = ui.add_enabled(
                                    animation_fixed_enabled,
                                    egui::DragValue::new(&mut animation_fixed_step)
                                        .speed(0.001)
                                        .range(0.001..=0.5)
                                        .suffix(" s"),
                                );
                                if response.changed() {
                                    animation_fixed_step = animation_fixed_step.max(0.0);
                                }
                            });
                            ui.label(format!("Accumulated remainder: {:.4} s", animation_remainder));
                            ui.separator();
                            if animation_group_entries.is_empty() {
                                ui.small("No group overrides active.");
                            } else {
                                ui.label("Group overrides");
                                let mut remove_indices = Vec::new();
                                for (index, entry) in animation_group_entries.iter_mut().enumerate() {
                                    let (group_name, value) = entry;
                                    let mut remove_flag = false;
                                    ui.horizontal(|ui| {
                                        ui.label(group_name.as_str());
                                        if ui
                                            .add(
                                                egui::Slider::new(value, 0.0..=4.0)
                                                    .clamping(SliderClamping::Always)
                                                    .text("Scale"),
                                            )
                                            .changed()
                                        {
                                            *value = value.max(0.0);
                                        }
                                        if ui.button("Remove").clicked() {
                                            remove_flag = true;
                                        }
                                    });
                                    if remove_flag {
                                        remove_indices.push(index);
                                    }
                                }
                                for index in remove_indices.into_iter().rev() {
                                    animation_group_entries.remove(index);
                                }
                                ui.small("Setting a group to 1.0 clears the override on apply.");
                            }
                            ui.separator();
                            ui.label("Add / update group override");
                            ui.horizontal(|ui| {
                                ui.label("Group");
                                ui.text_edit_singleline(&mut self.animation_group_input);
                            });
                            ui.horizontal(|ui| {
                                ui.label("Scale");
                                ui.add(
                                    egui::Slider::new(&mut self.animation_group_scale_input, 0.0..=4.0)
                                        .clamping(SliderClamping::Always)
                                        .text("x"),
                                );
                                if ui.button("Apply").clicked() {
                                    let name = self.animation_group_input.trim();
                                    if !name.is_empty() {
                                        let value = self.animation_group_scale_input.max(0.0);
                                        if let Some(entry) = animation_group_entries
                                            .iter_mut()
                                            .find(|(existing, _)| existing == name)
                                        {
                                            entry.1 = value;
                                        } else {
                                            animation_group_entries.push((name.to_string(), value));
                                            animation_group_entries.sort_by(|a, b| a.0.cmp(&b.0));
                                        }
                                        self.animation_group_input.clear();
                                        self.animation_group_scale_input = 1.0;
                                    }
                                }
                            });
                            ui.small("Group overrides drive per-tag multipliers for sprite animations.");
                        });
                        egui::CollapsingHeader::new("Profiler").default_open(false).show(ui, |ui| {
                            ui.monospace(frame_summary_text(frame_timing_sample.as_ref()));
                            if system_timings.is_empty() {
                                ui.label("System timings unavailable");
                            } else {
                                egui::Grid::new("system_profiler_grid").striped(true).show(ui, |ui| {
                                    ui.label("System");
                                    ui.label("Last (ms)");
                                    ui.label("Avg (ms)");
                                    ui.label("Max (ms)");
                                    ui.label("Samples");
                                    ui.end_row();
                                    for timing in system_timings.iter().take(12) {
                                        ui.label(timing.name);
                                        let values = system_row_strings(timing);
                                        ui.label(&values[0]);
                                        ui.label(&values[1]);
                                        ui.label(&values[2]);
                                        ui.label(&values[3]);
                                        ui.end_row();
                                    }
                                });
                            }
                        });
                    });

                    egui::CollapsingHeader::new("Debug Overlays").default_open(false).show(ui, |ui| {
                        if self.viewport_camera_mode != ViewportCameraMode::Ortho2D {
                            ui.label("Overlays render in the 2D viewport.");
                        }
                        ui.checkbox(&mut debug_show_spatial_hash, "Spatial hash cells");
                        ui.checkbox(&mut debug_show_colliders, "Collider bounds");
                    });

                    egui::CollapsingHeader::new("UI & Camera").default_open(false).show(ui, |ui| {
                        if ui.add(egui::Slider::new(&mut ui_scale, 0.5..=2.0).text("UI scale")).changed() {
                            ui_scale = ui_scale.clamp(0.5, 2.0);
                            self.egui_ctx.set_pixels_per_point(base_pixels_per_point * ui_scale);
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
                        if let Some(status) = self.sprite_guardrail_status.as_ref() {
                            ui.colored_label(egui::Color32::from_rgb(255, 180, 80), status);
                        }
                        ui.separator();
                        ui.label("Zoom guardrails");
                        let mut guardrail_dirty = false;
                        if ui
                            .add(
                                egui::Slider::new(&mut self.ui_camera_zoom_min, 0.05..=10.0)
                                    .text("Min zoom")
                                    .logarithmic(true),
                            )
                            .changed()
                        {
                            guardrail_dirty = true;
                        }
                        if ui
                            .add(
                                egui::Slider::new(&mut self.ui_camera_zoom_max, 0.1..=20.0)
                                    .text("Max zoom")
                                    .logarithmic(true),
                            )
                            .changed()
                        {
                            guardrail_dirty = true;
                        }
                        if ui
                            .add(
                                egui::Slider::new(&mut self.ui_sprite_guard_pixels, 256.0..=8192.0)
                                    .text("Sprite guard (px)")
                                    .logarithmic(true),
                            )
                            .changed()
                        {
                            guardrail_dirty = true;
                        }
                        let mut guard_mode = self.ui_sprite_guard_mode;
                        egui::ComboBox::from_id_salt("sprite_guardrail_mode")
                            .selected_text(guard_mode.label())
                            .show_ui(ui, |ui| {
                                for mode in [
                                    SpriteGuardrailMode::Off,
                                    SpriteGuardrailMode::Warn,
                                    SpriteGuardrailMode::Clamp,
                                    SpriteGuardrailMode::Strict,
                                ] {
                                    let label = mode.label();
                                    if ui.selectable_label(guard_mode == mode, label).clicked() {
                                        guard_mode = mode;
                                    }
                                }
                            });
                        if guard_mode != self.ui_sprite_guard_mode {
                            self.ui_sprite_guard_mode = guard_mode;
                            guardrail_dirty = true;
                        }
                        if guardrail_dirty {
                            editor_settings_dirty = true;
                        }
                        ui.separator();
                        ui.label("Camera bookmarks");
                        let combo_label = if let Some(target) = camera_follow_target.as_ref() {
                            format!("Following {}", target)
                        } else if let Some(active) = active_camera_bookmark.as_ref() {
                            format!("Bookmark: {active}")
                        } else {
                            "Free camera".to_string()
                        };
                        egui::ComboBox::from_id_salt("camera_bookmark_selector")
                            .selected_text(combo_label)
                            .show_ui(ui, |ui| {
                                let free_selected =
                                    camera_follow_target.is_none() && active_camera_bookmark.is_none();
                                if ui.selectable_label(free_selected, "Free camera").clicked() {
                                    camera_bookmark_select = Some(None);
                                }
                                for bookmark in &camera_bookmarks {
                                    let selected = camera_follow_target.is_none()
                                        && active_camera_bookmark.as_deref() == Some(bookmark.name.as_str());
                                    if ui.selectable_label(selected, bookmark.name.as_str()).clicked() {
                                        camera_bookmark_select = Some(Some(bookmark.name.clone()));
                                    }
                                }
                            });
                        ui.horizontal(|ui| {
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut camera_bookmark_input)
                                    .hint_text("Bookmark name"),
                            );
                            let trimmed = camera_bookmark_input.trim().to_string();
                            let can_save = !trimmed.is_empty();
                            if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && can_save {
                                camera_bookmark_save = Some(trimmed.clone());
                            }
                            if ui.add_enabled(can_save, egui::Button::new("Save / Overwrite")).clicked() {
                                camera_bookmark_save = Some(trimmed);
                            }
                        });
                        if let Some(active) = active_camera_bookmark.as_ref() {
                            ui.horizontal(|ui| {
                                if ui.button("Update Active").clicked() {
                                    camera_bookmark_save = Some(active.clone());
                                }
                                if ui.button("Delete Active").clicked() {
                                    camera_bookmark_delete = Some(active.clone());
                                }
                            });
                        }
                        ui.separator();
                        ui.label("Camera follow");
                        let follow_label = camera_follow_target
                            .as_ref()
                            .map(|id| format!("Following entity {id}"))
                            .unwrap_or_else(|| "Following entity: None".to_string());
                        ui.label(follow_label);
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(selection_details.is_some(), |ui| {
                                if ui.button("Follow Selection").clicked() {
                                    camera_follow_selection = true;
                                }
                            });
                            ui.add_enabled_ui(camera_follow_target.is_some(), |ui| {
                                if ui.button("Clear Follow").clicked() {
                                    camera_follow_clear = true;
                                }
                            });
                        });
                    });
                    // Lighting controls moved to the right panel.

                    // Spawn controls moved to the right panel.

                    egui::CollapsingHeader::new("Scripts").default_open(false).show(ui, |ui| {
                        if script_debugger.available {
                            if let Some(path) = script_debugger.script_path.as_ref() {
                                ui.label(format!("Path: {path}"));
                            }
                            let mut enabled = script_debugger.enabled;
                            if ui.checkbox(&mut enabled, "Enable scripts").changed() {
                                script_debugger.enabled = enabled;
                                script_debugger_output.set_enabled = Some(enabled);
                            }
                            let mut paused = script_debugger.paused;
                            if ui
                                .checkbox(&mut paused, "Pause updates")
                                .on_hover_text("Stop invoking update; use Step to run once while paused.")
                                .changed()
                            {
                                script_debugger.paused = paused;
                                script_debugger_output.set_paused = Some(paused);
                            }
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(script_debugger.paused, |ui| {
                                    if ui.button("Step").clicked() {
                                        script_debugger_output.step_once = true;
                                    }
                                });
                                if ui.button("Reload").clicked() {
                                    script_debugger_output.reload = true;
                                }
                                if ui.button("Open debugger").clicked() {
                                    script_debugger.open = true;
                                }
                            });
                            if let Some(err) = script_debugger.last_error.as_ref() {
                                ui.colored_label(egui::Color32::RED, format!("Error: {err}"));
                            } else if script_debugger.enabled {
                                let status = if script_debugger.paused { "paused" } else { "running" };
                                ui.label(format!("Scripts {status}"));
                            } else {
                                ui.label("Scripts disabled");
                            }
                        } else {
                            ui.label("Script plugin unavailable");
                        }
                    });
                    ui.separator();
                    {
                        let inspector_ctx = entity_inspector::InspectorAppContext {
                            ecs: &mut self.ecs,
                            gizmo_mode: &mut self.gizmo_mode,
                            gizmo_interaction: &mut self.gizmo_interaction,
                            input: &self.input,
                            inspector_status: &mut self.inspector_status,
                            material_registry: &mut self.material_registry,
                            mesh_registry: &mut self.mesh_registry,
                            scene_material_refs: &mut self.scene_material_refs,
                            assets: &self.assets,
                        };
                        entity_inspector::show_entity_inspector(
                            inspector_ctx,
                            ui,
                            &mut selected_entity,
                            &mut selection_details,
                            &mut id_lookup_input,
                            &mut id_lookup_active,
                            &mut frame_selection_request,
                            &persistent_materials,
                            &mut actions,
                        );
                    }
                });

            let mut lookup_open = id_lookup_active;
            let mut lookup_submit: Option<String> = None;
            let mut lookup_close = false;
            egui::Window::new("Entity Lookup")
                .open(&mut lookup_open)
                .resizable(false)
                .collapsible(false)
                .default_width(320.0)
                .anchor(egui::Align2::CENTER_TOP, [0.0, 40.0])
                .show(ctx, |ui| {
                    ui.label("Paste an entity ID to jump selection to that entity.");
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut id_lookup_input)
                            .hint_text("entity::...")
                            .desired_width(260.0),
                    );
                    let submitted = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let mut triggered = submitted;
                    ui.horizontal(|ui| {
                        if ui.button("Select").clicked() {
                            triggered = true;
                        }
                        if ui.button("Close").clicked() {
                            lookup_close = true;
                        }
                    });
                    if triggered {
                        let trimmed = id_lookup_input.trim();
                        if !trimmed.is_empty() {
                            lookup_submit = Some(trimmed.to_string());
                        }
                    }
                });
            if let Some(request) = lookup_submit {
                id_lookup_request = Some(request);
                lookup_open = false;
            }
            if lookup_close {
                lookup_open = false;
            }
            id_lookup_active = lookup_open;

            if script_debugger.open {
                let mut debugger_open = script_debugger.open;
                egui::Window::new("Script Debugger")
                    .open(&mut debugger_open)
                    .resizable(true)
                    .default_width(460.0)
                    .min_height(360.0)
                    .show(ctx, |ui| {
                        if !script_debugger.available {
                            ui.label("Script plugin unavailable.");
                            return;
                        }
                        if let Some(path) = script_debugger.script_path.as_ref() {
                            ui.label(format!("Path: {path}"));
                        }
                        let mut enabled = script_debugger.enabled;
                        if ui.checkbox(&mut enabled, "Enable scripts").changed() {
                            script_debugger.enabled = enabled;
                            script_debugger_output.set_enabled = Some(enabled);
                        }
                        let mut paused = script_debugger.paused;
                        if ui.checkbox(&mut paused, "Pause updates").changed() {
                            script_debugger.paused = paused;
                            script_debugger_output.set_paused = Some(paused);
                        }
                        ui.horizontal(|ui| {
                            ui.add_enabled_ui(script_debugger.paused, |ui| {
                                if ui.button("Step").clicked() {
                                    script_debugger_output.step_once = true;
                                }
                            });
                            if ui.button("Reload").clicked() {
                                script_debugger_output.reload = true;
                            }
                            if ui.button("Clear Console").clicked() {
                                script_debugger_output.clear_console = true;
                            }
                        });
                        if let Some(err) = script_debugger.last_error.as_ref() {
                            ui.colored_label(egui::Color32::RED, format!("Error: {err}"));
                        }
                        ui.separator();
                        ui.label("Console");
                        egui::ScrollArea::vertical().stick_to_bottom(true).max_height(220.0).show(ui, |ui| {
                            let entries = script_debugger.console_entries.as_ref();
                            if entries.is_empty() {
                                ui.small("No console output yet.");
                            } else {
                                for entry in entries {
                                    let color = match entry.kind {
                                        ScriptConsoleKind::Input => egui::Color32::from_rgb(130, 200, 255),
                                        ScriptConsoleKind::Output => egui::Color32::LIGHT_GREEN,
                                        ScriptConsoleKind::Error => egui::Color32::from_rgb(255, 120, 120),
                                        ScriptConsoleKind::Log => egui::Color32::WHITE,
                                    };
                                    ui.colored_label(color, entry.text.as_str());
                                }
                            }
                        });
                        ui.separator();
                        ui.label("REPL");
                        let mut submitted = false;
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut script_debugger.repl_input)
                                .desired_width(f32::INFINITY)
                                .hint_text(
                                    "world.spawn_sprite(\"atlas\", \"spark\", 0.0, 0.0, 1.0, 0.0, 0.0);",
                                ),
                        );
                        if script_debugger.focus_repl {
                            response.request_focus();
                            script_debugger.focus_repl = false;
                        }
                        let mut history_used = false;
                        let history_len = script_debugger.repl_history.len();
                        if response.has_focus() && history_len > 0 {
                            let (up, down) =
                                ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
                            let mut index = script_debugger.repl_history_index.unwrap_or(history_len);
                            if up {
                                if index == history_len {
                                    index = history_len.saturating_sub(1);
                                } else if index > 0 {
                                    index -= 1;
                                }
                                if index < history_len {
                                    script_debugger.repl_history_index = Some(index);
                                    script_debugger.repl_input =
                                        script_debugger.repl_history.get(index).cloned().unwrap_or_default();
                                    script_debugger.focus_repl = true;
                                    history_used = true;
                                }
                            } else if down {
                                if index < history_len {
                                    index += 1;
                                    if index >= history_len {
                                        script_debugger.repl_history_index = None;
                                        script_debugger.repl_input.clear();
                                    } else {
                                        script_debugger.repl_history_index = Some(index);
                                        script_debugger.repl_input = script_debugger
                                            .repl_history
                                            .get(index)
                                            .cloned()
                                            .unwrap_or_default();
                                    }
                                    script_debugger.focus_repl = true;
                                    history_used = true;
                                }
                            }
                        }
                        if response.changed() && !history_used {
                            script_debugger.repl_history_index = None;
                        }
                        if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            submitted = true;
                        }
                        ui.horizontal(|ui| {
                            if ui.button("Run").clicked() {
                                submitted = true;
                            }
                            if ui.button("Clear Input").clicked() {
                                script_debugger.repl_input.clear();
                                script_debugger.repl_history_index = None;
                                script_debugger.focus_repl = true;
                            }
                        });
                        if submitted {
                            let command = script_debugger.repl_input.trim().to_string();
                            if !command.is_empty() {
                                script_debugger_output.submit_command = Some(command);
                                script_debugger.repl_input.clear();
                                script_debugger.repl_history_index = None;
                                script_debugger.focus_repl = true;
                            }
                        }
                        ui.separator();
                        ui.label("History");
                        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                            if script_debugger.repl_history.is_empty() {
                                ui.small("No commands yet.");
                            } else {
                                for (idx, entry) in script_debugger.repl_history.iter().enumerate().rev() {
                                    let selected = script_debugger.repl_history_index == Some(idx);
                                    if ui.selectable_label(selected, entry).clicked() {
                                        script_debugger.repl_input = entry.clone();
                                        script_debugger.repl_history_index = Some(idx);
                                        script_debugger.focus_repl = true;
                                    }
                                }
                            }
                        });
                    });
                script_debugger.open = debugger_open;
            }
            let right_panel =
                egui::SidePanel::right("kestrel_right_panel").default_width(360.0).show(ctx, |ui| {
                    ui.heading("3D Preview");
                    egui::ComboBox::from_label("Mesh asset").selected_text(&preview_mesh_key).show_ui(
                        ui,
                        |ui| {
                            for key in mesh_keys.iter() {
                                let selected = preview_mesh_key == *key;
                                if ui.selectable_label(selected, key).clicked() && !selected {
                                    mesh_selection_request = Some(key.clone());
                                }
                            }
                        },
                    );
                    let mut mesh_control_mode = mesh_control_mode_state;
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
                    if mesh_control_mode != mesh_control_mode_state {
                        mesh_control_request = Some(mesh_control_mode);
                    }
                    let mut frustum_lock = mesh_frustum_lock_state;
                    if ui.checkbox(&mut frustum_lock, "Frustum lock (L)").changed() {
                        mesh_frustum_request = Some(frustum_lock);
                    }
                    if frustum_lock && ui.button("Snap to selection").clicked() {
                        mesh_frustum_snap = true;
                    }
                    if ui.button("Reset camera").clicked() {
                        mesh_reset_request = true;
                    }
                    if ui.button("Spawn mesh entity").clicked() {
                        actions.spawn_mesh = Some(preview_mesh_key.clone());
                    }
                    match mesh_control_mode_state {
                        MeshControlMode::Orbit => {
                            ui.label(format!("Orbit radius: {:.2}", mesh_orbit_radius));
                        }
                        MeshControlMode::Freefly => {
                            ui.label(format!("Free-fly speed: {:.2}", mesh_freefly_speed_state));
                        }
                        MeshControlMode::Disabled => {
                            ui.label(format!("Orbit radius: {:.2}", mesh_orbit_radius));
                        }
                    }
                    if let Some(status) = &mesh_status_message {
                        ui.label(status);
                    } else {
                        match mesh_control_mode_state {
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
                    ui.heading("Scene");
                    ui.horizontal(|ui| {
                        ui.label("Path");
                        ui.text_edit_singleline(&mut self.ui_scene_path);
                        ui.menu_button("Recent", |menu| {
                            if scene_history_list.is_empty() {
                                menu.label("No saved paths yet");
                            } else {
                                for entry in scene_history_list.iter() {
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
                            for atlas in atlas_snapshot.iter() {
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
                                persistent_meshes.len()
                            ));
                            for mesh_key in mesh_snapshot.iter() {
                                let scope =
                                    if persistent_meshes.contains(mesh_key) { "persistent" } else { "scene" };
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
                        if clip_snapshot.is_empty() {
                            ui.small("Clips: none retained");
                        } else {
                            ui.separator();
                            ui.label(format!("Clips retained: {}", clip_snapshot.len()));
                            for clip_key in clip_snapshot.iter() {
                                let loaded = self.assets.clip(clip_key).is_some();
                                let color = if loaded {
                                    egui::Color32::LIGHT_GREEN
                                } else {
                                    egui::Color32::from_rgb(220, 120, 120)
                                };
                                let status_label = if loaded { "loaded" } else { "missing" };
                                let path_opt = self.scene_dependencies.as_ref().and_then(|deps| {
                                    deps.clip_dependencies()
                                        .find(|dep| dep.key() == clip_key.as_str())
                                        .and_then(|dep| dep.path().map(|p| p.to_string()))
                                });
                                let path_display = path_opt.as_deref().unwrap_or("n/a");
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        color,
                                        format!("- {} ({}, path={})", clip_key, status_label, path_display),
                                    );
                                    if !loaded {
                                        if ui.button("Retain").clicked() {
                                            actions.retain_clips.push((clip_key.clone(), path_opt.clone()));
                                        }
                                        if path_opt.is_none() {
                                            ui.small("no recorded path");
                                        }
                                    }
                                });
                            }
                        }
                        if let Some(deps) = self.scene_dependencies.as_ref() {
                            if let Some(environment_dep) = deps.environment_dependency() {
                                let key = environment_dep.key();
                                let loaded = self.environment_registry.definition(key).is_some();
                                let scope = if self.persistent_environments.contains(key) {
                                    "persistent"
                                } else {
                                    "scene"
                                };
                                let color = if loaded {
                                    egui::Color32::LIGHT_GREEN
                                } else {
                                    egui::Color32::from_rgb(220, 120, 120)
                                };
                                let status_label = if loaded { "loaded" } else { "missing" };
                                let path_opt = environment_dep.path().map(|p| p.to_string());
                                let path_display = path_opt.as_deref().unwrap_or("n/a");
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        color,
                                        format!(
                                            "- Environment {} ({}, {}, path={})",
                                            key, scope, status_label, path_display
                                        ),
                                    );
                                    if !loaded {
                                        if ui.button("Retain").clicked() {
                                            actions
                                                .retain_environments
                                                .push((key.to_string(), path_opt.clone()));
                                        }
                                        if path_opt.is_none() {
                                            ui.small("no recorded path");
                                        }
                                    }
                                });
                            } else {
                                ui.small("Environment: none recorded");
                            }
                        } else {
                            ui.small("Load or save a scene to populate environment dependencies.");
                        }
                        if self.scene_dependencies.is_none() {
                            ui.small("Load or save a scene to populate dependency details.");
                        }
                    });

                    ui.separator();
                    egui::CollapsingHeader::new("Lighting & Environment").default_open(false).show(
                        ui,
                        |ui| {
                            let mut lighting_dirty = false;
                            let default_dir = glam::Vec3::new(0.4, 0.8, 0.35).normalize();
                            let mut light_dir = self.ui_light_direction;
                            ui.horizontal(|ui| {
                                ui.label("Direction (XYZ)");
                                let mut changed = false;
                                changed |= ui
                                    .add(egui::DragValue::new(&mut light_dir.x).speed(0.01).range(-1.0..=1.0))
                                    .changed();
                                changed |= ui
                                    .add(egui::DragValue::new(&mut light_dir.y).speed(0.01).range(-1.0..=1.0))
                                    .changed();
                                changed |= ui
                                    .add(egui::DragValue::new(&mut light_dir.z).speed(0.01).range(-1.0..=1.0))
                                    .changed();
                                if changed {
                                    if !light_dir.is_finite() || light_dir.length_squared() < 1e-4 {
                                        light_dir = default_dir;
                                    } else {
                                        light_dir = light_dir.normalize_or_zero();
                                        if light_dir.length_squared() < 1e-4 {
                                            light_dir = default_dir;
                                        }
                                    }
                                    self.ui_light_direction = light_dir;
                                    lighting_dirty = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Color");
                                let mut color_arr = self.ui_light_color.to_array();
                                if ui.color_edit_button_rgb(&mut color_arr).changed() {
                                    self.ui_light_color = Vec3::from_array(color_arr);
                                    lighting_dirty = true;
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label("Ambient");
                                let mut ambient_arr = self.ui_light_ambient.to_array();
                                if ui.color_edit_button_rgb(&mut ambient_arr).changed() {
                                    self.ui_light_ambient = Vec3::from_array(ambient_arr);
                                    lighting_dirty = true;
                                }
                            });
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.ui_light_exposure, 0.1..=5.0)
                                        .text("Exposure")
                                        .logarithmic(true),
                                )
                                .changed()
                            {
                                self.ui_light_exposure = self.ui_light_exposure.clamp(0.1, 20.0);
                                lighting_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.ui_shadow_distance, 5.0..=200.0)
                                        .text("Shadow distance"),
                                )
                                .changed()
                            {
                                self.ui_shadow_distance = self.ui_shadow_distance.clamp(5.0, 200.0);
                                lighting_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.ui_shadow_bias, 0.0001..=0.02)
                                        .text("Shadow bias")
                                        .logarithmic(true),
                                )
                                .changed()
                            {
                                self.ui_shadow_bias = self.ui_shadow_bias.clamp(0.0001, 0.02);
                                lighting_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.ui_shadow_strength, 0.0..=1.0)
                                        .text("Shadow strength"),
                                )
                                .changed()
                            {
                                self.ui_shadow_strength = self.ui_shadow_strength.clamp(0.0, 1.0);
                                lighting_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(
                                        &mut self.ui_shadow_cascade_count,
                                        1..=MAX_SHADOW_CASCADES as u32,
                                    )
                                    .text("Shadow cascades"),
                                )
                                .changed()
                            {
                                lighting_dirty = true;
                            }
                            let mut resolution_changed = false;
                            ui.horizontal(|ui| {
                                ui.label("Shadow resolution");
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut self.ui_shadow_resolution)
                                            .suffix(" px")
                                            .speed(64.0),
                                    )
                                    .changed()
                                {
                                    resolution_changed = true;
                                }
                            });
                            if resolution_changed {
                                self.ui_shadow_resolution = self.ui_shadow_resolution.clamp(256, 8192);
                                lighting_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.ui_shadow_split_lambda, 0.0..=1.0)
                                        .text("Cascade split bias"),
                                )
                                .changed()
                            {
                                self.ui_shadow_split_lambda = self.ui_shadow_split_lambda.clamp(0.0, 1.0);
                                lighting_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.ui_shadow_pcf_radius, 0.0..=4.0)
                                        .text("PCF radius"),
                                )
                                .changed()
                            {
                                self.ui_shadow_pcf_radius = self.ui_shadow_pcf_radius.clamp(0.0, 10.0);
                                lighting_dirty = true;
                            }
                            ui.separator();
                            let cluster_metrics = self.renderer.light_cluster_metrics();
                            ui.label("Clustered light culling");
                            ui.label(format!(
                                "Runtime lights: {} visible / {} total (culled {})",
                                cluster_metrics.visible_lights,
                                cluster_metrics.total_lights,
                                cluster_metrics.culled_lights()
                            ));
                            ui.label(format!(
                                "Grid: {}×{}×{} (active {} / {})",
                                cluster_metrics.grid_dims[0],
                                cluster_metrics.grid_dims[1],
                                cluster_metrics.grid_dims[2],
                                cluster_metrics.active_clusters,
                                cluster_metrics.total_clusters
                            ));
                            ui.label(format!(
                                "Avg lights/cluster: {:.2} (max {})",
                                cluster_metrics.average_lights_per_cluster,
                                cluster_metrics.max_lights_per_cluster
                            ));
                            if cluster_metrics.overflow_clusters > 0 {
                                ui.colored_label(
                                    egui::Color32::from_rgb(255, 140, 0),
                                    format!(
                                        "Cluster overflow events this frame: {}",
                                        cluster_metrics.overflow_clusters
                                    ),
                                );
                            } else {
                                ui.label("Cluster overflow events this frame: 0");
                            }

                            ui.separator();
                            let point_light_count = self.renderer.lighting().point_lights.len();
                            let point_lights_header = format!("Point lights ({point_light_count})");
                            egui::CollapsingHeader::new(point_lights_header)
                                .default_open(point_light_count > 0 && point_light_count <= 2)
                                .show(ui, |ui| {
                                    let point_lights = &mut self.renderer.lighting_mut().point_lights;
                                    if point_lights.is_empty() {
                                        ui.label("No point lights configured.");
                                    }
                                    let mut removal: Option<usize> = None;
                                    for (index, light) in point_lights.iter_mut().enumerate() {
                                        let header = format!("Light {}", index + 1);
                                        egui::CollapsingHeader::new(header).default_open(index == 0).show(
                                            ui,
                                            |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label("Position");
                                                    ui.add(
                                                        egui::DragValue::new(&mut light.position.x)
                                                            .speed(0.1),
                                                    );
                                                    ui.add(
                                                        egui::DragValue::new(&mut light.position.y)
                                                            .speed(0.1),
                                                    );
                                                    ui.add(
                                                        egui::DragValue::new(&mut light.position.z)
                                                            .speed(0.1),
                                                    );
                                                });
                                                ui.horizontal(|ui| {
                                                    ui.label("Color");
                                                    let mut color_arr = light.color.to_array();
                                                    if ui.color_edit_button_rgb(&mut color_arr).changed() {
                                                        light.color = Vec3::from_array(color_arr);
                                                    }
                                                });
                                                ui.add(
                                                    egui::Slider::new(&mut light.radius, 0.1..=100.0)
                                                        .text("Radius")
                                                        .logarithmic(true),
                                                );
                                                ui.add(
                                                    egui::Slider::new(&mut light.intensity, 0.0..=20.0)
                                                        .text("Intensity"),
                                                );
                                                if ui.button("Remove light").clicked() {
                                                    removal = Some(index);
                                                }
                                            },
                                        );
                                        if removal.is_some() {
                                            break;
                                        }
                                    }
                                    if let Some(idx) = removal {
                                        point_lights.remove(idx);
                                    }
                                    if ui.button("Add point light").clicked() {
                                        let default_position = Vec3::new(0.0, 2.0, 0.0);
                                        let default_color = Vec3::splat(1.0);
                                        point_lights.push(ScenePointLight::new(
                                            default_position,
                                            default_color,
                                            5.0,
                                            1.0,
                                        ));
                                    }
                                });

                            ui.separator();
                            ui.label("Environment");
                            if environment_options.is_empty() {
                                ui.label("No environments available.");
                            } else {
                                let mut selected_environment = active_environment.clone();
                                let current_label = environment_options
                                    .iter()
                                    .find(|(key, _)| key == &selected_environment)
                                    .map(|(_, label)| label.as_str())
                                    .unwrap_or(selected_environment.as_str());
                                egui::ComboBox::from_id_salt("environment_select")
                                    .selected_text(current_label)
                                    .show_ui(ui, |ui| {
                                        for (key, label) in environment_options.iter() {
                                            ui.selectable_value(
                                                &mut selected_environment,
                                                key.clone(),
                                                label,
                                            );
                                        }
                                    });
                                if selected_environment != active_environment {
                                    environment_selection_request = Some(selected_environment);
                                }
                            }
                            if ui
                                .add(
                                    egui::Slider::new(&mut ui_environment_intensity, 0.0..=5.0)
                                        .text("Environment intensity")
                                        .logarithmic(true),
                                )
                                .changed()
                            {
                                ui_environment_intensity = ui_environment_intensity.clamp(0.0, 20.0);
                            }

                            if ui.button("Reset lighting").clicked() {
                                let default_shadow = SceneShadowData::default();
                                self.ui_light_direction = default_dir;
                                self.ui_light_color = Vec3::new(1.05, 0.98, 0.92);
                                self.ui_light_ambient = Vec3::splat(0.03);
                                self.ui_light_exposure = 1.0;
                                self.ui_shadow_distance = default_shadow.distance;
                                self.ui_shadow_bias = default_shadow.bias;
                                self.ui_shadow_strength = default_shadow.strength;
                                self.ui_shadow_cascade_count = default_shadow.cascade_count;
                                self.ui_shadow_resolution = default_shadow.resolution;
                                self.ui_shadow_split_lambda = default_shadow.split_lambda;
                                self.ui_shadow_pcf_radius = default_shadow.pcf_radius;
                                self.ui_environment_intensity = 1.0;
                                ui_environment_intensity = 1.0;
                                self.renderer.lighting_mut().point_lights.clear();
                                lighting_dirty = true;
                            }
                            if lighting_dirty {
                                let lighting = self.renderer.lighting_mut();
                                lighting.direction = self.ui_light_direction;
                                lighting.color = self.ui_light_color;
                                lighting.ambient = self.ui_light_ambient;
                                lighting.exposure = self.ui_light_exposure;
                                lighting.shadow_distance = self.ui_shadow_distance.clamp(1.0, 500.0);
                                lighting.shadow_bias = self.ui_shadow_bias.clamp(0.00005, 0.05);
                                lighting.shadow_strength = self.ui_shadow_strength.clamp(0.0, 1.0);
                                lighting.shadow_cascade_count =
                                    self.ui_shadow_cascade_count.clamp(1, MAX_SHADOW_CASCADES as u32);
                                lighting.shadow_resolution = self.ui_shadow_resolution.clamp(256, 8192);
                                lighting.shadow_split_lambda = self.ui_shadow_split_lambda.clamp(0.0, 1.0);
                                lighting.shadow_pcf_radius = self.ui_shadow_pcf_radius.clamp(0.0, 10.0);
                                self.renderer.mark_shadow_settings_dirty();
                            }
                        },
                    );

                    ui.separator();
                    egui::CollapsingHeader::new("Spawn & Emitters").default_open(false).show(ui, |ui| {
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
                        ui.separator();
                        ui.label("Particle caps");
                        ui.add(
                            egui::Slider::new(&mut ui_particle_max_total, 0..=10_000)
                                .text("Max total particles"),
                        );
                        ui.add(
                            egui::Slider::new(&mut ui_particle_max_spawn_per_frame, 0..=2_000)
                                .text("Max spawn per frame"),
                        );
                        ui.add(
                            egui::Slider::new(&mut ui_particle_max_emitter_backlog, 0.0..=256.0)
                                .text("Emitter backlog cap"),
                        );
                        if ui_particle_max_spawn_per_frame > ui_particle_max_total {
                            ui_particle_max_spawn_per_frame = ui_particle_max_total;
                        }
                    });

                    ui.separator();
                    let event_count = recent_events.len();
                    let latest_event_text = recent_events
                        .last()
                        .map(|event| summarize_game_event(event))
                        .map(|(text, _)| ellipsize(&text, 48));
                    let events_header_label = if event_count == 0 {
                        "Recent Events (0)".to_string()
                    } else if let Some(text) = latest_event_text {
                        format!("Recent Events ({event_count}) - {text}")
                    } else {
                        format!("Recent Events ({event_count})")
                    };
                    egui::CollapsingHeader::new(events_header_label).default_open(false).show(ui, |ui| {
                        if recent_events.is_empty() {
                            ui.label("No events recorded");
                        } else {
                            const MAX_EVENT_ROWS: usize = 6;
                            for event in recent_events.iter().rev().take(MAX_EVENT_ROWS) {
                                let (text, color) = summarize_game_event(event);
                                ui.colored_label(color, text);
                            }
                            let remaining = recent_events.len().saturating_sub(MAX_EVENT_ROWS);
                            if remaining > 0 {
                                ui.small(format!("... {remaining} older events hidden"));
                            }
                        }
                    });

                    ui.separator();
                    ui.heading("Plugins");
                    if let Some(error) = self.plugin_host.manifest_error() {
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 120, 120),
                            format!("Manifest error: {error}"),
                        );
                    }
                    if ui.button("Reload plugins").clicked() {
                        actions.reload_plugins = true;
                    }
                    ui.small("Rebuild plugin cdylibs, then click reload to rescan manifest entries.");
                    ui.small(
                        "Toggle entries below to update config/plugins.json without leaving the editor.",
                    );
                    let status_snapshot = self.plugin_manager.status_snapshot();
                    let status_slice: &[PluginStatus] = status_snapshot.as_ref();
                    let capability_metrics = self.plugin_manager.capability_metrics();
                    let asset_metrics = self.plugin_manager.asset_readback_metrics();
                    let ecs_history = self.plugin_manager.ecs_query_history();
                    let watchdog_events = self.plugin_manager.watchdog_events();
                    let mut dynamic_statuses: BTreeMap<String, &PluginStatus> = BTreeMap::new();
                    let mut builtin_statuses: Vec<&PluginStatus> = Vec::new();
                    for status in status_slice {
                        if status.dynamic {
                            dynamic_statuses.insert(status.name.clone(), status);
                        } else {
                            builtin_statuses.push(status);
                        }
                    }
                    builtin_statuses.sort_by(|a, b| a.name.cmp(&b.name));
                    if let Some(manifest) = self.plugin_host.manifest() {
                        if let Some(path) = manifest.path() {
                            ui.small(format!("Manifest: {}", path.display()));
                        }
                    }
                    let manifest_entries =
                        self.plugin_host.manifest().map(|manifest| manifest.entries().to_vec());
                    if let Some(entries) = manifest_entries {
                        if entries.is_empty() {
                            ui.label("No dynamic plugins listed in manifest.");
                        } else {
                            for entry in entries {
                                let plugin_name = entry.name.clone();
                                let mut enabled_flag = entry.enabled;
                                let mut toggled = false;
                                let status = dynamic_statuses.remove(&plugin_name);
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        if ui.checkbox(&mut enabled_flag, &plugin_name).changed() {
                                            toggled = true;
                                        }
                                        if let Some(status) = status.as_ref() {
                                            let status = *status;
                                            let (color, summary) = plugin_status_summary(status);
                                            ui.colored_label(color, summary);
                                            if let Some(version) =
                                                status.version.as_deref().or(entry.version.as_deref())
                                            {
                                                ui.small(format!("v{}", version));
                                            }
                                            show_capability_info(
                                                ui,
                                                &status.capabilities,
                                                status.trust,
                                                capability_metrics.get(&status.name),
                                            );
                                        } else if let Some(version) = entry.version.as_deref() {
                                            ui.small(format!("v{} (manifest)", version));
                                        } else {
                                            ui.small("not loaded");
                                        }
                                    });
                                    if !entry.path.trim().is_empty() {
                                        ui.small(format!("Path: {}", entry.path));
                                    }
                                    if let Some(status) = status.as_ref() {
                                        let status = *status;
                                        if !status.depends_on.is_empty() {
                                            ui.small(format!("Depends on: {}", status.depends_on.join(", ")));
                                        }
                                        if !status.provides.is_empty() {
                                            ui.small(format!("Provides: {}", status.provides.join(", ")));
                                        }
                                    }
                                    if !entry.requires_features.is_empty() {
                                        ui.small(format!(
                                            "Requires (manifest): {}",
                                            entry.requires_features.join(", ")
                                        ));
                                    }
                                    if !entry.provides_features.is_empty() {
                                        ui.small(format!(
                                            "Provides (manifest): {}",
                                            entry.provides_features.join(", ")
                                        ));
                                    }
                                    if status.is_none() {
                                        show_capability_info(
                                            ui,
                                            &entry.capabilities,
                                            entry.trust,
                                            capability_metrics.get(&plugin_name),
                                        );
                                    }
                                    plugin_debug_ui(
                                        ui,
                                        &plugin_name,
                                        asset_metrics.as_ref(),
                                        ecs_history.as_ref(),
                                        watchdog_events.as_ref(),
                                        &mut self.plugin_manager,
                                        &mut self.ui_scene_status,
                                    );
                                });
                                if toggled {
                                    actions.plugin_toggles.push(PluginToggleRequest {
                                        name: plugin_name,
                                        kind: PluginToggleKind::Dynamic { new_enabled: enabled_flag },
                                    });
                                }
                            }
                        }
                    } else {
                        ui.label("No plugin manifest loaded.");
                    }
                    if !dynamic_statuses.is_empty() {
                        ui.separator();
                        ui.small("Dynamic plugins without manifest entries:");
                        for status in dynamic_statuses.values() {
                            let status = *status;
                            let (color, summary) = plugin_status_summary(status);
                            let label =
                                format!("{} v{}", status.name, status.version.as_deref().unwrap_or("n/a"));
                            ui.colored_label(color, format!("{label} - {summary}"));
                            if !status.depends_on.is_empty() {
                                ui.small(format!("Depends on: {}", status.depends_on.join(", ")));
                            }
                            if !status.provides.is_empty() {
                                ui.small(format!("Provides: {}", status.provides.join(", ")));
                            }
                            show_capability_info(
                                ui,
                                &status.capabilities,
                                status.trust,
                                capability_metrics.get(&status.name),
                            );
                            plugin_debug_ui(
                                ui,
                                &status.name,
                                asset_metrics.as_ref(),
                                ecs_history.as_ref(),
                                watchdog_events.as_ref(),
                                &mut self.plugin_manager,
                                &mut self.ui_scene_status,
                            );
                        }
                    }
                    if !builtin_statuses.is_empty() {
                        if !dynamic_statuses.is_empty() {
                            ui.separator();
                        }
                        let manifest_loaded = self.plugin_host.manifest().is_some();
                        for status in builtin_statuses {
                            let mut enabled_flag = self
                                .plugin_host
                                .manifest()
                                .map(|manifest| !manifest.is_builtin_disabled(&status.name))
                                .unwrap_or(!matches!(status.state, PluginState::Disabled(_)));
                            let mut toggled = false;
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    if manifest_loaded {
                                        if ui.checkbox(&mut enabled_flag, &status.name).changed() {
                                            toggled = true;
                                        }
                                    } else {
                                        ui.add_enabled(false, Checkbox::new(&mut enabled_flag, &status.name));
                                    }
                                    let (color, summary) = plugin_status_summary(&status);
                                    ui.colored_label(color, summary);
                                    let version_label = status.version.as_deref().unwrap_or("n/a");
                                    ui.small(format!("v{} (built-in)", version_label));
                                });
                                if !status.depends_on.is_empty() {
                                    ui.small(format!("Depends on: {}", status.depends_on.join(", ")));
                                }
                                if !status.provides.is_empty() {
                                    ui.small(format!("Provides: {}", status.provides.join(", ")));
                                }
                                show_capability_info(
                                    ui,
                                    &status.capabilities,
                                    status.trust,
                                    capability_metrics.get(&status.name),
                                );
                                plugin_debug_ui(
                                    ui,
                                    &status.name,
                                    asset_metrics.as_ref(),
                                    ecs_history.as_ref(),
                                    watchdog_events.as_ref(),
                                    &mut self.plugin_manager,
                                    &mut self.ui_scene_status,
                                );
                            });
                            if toggled {
                                actions.plugin_toggles.push(PluginToggleRequest {
                                    name: status.name.clone(),
                                    kind: PluginToggleKind::Builtin { disable: !enabled_flag },
                                });
                            }
                        }
                        if manifest_loaded {
                            ui.small("Built-in plugin changes take effect after restarting the engine.");
                        } else {
                            ui.small("Load config/plugins.json to edit built-in toggles.");
                        }
                    } else if self.plugin_host.manifest().is_none() && dynamic_statuses.is_empty() {
                        ui.label("No plugins reported");
                    }

                    ui.separator();
                    ui.heading("GPU Timings");
                    if !self.renderer.gpu_timing_supported() {
                        ui.small("Device does not support GPU timestamp queries.");
                    } else if self.gpu_timing_history.is_empty() {
                        ui.small("No GPU timing samples captured yet.");
                    } else {
                        let mut averages: BTreeMap<&'static str, (f32, u32)> = BTreeMap::new();
                        for frame in &self.gpu_timing_history {
                            for timing in &frame.timings {
                                let entry = averages.entry(timing.label).or_insert((0.0, 0));
                                entry.0 += timing.duration_ms;
                                entry.1 += 1;
                            }
                        }
                        let latest_gpu_timings = self.gpu_timings.as_ref();
                        if !latest_gpu_timings.is_empty() {
                            for timing in latest_gpu_timings.iter() {
                                let average = averages
                                    .get(&timing.label)
                                    .map(|(sum, count)| sum / (*count as f32))
                                    .unwrap_or(timing.duration_ms);
                                ui.label(format!(
                                    "{:<20} {:>6.2} ms (avg {:>6.2} ms)",
                                    timing.label, timing.duration_ms, average
                                ));
                            }
                        }
                        if ui.button("Export GPU CSV").clicked() {
                            gpu_export_requested = true;
                        }
                        if let Some(status) = self.gpu_metrics_status.as_ref() {
                            ui.small(status.as_str());
                        }
                    }

                    if !plugin_watchdog_log.is_empty() {
                        ui.separator();
                        ui.label("Plugin Watchdog Alerts");
                        for event in plugin_watchdog_log.iter().take(6) {
                            let ago = event
                                .timestamp
                                .elapsed()
                                .map(|duration| format!("{:.1}s ago", duration.as_secs_f32()))
                                .unwrap_or_else(|_| "just now".to_string());
                            ui.small(format!(
                                "[{}] {} - {} ({:.1} ms) [{}]",
                                ago, event.plugin, event.reason, event.elapsed_ms, event.last_request
                            ));
                        }
                    }

                    if !plugin_asset_readback_log.is_empty() {
                        ui.separator();
                        ui.label("Recent Asset Readbacks");
                        for event in plugin_asset_readback_log.iter().take(6) {
                            let ago = event
                                .timestamp
                                .elapsed()
                                .map(|duration| format!("{:.1}s ago", duration.as_secs_f32()))
                                .unwrap_or_else(|_| "just now".to_string());
                            let cache_hint = if event.cache_hit { "cache" } else { "live" };
                            ui.small(format!(
                                "[{}] {} -> {}:{} ({} bytes, {:.1} ms, {})",
                                ago,
                                event.plugin,
                                event.kind,
                                event.target,
                                event.bytes,
                                event.duration_ms,
                                cache_hint
                            ));
                        }
                    }

                    ui.separator();
                    ui.heading("Prefab Shelf");
                    if let Some(status) = prefab_status.as_ref() {
                        let color = match status.kind {
                            PrefabStatusKind::Info => egui::Color32::from_rgb(120, 180, 250),
                            PrefabStatusKind::Success => egui::Color32::LIGHT_GREEN,
                            PrefabStatusKind::Warning => egui::Color32::from_rgb(230, 200, 120),
                            PrefabStatusKind::Error => egui::Color32::from_rgb(240, 120, 120),
                        };
                        ui.colored_label(color, status.message.as_str());
                    }
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        ui.add(
                            egui::TextEdit::singleline(&mut prefab_name_input).hint_text("e.g. crate_small"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Format");
                        for format in [PrefabFormat::Json, PrefabFormat::Binary] {
                            let enabled = format != PrefabFormat::Binary || binary_prefabs_enabled;
                            let label = if enabled {
                                format.label().to_string()
                            } else {
                                format!("{} (requires 'binary_scene')", format.label())
                            };
                            let button = egui::Button::new(label).selected(prefab_format == format);
                            let response = ui.add_enabled(enabled, button);
                            if enabled && response.clicked() {
                                prefab_format = format;
                            }
                        }
                    });
                    if !binary_prefabs_enabled {
                        ui.small("Enable the 'binary_scene' Cargo feature to export binary prefabs.");
                    }
                    let drop_result =
                        ui.dnd_drop_zone::<PrefabDragPayload, _>(egui::Frame::group(&ui.style()), |ui| {
                            ui.set_min_height(48.0);
                            if selected_entity.is_some() {
                                ui.label("Drag the selected entity here to save it as a prefab.");
                            } else {
                                ui.label("Select an entity, then drag it here to save a prefab.");
                            }
                        });
                    let dropped_prefab = drop_result.1;
                    if let Some(payload) = dropped_prefab {
                        let payload = (*payload).clone();
                        let mut prefab_name = prefab_name_input.trim().to_string();
                        if prefab_name.is_empty() {
                            prefab_name = format!("prefab_{}", payload.entity.index());
                            prefab_name_input = prefab_name.clone();
                        }
                        actions.save_prefab = Some(PrefabSaveRequest {
                            entity: payload.entity,
                            name: prefab_name,
                            format: prefab_format,
                        });
                    }
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        if prefab_entries.is_empty() {
                            ui.small("No prefabs saved yet.");
                        } else {
                            for entry in prefab_entries.iter() {
                                let entry_label = format!("{} ({})", entry.name, entry.format.short_label());
                                let payload =
                                    PrefabSpawnPayload { name: entry.name.clone(), format: entry.format };
                                let drag_id = egui::Id::new((
                                    "prefab_shelf_entry",
                                    entry.name.as_str(),
                                    entry.format.short_label(),
                                ));
                                ui.dnd_drag_source(drag_id, payload.clone(), |ui| {
                                    ui.label(&entry_label);
                                    ui.weak(entry.path_display.as_str());
                                });
                            }
                        }
                    });

                    ui.separator();
                    let plugin_present = self.plugin_manager.get::<AudioPlugin>().is_some();
                    let parsed_triggers: Vec<ParsedAudioTrigger> =
                        audio_triggers.iter().map(|label| parse_audio_trigger(label)).collect();
                    let mut trigger_counts: BTreeMap<AudioTriggerKind, usize> = BTreeMap::new();
                    let mut peak_force = 0.0f32;
                    for parsed in &parsed_triggers {
                        *trigger_counts.entry(parsed.kind).or_insert(0) += 1;
                        if let Some(force) = parsed.force {
                            peak_force = peak_force.max(force);
                        }
                    }
                    let trigger_summary_line = if trigger_counts.is_empty() {
                        None
                    } else {
                        Some(
                            trigger_counts
                                .iter()
                                .map(|(kind, count)| format!("{count} x {}", audio_trigger_kind_label(*kind)))
                                .collect::<Vec<_>>()
                                .join(" | "),
                        )
                    };
                    let peak_force_text =
                        (peak_force > 0.0).then(|| format!("Peak collision force: {:.1}", peak_force));
                    let latest_trigger_summary = parsed_triggers
                        .last()
                        .map(|parsed| ellipsize(&parsed.summary, 48))
                        .unwrap_or_else(|| "no recent triggers".to_string());
                    let audio_status = if !plugin_present {
                        "plugin missing"
                    } else if !audio_health.playback_available {
                        "device unavailable"
                    } else if audio_enabled {
                        "enabled"
                    } else {
                        "muted"
                    };
                    let audio_header_label = format!(
                        "Audio Debug ({}) [{}] - {}",
                        parsed_triggers.len(),
                        audio_status,
                        latest_trigger_summary
                    );
                    egui::CollapsingHeader::new(audio_header_label).default_open(false).show(ui, |ui| {
                        if let (Some(name), Some(rate)) =
                            (audio_health.device_name.as_deref(), audio_health.sample_rate_hz)
                        {
                            ui.small(format!("Device: {name} @ {rate} Hz"));
                        } else if let Some(name) = audio_health.device_name.as_deref() {
                            ui.small(format!("Device: {name}"));
                        } else if let Some(rate) = audio_health.sample_rate_hz {
                            ui.small(format!("Sample rate: {rate} Hz"));
                        }
                        if ui.checkbox(&mut audio_enabled, "Enable audio triggers").changed() {
                            if let Some(audio) = self.plugin_manager.get_mut::<AudioPlugin>() {
                                audio.set_enabled(audio_enabled);
                            }
                        }
                        if !plugin_present {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 80, 80),
                                "Audio plugin unavailable; triggers will be silent.",
                            );
                        } else if !audio_health.playback_available {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 120, 80),
                                "Audio device unavailable; triggers will be silent.",
                            );
                        }
                        if audio_health.failed_playbacks > 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(230, 180, 80),
                                format!(
                                    "Recent audio failures: {}{}",
                                    audio_health.failed_playbacks,
                                    audio_health
                                        .last_error
                                        .as_ref()
                                        .map(|msg| format!(" (last error: {msg})"))
                                        .unwrap_or_default()
                                ),
                            );
                        }
                        if let Some(summary_line) = trigger_summary_line.as_deref() {
                            ui.small(summary_line);
                        }
                        if let Some(force_text) = peak_force_text.as_deref() {
                            ui.small(force_text);
                        }
                        if ui.button("Clear audio log").clicked() {
                            if let Some(audio) = self.plugin_manager.get_mut::<AudioPlugin>() {
                                audio.clear();
                            }
                        }
                        if parsed_triggers.is_empty() {
                            ui.label("No audio triggers");
                        } else {
                            const MAX_AUDIO_ROWS: usize = 8;
                            for parsed in parsed_triggers.iter().rev().take(MAX_AUDIO_ROWS) {
                                ui.colored_label(parsed.color, parsed.summary.as_str());
                            }
                            let remaining = parsed_triggers.len().saturating_sub(MAX_AUDIO_ROWS);
                            if remaining > 0 {
                                ui.small(format!("... {remaining} older triggers hidden"));
                            }
                        }
                    });
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

            let viewport_rect_points = egui::Rect::from_min_size(
                egui::pos2(
                    viewport_origin_vec2.x / ui_pixels_per_point,
                    viewport_origin_vec2.y / ui_pixels_per_point,
                ),
                egui::vec2(
                    viewport_size_vec2.x / ui_pixels_per_point,
                    viewport_size_vec2.y / ui_pixels_per_point,
                ),
            );
            if self.egui_ctx.input(|i| i.pointer.any_released()) {
                if let Some(pointer_pos) = self.egui_ctx.pointer_interact_pos() {
                    if viewport_rect_points.contains(pointer_pos) {
                        if let Some(payload) = DragAndDrop::take_payload::<PrefabSpawnPayload>(&self.egui_ctx)
                        {
                            let payload = (*payload).clone();
                            let drop_target = match self.viewport_camera_mode {
                                ViewportCameraMode::Ortho2D => cursor_world_2d.map(PrefabDropTarget::World2D),
                                ViewportCameraMode::Perspective3D => cursor_ray
                                    .and_then(|(origin, dir)| {
                                        Self::intersect_ray_plane(origin, dir, Vec3::ZERO, Vec3::Z)
                                    })
                                    .map(PrefabDropTarget::World3D),
                            };
                            actions.instantiate_prefab = Some(PrefabInstantiateRequest {
                                name: payload.name,
                                format: payload.format,
                                drop_target,
                            });
                        }
                    }
                }
            }

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
            if self.viewport_camera_mode == ViewportCameraMode::Ortho2D {
                if debug_show_spatial_hash {
                    for (min, max) in &spatial_hash_rects {
                        if let Some((min_px_view, max_px_view)) =
                            self.camera.world_rect_to_screen_bounds(*min, *max, viewport_size_physical)
                        {
                            let min_screen = min_px_view + viewport_origin_vec2;
                            let max_screen = max_px_view + viewport_origin_vec2;
                            let cell_rect = egui::Rect::from_two_pos(
                                egui::pos2(
                                    min_screen.x / ui_pixels_per_point,
                                    min_screen.y / ui_pixels_per_point,
                                ),
                                egui::pos2(
                                    max_screen.x / ui_pixels_per_point,
                                    max_screen.y / ui_pixels_per_point,
                                ),
                            );
                            painter.rect_stroke(
                                cell_rect,
                                0.0,
                                egui::Stroke::new(
                                    1.0,
                                    egui::Color32::from_rgba_premultiplied(80, 200, 255, 80),
                                ),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                }
                if debug_show_colliders {
                    for (min, max) in &collider_rects {
                        if let Some((min_px_view, max_px_view)) =
                            self.camera.world_rect_to_screen_bounds(*min, *max, viewport_size_physical)
                        {
                            let min_screen = min_px_view + viewport_origin_vec2;
                            let max_screen = max_px_view + viewport_origin_vec2;
                            let collider_rect = egui::Rect::from_two_pos(
                                egui::pos2(
                                    min_screen.x / ui_pixels_per_point,
                                    min_screen.y / ui_pixels_per_point,
                                ),
                                egui::pos2(
                                    max_screen.x / ui_pixels_per_point,
                                    max_screen.y / ui_pixels_per_point,
                                ),
                            );
                            painter.rect_stroke(
                                collider_rect,
                                0.0,
                                egui::Stroke::new(
                                    1.5,
                                    egui::Color32::from_rgba_premultiplied(255, 140, 60, 120),
                                ),
                                egui::StrokeKind::Inside,
                            );
                        }
                    }
                }
                if let Some(sample) = animation_budget_sample {
                    draw_animation_budget_overlay(ctx, viewport_outline, sample);
                }
                if let Some(metrics) = light_cluster_metrics_overlay {
                    draw_light_cluster_overlay(ctx, viewport_outline, metrics);
                }
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
        if let Some(event) = keyframe_panel_toggle_event {
            self.log_keyframe_editor_event(event);
        }
        self.show_animation_keyframe_panel(&keyframe_panel_ctx, &animation_snapshot);

        script_debugger_output.open = script_debugger.open;
        script_debugger_output.repl_input = script_debugger.repl_input.clone();
        script_debugger_output.repl_history_index = script_debugger.repl_history_index;
        script_debugger_output.focus_repl = script_debugger.focus_repl;

        if gpu_export_requested {
            match self.export_gpu_timings_csv("target/gpu_timings.csv") {
                Ok(path) => {
                    self.gpu_metrics_status = Some(format!("GPU timings exported to {}", path.display()));
                }
                Err(err) => {
                    self.gpu_metrics_status = Some(format!("GPU timing export failed: {err}"));
                }
            }
        }

        if editor_settings_dirty {
            self.apply_editor_camera_settings();
        }

        let approx_eq = |a: f32, b: f32| (a - b).abs() <= 1e-4;
        if !approx_eq(animation_scale, animation_snapshot.scale) {
            self.ecs.set_animation_time_scale(animation_scale);
        }
        if animation_paused != animation_snapshot.paused {
            self.ecs.set_animation_time_paused(animation_paused);
        }
        let desired_fixed_step =
            if animation_fixed_enabled { Some(animation_fixed_step.max(std::f32::EPSILON)) } else { None };
        let fixed_changed = match (animation_snapshot.fixed_step, desired_fixed_step) {
            (Some(prev), Some(next)) => !approx_eq(prev, next),
            (None, Some(_)) | (Some(_), None) => true,
            (None, None) => false,
        };
        if fixed_changed {
            self.ecs.set_animation_time_fixed_step(desired_fixed_step);
        }
        let final_group_map: HashMap<String, f32> = animation_group_entries.into_iter().collect();
        for (name, value) in &final_group_map {
            match animation_snapshot.group_scales.get(name) {
                Some(prev) if approx_eq(*prev, *value) => {}
                _ => self.ecs.set_animation_group_scale(name, *value),
            }
        }
        for name in animation_snapshot.group_scales.keys() {
            if !final_group_map.contains_key(name) {
                self.ecs.set_animation_group_scale(name, 1.0);
            }
        }

        EditorUiOutput {
            full_output,
            actions,
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
            selection: SelectionResult { entity: selected_entity, details: selection_details },
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
            vsync_request: vsync_toggle_request,
            script_debugger: script_debugger_output,
            prefab_name_input,
            prefab_format,
            prefab_status,
        }
    }
}

fn frame_summary_text(sample: Option<&FrameTimingSample>) -> String {
    if let Some(sample) = sample {
        format!(
            "Frame {frame:.2} ms | Update {update:.2} ms | Fixed {fixed:.2} ms | Render {render:.2} ms | UI {ui:.2} ms",
            frame = sample.frame_ms,
            update = sample.update_ms,
            fixed = sample.fixed_ms,
            render = sample.render_ms,
            ui = sample.ui_ms
        )
    } else {
        "Frame timings unavailable".to_string()
    }
}

fn system_row_strings(timing: &SystemTimingSummary) -> [String; 4] {
    [
        format!("{:.2}", timing.last_ms),
        format!("{:.2}", timing.average_ms),
        format!("{:.2}", timing.max_ms),
        format!("{}", timing.samples),
    ]
}

fn sprite_stage_bar(ui: &mut egui::Ui, label: &str, value_ms: Option<f32>, budget_ms: f32) {
    match value_ms {
        Some(value) => {
            let ratio = (value / budget_ms).clamp(0.0, 1.0);
            let over_budget = value > budget_ms;
            let color = if over_budget {
                egui::Color32::from_rgb(220, 120, 20)
            } else {
                egui::Color32::from_rgb(120, 200, 120)
            };
            let text = format!("{label}: {value:.3} ms (budget {budget_ms:.3} ms)");
            ui.add(egui::ProgressBar::new(ratio).fill(color).text(text));
            if over_budget {
                ui.colored_label(color, format!("{} over budget by {:.3} ms", label, value - budget_ms));
            }
        }
        None => {
            if label.contains("Upload") {
                ui.label(format!("{label}: enable GPU timers to capture sprite pass"));
            } else {
                ui.label(format!("{label}: timing unavailable"));
            }
        }
    }
}

fn draw_animation_budget_overlay(
    ctx: &egui::Context,
    viewport_rect: egui::Rect,
    sample: AnimationBudgetSample,
) {
    let pos = egui::pos2(viewport_rect.left() + 10.0, viewport_rect.top() + 10.0);
    egui::Area::new(egui::Id::new("animation_budget_overlay"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .movable(false)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color.gamma_multiply(0.9))
                .stroke(egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 6));
            frame.show(ui, |ui| {
                ui.set_width(240.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("Animation HUD").strong());
                });
                ui.add_space(4.0);
                hud_budget_row(
                    ui,
                    "Sprite Eval",
                    sample.sprite_eval_ms,
                    SPRITE_EVAL_BUDGET_MS,
                    format!("{} animators", sample.sprite_animators),
                );
                hud_budget_row(
                    ui,
                    "Sprite Pack",
                    sample.sprite_pack_ms,
                    SPRITE_PACK_BUDGET_MS,
                    String::new(),
                );
                if let Some(upload) = sample.sprite_upload_ms {
                    hud_budget_row(ui, "Sprite Upload", upload, SPRITE_UPLOAD_BUDGET_MS, String::new());
                } else {
                    ui.small("Sprite Upload: GPU timers disabled");
                }
                ui.separator();
                hud_budget_row(
                    ui,
                    "Transform Clips",
                    sample.transform_eval_ms,
                    TRANSFORM_CLIP_BUDGET_MS,
                    format!("{} active clips", sample.transform_clip_count),
                );
                hud_budget_row(
                    ui,
                    "Skeletal Eval",
                    sample.skeletal_eval_ms,
                    SKELETAL_EVAL_BUDGET_MS,
                    format!("{} rigs / {} bones", sample.skeletal_instance_count, sample.skeletal_bone_count),
                );
                if let Some(palette_ms) = sample.palette_upload_ms {
                    hud_budget_row(
                        ui,
                        "Palette Upload",
                        palette_ms,
                        GPU_PALETTE_UPLOAD_BUDGET_MS,
                        format!(
                            "{} uploads ({} joints)",
                            sample.palette_upload_calls, sample.palette_uploaded_joints
                        ),
                    );
                } else {
                    ui.small("Palette Upload: no skinning this frame");
                }
            });
        });
}

fn draw_light_cluster_overlay(ctx: &egui::Context, viewport_rect: egui::Rect, metrics: LightClusterMetrics) {
    if metrics.truncated_lights == 0 {
        return;
    }
    let pos = egui::pos2(viewport_rect.left() + 10.0, viewport_rect.top() + 170.0);
    egui::Area::new(egui::Id::new("light_cluster_overlay"))
        .order(egui::Order::Foreground)
        .interactable(false)
        .movable(false)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color.gamma_multiply(0.9))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 90, 90)))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(10, 6));
            frame.show(ui, |ui| {
                ui.set_width(260.0);
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new("Lighting Budget")
                            .strong()
                            .color(egui::Color32::from_rgb(255, 120, 120)),
                    );
                });
                ui.add_space(4.0);
                ui.label(format!(
                    "Point lights over budget: {} / {}",
                    metrics.truncated_lights, LIGHT_CLUSTER_MAX_LIGHTS
                ));
                ui.label(format!(
                    "Visible lights this frame: {} ({} total)",
                    metrics.visible_lights, metrics.total_lights
                ));
                if metrics.overflow_clusters > 0 {
                    ui.small(format!("Clusters saturated: {}", metrics.overflow_clusters));
                }
                ui.small("Reduce point lights or adjust clustered-light settings to restore coverage.");
            });
        });
}

fn hud_budget_row(ui: &mut egui::Ui, label: &str, value_ms: f32, budget_ms: f32, detail: String) {
    let color = budget_color(value_ms, budget_ms);
    ui.colored_label(
        color,
        format!("{label}: {value:.3} ms (budget {budget:.3} ms)", value = value_ms, budget = budget_ms),
    );
    if !detail.is_empty() {
        ui.small(detail);
    }
}

fn budget_color(value: f32, budget: f32) -> egui::Color32 {
    if value <= budget * 0.8 {
        egui::Color32::from_rgb(120, 200, 120)
    } else if value <= budget {
        egui::Color32::from_rgb(255, 200, 80)
    } else {
        egui::Color32::from_rgb(255, 120, 80)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_summary_snapshot() {
        let sample = FrameTimingSample {
            frame_ms: 16.67,
            update_ms: 5.25,
            fixed_ms: 3.5,
            render_ms: 6.1,
            ui_ms: 1.82,
        };
        assert_eq!(
            frame_summary_text(Some(&sample)),
            "Frame 16.67 ms | Update 5.25 ms | Fixed 3.50 ms | Render 6.10 ms | UI 1.82 ms"
        );
        assert_eq!(frame_summary_text(None), "Frame timings unavailable");
    }

    #[test]
    fn system_row_snapshot() {
        let timing = SystemTimingSummary {
            name: "sys_example",
            last_ms: 0.42,
            average_ms: 0.25,
            max_ms: 1.05,
            samples: 12,
        };
        assert_eq!(
            system_row_strings(&timing),
            ["0.42".to_string(), "0.25".to_string(), "1.05".to_string(), "12".to_string()]
        );
    }
}
