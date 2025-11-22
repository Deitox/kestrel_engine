use super::*;

#[derive(Debug, Clone)]
pub(crate) struct CameraBookmark {
    pub(crate) name: String,
    pub(crate) position: Vec2,
    pub(crate) zoom: f32,
}

impl CameraBookmark {
    pub(crate) fn to_scene(&self) -> SceneCameraBookmark {
        SceneCameraBookmark {
            name: self.name.clone(),
            position: Vec2Data::from(self.position),
            zoom: self.zoom,
        }
    }

    pub(crate) fn from_scene(bookmark: &SceneCameraBookmark) -> Self {
        Self {
            name: bookmark.name.clone(),
            position: Vec2::from(bookmark.position.clone()),
            zoom: bookmark.zoom,
        }
    }
}

impl App {
    pub(crate) fn selected_entity(&self) -> Option<Entity> {
        self.editor_ui_state().selected_entity
    }

    pub(crate) fn set_selected_entity(&self, entity: Option<Entity>) {
        self.editor_ui_state_mut().selected_entity = entity;
    }

    pub(crate) fn gizmo_mode(&self) -> GizmoMode {
        self.editor_ui_state().gizmo_mode
    }

    pub(crate) fn set_gizmo_mode(&self, mode: GizmoMode) {
        self.with_editor_ui_state_mut(|state| {
            if state.gizmo_mode != mode {
                state.gizmo_mode = mode;
                state.gizmo_interaction = None;
            }
        });
    }

    pub(crate) fn gizmo_interaction(&self) -> Option<GizmoInteraction> {
        self.editor_ui_state().gizmo_interaction
    }

    pub(crate) fn set_gizmo_interaction(&self, interaction: Option<GizmoInteraction>) {
        self.editor_ui_state_mut().gizmo_interaction = interaction;
    }

    pub(crate) fn take_gizmo_interaction(&self) -> Option<GizmoInteraction> {
        self.with_editor_ui_state_mut(|state| state.gizmo_interaction.take())
    }

    pub(crate) fn camera_bookmarks(&self) -> Vec<CameraBookmark> {
        self.editor_ui_state().camera_bookmarks.clone()
    }

    pub(crate) fn active_camera_bookmark(&self) -> Option<String> {
        self.editor_ui_state().active_camera_bookmark.clone()
    }

    pub(crate) fn set_active_camera_bookmark(&self, bookmark: Option<String>) {
        self.editor_ui_state_mut().active_camera_bookmark = bookmark;
    }

    pub(crate) fn apply_camera_bookmark_by_name(&mut self, name: &str) -> bool {
        let bookmark = {
            let state = self.editor_ui_state();
            state.camera_bookmarks.iter().find(|b| b.name == name).cloned()
        };
        if let Some(bookmark) = bookmark {
            self.camera.position = bookmark.position;
            self.camera.set_zoom(bookmark.zoom);
            self.set_active_camera_bookmark(Some(bookmark.name.clone()));
            self.camera_follow_target = None;
            true
        } else {
            false
        }
    }

    pub(crate) fn upsert_camera_bookmark(&mut self, name: &str) -> bool {
        let bookmark_name = name.trim();
        if bookmark_name.is_empty() {
            return false;
        }
        let position = self.camera.position;
        let zoom = self.camera.zoom;
        self.with_editor_ui_state_mut(|state| {
            let trimmed = bookmark_name;
            if let Some(existing) = state.camera_bookmarks.iter_mut().find(|b| b.name == trimmed) {
                existing.position = position;
                existing.zoom = zoom;
            } else {
                state.camera_bookmarks.push(CameraBookmark {
                    name: bookmark_name.to_string(),
                    position,
                    zoom,
                });
                state.camera_bookmarks.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            }
            state.active_camera_bookmark = Some(bookmark_name.to_string());
        });
        true
    }

    pub(crate) fn delete_camera_bookmark(&mut self, name: &str) -> bool {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return false;
        }
        let mut deleted = false;
        self.with_editor_ui_state_mut(|state| {
            let before = state.camera_bookmarks.len();
            state.camera_bookmarks.retain(|bookmark| bookmark.name != trimmed);
            deleted = state.camera_bookmarks.len() != before;
            if deleted && state.active_camera_bookmark.as_deref() == Some(trimmed) {
                state.active_camera_bookmark = None;
            }
        });
        deleted
    }

    pub(crate) fn refresh_camera_follow(&mut self) -> bool {
        let Some(target_id) = self.camera_follow_target.as_ref().map(|id| id.as_str().to_string()) else {
            return false;
        };
        let Some(entity) = self.ecs.find_entity_by_scene_id(&target_id) else {
            return false;
        };
        let Some(info) = self.ecs.entity_info(entity) else {
            return false;
        };
        self.camera.position = info.translation;
        true
    }

    pub(crate) fn set_camera_follow_scene_id(&mut self, scene_id: SceneEntityId) -> bool {
        self.camera_follow_target = Some(scene_id);
        if self.refresh_camera_follow() {
            self.set_active_camera_bookmark(None);
            true
        } else {
            self.camera_follow_target = None;
            false
        }
    }

    pub(crate) fn clear_camera_follow(&mut self) {
        self.camera_follow_target = None;
    }

    pub(crate) fn focus_selection(&mut self) -> bool {
        let Some(entity) = self.selected_entity() else {
            return false;
        };
        let Some(info) = self.ecs.entity_info(entity) else {
            return false;
        };
        self.camera_follow_target = None;
        self.set_active_camera_bookmark(None);
        self.camera.position = info.translation;
        if let Some(plugin) = self.mesh_preview_plugin_mut() {
            plugin.focus_selection_with_info(&info)
        } else {
            true
        }
    }
}
