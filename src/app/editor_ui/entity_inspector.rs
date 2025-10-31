use super::{PrefabDragPayload, SpriteAtlasRequest, UiActions};
use crate::assets::AssetManager;
use crate::ecs::{EcsWorld, EntityInfo, PropertyTrackPlayer, SpriteInfo, TransformClipInfo, TransformTrackPlayer};
use crate::gizmo::{GizmoInteraction, GizmoMode, ScaleHandle};
use crate::input::Input;
use crate::material_registry::MaterialRegistry;
use crate::mesh_registry::MeshRegistry;
use bevy_ecs::prelude::Entity;
use egui::Ui;
use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use std::{cmp::Ordering, collections::HashSet};

pub(super) struct InspectorAppContext<'a> {
    pub ecs: &'a mut EcsWorld,
    pub gizmo_mode: &'a mut GizmoMode,
    pub gizmo_interaction: &'a mut Option<GizmoInteraction>,
    pub input: &'a Input,
    pub inspector_status: &'a mut Option<String>,
    pub material_registry: &'a mut MaterialRegistry,
    pub mesh_registry: &'a mut MeshRegistry,
    pub scene_material_refs: &'a mut HashSet<String>,
    pub assets: &'a AssetManager,
}

pub(super) fn show_entity_inspector(
    app: InspectorAppContext<'_>,
    ui: &mut Ui,
    selected_entity: &mut Option<Entity>,
    selection_details: &mut Option<EntityInfo>,
    id_lookup_input: &mut String,
    id_lookup_active: &mut bool,
    frame_selection_request: &mut bool,
    persistent_materials: &HashSet<String>,
    actions: &mut UiActions,
) {
    let mut selected_entity_value = *selected_entity;
    let mut selection_details_value = selection_details.clone();

    if let Some(entity) = selected_entity_value {
        ui.heading("Entity Inspector");
        ui.label(format!("Entity: {:?}", entity));
        ui.horizontal(|ui| {
            ui.label("Gizmo");
            ui.selectable_value(app.gizmo_mode, GizmoMode::Translate, "Translate");
            ui.selectable_value(app.gizmo_mode, GizmoMode::Rotate, "Rotate");
            ui.selectable_value(app.gizmo_mode, GizmoMode::Scale, "Scale");
        });
        match *app.gizmo_mode {
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
        if let Some(interaction) = app.gizmo_interaction.as_ref() {
            match interaction {
                GizmoInteraction::Translate { axis_lock, .. } => {
                    let mut msg = String::from("Translate gizmo active");
                    if let Some(axis) = axis_lock {
                        msg.push_str(&format!(" ({} axis)", axis.label()));
                    }
                    if app.input.ctrl_held() {
                        msg.push_str(" [snap]");
                    }
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
                GizmoInteraction::Translate3D { .. } => {
                    let msg = if app.input.ctrl_held() {
                        "3D translate gizmo active [snap]"
                    } else {
                        "3D translate gizmo active"
                    };
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
                GizmoInteraction::Rotate { .. } => {
                    let msg = if app.input.ctrl_held() {
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
                            if app.input.ctrl_held() {
                                "Scale gizmo active (uniform) [snap]"
                            } else {
                                "Scale gizmo active (uniform)"
                            },
                        ),
                        ScaleHandle::Axis { axis, .. } => ui.colored_label(
                            egui::Color32::LIGHT_GREEN,
                            if app.input.ctrl_held() {
                                format!("Scale gizmo active ({}) [snap]", axis.label())
                            } else {
                                format!("Scale gizmo active ({})", axis.label())
                            },
                        ),
                    };
                }
                GizmoInteraction::Rotate3D { .. } => {
                    let msg = if app.input.ctrl_held() {
                        "3D rotate gizmo active [snap]"
                    } else {
                        "3D rotate gizmo active"
                    };
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
                GizmoInteraction::Scale3D { .. } => {
                    let mut msg = String::from("3D scale gizmo active");
                    if app.input.shift_held() {
                        msg.push_str(" (uniform)");
                    }
                    if app.input.ctrl_held() {
                        msg.push_str(" [snap]");
                    }
                    ui.colored_label(egui::Color32::LIGHT_GREEN, msg);
                }
            }
        }
        let mut inspector_refresh = false;
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
                    if app.ecs.set_translation(entity, translation) {
                        info.translation = translation;
                        inspector_refresh = true;
                        *app.inspector_status = None;
                    }
                }
            });

            let mut rotation_deg = info.rotation.to_degrees();
            if ui.add(egui::DragValue::new(&mut rotation_deg).speed(1.0).suffix(" deg")).changed() {
                let rotation_rad = rotation_deg.to_radians();
                if app.ecs.set_rotation(entity, rotation_rad) {
                    info.rotation = rotation_rad;
                    inspector_refresh = true;
                    *app.inspector_status = None;
                }
            }

            let mut scale = info.scale;
            ui.horizontal(|ui| {
                ui.label("Scale");
                if ui.add(egui::DragValue::new(&mut scale.x).speed(0.01)).changed()
                    | ui.add(egui::DragValue::new(&mut scale.y).speed(0.01)).changed()
                {
                    let clamped = Vec2::new(scale.x.max(0.01), scale.y.max(0.01));
                    if app.ecs.set_scale(entity, clamped) {
                        info.scale = clamped;
                        inspector_refresh = true;
                        *app.inspector_status = None;
                    }
                }
            });

            if let Some(mut velocity) = info.velocity {
                ui.horizontal(|ui| {
                    ui.label("Velocity");
                    if ui.add(egui::DragValue::new(&mut velocity.x).speed(0.01)).changed()
                        | ui.add(egui::DragValue::new(&mut velocity.y).speed(0.01)).changed()
                    {
                        if app.ecs.set_velocity(entity, velocity) {
                            info.velocity = Some(velocity);
                            inspector_refresh = true;
                            *app.inspector_status = None;
                        }
                    }
                });
            } else {
                ui.label("Velocity: n/a");
            }

            ui.separator();
            let mut clip_info_opt: Option<TransformClipInfo> = info.transform_clip.clone();
            let mut transform_mask_opt: Option<TransformTrackPlayer> = info.transform_tracks;
            let mut property_mask_opt: Option<PropertyTrackPlayer> = info.property_tracks;
            let mut clip_keys = app.assets.clip_keys();
            clip_keys.sort();
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
                    if app.ecs.clear_transform_clip(entity) {
                        clip_info_opt = None;
                        inspector_refresh = true;
                        *app.inspector_status = Some("Transform clip cleared".to_string());
                    } else {
                        *app.inspector_status = Some("Failed to clear transform clip".to_string());
                    }
                }
            } else if clip_info_opt
                .as_ref()
                .map(|clip| clip.clip_key.as_str() != clip_combo.as_str())
                .unwrap_or(true)
            {
                if app.ecs.set_transform_clip(entity, &app.assets, &clip_combo) {
                    inspector_refresh = true;
                    *app.inspector_status = Some(format!("Transform clip set to {}", clip_combo));
                } else {
                    *app.inspector_status = Some(format!("Transform clip '{}' not available", clip_combo));
                }
            }

            if let Some(mut clip_info) = clip_info_opt.clone() {
                if let Some(source) = app.assets.clip_source(&clip_info.clip_key) {
                    ui.small(format!("Source: {}", source));
                } else {
                    ui.small("Source: n/a");
                }
                ui.horizontal(|ui| {
                    let mut playing = clip_info.playing;
                    if ui.checkbox(&mut playing, "Playing").changed() {
                        if app.ecs.set_transform_clip_playing(entity, playing) {
                            clip_info.playing = playing;
                            inspector_refresh = true;
                            *app.inspector_status = None;
                        }
                    }
                    if ui.button("Reset").clicked() {
                        if app.ecs.reset_transform_clip(entity) {
                            inspector_refresh = true;
                            *app.inspector_status = Some("Transform clip reset".to_string());
                        } else {
                            *app.inspector_status = Some("Failed to reset transform clip".to_string());
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Speed");
                    let mut speed = clip_info.speed;
                    if ui
                        .add(egui::DragValue::new(&mut speed).speed(0.05).range(0.0..=8.0).suffix("x"))
                        .changed()
                    {
                        if app.ecs.set_transform_clip_speed(entity, speed) {
                            clip_info.speed = speed;
                            inspector_refresh = true;
                            *app.inspector_status = None;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Group");
                    let mut group_value = clip_info.group.clone().unwrap_or_default();
                    let response = ui
                        .add(egui::TextEdit::singleline(&mut group_value).hint_text("optional group id"));
                    if response.changed() {
                        let trimmed = group_value.trim();
                        let result = if trimmed.is_empty() {
                            app.ecs.set_transform_clip_group(entity, None)
                        } else {
                            app.ecs.set_transform_clip_group(entity, Some(trimmed))
                        };
                        if result {
                            clip_info.group = if trimmed.is_empty() { None } else { Some(trimmed.to_string()) };
                            inspector_refresh = true;
                        }
                    }
                });
                let duration = clip_info.duration.max(0.0);
                let mut clip_time = clip_info.time.clamp(0.0, duration);
                let slider_response = ui.add_enabled(
                    duration > 0.0,
                    egui::Slider::new(&mut clip_time, 0.0..=duration)
                        .text("Time (s)")
                        .smart_aim(false),
                );
                if slider_response.changed() {
                    if app.ecs.set_transform_clip_time(entity, clip_time) {
                        clip_info.time = clip_time;
                        inspector_refresh = true;
                    }
                }
                if duration <= 0.0 {
                    ui.label("Duration: 0 (static clip)");
                } else {
                    ui.label(format!("Duration: {:.3} s", duration));
                }
                if let Some(asset_clip) = app.assets.clip(&clip_info.clip_key) {
                    let mut markers: Vec<f32> = Vec::new();
                    if let Some(track) = asset_clip.translation.as_ref() {
                        markers.extend(track.keyframes.iter().map(|kf| kf.time));
                    }
                    if let Some(track) = asset_clip.rotation.as_ref() {
                        markers.extend(track.keyframes.iter().map(|kf| kf.time));
                    }
                    if let Some(track) = asset_clip.scale.as_ref() {
                        markers.extend(track.keyframes.iter().map(|kf| kf.time));
                    }
                    if let Some(track) = asset_clip.tint.as_ref() {
                        markers.extend(track.keyframes.iter().map(|kf| kf.time));
                    }
                    markers.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
                    markers.dedup_by(|a, b| (*a - *b).abs() <= 1e-4);
                    if !markers.is_empty() {
                        let formatted = markers
                            .iter()
                            .map(|t| format!("{:.3}", t))
                            .collect::<Vec<_>>()
                            .join(", ");
                        ui.small(format!("Keyframes: {}", formatted));
                    }
                }

                let mut transform_mask = transform_mask_opt.unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Apply Transform");
                    let mut apply_translation = transform_mask.apply_translation;
                    if ui.checkbox(&mut apply_translation, "Translation").changed() {
                        transform_mask.apply_translation = apply_translation;
                        if app.ecs.set_transform_track_mask(entity, transform_mask) {
                            transform_mask_opt = Some(transform_mask);
                            inspector_refresh = true;
                        }
                    }
                    let mut apply_rotation = transform_mask.apply_rotation;
                    if ui.checkbox(&mut apply_rotation, "Rotation").changed() {
                        transform_mask.apply_rotation = apply_rotation;
                        if app.ecs.set_transform_track_mask(entity, transform_mask) {
                            transform_mask_opt = Some(transform_mask);
                            inspector_refresh = true;
                        }
                    }
                    let mut apply_scale = transform_mask.apply_scale;
                    if ui.checkbox(&mut apply_scale, "Scale").changed() {
                        transform_mask.apply_scale = apply_scale;
                        if app.ecs.set_transform_track_mask(entity, transform_mask) {
                            transform_mask_opt = Some(transform_mask);
                            inspector_refresh = true;
                        }
                    }
                });

                let mut property_mask = property_mask_opt.unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label("Apply Properties");
                    let mut apply_tint = property_mask.apply_tint;
                    if ui.checkbox(&mut apply_tint, "Tint").changed() {
                        property_mask.apply_tint = apply_tint;
                        if app.ecs.set_property_track_mask(entity, property_mask) {
                            property_mask_opt = Some(property_mask);
                            inspector_refresh = true;
                        }
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Tracks");
                    track_badge(ui, "Translation", clip_info.has_translation, transform_mask_opt
                        .map(|mask| mask.apply_translation)
                        .unwrap_or(false));
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

            if let Some(mut sprite) = info.sprite.clone() {
                ui.separator();
                let mut skip_sprite_controls = false;
                let mut atlas_selection = sprite.atlas.clone();
                let mut atlas_keys = app.assets.atlas_keys();
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
                    if app.ecs.set_sprite_atlas(entity, &app.assets, &atlas_selection) {
                        if let Some(updated) = app.ecs.entity_info(entity).and_then(|data| data.sprite) {
                            sprite = updated.clone();
                            info.sprite = Some(updated);
                        } else {
                            info.sprite = None;
                        }
                        inspector_refresh = true;
                        let status = if had_animation {
                            format!("Sprite atlas set to {} (timeline cleared)", atlas_selection)
                        } else {
                            format!("Sprite atlas set to {}", atlas_selection)
                        };
                        *app.inspector_status = Some(status);
                        skip_sprite_controls = true;
                    } else {
                        *app.inspector_status = Some(format!("Atlas '{}' not available", atlas_selection));
                    }
                }
                if let Some(source) = app.assets.atlas_source(&sprite.atlas) {
                    ui.small(format!("Source: {}", source));
                } else {
                    ui.small("Source: n/a");
                }
                let key_buffer_id = egui::Id::new(("sprite_atlas_new_key", entity.index()));
                let mut atlas_key_input = ui
                    .ctx()
                    .data_mut(|d| d.get_persisted::<String>(key_buffer_id))
                    .unwrap_or_else(|| sprite.atlas.clone());
                let path_buffer_id = egui::Id::new(("sprite_atlas_path", entity.index()));
                let default_source = app.assets.atlas_source(&sprite.atlas).unwrap_or_default().to_string();
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
                            *app.inspector_status = Some("Atlas key and path required".to_string());
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
                            *app.inspector_status =
                                Some(format!("Loading atlas '{}' from {}", key_trimmed, path_trimmed));
                            skip_sprite_controls = true;
                        }
                    }
                });
                if !skip_sprite_controls {
                    let mut region = sprite.region.clone();
                    if ui.text_edit_singleline(&mut region).changed() {
                        if app.ecs.set_sprite_region(entity, &app.assets, &region) {
                            let updated_sprite = SpriteInfo {
                                atlas: sprite.atlas.clone(),
                                region: region.clone(),
                                animation: None,
                            };
                            info.sprite = Some(updated_sprite.clone());
                            sprite = updated_sprite;
                            inspector_refresh = true;
                            *app.inspector_status = Some(format!("Sprite region set to {}", region));
                        } else {
                            *app.inspector_status =
                                Some(format!("Region '{}' not found in atlas {}", region, sprite.atlas));
                        }
                    }
                    let mut timeline_names = app.assets.atlas_timeline_names(&sprite.atlas);
                    timeline_names.sort();
                    timeline_names.dedup();
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
                            let success =
                                app.ecs.set_sprite_timeline(entity, &app.assets, desired_timeline.as_deref());
                            if success {
                                inspector_refresh = true;
                                *app.inspector_status = desired_timeline
                                    .as_ref()
                                    .map(|name| format!("Sprite timeline set to {name}"))
                                    .or_else(|| Some("Sprite timeline disabled".to_string()));
                                if let Some(updated) =
                                    app.ecs.entity_info(entity).and_then(|data| data.sprite)
                                {
                                    sprite = updated.clone();
                                    info.sprite = Some(updated);
                                }
                            } else if let Some(name) = desired_timeline {
                                *app.inspector_status = Some(format!("Timeline '{name}' unavailable"));
                            } else {
                                *app.inspector_status = Some("Failed to change sprite timeline".to_string());
                            }
                        }
                        if let Some(anim) = sprite.animation.as_ref() {
                            ui.label(format!("Loop Mode: {}", anim.loop_mode));
                            ui.horizontal(|ui| {
                                let play_label = if anim.playing { "Pause" } else { "Play" };
                                if ui.button(play_label).clicked() {
                                    if app.ecs.set_sprite_animation_playing(entity, !anim.playing) {
                                        inspector_refresh = true;
                                    }
                                }
                                if ui.button("Reset").clicked() {
                                    if app.ecs.reset_sprite_animation(entity) {
                                        inspector_refresh = true;
                                    }
                                }
                                let mut looped = anim.looped;
                                if ui.checkbox(&mut looped, "Loop").changed() {
                                    if app.ecs.set_sprite_animation_looped(entity, looped) {
                                        inspector_refresh = true;
                                    }
                                }
                            });
                            let mut speed = anim.speed;
                            if ui.add(egui::Slider::new(&mut speed, 0.0..=5.0).text("Speed")).changed() {
                                if app.ecs.set_sprite_animation_speed(entity, speed) {
                                    inspector_refresh = true;
                                }
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
                                    if app.ecs.set_sprite_animation_start_offset(entity, start_offset) {
                                        inspector_refresh = true;
                                    }
                                }
                            });
                            let mut random_start = anim.random_start;
                            if ui.checkbox(&mut random_start, "Randomize Start").changed() {
                                if app.ecs.set_sprite_animation_random_start(entity, random_start) {
                                    inspector_refresh = true;
                                }
                            }
                            let mut group_label = anim.group.clone().unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.label("Group");
                                if ui.text_edit_singleline(&mut group_label).changed() {
                                    let trimmed = group_label.trim();
                                    let success = if trimmed.is_empty() {
                                        app.ecs.set_sprite_animation_group(entity, None)
                                    } else {
                                        app.ecs.set_sprite_animation_group(entity, Some(trimmed))
                                    };
                                    if success {
                                        inspector_refresh = true;
                                    }
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
                                    if app.ecs.seek_sprite_animation_frame(entity, target) {
                                        inspector_refresh = true;
                                        if preview_events_enabled {
                                            preview_sprite_events(
                                                app.assets,
                                                app.inspector_status,
                                                &sprite.atlas,
                                                &anim.timeline,
                                                target,
                                            );
                                        } else {
                                            *app.inspector_status = None;
                                        }
                                    }
                                }
                                ui.horizontal(|ui| {
                                    let has_prev = frame_index > 0;
                                    let has_next = frame_index + 1 < frame_count;
                                    if ui.add_enabled(has_prev, egui::Button::new("<")).clicked() {
                                        let target = frame_index.saturating_sub(1);
                                        if app.ecs.seek_sprite_animation_frame(entity, target) {
                                            inspector_refresh = true;
                                            if preview_events_enabled {
                                                preview_sprite_events(
                                                    app.assets,
                                                    app.inspector_status,
                                                    &sprite.atlas,
                                                    &anim.timeline,
                                                    target,
                                                );
                                            } else {
                                                *app.inspector_status = None;
                                            }
                                        }
                                    }
                                    if ui.add_enabled(has_next, egui::Button::new(">")).clicked() {
                                        let target = (frame_index + 1).min(frame_count - 1);
                                        if app.ecs.seek_sprite_animation_frame(entity, target) {
                                            inspector_refresh = true;
                                            if preview_events_enabled {
                                                preview_sprite_events(
                                                    app.assets,
                                                    app.inspector_status,
                                                    &sprite.atlas,
                                                    &anim.timeline,
                                                    target,
                                                );
                                            } else {
                                                *app.inspector_status = None;
                                            }
                                        }
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

            if let Some(mesh) = info.mesh.clone() {
                ui.separator();
                ui.label(format!("Mesh: {}", mesh.key));
                let mut material_options: Vec<(String, String)> = app
                    .material_registry
                    .keys()
                    .map(|key| {
                        let label = app
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
                            .or_else(|| app.material_registry.definition(key).map(|def| def.label.clone()))
                            .unwrap_or_else(|| key.clone());
                        format!("{label} ({key})")
                    }
                    None => "Use mesh material (asset default)".to_string(),
                };
                egui::ComboBox::from_label("Material Override").selected_text(selected_text).show_ui(
                    ui,
                    |ui| {
                        ui.selectable_value(&mut desired_material, None, "Use mesh material (asset default)");
                        for (key, label) in &material_options {
                            let entry_label = format!("{label} ({key})");
                            ui.selectable_value(&mut desired_material, Some(key.clone()), entry_label);
                        }
                    },
                );
                if desired_material != mesh.material {
                    let previous_material = mesh.material.clone();
                    let mut retained_new = false;
                    let mut apply_change = true;
                    if let Some(ref key) = desired_material {
                        if !app.material_registry.has(key.as_str()) {
                            *app.inspector_status = Some(format!("Material '{}' not registered", key));
                            apply_change = false;
                        } else if let Err(err) = app.material_registry.retain(key) {
                            *app.inspector_status =
                                Some(format!("Failed to retain material '{}': {err}", key));
                            apply_change = false;
                        } else {
                            retained_new = true;
                        }
                    }
                    if apply_change {
                        if app.ecs.set_mesh_material(entity, desired_material.clone()) {
                            inspector_refresh = true;
                            *app.inspector_status = None;
                            if let Some(prev) = previous_material {
                                if desired_material.as_ref() != Some(&prev) {
                                    app.material_registry.release(&prev);
                                }
                            }
                            let mut refs = persistent_materials.clone();
                            for instance in app.ecs.collect_mesh_instances() {
                                if let Some(material) = instance.material {
                                    refs.insert(material);
                                }
                            }
                            *app.scene_material_refs = refs;
                        } else {
                            *app.inspector_status = Some("Failed to update mesh material".to_string());
                            if retained_new {
                                if let Some(ref key) = desired_material {
                                    app.material_registry.release(key);
                                }
                            }
                        }
                    } else if retained_new {
                        if let Some(ref key) = desired_material {
                            app.material_registry.release(key);
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
                    if app.ecs.set_mesh_shadow_flags(entity, cast_shadows, receive_shadows) {
                        inspector_refresh = true;
                        *app.inspector_status = None;
                    } else {
                        *app.inspector_status = Some("Failed to update mesh shadow flags".to_string());
                    }
                }
                if let Some(subsets) = app.mesh_registry.mesh_subsets(&mesh.key) {
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
                    if emissive_enabled {
                        if ui.color_edit_button_rgb(&mut emissive_arr).changed() {
                            emissive_changed = true;
                        }
                    }
                });

                let material_changed =
                    base_color_changed || metallic_changed || roughness_changed || emissive_changed;
                if material_changed {
                    let base_color_vec = Vec3::from_array(base_color_arr);
                    let emissive_opt =
                        if emissive_enabled { Some(Vec3::from_array(emissive_arr)) } else { None };
                    if app.ecs.set_mesh_material_params(
                        entity,
                        base_color_vec,
                        metallic,
                        roughness,
                        emissive_opt,
                    ) {
                        inspector_refresh = true;
                        *app.inspector_status = None;
                    } else {
                        *app.inspector_status = Some("Failed to update mesh material".to_string());
                    }
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
                            if app.ecs.set_mesh_translation(entity, translation3) {
                                mesh_tx.translation = translation3;
                                inspector_refresh = true;
                                *app.inspector_status = None;
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
                        changed |= ui.add(egui::DragValue::new(&mut rotation_deg.x).speed(0.5)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut rotation_deg.y).speed(0.5)).changed();
                        changed |= ui.add(egui::DragValue::new(&mut rotation_deg.z).speed(0.5)).changed();
                        if changed {
                            let radians = Vec3::new(
                                rotation_deg.x.to_radians(),
                                rotation_deg.y.to_radians(),
                                rotation_deg.z.to_radians(),
                            );
                            if app.ecs.set_mesh_rotation_euler(entity, radians) {
                                mesh_tx.rotation =
                                    Quat::from_euler(EulerRot::XYZ, radians.x, radians.y, radians.z);
                                inspector_refresh = true;
                                *app.inspector_status = None;
                            }
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
                            if app.ecs.set_mesh_scale(entity, clamped) {
                                mesh_tx.scale = clamped;
                                inspector_refresh = true;
                                *app.inspector_status = None;
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
                    if app.ecs.set_tint(entity, Some(color)) {
                        info.tint = Some(color);
                        inspector_refresh = true;
                        *app.inspector_status = None;
                    }
                } else if app.ecs.set_tint(entity, None) {
                    info.tint = None;
                    inspector_refresh = true;
                    *app.inspector_status = None;
                }
            }
            if let Some(color) = info.tint {
                let mut color_arr = color.to_array();
                if ui.color_edit_button_rgba_unmultiplied(&mut color_arr).changed() {
                    let vec = Vec4::from_array(color_arr);
                    if app.ecs.set_tint(entity, Some(vec)) {
                        info.tint = Some(vec);
                        inspector_refresh = true;
                        *app.inspector_status = None;
                    }
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

        if inspector_refresh {
            selection_details_value = selected_entity_value.and_then(|entity| app.ecs.entity_info(entity));
        } else {
            selection_details_value = inspector_info;
        }
        if let Some(status) = app.inspector_status.as_ref() {
            ui.colored_label(egui::Color32::YELLOW, status);
        }
        if ui.button("Delete selected").clicked() {
            actions.delete_entity = Some(entity);
            selected_entity_value = None;
            selection_details_value = None;
            *app.inspector_status = None;
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

fn preview_sprite_events(
    assets: &AssetManager,
    inspector_status: &mut Option<String>,
    atlas: &str,
    timeline: &str,
    frame_index: usize,
) {
    if let Some(timeline_data) = assets.atlas_timeline(atlas, timeline) {
        if let Some(frame) = timeline_data.frames.get(frame_index) {
            if frame.events.is_empty() {
                *inspector_status = Some(format!("Preview events: none (frame {})", frame_index + 1));
            } else {
                let joined = frame.events.join(", ");
                println!(
                    "[animation] Preview events for {}::{} frame {} => {}",
                    atlas, timeline, frame_index, joined
                );
                *inspector_status = Some(format!("Preview events: {}", joined));
            }
        } else {
            *inspector_status =
                Some(format!("Preview events unavailable: frame {} out of range", frame_index + 1));
        }
    } else {
        *inspector_status =
            Some(format!("Preview events unavailable: timeline '{}' not found in atlas {}", timeline, atlas));
    }
}
