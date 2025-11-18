use std::sync::Arc;

use super::App;

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
        while state.scene_history.len() > 8 {
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

    pub(super) fn focus_selection(&mut self) -> bool {
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
