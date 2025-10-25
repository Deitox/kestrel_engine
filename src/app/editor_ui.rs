use super::{App, MeshControlMode, ViewportCameraMode};
use crate::audio::AudioPlugin;
use crate::camera3d::Camera3D;
use crate::ecs::{EntityInfo, SpriteInfo};
use crate::events::GameEvent;
use crate::gizmo::{
    Axis2, GizmoInteraction, GizmoMode, ScaleHandle, ScaleHandleKind, GIZMO_ROTATE_INNER_RADIUS_PX,
    GIZMO_ROTATE_OUTER_RADIUS_PX, GIZMO_SCALE_AXIS_LENGTH_PX, GIZMO_SCALE_AXIS_THICKNESS_PX,
    GIZMO_SCALE_HANDLE_SIZE_PX, GIZMO_SCALE_INNER_RADIUS_PX, GIZMO_SCALE_OUTER_RADIUS_PX,
};
use crate::mesh_preview::{GIZMO_3D_AXIS_LENGTH_SCALE, GIZMO_3D_AXIS_MAX, GIZMO_3D_AXIS_MIN};
use crate::plugins::PluginState;

use bevy_ecs::prelude::Entity;
use egui::Key;
use egui_plot as eplot;
use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use std::collections::HashSet;
use winit::dpi::PhysicalSize;

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
    pub retain_meshes: Vec<(String, Option<String>)>,
    pub retain_environments: Vec<(String, Option<String>)>,
    pub reload_plugins: bool,
}

pub(super) struct SelectionResult {
    pub entity: Option<Entity>,
    pub details: Option<EntityInfo>,
}

pub(super) struct EditorUiParams {
    pub raw_input: egui::RawInput,
    pub base_pixels_per_point: f32,
    pub hist_points: Vec<[f64; 2]>,
    pub entity_count: usize,
    pub instances_drawn: usize,
    pub ui_scale: f32,
    pub ui_cell_size: f32,
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
    pub hovered_scale_kind: Option<ScaleHandleKind>,
    pub window_size: PhysicalSize<u32>,
    pub mesh_camera_for_ui: Camera3D,
    pub camera_position: Vec2,
    pub camera_zoom: f32,
    pub mesh_keys: Vec<String>,
    pub environment_options: Vec<(String, String)>,
    pub active_environment: String,
    pub debug_show_spatial_hash: bool,
    pub debug_show_colliders: bool,
    pub spatial_hash_rects: Vec<(Vec2, Vec2)>,
    pub collider_rects: Vec<(Vec2, Vec2)>,
    pub scene_history_list: Vec<String>,
    pub atlas_snapshot: Vec<String>,
    pub mesh_snapshot: Vec<String>,
    pub recent_events: Vec<GameEvent>,
    pub audio_triggers: Vec<String>,
    pub audio_enabled: bool,
    pub id_lookup_input: String,
    pub id_lookup_active: bool,
}

pub(super) struct EditorUiOutput {
    pub full_output: egui::FullOutput,
    pub actions: UiActions,
    pub pending_viewport: Option<(Vec2, Vec2)>,
    pub ui_scale: f32,
    pub ui_cell_size: f32,
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
    pub debug_show_spatial_hash: bool,
    pub debug_show_colliders: bool,
}

impl App {
    pub(super) fn render_editor_ui(&mut self, params: EditorUiParams) -> EditorUiOutput {
        let EditorUiParams {
            raw_input,
            base_pixels_per_point,
            hist_points,
            entity_count,
            instances_drawn,
            mut ui_scale,
            mut ui_cell_size,
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
            hovered_scale_kind,
            window_size,
            mesh_camera_for_ui,
            camera_position,
            camera_zoom,
            mut mesh_keys,
            mut environment_options,
            active_environment,
            mut debug_show_spatial_hash,
            mut debug_show_colliders,
            spatial_hash_rects,
            collider_rects,
            scene_history_list,
            atlas_snapshot,
            mesh_snapshot,
            recent_events,
            audio_triggers,
            mut audio_enabled,
            mut id_lookup_input,
            mut id_lookup_active,
        } = params;

        mesh_keys.sort();
        environment_options.sort_by(|a, b| a.1.cmp(&b.1));
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

        let mut ui_pixels_per_point = self.egui_ctx.pixels_per_point();
        if let Some(screen) = self.egui_screen.as_mut() {
            screen.pixels_per_point = ui_pixels_per_point;
        }

        let mut script_enable_request: Option<bool> = None;
        let mut script_reload_request = false;
        let mut script_panel_data = self.script_plugin().map(|plugin| {
            (
                plugin.script_path().display().to_string(),
                plugin.enabled(),
                plugin.last_error().map(|err| err.to_string()),
            )
        });

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
                        if ui.button("Find entity by ID...").clicked() {
                            id_lookup_active = true;
                        }
                    });

                    egui::CollapsingHeader::new("Debug Overlays").default_open(false).show(ui, |ui| {
                        if self.viewport_camera_mode != ViewportCameraMode::Ortho2D {
                            ui.label("Overlays render in the 2D viewport.");
                        }
                        ui.checkbox(&mut debug_show_spatial_hash, "Spatial hash cells");
                        ui.checkbox(&mut debug_show_colliders, "Collider bounds");
                    });

                    egui::CollapsingHeader::new("UI & Camera").default_open(true).show(ui, |ui| {
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
                    });
                    egui::CollapsingHeader::new("Lighting").default_open(true).show(ui, |ui| {
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
                                    for (key, label) in &environment_options {
                                        ui.selectable_value(&mut selected_environment, key.clone(), label);
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
                            self.ui_light_direction = default_dir;
                            self.ui_light_color = Vec3::new(1.05, 0.98, 0.92);
                            self.ui_light_ambient = Vec3::splat(0.03);
                            self.ui_light_exposure = 1.0;
                            self.ui_shadow_distance = 35.0;
                            self.ui_shadow_bias = 0.002;
                            self.ui_shadow_strength = 1.0;
                            self.ui_environment_intensity = 1.0;
                            ui_environment_intensity = 1.0;

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
                            self.renderer.mark_shadow_settings_dirty();
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

                    egui::CollapsingHeader::new("Scripts").default_open(false).show(ui, |ui| {
                        if let Some((path, enabled, last_error)) = script_panel_data.as_mut() {
                            ui.label(format!("Path: {}", path));
                            let mut scripts_enabled = *enabled;
                            if ui.checkbox(&mut scripts_enabled, "Enable scripts").changed() {
                                *enabled = scripts_enabled;
                                script_enable_request = Some(scripts_enabled);
                            }
                            if ui.button("Reload script").clicked() {
                                script_reload_request = true;
                                *last_error = None;
                            }
                            if let Some(err) = last_error.as_ref() {
                                ui.colored_label(egui::Color32::RED, format!("Error: {err}"));
                            } else if *enabled {
                                ui.label("Script running");
                            } else {
                                ui.label("Scripts disabled");
                            }
                        } else {
                            ui.label("Script plugin unavailable");
                        }
                    });
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

            let right_panel =
                egui::SidePanel::right("kestrel_right_panel").default_width(360.0).show(ctx, |ui| {
                    ui.heading("3D Preview");
                    egui::ComboBox::from_label("Mesh asset").selected_text(&preview_mesh_key).show_ui(
                        ui,
                        |ui| {
                            for key in &mesh_keys {
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
                            ui.horizontal(|ui| {
                                ui.label("Entity ID");
                                ui.monospace(info.scene_id.as_str());
                                if ui.button("Copy").clicked() {
                                    let id_string = info.scene_id.as_str().to_string();
                                    ui.ctx().copy_text(id_string);
                                }
                                if ui.button("Find by ID").clicked() {
                                    id_lookup_input = info.scene_id.as_str().to_string();
                                    id_lookup_active = true;
                                }
                            });
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
                                let mut material_options: Vec<(String, String)> = self
                                    .material_registry
                                    .keys()
                                    .map(|key| {
                                        let label = self
                                            .material_registry
                                            .definition(key)
                                            .map(|def| def.label.clone())
                                            .unwrap_or_else(|| key.to_string());
                                        (key.to_string(), label)
                                    })
                                    .collect();
                                material_options.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                                let mut desired_material = mesh.material.clone();
                                let selected_text = match mesh.material.as_ref() {
                                    Some(key) => {
                                        let label = material_options
                                            .iter()
                                            .find(|(candidate, _)| candidate == key)
                                            .map(|(_, label)| label.clone())
                                            .or_else(|| {
                                                self.material_registry
                                                    .definition(key)
                                                    .map(|def| def.label.clone())
                                            })
                                            .unwrap_or_else(|| key.clone());
                                        format!("{label} ({key})")
                                    }
                                    None => "Use mesh material (asset default)".to_string(),
                                };
                                egui::ComboBox::from_label("Material Override")
                                    .selected_text(selected_text)
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut desired_material,
                                            None,
                                            "Use mesh material (asset default)",
                                        );
                                        for (key, label) in &material_options {
                                            let entry_label = format!("{label} ({key})");
                                            ui.selectable_value(
                                                &mut desired_material,
                                                Some(key.clone()),
                                                entry_label,
                                            );
                                        }
                                    });
                                if desired_material != mesh.material {
                                    let previous_material = mesh.material.clone();
                                    let mut retained_new = false;
                                    let mut apply_change = true;
                                    if let Some(ref key) = desired_material {
                                        if !self.material_registry.has(key.as_str()) {
                                            self.inspector_status =
                                                Some(format!("Material '{}' not registered", key));
                                            apply_change = false;
                                        } else if let Err(err) = self.material_registry.retain(key) {
                                            self.inspector_status =
                                                Some(format!("Failed to retain material '{}': {err}", key));
                                            apply_change = false;
                                        } else {
                                            retained_new = true;
                                        }
                                    }
                                    if apply_change {
                                        if self.ecs.set_mesh_material(entity, desired_material.clone()) {
                                            inspector_refresh = true;
                                            self.inspector_status = None;
                                            if let Some(prev) = previous_material {
                                                if desired_material.as_ref() != Some(&prev) {
                                                    self.material_registry.release(&prev);
                                                }
                                            }
                                            let mut refs = persistent_materials.clone();
                                            for instance in self.ecs.collect_mesh_instances() {
                                                if let Some(material) = instance.material {
                                                    refs.insert(material);
                                                }
                                            }
                                            self.scene_material_refs = refs;
                                        } else {
                                            self.inspector_status =
                                                Some("Failed to update mesh material".to_string());
                                            if retained_new {
                                                if let Some(ref key) = desired_material {
                                                    self.material_registry.release(key);
                                                }
                                            }
                                        }
                                    } else if retained_new {
                                        if let Some(ref key) = desired_material {
                                            self.material_registry.release(key);
                                        }
                                    }
                                }
                                let mut cast_shadows = mesh.lighting.cast_shadows;
                                let mut receive_shadows = mesh.lighting.receive_shadows;
                                let mut shadow_flags_changed = false;
                                ui.horizontal(|ui| {
                                    ui.label("Shadows");
                                    if ui.checkbox(&mut cast_shadows, "Cast").changed() {
                                        shadow_flags_changed = true;
                                    }
                                    if ui.checkbox(&mut receive_shadows, "Receive").changed() {
                                        shadow_flags_changed = true;
                                    }
                                });
                                if shadow_flags_changed {
                                    if self.ecs.set_mesh_shadow_flags(entity, cast_shadows, receive_shadows) {
                                        inspector_refresh = true;
                                        self.inspector_status = None;
                                    } else {
                                        self.inspector_status =
                                            Some("Failed to update mesh shadow flags".to_string());
                                    }
                                }
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
                                persistent_meshes.len()
                            ));
                            for mesh_key in &mesh_snapshot {
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
                    ui.heading("Recent Events");
                    if recent_events.is_empty() {
                        ui.label("No events recorded");
                    } else {
                        for event in recent_events.iter().rev().take(10) {
                            ui.label(event.to_string());
                        }
                    }

                    ui.separator();
                    ui.heading("Plugins");
                    if ui.button("Reload plugins").clicked() {
                        actions.reload_plugins = true;
                    }
                    ui.small("Rebuild plugin cdylibs, then click reload to rescan manifest entries.");
                    let statuses = self.plugins.statuses();
                    if statuses.is_empty() {
                        ui.label("No plugins reported");
                    } else {
                        for status in statuses {
                            let label = format!(
                                "{} v{} ({})",
                                status.name,
                                status.version.as_deref().unwrap_or("n/a"),
                                if status.dynamic { "dynamic" } else { "built-in" }
                            );
                            match &status.state {
                                PluginState::Loaded => {
                                    ui.colored_label(egui::Color32::LIGHT_GREEN, label);
                                }
                                PluginState::Disabled(reason) => {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(220, 180, 80),
                                        format!("{label} - disabled: {reason}"),
                                    );
                                }
                                PluginState::Failed(reason) => {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(220, 120, 120),
                                        format!("{label} - failed: {reason}"),
                                    );
                                }
                            }
                            if !status.depends_on.is_empty() {
                                ui.small(format!("Depends on: {}", status.depends_on.join(", ")));
                            }
                            if !status.provides.is_empty() {
                                ui.small(format!("Provides: {}", status.provides.join(", ")));
                            }
                        }
                    }

                    ui.separator();
                    ui.heading("Audio Debug");
                    if ui.checkbox(&mut audio_enabled, "Enable audio triggers").changed() {
                        if let Some(audio) = self.plugins.get_mut::<AudioPlugin>() {
                            audio.set_enabled(audio_enabled);
                        }
                    }
                    match self.plugins.get::<AudioPlugin>() {
                        Some(audio) => {
                            if !audio.available() {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 80, 80),
                                    "Audio device unavailable; triggers will be silent.",
                                );
                            }
                        }
                        None => {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 80, 80),
                                "Audio plugin unavailable; triggers will be silent.",
                            );
                        }
                    }
                    if ui.button("Clear audio log").clicked() {
                        if let Some(audio) = self.plugins.get_mut::<AudioPlugin>() {
                            audio.clear();
                        }
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

        if let Some(enabled) = script_enable_request {
            if let Some(plugin) = self.script_plugin_mut() {
                plugin.set_enabled(enabled);
            }
        }
        if script_reload_request {
            if let Some(plugin) = self.script_plugin_mut() {
                if let Err(err) = plugin.force_reload() {
                    plugin.set_error_message(err.to_string());
                }
            }
        }

        EditorUiOutput {
            full_output,
            actions,
            pending_viewport,
            ui_scale,
            ui_cell_size,
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
            debug_show_spatial_hash,
            debug_show_colliders,
        }
    }
}
