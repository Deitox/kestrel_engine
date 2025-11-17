use super::{editor_ui, App, BINARY_PREFABS_ENABLED};
use crate::prefab::{PrefabFormat, PrefabStatusKind, PrefabStatusMessage};
use crate::scene::Scene;
use glam::{Vec2, Vec3};
use std::collections::HashMap;

impl App {
    pub(super) fn set_prefab_status(&mut self, kind: PrefabStatusKind, message: impl Into<String>) {
        self.editor_ui_state_mut().prefab_status =
            Some(PrefabStatusMessage { kind, message: message.into() });
    }

    pub(super) fn handle_save_prefab(&mut self, request: editor_ui::PrefabSaveRequest) {
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
                self.material_registry.material_source(key).map(|path| (key.to_string(), path.to_string()))
            })
            .collect();
        let Some(scene) = self.ecs.export_prefab_with_sources(
            request.entity,
            &self.assets,
            |key| mesh_source_map.get(key).cloned(),
            |key| material_source_map.get(key).cloned(),
        ) else {
            self.set_prefab_status(PrefabStatusKind::Error, "Failed to export selection to prefab.");
            return;
        };
        let path = self.prefab_library.path_for(trimmed, request.format);
        let existed = path.exists();
        let sanitized_name = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or(trimmed).to_string();
        match scene.save_to_path(&path) {
            Ok(()) => {
                self.editor_ui_state_mut().prefab_name_input = sanitized_name.clone();
                if let Err(err) = self.prefab_library.refresh() {
                    self.set_prefab_status(
                        PrefabStatusKind::Warning,
                        format!("Prefab '{sanitized_name}' saved but refresh failed: {err}"),
                    );
                } else {
                    self.set_prefab_status(
                        if existed { PrefabStatusKind::Info } else { PrefabStatusKind::Success },
                        if existed {
                            format!(
                                "Overwrote prefab '{sanitized_name}' ({})",
                                request.format.short_label()
                            )
                        } else {
                            format!("Saved prefab '{sanitized_name}' ({})", request.format.short_label())
                        },
                    );
                }
            }
            Err(err) => {
                self.set_prefab_status(PrefabStatusKind::Error, format!("Saving prefab failed: {err}"));
            }
        }
    }

    pub(super) fn handle_instantiate_prefab(&mut self, request: editor_ui::PrefabInstantiateRequest) {
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
}
