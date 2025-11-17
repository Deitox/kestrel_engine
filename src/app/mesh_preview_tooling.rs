use super::{App, MeshControlMode};
use crate::mesh_preview::MeshPreviewPlugin;

impl App {
    pub(super) fn set_mesh_status<S: Into<String>>(&mut self, message: S) {
        if let Some(plugin) = self.mesh_preview_plugin_mut() {
            plugin.set_status(message);
        }
    }

    pub(super) fn set_mesh_control_mode(&mut self, mode: MeshControlMode) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.set_mesh_control_mode(ctx, mode) {
                    eprintln!("[mesh_preview] set_mesh_control_mode failed: {err:?}");
                }
            }
        });
    }

    pub(super) fn set_frustum_lock(&mut self, enabled: bool) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.set_frustum_lock(ctx, enabled) {
                    eprintln!("[mesh_preview] set_frustum_lock failed: {err:?}");
                }
            }
        });
    }

    pub(super) fn reset_mesh_camera(&mut self) {
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.reset_mesh_camera(ctx) {
                    eprintln!("[mesh_preview] reset_mesh_camera failed: {err:?}");
                }
            }
        });
    }

    pub(super) fn set_preview_mesh(&mut self, new_key: String) {
        let scene_refs = self.scene_material_refs.clone();
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                if let Err(err) = plugin.set_preview_mesh(ctx, &scene_refs, new_key.clone()) {
                    eprintln!("[mesh_preview] set_preview_mesh failed: {err:?}");
                }
            }
        });
    }

    pub(super) fn spawn_mesh_entity(&mut self, mesh_key: &str) {
        let key = mesh_key.to_string();
        let mut spawned = None;
        self.with_plugins(|plugins, ctx| {
            if let Some(plugin) = plugins.get_mut::<MeshPreviewPlugin>() {
                match plugin.spawn_mesh_entity(ctx, &key) {
                    Ok(entity) => spawned = entity,
                    Err(err) => eprintln!("[mesh_preview] spawn_mesh_entity failed: {err:?}"),
                }
            }
        });
        if let Some(entity) = spawned {
            self.selected_entity = Some(entity);
        }
    }
}
