use std::{collections::HashSet, sync::Arc};

use super::{editor_shell::SCENE_HISTORY_CAPACITY, editor_ui, App};
use crate::ecs::{ForceField, ParticleAttractor};

impl App {
    pub(super) fn set_inspector_status(&self, status: Option<String>) {
        self.editor_ui_state_mut().inspector_status = status;
    }

    pub(super) fn remember_scene_path(&mut self, path: &str) {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return;
        }
        let mut state = self.editor_ui_state_mut();
        if let Some(pos) = state.scene_history.iter().position(|entry| entry == trimmed) {
            state.scene_history.remove(pos);
        }
        state.scene_history.push_front(trimmed.to_string());
        while state.scene_history.len() > SCENE_HISTORY_CAPACITY {
            state.scene_history.pop_back();
        }
        state.scene_history_snapshot = None;
    }

    pub(super) fn scene_history_arc(&mut self) -> Arc<[String]> {
        let mut state = self.editor_ui_state_mut();
        if let Some(cache) = &state.scene_history_snapshot {
            return Arc::clone(cache);
        }
        let data = state.scene_history.iter().cloned().collect::<Vec<_>>();
        let arc = Arc::from(data.into_boxed_slice());
        state.scene_history_snapshot = Some(Arc::clone(&arc));
        arc
    }

    pub(super) fn scene_atlas_refs_arc(&mut self) -> Arc<[String]> {
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

    pub(super) fn scene_mesh_refs_arc(&mut self) -> Arc<[String]> {
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

    pub(super) fn scene_clip_refs_arc(&mut self) -> Arc<[String]> {
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

    pub(super) fn handle_inspector_actions(&mut self, actions: &mut Vec<editor_ui::InspectorAction>) {
        for op in actions.drain(..) {
            match op {
                editor_ui::InspectorAction::SetTranslation { entity, translation } => {
                    if self.ecs.set_translation(entity, translation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update position.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetRotation { entity, rotation } => {
                    if self.ecs.set_rotation(entity, rotation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update rotation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetScale { entity, scale } => {
                    if self.ecs.set_scale(entity, scale) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update scale.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetVelocity { entity, velocity } => {
                    if self.ecs.set_velocity(entity, velocity) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update velocity.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetScript { entity, path } => {
                    let trimmed = path.trim();
                    if trimmed.is_empty() {
                        self.set_inspector_status(Some("Script path cannot be empty.".to_string()));
                    } else {
                        let mut entity_ref = self.ecs.world.entity_mut(entity);
                        entity_ref.insert(crate::scripts::ScriptBehaviour::new(trimmed.to_string()));
                        self.set_inspector_status(Some(format!("Script set to {trimmed}.")));
                    }
                }
                editor_ui::InspectorAction::RemoveScript { entity } => {
                    let mut entity_ref = self.ecs.world.entity_mut(entity);
                    entity_ref.remove::<crate::scripts::ScriptBehaviour>();
                    self.set_inspector_status(Some("Script removed.".to_string()));
                }
                editor_ui::InspectorAction::SetEmitterTrail { entity, trail } => {
                    self.ecs.set_emitter_trail(entity, trail);
                    self.set_inspector_status(Some("Emitter trail updated.".to_string()));
                }
                editor_ui::InspectorAction::SetForceField { entity, field } => {
                    let field = field.map(|(kind, strength, radius, falloff, direction)| ForceField {
                        kind,
                        strength,
                        radius,
                        falloff,
                        direction,
                    });
                    self.ecs.set_force_field(entity, field);
                    self.set_inspector_status(Some("Force field updated.".to_string()));
                }
                editor_ui::InspectorAction::SetAttractor { entity, attractor } => {
                    let attractor =
                        attractor.map(|(strength, radius, min_distance, max_acceleration, falloff)| {
                            ParticleAttractor { strength, radius, min_distance, max_acceleration, falloff }
                        });
                    self.ecs.set_attractor(entity, attractor);
                    self.set_inspector_status(Some("Attractor updated.".to_string()));
                }
                editor_ui::InspectorAction::ClearTransformClip { entity } => {
                    if self.ecs.clear_transform_clip(entity) {
                        self.set_inspector_status(Some("Transform clip cleared.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to clear transform clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClip { entity, clip_key } => {
                    if self.ecs.set_transform_clip(entity, &self.assets, &clip_key) {
                        self.set_inspector_status(Some(format!("Transform clip set to {}", clip_key)));
                    } else {
                        self.set_inspector_status(Some(format!(
                            "Transform clip '{}' not available",
                            clip_key
                        )));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipPlaying { entity, playing } => {
                    if self.ecs.set_transform_clip_playing(entity, playing) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update clip playback.".to_string()));
                    }
                }
                editor_ui::InspectorAction::ResetTransformClip { entity } => {
                    if self.ecs.reset_transform_clip(entity) {
                        self.set_inspector_status(Some("Transform clip reset.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to reset transform clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipSpeed { entity, speed } => {
                    if self.ecs.set_transform_clip_speed(entity, speed) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update clip speed.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipGroup { entity, group } => {
                    if self.ecs.set_transform_clip_group(entity, group.as_deref()) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update clip group.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformClipTime { entity, time } => {
                    if self.ecs.set_transform_clip_time(entity, time) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to scrub clip time.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetTransformTrackMask { entity, mask } => {
                    if self.ecs.set_transform_track_mask(entity, mask) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update transform track mask.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetPropertyTrackMask { entity, mask } => {
                    if self.ecs.set_property_track_mask(entity, mask) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update property track mask.".to_string()));
                    }
                }
                editor_ui::InspectorAction::ClearSkeleton { entity } => {
                    if self.ecs.clear_skeleton(entity) {
                        self.set_inspector_status(Some("Skeleton detached.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to detach skeleton.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeleton { entity, skeleton_key } => {
                    if self.ecs.set_skeleton(entity, &self.assets, &skeleton_key) {
                        self.set_inspector_status(Some(format!("Skeleton set to {}", skeleton_key)));
                    } else {
                        self.set_inspector_status(Some(format!("Skeleton '{}' unavailable", skeleton_key)));
                    }
                }
                editor_ui::InspectorAction::ClearSkeletonClip { entity } => {
                    if self.ecs.clear_skeleton_clip(entity) {
                        self.set_inspector_status(Some("Skeletal clip cleared.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to clear skeletal clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClip { entity, clip_key } => {
                    if self.ecs.set_skeleton_clip(entity, &self.assets, &clip_key) {
                        self.set_inspector_status(Some(format!("Skeletal clip set to {}", clip_key)));
                    } else {
                        self.set_inspector_status(Some(format!("Skeletal clip '{}' unavailable", clip_key)));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipPlaying { entity, playing } => {
                    if self.ecs.set_skeleton_clip_playing(entity, playing) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some(
                            "Failed to update skeletal clip playback.".to_string(),
                        ));
                    }
                }
                editor_ui::InspectorAction::ResetSkeletonPose { entity } => {
                    if self.ecs.reset_skeleton_pose(entity) {
                        self.set_inspector_status(Some("Skeletal pose reset.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to reset skeletal pose.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipSpeed { entity, speed } => {
                    if self.ecs.set_skeleton_clip_speed(entity, speed) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update skeletal clip speed.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipGroup { entity, group } => {
                    if self.ecs.set_skeleton_clip_group(entity, group.as_deref()) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update skeletal clip group.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkeletonClipTime { entity, time } => {
                    if self.ecs.set_skeleton_clip_time(entity, time) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to scrub skeletal clip.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAtlas { entity, atlas, cleared_timeline } => {
                    if self.ecs.set_sprite_atlas(entity, &self.assets, &atlas) {
                        if cleared_timeline {
                            self.set_inspector_status(Some(format!(
                                "Sprite atlas set to {} (timeline cleared)",
                                atlas
                            )));
                        } else {
                            self.set_inspector_status(Some(format!("Sprite atlas set to {}", atlas)));
                        }
                    } else {
                        self.set_inspector_status(Some(format!("Atlas '{}' unavailable", atlas)));
                    }
                }
                editor_ui::InspectorAction::SetSpriteRegion { entity, atlas, region } => {
                    if self.ecs.set_sprite_region(entity, &self.assets, &region) {
                        self.set_inspector_status(Some(format!("Sprite region set to {}", region)));
                    } else {
                        self.set_inspector_status(Some(format!(
                            "Region '{}' not found in atlas {}",
                            region, atlas
                        )));
                    }
                }
                editor_ui::InspectorAction::SetSpriteTimeline { entity, timeline } => {
                    if self.ecs.set_sprite_timeline(entity, &self.assets, timeline.as_deref()) {
                        self.set_inspector_status(
                            timeline
                                .as_ref()
                                .map(|name| format!("Sprite timeline set to {name}"))
                                .or_else(|| Some("Sprite timeline cleared".to_string())),
                        );
                    } else if let Some(name) = timeline {
                        self.set_inspector_status(Some(format!("Timeline '{name}' unavailable")));
                    } else {
                        self.set_inspector_status(Some("Failed to change sprite timeline.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationPlaying { entity, playing } => {
                    if self.ecs.set_sprite_animation_playing(entity, playing) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update animation playback.".to_string()));
                    }
                }
                editor_ui::InspectorAction::ResetSpriteAnimation { entity } => {
                    if self.ecs.reset_sprite_animation(entity) {
                        self.set_inspector_status(Some("Sprite animation reset.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to reset sprite animation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationLooped { entity, looped } => {
                    if self.ecs.set_sprite_animation_looped(entity, looped) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update loop flag.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationSpeed { entity, speed } => {
                    if self.ecs.set_sprite_animation_speed(entity, speed) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update animation speed.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationStartOffset { entity, start_offset } => {
                    if self.ecs.set_sprite_animation_start_offset(entity, start_offset) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update start offset.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationRandomStart { entity, random_start } => {
                    if self.ecs.set_sprite_animation_random_start(entity, random_start) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update random start.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSpriteAnimationGroup { entity, group } => {
                    if self.ecs.set_sprite_animation_group(entity, group.as_deref()) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update animation group.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SeekSpriteAnimationFrame {
                    entity,
                    frame,
                    preview_events,
                    atlas,
                    timeline,
                } => {
                    if self.ecs.seek_sprite_animation_frame(entity, frame) {
                        if preview_events {
                            self.preview_sprite_events(&atlas, &timeline, frame);
                        } else {
                            self.set_inspector_status(None);
                        }
                    } else {
                        self.set_inspector_status(Some("Failed to seek animation frame.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshMaterial { entity, material } => {
                    let previous = self
                        .ecs
                        .entity_info(entity)
                        .and_then(|info| info.mesh.as_ref().and_then(|mesh| mesh.material.clone()));
                    let mut apply_change = true;
                    if let Some(ref key) = material {
                        if !self.material_registry.has(key) {
                            self.set_inspector_status(Some(format!("Material '{}' not registered", key)));
                            apply_change = false;
                        } else if let Err(err) = self.material_registry.retain(key) {
                            self.set_inspector_status(Some(format!(
                                "Failed to retain material '{}': {err}",
                                key
                            )));
                            apply_change = false;
                        }
                    }
                    if apply_change {
                        if self.ecs.set_mesh_material(entity, material.clone()) {
                            if let Some(prev) = previous {
                                if material.as_ref() != Some(&prev) {
                                    self.material_registry.release(&prev);
                                }
                            }
                            let persistent_materials: HashSet<String> = self
                                .mesh_preview_plugin()
                                .map(|plugin| plugin.persistent_materials().iter().cloned().collect())
                                .unwrap_or_default();
                            let mut refs = persistent_materials.clone();
                            for instance in self.ecs.collect_mesh_instances() {
                                if let Some(mat) = instance.material {
                                    refs.insert(mat);
                                }
                            }
                            self.scene_material_refs = refs;
                            self.set_inspector_status(None);
                        } else {
                            if let Some(ref key) = material {
                                self.material_registry.release(key);
                            }
                            self.set_inspector_status(Some("Failed to update mesh material.".to_string()));
                        }
                    } else if let Some(ref key) = material {
                        if material.as_ref() != previous.as_ref() {
                            self.material_registry.release(key);
                        }
                    }
                }
                editor_ui::InspectorAction::SetMeshShadowFlags { entity, cast, receive } => {
                    if self.ecs.set_mesh_shadow_flags(entity, cast, receive) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh shadow flags.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshMaterialParams {
                    entity,
                    base_color,
                    metallic,
                    roughness,
                    emissive,
                } => {
                    if self.ecs.set_mesh_material_params(entity, base_color, metallic, roughness, emissive) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some(
                            "Failed to update mesh material parameters.".to_string(),
                        ));
                    }
                }
                editor_ui::InspectorAction::SetMeshTranslation { entity, translation } => {
                    if self.ecs.set_mesh_translation(entity, translation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh translation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshRotationEuler { entity, rotation } => {
                    if self.ecs.set_mesh_rotation_euler(entity, rotation) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh rotation.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshScale3D { entity, scale } => {
                    if self.ecs.set_mesh_scale(entity, scale) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update mesh scale.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetMeshTint { entity, tint } => {
                    if self.ecs.set_tint(entity, tint) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some("Failed to update tint.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SetSkinMeshJointCount { entity, joint_count } => {
                    if self.ecs.set_skin_mesh_joint_count(entity, joint_count) {
                        self.set_inspector_status(None);
                    } else {
                        self.set_inspector_status(Some(
                            "Failed to update skin mesh joint count.".to_string(),
                        ));
                    }
                }
                editor_ui::InspectorAction::SetSkinMeshSkeleton { entity, skeleton } => {
                    if self.ecs.set_skin_mesh_skeleton(entity, skeleton) {
                        let status = skeleton
                            .map(|skel| format!("Skin mesh bound to skeleton #{:04}", skel.index()))
                            .unwrap_or_else(|| "Skin mesh skeleton cleared.".to_string());
                        self.set_inspector_status(Some(status));
                    } else {
                        self.set_inspector_status(Some("Failed to update skin mesh skeleton.".to_string()));
                    }
                }
                editor_ui::InspectorAction::SyncSkinMeshJointCount { entity } => {
                    let skeleton = self
                        .ecs
                        .entity_info(entity)
                        .and_then(|info| info.skin_mesh.as_ref().and_then(|sm| sm.skeleton_entity));
                    match skeleton {
                        Some(skel_entity) => {
                            if let Some(skeleton_info) =
                                self.ecs.entity_info(skel_entity).and_then(|info| info.skeleton)
                            {
                                if self.ecs.set_skin_mesh_joint_count(entity, skeleton_info.joint_count) {
                                    self.set_inspector_status(Some(format!(
                                        "Skin mesh joints set to {}",
                                        skeleton_info.joint_count
                                    )));
                                } else {
                                    self.set_inspector_status(Some(
                                        "Failed to sync joint count from skeleton.".to_string(),
                                    ));
                                }
                            } else {
                                self.set_inspector_status(Some(
                                    "Selected skeleton is missing SkeletonInstance.".to_string(),
                                ));
                            }
                        }
                        None => {
                            self.set_inspector_status(Some(
                                "Assign a skeleton before syncing joints.".to_string(),
                            ));
                        }
                    }
                }
                editor_ui::InspectorAction::DetachSkinMesh { entity } => {
                    if self.ecs.detach_skin_mesh(entity) {
                        self.set_inspector_status(Some("Skin mesh component removed.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to remove skin mesh.".to_string()));
                    }
                }
                editor_ui::InspectorAction::AttachSkinMesh { entity } => {
                    if self.ecs.attach_skin_mesh(entity, 0) {
                        self.set_inspector_status(Some("Skin mesh component added.".to_string()));
                    } else {
                        self.set_inspector_status(Some("Failed to add skin mesh component.".to_string()));
                    }
                }
            }
        }
    }
}
