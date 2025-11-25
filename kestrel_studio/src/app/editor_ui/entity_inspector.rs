use super::{
    AtlasAssetSummary, ClipAssetSummary, InputModifierState, InspectorAction, MaterialOption,
    MeshSubsetEntry, PrefabDragPayload, SkeletonAssetSummary, SkeletonEntityBinding, SpriteAtlasRequest,
    UiActions,
};
use crate::ecs::{
    EntityInfo, ForceFalloff, ForceFieldKind, ParticleAttractor, ParticleTrail, PropertyTrackPlayer, ScriptInfo,
    SkeletonInfo, TransformClipInfo, TransformTrackPlayer,
};
use crate::gizmo::{GizmoInteraction, GizmoMode, ScaleHandle};
use bevy_ecs::prelude::Entity;
use egui::Ui;
use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use std::collections::HashMap;
use std::sync::Arc;

pub(super) struct InspectorContext<'a> {
    pub gizmo_mode: &'a mut GizmoMode,
    pub gizmo_interaction: &'a mut Option<GizmoInteraction>,
    pub inspector_status: &'a mut Option<String>,
    pub input: InputModifierState,
    pub clip_keys: &'a [String],
    pub clip_assets: &'a HashMap<String, ClipAssetSummary>,
    pub skeleton_keys: &'a [String],
    pub skeleton_assets: &'a HashMap<String, SkeletonAssetSummary>,
    pub atlas_keys: &'a [String],
    pub atlas_assets: &'a HashMap<String, AtlasAssetSummary>,
    pub script_paths: &'a [String],
    pub script_error: Option<&'a str>,
    pub script_error_for_entity: bool,
    pub skeleton_entities: &'a [SkeletonEntityBinding],
    pub material_options: &'a [MaterialOption],
    pub mesh_subsets: &'a HashMap<String, Arc<[MeshSubsetEntry]>>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn show_entity_inspector(
    ctx: InspectorContext<'_>,
    ui: &mut Ui,
    selected_entity: &mut Option<Entity>,
    selection_details: &mut Option<EntityInfo>,
    id_lookup_input: &mut String,
    id_lookup_active: &mut bool,
    frame_selection_request: &mut bool,
    actions: &mut UiActions,
) {
    let mut selected_entity_value = *selected_entity;
    let mut selection_details_value = selection_details.clone();

    if let Some(entity) = selected_entity_value {
        ui.heading("Entity Inspector");
        ui.label(format!("Entity: {:?}", entity));
        ui.horizontal(|ui| {
            ui.label("Gizmo");
            ui.selectable_value(ctx.gizmo_mode, GizmoMode::Translate, "Translate");
            ui.selectable_value(ctx.gizmo_mode, GizmoMode::Rotate, "Rotate");
            ui.selectable_value(ctx.gizmo_mode, GizmoMode::Scale, "Scale");
        });
        match *ctx.gizmo_mode {
            GizmoMode::Scale => {
                ui.small("Shift = uniform scale, Ctrl = snap steps");
            }
            GizmoMode::Translate => {
                ui.small("Shift = lock axis, Ctrl = snap to grid");
            }
            GizmoMode::Rotate => {
                ui.small("Ctrl = snap to 15 deg increments");
            }
        }
        if let Some(interaction) = ctx.gizmo_interaction.as_ref() {
            match interaction {
                GizmoInteraction::Translate { axis_lock, .. } => {
                    let mut msg = String::from("Translate gizmo active");
                    if let Some(axis) = axis_lock {
                        msg.push_str(&format!(" ({} axis)", axis.label()));
                    }
                    if ctx.input.ctrl {
                        msg.push_str(" [snap]");
                    }
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
                GizmoInteraction::Translate3D { .. } => {
                    let msg = if ctx.input.ctrl {
                        "3D translate gizmo active [snap]"
                    } else {
                        "3D translate gizmo active"
                    };
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
                GizmoInteraction::Rotate { .. } => {
                    let msg =
                        if ctx.input.ctrl { "Rotate gizmo active [snap]" } else { "Rotate gizmo active" };
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
                GizmoInteraction::Scale { handle, .. } => {
                    match handle {
                        ScaleHandle::Uniform { .. } => ui.colored_label(
                            egui::Color32::LIGHT_GREEN,
                            if ctx.input.ctrl {
                                "Scale gizmo active (uniform) [snap]"
                            } else {
                                "Scale gizmo active (uniform)"
                            },
                        ),
                        ScaleHandle::Axis { axis, .. } => ui.colored_label(
                            egui::Color32::LIGHT_GREEN,
                            if ctx.input.ctrl {
                                format!("Scale gizmo active ({}) [snap]", axis.label())
                            } else {
                                format!("Scale gizmo active ({})", axis.label())
                            },
                        ),
                    };
                }
                GizmoInteraction::Rotate3D { .. } => {
                    let msg = if ctx.input.ctrl {
                        "3D rotate gizmo active [snap]"
                    } else {
                        "3D rotate gizmo active"
                    };
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
                GizmoInteraction::Scale3D { .. } => {
                    let mut msg = String::from("3D scale gizmo active");
                    if ctx.input.shift {
                        msg.push_str(" (uniform)");
                    }
                    if ctx.input.ctrl {
                        msg.push_str(" [snap]");
                    }
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
            }
        }
        let mut _inspector_refresh = false;
        let mut inspector_info = selection_details_value.clone();
    if let Some(mut info) = inspector_info {
            ui.horizontal(|ui| {
                ui.label("Entity ID");
                ui.monospace(info.scene_id.as_str());
                if ui.button("Copy").clicked() {
                    let id_string = info.scene_id.as_str().to_string();
                    ui.ctx().copy_text(id_string);
                }
                if ui.button("Find by ID").clicked() {
                    *id_lookup_input = info.scene_id.as_str().to_string();
                    *id_lookup_active = true;
                }
            });
            let mut translation = info.translation;
            ui.horizontal(|ui| {
                ui.label("Position");
                if ui.add(egui::DragValue::new(&mut translation.x).speed(0.01)).changed()
                    | ui.add(egui::DragValue::new(&mut translation.y).speed(0.01)).changed()
                {
                    actions.inspector_actions.push(InspectorAction::SetTranslation { entity, translation });
                    info.translation = translation;
                    _inspector_refresh = true;
                }
            });

            let mut rotation_deg = info.rotation.to_degrees();
            if ui.add(egui::DragValue::new(&mut rotation_deg).speed(1.0).suffix(" deg")).changed() {
                let rotation_rad = rotation_deg.to_radians();
                actions
                    .inspector_actions
                    .push(InspectorAction::SetRotation { entity, rotation: rotation_rad });
                info.rotation = rotation_rad;
                _inspector_refresh = true;
            }

            let mut scale = info.scale;
            ui.horizontal(|ui| {
                ui.label("Scale");
                if ui.add(egui::DragValue::new(&mut scale.x).speed(0.01)).changed()
                    | ui.add(egui::DragValue::new(&mut scale.y).speed(0.01)).changed()
                {
                    let clamped = Vec2::new(scale.x.max(0.01), scale.y.max(0.01));
                    actions.inspector_actions.push(InspectorAction::SetScale { entity, scale: clamped });
                    info.scale = clamped;
                    _inspector_refresh = true;
                }
            });

            if let Some(mut velocity) = info.velocity {
                ui.horizontal(|ui| {
                    ui.label("Velocity");
                    if ui.add(egui::DragValue::new(&mut velocity.x).speed(0.01)).changed()
                        | ui.add(egui::DragValue::new(&mut velocity.y).speed(0.01)).changed()
                    {
                        actions.inspector_actions.push(InspectorAction::SetVelocity { entity, velocity });
                        info.velocity = Some(velocity);
                        _inspector_refresh = true;
                    }
                });
            } else {
                ui.label("Velocity: n/a");
            }

        ui.separator();
        ui.collapsing("Script", |ui| {
            let mut script_path = info.script.as_ref().map(|s| s.path.clone()).unwrap_or_default();
            let instance_id = info.script.as_ref().map(|s| s.instance_id).unwrap_or(0);
            let has_script = info.script.is_some();
            let path_trimmed = script_path.trim();
            let path_known = ctx.script_paths.iter().any(|p| p == path_trimmed);

            if let Some(err) = ctx.script_error {
                ui.colored_label(egui::Color32::RED, format!("Last script error: {err}"));
            }
            if ctx.script_error_for_entity {
                ui.colored_label(
                    egui::Color32::RED,
                    "This entity's script is currently errored; callbacks are paused until it succeeds.",
                );
            }

            ui.label("Assign a script to this entity:");
            ui.horizontal(|ui| {
                ui.label("Path");
                let edit_response = ui
                    .add(egui::TextEdit::singleline(&mut script_path).hint_text("assets/scripts/example.rhai"));
                if edit_response.changed() {
                    if script_path.trim().is_empty() {
                        info.script = None;
                    } else {
                        info.script = Some(ScriptInfo { path: script_path.clone(), instance_id });
                    }
                }
                if ui.button("Apply").clicked() && !script_path.trim().is_empty() {
                    let trimmed = script_path.trim().to_string();
                    actions.inspector_actions.push(InspectorAction::SetScript { entity, path: trimmed.clone() });
                    info.script = Some(ScriptInfo { path: trimmed, instance_id: 0 });
                    _inspector_refresh = true;
                }
                ui.add_enabled_ui(has_script, |ui| {
                    if ui.button("Remove").clicked() {
                        actions.inspector_actions.push(InspectorAction::RemoveScript { entity });
                        info.script = None;
                        script_path.clear();
                        _inspector_refresh = true;
                    }
                });
            });

            let mut picker_selection: Option<String> = None;
            let selected_label = if script_path.trim().is_empty() {
                "Select script asset".to_string()
            } else {
                script_path.clone()
            };
            egui::ComboBox::from_id_salt(("script_picker", entity.index()))
                .selected_text(selected_label)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(script_path.is_empty(), "<None>").clicked() {
                        picker_selection = Some(String::new());
                    }
                    for path in ctx.script_paths {
                        if ui.selectable_label(script_path == *path, path).clicked() {
                            picker_selection = Some(path.clone());
                        }
                    }
            });
            if let Some(picked) = picker_selection {
                let trimmed = picked.trim().to_string();
                if trimmed.is_empty() {
                    actions.inspector_actions.push(InspectorAction::RemoveScript { entity });
                    info.script = None;
                    script_path.clear();
                } else {
                    actions.inspector_actions.push(InspectorAction::SetScript { entity, path: trimmed.clone() });
                    info.script = Some(ScriptInfo { path: trimmed, instance_id: 0 });
                }
                _inspector_refresh = true;
            }

            if has_script && !script_path.trim().is_empty() && !path_known {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    "Script not found under assets/scripts; path will be kept as-is.",
                );
            }
            if has_script {
                if instance_id != 0 {
                    ui.small(format!("Instance id (runtime): {instance_id}"));
                } else {
                    ui.small("Instance id will be assigned at runtime.");
                }
            }
            ui.small("Scripts are relative to the project root, e.g. assets/scripts/my_behaviour.rhai");
        });
        ui.collapsing("Particles", |ui| {
            if let Some(mut emitter) = info.particle_emitter {
                    let mut trail_enabled = emitter.trail.is_some();
                    let mut trail: ParticleTrail = emitter.trail.unwrap_or_default();
                    ui.label("Emitter trail");
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut trail_enabled, "Enable");
                        ui.label("Length scale");
                        ui.add(egui::DragValue::new(&mut trail.length_scale).range(0.01..=2.0).speed(0.01));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Min len");
                        ui.add(egui::DragValue::new(&mut trail.min_length).range(0.0..=5.0).speed(0.01));
                        ui.label("Max len");
                        ui.add(egui::DragValue::new(&mut trail.max_length).range(0.01..=5.0).speed(0.01));
                    });
                    ui.horizontal(|ui| {
                        ui.label("Width");
                        ui.add(egui::DragValue::new(&mut trail.width).range(0.01..=1.0).speed(0.01));
                        ui.label("Fade");
                        ui.add(egui::DragValue::new(&mut trail.fade).range(0.0..=1.0).speed(0.01));
                    });
                    let desired_trail = if trail_enabled { Some(trail) } else { None };
                    if desired_trail != emitter.trail {
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetEmitterTrail { entity, trail: desired_trail });
                        emitter.trail = desired_trail;
                        info.particle_emitter = Some(emitter);
                        _inspector_refresh = true;
                    }
                } else {
                    ui.label("No particle emitter on entity");
                }

                ui.separator();
                ui.label("Force Field");
                let mut field_enabled = info.force_field.is_some();
                let mut field = info.force_field.unwrap_or_default();
                let mut kind_label = match field.kind {
                    ForceFieldKind::Radial => "Radial",
                    ForceFieldKind::Directional => "Directional",
                }
                .to_string();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut field_enabled, "Enabled");
                    ui.label("Strength");
                    ui.add(egui::DragValue::new(&mut field.strength).speed(0.05));
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(&mut field.radius).range(0.0..=10.0).speed(0.05));
                });
                egui::ComboBox::from_id_salt(("force_field_kind", entity.index()))
                    .selected_text(kind_label.clone())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut kind_label, "Radial".to_string(), "Radial");
                        ui.selectable_value(&mut kind_label, "Directional".to_string(), "Directional");
                    });
                field.kind = if kind_label == "Directional" {
                    ForceFieldKind::Directional
                } else {
                    ForceFieldKind::Radial
                };
                egui::ComboBox::from_id_salt(("force_field_falloff", entity.index()))
                    .selected_text(match field.falloff {
                        ForceFalloff::None => "None",
                        ForceFalloff::Linear => "Linear",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut field.falloff, ForceFalloff::None, "None");
                        ui.selectable_value(&mut field.falloff, ForceFalloff::Linear, "Linear");
                    });
                if matches!(field.kind, ForceFieldKind::Directional) {
                    ui.horizontal(|ui| {
                        ui.label("Direction");
                        ui.add(egui::DragValue::new(&mut field.direction.x).speed(0.01));
                        ui.add(egui::DragValue::new(&mut field.direction.y).speed(0.01));
                    });
                }
                let desired_field = if field_enabled { Some(field) } else { None };
                if desired_field != info.force_field {
                    let dir = match field.kind {
                        ForceFieldKind::Directional => field.direction,
                        ForceFieldKind::Radial => Vec2::Y,
                    };
                    actions.inspector_actions.push(InspectorAction::SetForceField {
                        entity,
                        field: desired_field.map(|f| (f.kind, f.strength, f.radius, f.falloff, dir)),
                    });
                    info.force_field = desired_field;
                    _inspector_refresh = true;
                }

                ui.separator();
                ui.label("Attractor");
                let mut attractor_enabled = info.attractor.is_some();
                let mut attractor: ParticleAttractor = info.attractor.unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut attractor_enabled, "Enabled");
                    ui.label("Strength");
                    ui.add(egui::DragValue::new(&mut attractor.strength).speed(0.05));
                    ui.label("Radius");
                    ui.add(egui::DragValue::new(&mut attractor.radius).range(0.0..=10.0).speed(0.05));
                });
                ui.horizontal(|ui| {
                    ui.label("Min dist");
                    ui.add(egui::DragValue::new(&mut attractor.min_distance).range(0.0..=5.0).speed(0.01));
                    ui.label("Max accel");
                    ui.add(
                        egui::DragValue::new(&mut attractor.max_acceleration).range(0.0..=50.0).speed(0.05),
                    );
                });
                egui::ComboBox::from_id_salt(("attractor_falloff", entity.index()))
                    .selected_text(match attractor.falloff {
                        ForceFalloff::None => "None",
                        ForceFalloff::Linear => "Linear",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut attractor.falloff, ForceFalloff::None, "None");
                        ui.selectable_value(&mut attractor.falloff, ForceFalloff::Linear, "Linear");
                    });
                let desired_attractor = if attractor_enabled { Some(attractor) } else { None };
                if desired_attractor != info.attractor {
                    actions.inspector_actions.push(InspectorAction::SetAttractor {
                        entity,
                        attractor: desired_attractor
                            .map(|a| (a.strength, a.radius, a.min_distance, a.max_acceleration, a.falloff)),
                    });
                    info.attractor = desired_attractor;
                    _inspector_refresh = true;
                }
            });
            ui.separator();
            let mut clip_info_opt: Option<TransformClipInfo> = info.transform_clip.clone();
            let mut transform_mask_opt: Option<TransformTrackPlayer> = info.transform_tracks;
            let mut property_mask_opt: Option<PropertyTrackPlayer> = info.property_tracks;
            let mut clip_keys: Vec<String> = ctx.clip_keys.to_vec();
            if let Some(ref clip_info) = clip_info_opt {
                if !clip_keys.iter().any(|key| key == &clip_info.clip_key) {
                    clip_keys.push(clip_info.clip_key.clone());
                    clip_keys.sort();
                }
            }
            let mut clip_combo = clip_info_opt
                .as_ref()
                .map(|clip| clip.clip_key.clone())
                .unwrap_or_else(|| "<None>".to_string());
            let mut combo_items = clip_keys.clone();
            combo_items.insert(0, "<None>".to_string());
            ui.horizontal(|ui| {
                ui.label("Transform Clip");
                egui::ComboBox::from_id_salt(("transform_clip_selector", entity.index()))
                    .selected_text(clip_combo.clone())
                    .show_ui(ui, |ui| {
                        for key in &combo_items {
                            ui.selectable_value(&mut clip_combo, key.clone(), key);
                        }
                    });
            });
            if clip_combo == "<None>" {
                if clip_info_opt.is_some() {
                    actions.inspector_actions.push(InspectorAction::ClearTransformClip { entity });
                    clip_info_opt = None;
                    _inspector_refresh = true;
                }
            } else if clip_info_opt
                .as_ref()
                .map(|clip| clip.clip_key.as_str() != clip_combo.as_str())
                .unwrap_or(true)
            {
                actions
                    .inspector_actions
                    .push(InspectorAction::SetTransformClip { entity, clip_key: clip_combo.clone() });
                _inspector_refresh = true;
            }

            if let Some(mut clip_info) = clip_info_opt.clone() {
                if let Some(summary) = ctx.clip_assets.get(&clip_info.clip_key) {
                    if let Some(source) = summary.source.as_deref() {
                        ui.small(format!("Source: {}", source));
                    } else {
                        ui.small("Source: n/a");
                    }
                } else {
                    ui.small("Source: n/a");
                }
                ui.horizontal(|ui| {
                    let mut playing = clip_info.playing;
                    if ui.checkbox(&mut playing, "Playing").changed() {
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetTransformClipPlaying { entity, playing });
                        clip_info.playing = playing;
                        _inspector_refresh = true;
                    }
                    if ui.button("Reset").clicked() {
                        actions.inspector_actions.push(InspectorAction::ResetTransformClip { entity });
                        _inspector_refresh = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Speed");
                    let mut speed = clip_info.speed;
                    if ui
                        .add(egui::DragValue::new(&mut speed).speed(0.05).range(0.0..=8.0).suffix("x"))
                        .changed()
                    {
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetTransformClipSpeed { entity, speed });
                        clip_info.speed = speed;
                        _inspector_refresh = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Group");
                    let mut group_value = clip_info.group.clone().unwrap_or_default();
                    let response =
                        ui.add(egui::TextEdit::singleline(&mut group_value).hint_text("optional group id"));
                    if response.changed() {
                        let trimmed = group_value.trim();
                        let group = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetTransformClipGroup { entity, group: group.clone() });
                        clip_info.group = group;
                        _inspector_refresh = true;
                    }
                });
                let duration = clip_info.duration.max(0.0);
                let mut clip_time = clip_info.time.clamp(0.0, duration);
                let slider_response = ui.add_enabled(
                    duration > 0.0,
                    egui::Slider::new(&mut clip_time, 0.0..=duration).text("Time (s)").smart_aim(false),
                );
                if slider_response.changed() {
                    actions
                        .inspector_actions
                        .push(InspectorAction::SetTransformClipTime { entity, time: clip_time });
                    clip_info.time = clip_time;
                    _inspector_refresh = true;
                }
                if duration <= 0.0 {
                    ui.label("Duration: 0 (static clip)");
                } else {
                    ui.label(format!("Duration: {:.3} s", duration));
                }
                if let Some(summary) = ctx.clip_assets.get(&clip_info.clip_key) {
                    let markers: Vec<f32> = summary.keyframe_markers.iter().copied().collect();
                    if !markers.is_empty() {
                        let formatted =
                            markers.iter().map(|t| format!("{:.3}", t)).collect::<Vec<_>>().join(", ");
                        ui.small(format!("Keyframes: {}", formatted));
                    }
                }

                let mut transform_mask = transform_mask_opt.unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Apply Transform");
                    let mut apply_translation = transform_mask.apply_translation;
                    if ui.checkbox(&mut apply_translation, "Translation").changed() {
                        transform_mask.apply_translation = apply_translation;
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetTransformTrackMask { entity, mask: transform_mask });
                        transform_mask_opt = Some(transform_mask);
                        _inspector_refresh = true;
                    }
                    let mut apply_rotation = transform_mask.apply_rotation;
                    if ui.checkbox(&mut apply_rotation, "Rotation").changed() {
                        transform_mask.apply_rotation = apply_rotation;
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetTransformTrackMask { entity, mask: transform_mask });
                        transform_mask_opt = Some(transform_mask);
                        _inspector_refresh = true;
                    }
                    let mut apply_scale = transform_mask.apply_scale;
                    if ui.checkbox(&mut apply_scale, "Scale").changed() {
                        transform_mask.apply_scale = apply_scale;
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetTransformTrackMask { entity, mask: transform_mask });
                        transform_mask_opt = Some(transform_mask);
                        _inspector_refresh = true;
                    }
                });

                let mut property_mask = property_mask_opt.unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Apply Properties");
                    let mut apply_tint = property_mask.apply_tint;
                    if ui.checkbox(&mut apply_tint, "Tint").changed() {
                        property_mask.apply_tint = apply_tint;
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetPropertyTrackMask { entity, mask: property_mask });
                        property_mask_opt = Some(property_mask);
                        _inspector_refresh = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Tracks");
                    track_badge(
                        ui,
                        "Translation",
                        clip_info.has_translation,
                        transform_mask_opt.map(|mask| mask.apply_translation).unwrap_or(false),
                    );
                    track_badge(
                        ui,
                        "Rotation",
                        clip_info.has_rotation,
                        transform_mask_opt.map(|mask| mask.apply_rotation).unwrap_or(false),
                    );
                    track_badge(
                        ui,
                        "Scale",
                        clip_info.has_scale,
                        transform_mask_opt.map(|mask| mask.apply_scale).unwrap_or(false),
                    );
                    track_badge(
                        ui,
                        "Tint",
                        clip_info.has_tint,
                        property_mask_opt.map(|mask| mask.apply_tint).unwrap_or(false),
                    );
                });

                if let Some(value) = clip_info.sample_translation {
                    ui.label(format!("Current translation: {}", format_vec2(value)));
                }
                if let Some(value) = clip_info.sample_rotation {
                    ui.label(format!("Current rotation: {:.3} rad", value));
                }
                if let Some(value) = clip_info.sample_scale {
                    ui.label(format!("Current scale: {}", format_vec2(value)));
                }
                if let Some(value) = clip_info.sample_tint {
                    ui.label(format!("Current tint: {}", format_vec4(value)));
                }

                clip_info_opt = Some(clip_info);
            } else if clip_keys.is_empty() {
                ui.label("Transform Clip: n/a");
            }

            info.transform_clip = clip_info_opt;
            info.transform_tracks = transform_mask_opt;
            info.property_tracks = property_mask_opt;

            ui.separator();
            let mut skeleton_info_opt: Option<SkeletonInfo> = info.skeleton.clone();
            let mut skeleton_keys: Vec<String> = ctx.skeleton_keys.to_vec();
            if let Some(ref skeleton_info) = skeleton_info_opt {
                if !skeleton_keys.contains(&skeleton_info.skeleton_key) {
                    skeleton_keys.push(skeleton_info.skeleton_key.clone());
                    skeleton_keys.sort();
                }
            }
            let mut skeleton_items = skeleton_keys.clone();
            skeleton_items.insert(0, "<None>".to_string());
            let mut skeleton_combo = skeleton_info_opt
                .as_ref()
                .map(|s| s.skeleton_key.clone())
                .unwrap_or_else(|| "<None>".to_string());
            ui.horizontal(|ui| {
                ui.label("Skeleton");
                egui::ComboBox::from_id_salt(("skeleton_selector", entity.index()))
                    .selected_text(skeleton_combo.clone())
                    .show_ui(ui, |ui| {
                        for key in &skeleton_items {
                            ui.selectable_value(&mut skeleton_combo, key.clone(), key);
                        }
                    });
            });
            if skeleton_combo == "<None>" {
                if skeleton_info_opt.is_some() {
                    actions.inspector_actions.push(InspectorAction::ClearSkeleton { entity });
                    skeleton_info_opt = None;
                    _inspector_refresh = true;
                }
            } else if skeleton_info_opt
                .as_ref()
                .map(|info| info.skeleton_key.as_str() != skeleton_combo.as_str())
                .unwrap_or(true)
            {
                actions
                    .inspector_actions
                    .push(InspectorAction::SetSkeleton { entity, skeleton_key: skeleton_combo.clone() });
                _inspector_refresh = true;
            }

            if let Some(mut skeleton_info) = skeleton_info_opt.clone() {
                if let Some(summary) = ctx.skeleton_assets.get(&skeleton_info.skeleton_key) {
                    if let Some(source) = summary.source.as_deref() {
                        ui.small(format!("Source: {}", source));
                    } else {
                        ui.small("Source: n/a");
                    }
                } else {
                    ui.small("Source: n/a");
                }
                ui.horizontal(|ui| {
                    ui.label(format!("Bones: {}", skeleton_info.joint_count));
                    let palette_text = format!(
                        "Palette: {}/{}",
                        skeleton_info.palette_joint_count, skeleton_info.joint_count
                    );
                    if !skeleton_info.has_bone_transforms {
                        ui.colored_label(egui::Color32::YELLOW, "Bone transforms missing");
                    } else if skeleton_info.palette_joint_count >= skeleton_info.joint_count {
                        ui.colored_label(egui::Color32::LIGHT_GREEN, palette_text);
                    } else {
                        ui.colored_label(egui::Color32::YELLOW, palette_text);
                    }
                });
                let mut clip_keys = ctx
                    .skeleton_assets
                    .get(&skeleton_info.skeleton_key)
                    .map(|summary| summary.clip_keys.iter().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                if let Some(ref clip) = skeleton_info.clip {
                    if !clip_keys.contains(&clip.clip_key) {
                        clip_keys.push(clip.clip_key.clone());
                        clip_keys.sort();
                    }
                }
                let mut clip_combo = skeleton_info
                    .clip
                    .as_ref()
                    .map(|clip| clip.clip_key.clone())
                    .unwrap_or_else(|| "<None>".to_string());
                clip_keys.insert(0, "<None>".to_string());
                ui.horizontal(|ui| {
                    ui.label("Skeletal Clip");
                    egui::ComboBox::from_id_salt(("skeletal_clip_selector", entity.index()))
                        .selected_text(clip_combo.clone())
                        .show_ui(ui, |ui| {
                            for key in &clip_keys {
                                ui.selectable_value(&mut clip_combo, key.clone(), key);
                            }
                        });
                });
                if clip_combo == "<None>" {
                    if skeleton_info.clip.is_some() {
                        actions.inspector_actions.push(InspectorAction::ClearSkeletonClip { entity });
                        skeleton_info.clip = None;
                        _inspector_refresh = true;
                    }
                } else if skeleton_info
                    .clip
                    .as_ref()
                    .map(|clip| clip.clip_key.as_str() != clip_combo.as_str())
                    .unwrap_or(true)
                {
                    actions
                        .inspector_actions
                        .push(InspectorAction::SetSkeletonClip { entity, clip_key: clip_combo.clone() });
                    skeleton_info.clip = None;
                    _inspector_refresh = true;
                }
                if let Some(mut clip_info) = skeleton_info.clip.clone() {
                    ui.horizontal(|ui| {
                        let mut playing = clip_info.playing;
                        if ui.checkbox(&mut playing, "Playing").changed() {
                            actions
                                .inspector_actions
                                .push(InspectorAction::SetSkeletonClipPlaying { entity, playing });
                            clip_info.playing = playing;
                            _inspector_refresh = true;
                        }
                        if ui.button("Reset Pose").clicked() {
                            actions.inspector_actions.push(InspectorAction::ResetSkeletonPose { entity });
                            _inspector_refresh = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Speed");
                        let mut speed = clip_info.speed;
                        if ui
                            .add(egui::DragValue::new(&mut speed).speed(0.05).range(0.0..=4.0).suffix("x"))
                            .changed()
                        {
                            actions
                                .inspector_actions
                                .push(InspectorAction::SetSkeletonClipSpeed { entity, speed });
                            clip_info.speed = speed;
                            _inspector_refresh = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Group");
                        let mut group_value = clip_info.group.clone().unwrap_or_default();
                        let response = ui
                            .add(egui::TextEdit::singleline(&mut group_value).hint_text("optional group id"));
                        if response.changed() {
                            let trimmed = group_value.trim();
                            let group = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                            actions
                                .inspector_actions
                                .push(InspectorAction::SetSkeletonClipGroup { entity, group: group.clone() });
                            clip_info.group = group;
                            _inspector_refresh = true;
                        }
                    });
                    let duration = clip_info.duration.max(0.0);
                    let mut clip_time = clip_info.time.clamp(0.0, duration);
                    let slider_response = ui.add_enabled(
                        duration > 0.0,
                        egui::Slider::new(&mut clip_time, 0.0..=duration).text("Time (s)").smart_aim(false),
                    );
                    if slider_response.changed() {
                        actions
                            .inspector_actions
                            .push(InspectorAction::SetSkeletonClipTime { entity, time: clip_time });
                        clip_info.time = clip_time;
                        _inspector_refresh = true;
                    }
                    if duration <= 0.0 {
                        ui.label("Duration: 0 (static pose)");
                    } else {
                        ui.label(format!("Duration: {:.3} s", duration));
                    }
                    skeleton_info.clip = Some(clip_info);
                } else {
                    ui.label("Skeletal clip: n/a");
                }
                skeleton_info_opt = Some(skeleton_info);
            } else if skeleton_items.len() <= 1 {
                ui.label("Skeleton: n/a");
            }
            info.skeleton = skeleton_info_opt;

            if let Some(mut sprite) = info.sprite.clone() {
                ui.separator();
                let mut skip_sprite_controls = false;
                let mut atlas_selection = sprite.atlas.clone();
                let mut atlas_keys: Vec<String> = ctx.atlas_keys.to_vec();
                if !atlas_keys.contains(&atlas_selection) {
                    atlas_keys.push(atlas_selection.clone());
                    atlas_keys.sort();
                }
                ui.horizontal(|ui| {
                    ui.label("Atlas");
                    egui::ComboBox::from_id_salt(("sprite_atlas_combo", entity.index()))
                        .selected_text(atlas_selection.clone())
                        .show_ui(ui, |ui| {
                            for key in &atlas_keys {
                                ui.selectable_value(&mut atlas_selection, key.clone(), key);
                            }
                        });
                });
                if atlas_selection != sprite.atlas {
                    let had_animation = sprite.animation.is_some();
                    actions.inspector_actions.push(InspectorAction::SetSpriteAtlas {
                        entity,
                        atlas: atlas_selection.clone(),
                        cleared_timeline: had_animation,
                    });
                    sprite.atlas = atlas_selection.clone();
                    sprite.animation = None;
                    info.sprite = Some(sprite.clone());
                    _inspector_refresh = true;
                    skip_sprite_controls = true;
                }
                if let Some(summary) = ctx.atlas_assets.get(&sprite.atlas) {
                    if let Some(source) = summary.source.as_deref() {
                        ui.small(format!("Source: {}", source));
                    } else {
                        ui.small("Source: n/a");
                    }
                } else {
                    ui.small("Source: n/a");
                }
                let key_buffer_id = egui::Id::new(("sprite_atlas_new_key", entity.index()));
                let mut atlas_key_input = ui
                    .ctx()
                    .data_mut(|d| d.get_persisted::<String>(key_buffer_id))
                    .unwrap_or_else(|| sprite.atlas.clone());
                let path_buffer_id = egui::Id::new(("sprite_atlas_path", entity.index()));
                let default_source = ctx
                    .atlas_assets
                    .get(&sprite.atlas)
                    .and_then(|summary| summary.source.clone())
                    .unwrap_or_default();
                let mut atlas_path_input = ui
                    .ctx()
                    .data_mut(|d| d.get_persisted::<String>(path_buffer_id))
                    .unwrap_or_else(|| default_source.clone());
                ui.horizontal(|ui| {
                    ui.label("Load atlas");
                    if ui
                        .add(egui::TextEdit::singleline(&mut atlas_key_input).hint_text("atlas key"))
                        .changed()
                    {
                        ui.ctx().data_mut(|d| {
                            if atlas_key_input.trim().is_empty() {
                                d.remove::<String>(key_buffer_id);
                            } else {
                                d.insert_persisted(key_buffer_id, atlas_key_input.clone());
                            }
                        });
                    }
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut atlas_path_input).hint_text("path/to/atlas.json"),
                        )
                        .changed()
                    {
                        ui.ctx().data_mut(|d| {
                            if atlas_path_input.trim().is_empty() {
                                d.remove::<String>(path_buffer_id);
                            } else {
                                d.insert_persisted(path_buffer_id, atlas_path_input.clone());
                            }
                        });
                    }
                    if ui.button("Load & Assign").clicked() {
                        let key_trimmed = atlas_key_input.trim();
                        let path_trimmed = atlas_path_input.trim();
                        if key_trimmed.is_empty() || path_trimmed.is_empty() {
                            *ctx.inspector_status = Some("Atlas key and path required".to_string());
                        } else {
                            actions.sprite_atlas_requests.push(SpriteAtlasRequest {
                                entity,
                                atlas: key_trimmed.to_string(),
                                path: Some(path_trimmed.to_string()),
                            });
                            ui.ctx().data_mut(|d| {
                                d.insert_persisted(key_buffer_id, key_trimmed.to_string());
                                d.insert_persisted(path_buffer_id, path_trimmed.to_string());
                            });
                            *ctx.inspector_status =
                                Some(format!("Loading atlas '{}' from {}", key_trimmed, path_trimmed));
                            skip_sprite_controls = true;
                        }
                    }
                });
                if !skip_sprite_controls {
                    let mut region = sprite.region.clone();
                    if ui.text_edit_singleline(&mut region).changed() {
                        actions.inspector_actions.push(InspectorAction::SetSpriteRegion {
                            entity,
                            atlas: sprite.atlas.clone(),
                            region: region.clone(),
                        });
                        sprite.region = region.clone();
                        sprite.animation = None;
                        info.sprite = Some(sprite.clone());
                        _inspector_refresh = true;
                    }
                    let timeline_names = ctx
                        .atlas_assets
                        .get(&sprite.atlas)
                        .map(|summary| summary.timeline_names.iter().cloned().collect::<Vec<_>>())
                        .unwrap_or_default();
                    if timeline_names.is_empty() {
                        ui.label("Timelines: none defined for atlas");
                    } else {
                        let mut desired_timeline =
                            sprite.animation.as_ref().map(|anim| anim.timeline.clone());
                        let original_timeline = desired_timeline.clone();
                        ui.horizontal(|ui| {
                            ui.label("Timeline");
                            let combo_id = ("sprite_timeline_combo", entity.index());
                            egui::ComboBox::from_id_salt(combo_id)
                                .selected_text(desired_timeline.as_deref().unwrap_or("None").to_string())
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut desired_timeline, None, "None");
                                    for name in &timeline_names {
                                        ui.selectable_value(&mut desired_timeline, Some(name.clone()), name);
                                    }
                                });
                        });
                        if desired_timeline != original_timeline {
                            actions.inspector_actions.push(InspectorAction::SetSpriteTimeline {
                                entity,
                                timeline: desired_timeline.clone(),
                            });
                            _inspector_refresh = true;
                            sprite.animation = None;
                            info.sprite = Some(sprite.clone());
                        }
                        if let Some(anim) = sprite.animation.as_ref() {
                            ui.label(format!("Loop Mode: {}", anim.loop_mode));
                            ui.horizontal(|ui| {
                                let play_label = if anim.playing { "Pause" } else { "Play" };
                                if ui.button(play_label).clicked() {
                                    actions.inspector_actions.push(
                                        InspectorAction::SetSpriteAnimationPlaying {
                                            entity,
                                            playing: !anim.playing,
                                        },
                                    );
                                    _inspector_refresh = true;
                                }
                                if ui.button("Reset").clicked() {
                                    actions
                                        .inspector_actions
                                        .push(InspectorAction::ResetSpriteAnimation { entity });
                                    _inspector_refresh = true;
                                }
                                let mut looped = anim.looped;
                                if ui.checkbox(&mut looped, "Loop").changed() {
                                    actions
                                        .inspector_actions
                                        .push(InspectorAction::SetSpriteAnimationLooped { entity, looped });
                                    _inspector_refresh = true;
                                }
                            });
                            let mut speed = anim.speed;
                            if ui.add(egui::Slider::new(&mut speed, 0.0..=5.0).text("Speed")).changed() {
                                actions
                                    .inspector_actions
                                    .push(InspectorAction::SetSpriteAnimationSpeed { entity, speed });
                                _inspector_refresh = true;
                            }
                            let mut start_offset = anim.start_offset;
                            ui.horizontal(|ui| {
                                ui.label("Start Offset");
                                if ui
                                    .add(
                                        egui::DragValue::new(&mut start_offset)
                                            .speed(0.01)
                                            .range(0.0..=10_000.0)
                                            .suffix(" s"),
                                    )
                                    .changed()
                                {
                                    actions.inspector_actions.push(
                                        InspectorAction::SetSpriteAnimationStartOffset {
                                            entity,
                                            start_offset,
                                        },
                                    );
                                    _inspector_refresh = true;
                                }
                            });
                            let mut random_start = anim.random_start;
                            if ui.checkbox(&mut random_start, "Randomize Start").changed() {
                                actions.inspector_actions.push(
                                    InspectorAction::SetSpriteAnimationRandomStart { entity, random_start },
                                );
                                _inspector_refresh = true;
                            }
                            let mut group_label = anim.group.clone().unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.label("Group");
                                if ui.text_edit_singleline(&mut group_label).changed() {
                                    let trimmed = group_label.trim();
                                    let group =
                                        if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                                    actions
                                        .inspector_actions
                                        .push(InspectorAction::SetSpriteAnimationGroup { entity, group });
                                    _inspector_refresh = true;
                                }
                            });
                            if anim.frame_count > 0 {
                                let frame_count = anim.frame_count;
                                let frame_index = anim.frame_index.min(frame_count - 1);
                                let region_label = anim.frame_region.as_deref().unwrap_or("n/a");
                                let duration_ms =
                                    (anim.frame_duration.max(0.0) * 1_000.0).clamp(0.0, f32::MAX);
                                let elapsed_ms = (anim.frame_elapsed.max(0.0) * 1_000.0).clamp(0.0, f32::MAX);
                                ui.label(format!(
                                    "Frame {}/{} - duration: {:.0} ms - elapsed: {:.0} ms - region: {}",
                                    frame_index + 1,
                                    frame_count,
                                    duration_ms,
                                    elapsed_ms,
                                    region_label
                                ));
                                if anim.frame_events.is_empty() {
                                    ui.label("Events: none");
                                } else {
                                    let joined = anim.frame_events.join(", ");
                                    ui.colored_label(
                                        egui::Color32::LIGHT_YELLOW,
                                        format!("Events: {}", joined),
                                    );
                                }

                                let preview_toggle_id =
                                    egui::Id::new(("sprite_event_preview", entity.index()));
                                let mut preview_events_enabled = ui.ctx().data_mut(|d| {
                                    d.get_persisted::<bool>(preview_toggle_id).unwrap_or(false)
                                });
                                let preview_toggle_response =
                                    ui.checkbox(&mut preview_events_enabled, "Preview events").on_hover_text(
                                        "When enabled, scrubbing logs frame events to the inspector log",
                                    );
                                if preview_toggle_response.changed() {
                                    ui.ctx().data_mut(|d| {
                                        if preview_events_enabled {
                                            d.insert_persisted(preview_toggle_id, preview_events_enabled);
                                        } else {
                                            d.remove::<bool>(preview_toggle_id);
                                        }
                                    });
                                }
                                let preview_events_enabled = preview_events_enabled;

                                ui.separator();
                                let max_index = (frame_count - 1) as i32;
                                let mut preview_frame = frame_index as i32;
                                let slider =
                                    egui::Slider::new(&mut preview_frame, 0..=max_index).text("Scrub");
                                if ui.add(slider).changed() {
                                    let target = preview_frame.clamp(0, max_index) as usize;
                                    actions.inspector_actions.push(
                                        InspectorAction::SeekSpriteAnimationFrame {
                                            entity,
                                            frame: target,
                                            preview_events: preview_events_enabled,
                                            atlas: sprite.atlas.clone(),
                                            timeline: anim.timeline.clone(),
                                        },
                                    );
                                    _inspector_refresh = true;
                                }
                                ui.horizontal(|ui| {
                                    let has_prev = frame_index > 0;
                                    let has_next = frame_index + 1 < frame_count;
                                    if ui.add_enabled(has_prev, egui::Button::new("<")).clicked() {
                                        let target = frame_index.saturating_sub(1);
                                        actions.inspector_actions.push(
                                            InspectorAction::SeekSpriteAnimationFrame {
                                                entity,
                                                frame: target,
                                                preview_events: preview_events_enabled,
                                                atlas: sprite.atlas.clone(),
                                                timeline: anim.timeline.clone(),
                                            },
                                        );
                                        _inspector_refresh = true;
                                    }
                                    if ui.add_enabled(has_next, egui::Button::new(">")).clicked() {
                                        let target = (frame_index + 1).min(frame_count - 1);
                                        actions.inspector_actions.push(
                                            InspectorAction::SeekSpriteAnimationFrame {
                                                entity,
                                                frame: target,
                                                preview_events: preview_events_enabled,
                                                atlas: sprite.atlas.clone(),
                                                timeline: anim.timeline.clone(),
                                            },
                                        );
                                        _inspector_refresh = true;
                                    }
                                });
                            } else {
                                ui.colored_label(egui::Color32::YELLOW, "Timeline has no frames to preview.");
                            }
                        }
                    }
                }
            } else {
                ui.label("Sprite: n/a");
            }

            if let Some(mut mesh) = info.mesh.clone() {
                ui.separator();
                ui.label(format!("Mesh: {}", mesh.key));
                let mut desired_material = mesh.material.clone();
                let selected_text = match mesh.material.as_ref() {
                    Some(key) => {
                        let label = ctx
                            .material_options
                            .iter()
                            .find(|option| option.key == *key)
                            .map(|option| option.label.clone())
                            .unwrap_or_else(|| key.clone());
                        format!("{label} ({key})")
                    }
                    None => "Use mesh material (asset default)".to_string(),
                };
                egui::ComboBox::from_label("Material Override").selected_text(selected_text).show_ui(
                    ui,
                    |ui| {
                        ui.selectable_value(&mut desired_material, None, "Use mesh material (asset default)");
                        for option in ctx.material_options {
                            let entry_label = format!("{} ({})", option.label, option.key);
                            ui.selectable_value(&mut desired_material, Some(option.key.clone()), entry_label);
                        }
                    },
                );
                if desired_material != mesh.material {
                    actions.inspector_actions.push(InspectorAction::SetMeshMaterial {
                        entity,
                        material: desired_material.clone(),
                    });
                    mesh.material = desired_material.clone();
                    info.mesh = Some(mesh.clone());
                    _inspector_refresh = true;
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
                    actions.inspector_actions.push(InspectorAction::SetMeshShadowFlags {
                        entity,
                        cast: cast_shadows,
                        receive: receive_shadows,
                    });
                    mesh.lighting.cast_shadows = cast_shadows;
                    mesh.lighting.receive_shadows = receive_shadows;
                    info.mesh = Some(mesh.clone());
                    _inspector_refresh = true;
                }
                if let Some(subsets) = ctx.mesh_subsets.get(&mesh.key).map(|arc| arc.as_ref()) {
                    ui.collapsing("Submeshes", |ui| {
                        for (index, subset) in subsets.iter().enumerate() {
                            let subset_name = subset.name.as_deref().unwrap_or("unnamed");
                            let material_label = subset.material.as_deref().unwrap_or("default");
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
                let mut emissive_arr = mesh.lighting.emissive.unwrap_or(Vec3::ZERO).to_array();

                let base_color_changed = ui
                    .horizontal(|ui| {
                        ui.label("Base Color");
                        ui.color_edit_button_rgb(&mut base_color_arr).changed()
                    })
                    .inner;
                let metallic_changed =
                    ui.add(egui::Slider::new(&mut metallic, 0.0..=1.0).text("Metallic")).changed();
                let roughness_changed =
                    ui.add(egui::Slider::new(&mut roughness, 0.04..=1.0).text("Roughness")).changed();
                let mut emissive_changed = false;
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut emissive_enabled, "Emissive").changed() {
                        emissive_changed = true;
                    }
                    if emissive_enabled && ui.color_edit_button_rgb(&mut emissive_arr).changed() {
                        emissive_changed = true;
                    }
                });

                let material_changed =
                    base_color_changed || metallic_changed || roughness_changed || emissive_changed;
                if material_changed {
                    let base_color_vec = Vec3::from_array(base_color_arr);
                    let emissive_opt =
                        if emissive_enabled { Some(Vec3::from_array(emissive_arr)) } else { None };
                    actions.inspector_actions.push(InspectorAction::SetMeshMaterialParams {
                        entity,
                        base_color: base_color_vec,
                        metallic,
                        roughness,
                        emissive: emissive_opt,
                    });
                    mesh.lighting.base_color = base_color_vec;
                    mesh.lighting.metallic = metallic;
                    mesh.lighting.roughness = roughness;
                    mesh.lighting.emissive = emissive_opt;
                    info.mesh = Some(mesh.clone());
                    _inspector_refresh = true;
                }
                if let Some(mut mesh_tx) = info.mesh_transform.clone() {
                    let mut translation3 = mesh_tx.translation;
                    ui.horizontal(|ui| {
                        ui.label("Position (X/Y/Z)");
                        let mut changed = false;
                        changed |= ui.add(egui::DragValue::new(&mut translation3.x).speed(0.01)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut translation3.y).speed(0.01)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut translation3.z).speed(0.01)).changed();
                        if changed {
                            actions.inspector_actions.push(InspectorAction::SetMeshTranslation {
                                entity,
                                translation: translation3,
                            });
                            mesh_tx.translation = translation3;
                            _inspector_refresh = true;
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
                        changed |= ui.add(egui::DragValue::new(&mut rotation_deg.x).speed(0.5)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut rotation_deg.y).speed(0.5)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut rotation_deg.z).speed(0.5)).changed();
                        if changed {
                            let radians = Vec3::new(
                                rotation_deg.x.to_radians(),
                                rotation_deg.y.to_radians(),
                                rotation_deg.z.to_radians(),
                            );
                            actions
                                .inspector_actions
                                .push(InspectorAction::SetMeshRotationEuler { entity, rotation: radians });
                            mesh_tx.rotation =
                                Quat::from_euler(EulerRot::XYZ, radians.x, radians.y, radians.z);
                            _inspector_refresh = true;
                        }
                    });

                    let mut scale3 = mesh_tx.scale;
                    ui.horizontal(|ui| {
                        ui.label("Scale (XYZ)");
                        let mut changed = false;
                        changed |= ui.add(egui::DragValue::new(&mut scale3.x).speed(0.01)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut scale3.y).speed(0.01)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut scale3.z).speed(0.01)).changed();
                        if changed {
                            let clamped =
                                Vec3::new(scale3.x.max(0.01), scale3.y.max(0.01), scale3.z.max(0.01));
                            actions
                                .inspector_actions
                                .push(InspectorAction::SetMeshScale3D { entity, scale: clamped });
                            mesh_tx.scale = clamped;
                            _inspector_refresh = true;
                        }
                    });

                    info.mesh_transform = Some(mesh_tx);
                } else {
                    ui.label("Mesh transform: n/a");
                }
            }

            let skeleton_entities = ctx.skeleton_entities;
            if let Some(mut skin_mesh) = info.skin_mesh.clone() {
                ui.separator();
                ui.label("Skinning");
                if let Some(ref mesh_key) = skin_mesh.mesh_key {
                    ui.small(format!("Mesh key: {}", mesh_key));
                }
                let mut joint_count = skin_mesh.joint_count as u32;
                ui.horizontal(|ui| {
                    ui.label("Joint Count");
                    if ui.add(egui::DragValue::new(&mut joint_count).speed(1.0).range(0..=4096)).changed() {
                        actions.inspector_actions.push(InspectorAction::SetSkinMeshJointCount {
                            entity,
                            joint_count: joint_count as usize,
                        });
                        skin_mesh.joint_count = joint_count as usize;
                        _inspector_refresh = true;
                    }
                });
                let mut desired_skeleton = skin_mesh.skeleton_entity;
                let mut options: Vec<(Option<Entity>, String)> =
                    Vec::with_capacity(skeleton_entities.len() + 1);
                options.push((None, "<None>".to_string()));
                for binding in skeleton_entities {
                    let label = format!("{} (#{} )", binding.scene_id.as_str(), binding.entity.index());
                    options.push((Some(binding.entity), label));
                }
                if let Some(current) = skin_mesh.skeleton_entity {
                    if !options.iter().any(|(entity_opt, _)| *entity_opt == Some(current)) {
                        let label = skin_mesh
                            .skeleton_scene_id
                            .as_ref()
                            .map(|id| format!("{} (#{} )", id.as_str(), current.index()))
                            .unwrap_or_else(|| format!("Entity #{}", current.index()));
                        options.push((Some(current), label));
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Skeleton");
                    egui::ComboBox::from_id_salt(("skin_mesh_skeleton", entity.index()))
                        .selected_text(match desired_skeleton {
                            Some(current) => skin_mesh
                                .skeleton_scene_id
                                .as_ref()
                                .map(|id| format!("{} (#{} )", id.as_str(), current.index()))
                                .unwrap_or_else(|| format!("Entity #{}", current.index())),
                            None => "<None>".to_string(),
                        })
                        .show_ui(ui, |ui| {
                            for (value, label) in &options {
                                ui.selectable_value(&mut desired_skeleton, *value, label);
                            }
                        });
                });
                if desired_skeleton != skin_mesh.skeleton_entity {
                    actions
                        .inspector_actions
                        .push(InspectorAction::SetSkinMeshSkeleton { entity, skeleton: desired_skeleton });
                    skin_mesh.skeleton_entity = desired_skeleton;
                    skin_mesh.skeleton_scene_id = desired_skeleton.and_then(|skel| {
                        skeleton_entities
                            .iter()
                            .find(|binding| binding.entity == skel)
                            .map(|binding| binding.scene_id.clone())
                    });
                    _inspector_refresh = true;
                }
                let mut skin_mesh_removed = false;
                ui.horizontal(|ui| {
                    if ui.button("Reset Joint Count from Skeleton").clicked() {
                        actions.inspector_actions.push(InspectorAction::SyncSkinMeshJointCount { entity });
                        _inspector_refresh = true;
                    }
                    if ui.button("Remove Skin Mesh").clicked() {
                        actions.inspector_actions.push(InspectorAction::DetachSkinMesh { entity });
                        _inspector_refresh = true;
                        info.skin_mesh = None;
                        skin_mesh_removed = true;
                    }
                });
                if !skin_mesh_removed {
                    info.skin_mesh = Some(skin_mesh);
                }
            } else {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Skinning: n/a");
                    if ui.button("Add Skin Mesh").clicked() {
                        actions.inspector_actions.push(InspectorAction::AttachSkinMesh { entity });
                        _inspector_refresh = true;
                    }
                });
            }

            ui.separator();
            let mut tinted = info.tint.is_some();
            if ui.checkbox(&mut tinted, "Tint override").changed() {
                if tinted {
                    let color = Vec4::splat(1.0);
                    actions
                        .inspector_actions
                        .push(InspectorAction::SetMeshTint { entity, tint: Some(color) });
                    info.tint = Some(color);
                } else {
                    actions.inspector_actions.push(InspectorAction::SetMeshTint { entity, tint: None });
                    info.tint = None;
                }
                _inspector_refresh = true;
            }
            if let Some(color) = info.tint {
                let mut color_arr = color.to_array();
                if ui.color_edit_button_rgba_unmultiplied(&mut color_arr).changed() {
                    let vec = Vec4::from_array(color_arr);
                    actions.inspector_actions.push(InspectorAction::SetMeshTint { entity, tint: Some(vec) });
                    info.tint = Some(vec);
                    _inspector_refresh = true;
                }
            }

            inspector_info = Some(info);
        } else {
            ui.label("Selection data unavailable");
        }

        ui.horizontal(|ui| {
            if ui.button("Frame selection").clicked() {
                *frame_selection_request = true;
            }
        });

        let drag_id = egui::Id::new(("prefab_drag_source", entity.index()));
        let drag_response = ui
            .dnd_drag_source(drag_id, PrefabDragPayload { entity }, |ui| {
                ui.label("Drag to Prefab Shelf");
            })
            .response;
        drag_response
            .on_hover_text("Drop onto the Prefab Shelf to save this entity (and children) as a prefab.");

        selection_details_value = inspector_info;
        if let Some(status) = ctx.inspector_status.as_ref() {
            ui.colored_label(egui::Color32::YELLOW, status);
        }
        if ui.button("Delete selected").clicked() {
            actions.delete_entity = Some(entity);
            selected_entity_value = None;
            selection_details_value = None;
            *ctx.inspector_status = None;
        }
    } else {
        ui.label("No entity selected");
    }

    *selected_entity = selected_entity_value;
    *selection_details = selection_details_value;
}

fn track_badge(ui: &mut egui::Ui, label: &str, available: bool, enabled: bool) {
    let (color, text) = if !available {
        (egui::Color32::DARK_GRAY, format!("{label}: n/a"))
    } else if enabled {
        (egui::Color32::LIGHT_GREEN, format!("{label}: on"))
    } else {
        (egui::Color32::LIGHT_GRAY, format!("{label}: off"))
    };
    ui.colored_label(color, text);
}

fn format_vec2(value: Vec2) -> String {
    format!("({:.3}, {:.3})", value.x, value.y)
}

fn format_vec4(value: Vec4) -> String {
    format!("({:.3}, {:.3}, {:.3}, {:.3})", value.x, value.y, value.z, value.w)
}
