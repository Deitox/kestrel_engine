use super::{PrefabDragPayload, UiActions};
use crate::assets::AssetManager;
use crate::ecs::{EcsWorld, EntityInfo, SpriteInfo};
use crate::gizmo::{GizmoInteraction, GizmoMode, ScaleHandle};
use crate::input::Input;
use crate::material_registry::MaterialRegistry;
use crate::mesh_registry::MeshRegistry;
use bevy_ecs::prelude::Entity;
use egui::Ui;
use glam::{EulerRot, Quat, Vec2, Vec3, Vec4};
use std::collections::HashSet;

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

            if let Some(sprite) = info.sprite.clone() {
                ui.separator();
                ui.label(format!("Atlas: {}", sprite.atlas));
                let mut region = sprite.region.clone();
                if ui.text_edit_singleline(&mut region).changed() {
                    if app.ecs.set_sprite_region(entity, &app.assets, &region) {
                        info.sprite = Some(SpriteInfo {
                            atlas: sprite.atlas.clone(),
                            region: region.clone(),
                            animation: None,
                        });
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
                    let mut desired_timeline = sprite.animation.as_ref().map(|anim| anim.timeline.clone());
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
                        let frame_count = anim.frame_count.max(1);
                        ui.label(format!(
                            "Frame {}/{}",
                            (anim.frame_index + 1).min(frame_count),
                            frame_count
                        ));
                        if anim.frame_count > 0 {
                            ui.separator();
                            let max_index = (anim.frame_count - 1) as i32;
                            let mut preview_frame = anim.frame_index as i32;
                            let slider = egui::Slider::new(&mut preview_frame, 0..=max_index).text("Scrub");
                            if ui.add(slider).changed() {
                                let target = preview_frame.clamp(0, max_index) as usize;
                                if app.ecs.seek_sprite_animation_frame(entity, target) {
                                    inspector_refresh = true;
                                    *app.inspector_status = None;
                                }
                            }
                            ui.horizontal(|ui| {
                                let has_prev = anim.frame_index > 0;
                                let has_next = anim.frame_index + 1 < anim.frame_count;
                                if ui.add_enabled(has_prev, egui::Button::new("<")).clicked() {
                                    let target = anim.frame_index.saturating_sub(1);
                                    if app.ecs.seek_sprite_animation_frame(entity, target) {
                                        inspector_refresh = true;
                                        *app.inspector_status = None;
                                    }
                                }
                                if ui.add_enabled(has_next, egui::Button::new(">")).clicked() {
                                    let target = (anim.frame_index + 1).min(anim.frame_count - 1);
                                    if app.ecs.seek_sprite_animation_frame(entity, target) {
                                        inspector_refresh = true;
                                        *app.inspector_status = None;
                                    }
                                }
                            });
                        } else {
                            ui.colored_label(egui::Color32::YELLOW, "Timeline has no frames to preview.");
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
