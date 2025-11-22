use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::animation_validation::{AnimationValidationEvent, AnimationValidationSeverity};
use crate::assets::TextureAtlasDiagnostics;

use super::animation_reload::{
    AnimationAssetReload, AnimationReloadData, AnimationReloadRequest, AnimationReloadResult,
    AnimationValidationJob,
};
use super::animation_watch::AnimationAssetKind;
use super::App;

pub(super) struct SkeletonPlaybackSnapshot {
    pub(super) entity: bevy_ecs::prelude::Entity,
    pub(super) clip_key: Option<String>,
    pub(super) time: f32,
    pub(super) playing: bool,
    pub(super) speed: f32,
    pub(super) group: Option<String>,
}

pub(super) fn default_graph_key(path: &Path) -> String {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| stem.to_string())
        .unwrap_or_else(|| path.display().to_string())
}

impl App {
    pub(super) fn prepare_animation_reload_request(
        &self,
        path: PathBuf,
        kind: AnimationAssetKind,
    ) -> Option<AnimationReloadRequest> {
        let key = match kind {
            AnimationAssetKind::Clip => self.assets.clip_key_for_source_path(&path)?,
            AnimationAssetKind::Graph => {
                self.assets.graph_key_for_source_path(&path).unwrap_or_else(|| default_graph_key(&path))
            }
            AnimationAssetKind::Skeletal => self.assets.skeleton_key_for_source_path(&path)?,
        };
        Some(AnimationReloadRequest { path, key, kind, skip_validation: false })
    }

    pub(super) fn enqueue_animation_reload(&mut self, request: AnimationReloadRequest) {
        let results = self.animation_reload.enqueue(request);
        for result in results {
            self.apply_animation_reload_result(result);
        }
    }

    pub(super) fn dispatch_animation_reload_queue(&mut self) {
        let results = self.animation_reload.dispatch_queue();
        for result in results {
            self.apply_animation_reload_result(result);
        }
    }

    pub(super) fn drain_animation_reload_results(&mut self) {
        let results = self.animation_reload.drain_animation_reload_results();
        for result in results {
            self.apply_animation_reload_result(result);
        }
    }

    pub(super) fn apply_animation_reload_result(&mut self, result: AnimationReloadResult) {
        match result.data {
            Ok(AnimationReloadData::Clip { clip, bytes }) => {
                let key = result.request.key.clone();
                let path_string = result.request.path.to_string_lossy().to_string();
                self.assets.replace_clip(&key, &path_string, *clip);
                self.queue_animation_watch_root(&result.request.path, AnimationAssetKind::Clip);
                if let Some(updated) = self.assets.clip(&key) {
                    let canonical = Arc::new(updated.clone());
                    {
                        let mut state = self.editor_ui_state_mut();
                        state.clip_edit_overrides.remove(&key);
                        state.clip_dirty.remove(&key);
                        state.animation_clip_status =
                            Some(format!("Reloaded clip '{}' from {}", key, result.request.path.display()));
                    }
                    self.apply_clip_override_to_instances(&key, Arc::clone(&canonical));
                }
                if !result.request.skip_validation {
                    self.enqueue_animation_validation_job(AnimationAssetReload {
                        path: result.request.path.clone(),
                        kind: AnimationAssetKind::Clip,
                        bytes: Some(bytes),
                    });
                }
            }
            Ok(AnimationReloadData::Graph { graph, bytes }) => {
                let key = result.request.key.clone();
                let path_string = result.request.path.to_string_lossy().to_string();
                self.assets.replace_animation_graph(&key, &path_string, graph);
                self.queue_animation_watch_root(&result.request.path, AnimationAssetKind::Graph);
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status = Some(format!(
                        "Reloaded animation graph '{}' from {}",
                        key,
                        result.request.path.display()
                    ));
                });
                if !result.request.skip_validation {
                    self.enqueue_animation_validation_job(AnimationAssetReload {
                        path: result.request.path.clone(),
                        kind: AnimationAssetKind::Graph,
                        bytes: Some(bytes),
                    });
                }
            }
            Ok(AnimationReloadData::Skeletal { import }) => {
                let key = result.request.key.clone();
                let path_string = result.request.path.to_string_lossy().to_string();
                self.assets.replace_skeleton_from_import(&key, &path_string, import);
                self.queue_animation_watch_root(&result.request.path, AnimationAssetKind::Skeletal);
                let mut snapshots: Vec<SkeletonPlaybackSnapshot> = Vec::new();
                {
                    let mut query =
                        self.ecs.world.query::<(bevy_ecs::prelude::Entity, &crate::ecs::SkeletonInstance)>();
                    for (entity, instance) in query.iter(&self.ecs.world) {
                        if instance.skeleton_key.as_ref() == key.as_str() {
                            snapshots.push(SkeletonPlaybackSnapshot {
                                entity,
                                clip_key: instance.active_clip_key.as_ref().map(|k| k.as_ref().to_string()),
                                time: instance.time,
                                playing: instance.playing,
                                speed: instance.speed,
                                group: instance.group.clone(),
                            });
                        }
                    }
                }
                for snapshot in snapshots {
                    self.ecs.set_skeleton(snapshot.entity, &self.assets, &key);
                    if let Some(ref clip_key) = snapshot.clip_key {
                        let _ = self.ecs.set_skeleton_clip(snapshot.entity, &self.assets, clip_key);
                        let _ = self.ecs.set_skeleton_clip_time(snapshot.entity, snapshot.time);
                        let _ = self.ecs.set_skeleton_clip_playing(snapshot.entity, snapshot.playing);
                        let _ = self.ecs.set_skeleton_clip_speed(snapshot.entity, snapshot.speed);
                        let _ = self.ecs.set_skeleton_clip_group(snapshot.entity, snapshot.group.as_deref());
                    }
                }
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status =
                        Some(format!("Reloaded skeleton '{}' from {}", key, result.request.path.display()));
                });
                if !result.request.skip_validation {
                    self.enqueue_animation_validation_job(AnimationAssetReload {
                        path: result.request.path.clone(),
                        kind: AnimationAssetKind::Skeletal,
                        bytes: None,
                    });
                }
            }
            Err(err) => {
                eprintln!("[animation] reload failed for {}: {err:?}", result.request.path.display());
                self.with_editor_ui_state_mut(|state| {
                    state.animation_clip_status = Some(format!(
                        "Reload failed for {} from {}: {err}",
                        result.request.key,
                        result.request.path.display()
                    ));
                });
            }
        }
    }

    pub(super) fn enqueue_animation_validation_job(&mut self, reload: AnimationAssetReload) {
        let job = AnimationValidationJob { path: reload.path, kind: reload.kind, bytes: reload.bytes };
        if let Some(result) = self.animation_reload.submit_validation_job(job) {
            self.handle_validation_events(result.kind.label(), result.path.as_path(), result.events);
        }
    }

    pub(super) fn drain_animation_validation_results(&mut self) {
        for result in self.animation_reload.drain_validation_results() {
            self.handle_validation_events(result.kind.label(), result.path.as_path(), result.events);
        }
    }

    pub(super) fn handle_validation_events(
        &mut self,
        context: &str,
        path: &Path,
        events: Vec<AnimationValidationEvent>,
    ) {
        if events.is_empty() {
            eprintln!(
                "[animation] detected change for {} ({context}) but no validations ran",
                path.display()
            );
            self.with_editor_ui_state_mut(|state| {
                state.animation_clip_status =
                    Some(format!("Detected {context} change but no validators ran: {}", path.display()));
            });
            return;
        }
        for event in events {
            self.with_editor_ui_state_mut(|state| {
                state.pending_animation_validation_events.push(event.clone())
            });
            self.log_animation_validation_event(event);
        }
    }

    pub(super) fn record_atlas_validation_results(
        &mut self,
        key: &str,
        diagnostics: TextureAtlasDiagnostics,
    ) {
        let Some(source_path) = self.assets.atlas_source(key).map(|s| s.to_string()) else {
            eprintln!("[animation] atlas '{key}' hot-reloaded without a recorded source path");
            return;
        };
        let path_buf = PathBuf::from(&source_path);
        if self.consume_validation_suppression(&path_buf) {
            return;
        }
        let mut events = Vec::new();
        let info_message = if let Some(snapshot) = self.assets.atlas_snapshot(key) {
            let region_count = snapshot.regions.len();
            let timeline_count = snapshot.animations.len();
            let image_label = snapshot.image_path.display().to_string();
            format!(
                "Parsed atlas '{key}' with {region_count} region{} and {timeline_count} timeline{} (image: {image_label}).",
                if region_count == 1 { "" } else { "s" },
                if timeline_count == 1 { "" } else { "s" }
            )
        } else {
            format!("Reloaded atlas '{key}' ({source_path})")
        };
        events.push(AnimationValidationEvent {
            severity: AnimationValidationSeverity::Info,
            path: path_buf.clone(),
            message: info_message,
        });
        for warning in diagnostics.warnings {
            events.push(AnimationValidationEvent {
                severity: AnimationValidationSeverity::Warning,
                path: path_buf.clone(),
                message: warning,
            });
        }
        for event in events {
            self.with_editor_ui_state_mut(|state| {
                state.pending_animation_validation_events.push(event.clone())
            });
            self.log_animation_validation_event(event);
        }
    }

    pub(super) fn suppress_validation_for_path(&mut self, path: &Path) {
        let normalized = Self::normalize_validation_path(path);
        self.with_editor_ui_state_mut(|state| {
            state.suppressed_validation_paths.insert(normalized);
        });
    }

    pub(super) fn consume_validation_suppression(&mut self, path: &Path) -> bool {
        let normalized = Self::normalize_validation_path(path);
        self.with_editor_ui_state_mut(|state| state.suppressed_validation_paths.remove(&normalized))
    }

    pub(super) fn normalize_validation_path(path: &Path) -> PathBuf {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    pub(super) fn log_animation_validation_event(&mut self, event: AnimationValidationEvent) {
        let severity = event.severity.to_string();
        let formatted =
            format!("[animation] validation {severity} for {}: {}", event.path.display(), event.message);
        eprintln!("{formatted}");
        self.with_editor_ui_state_mut(|state| state.animation_clip_status = Some(formatted.clone()));
        if matches!(event.severity, AnimationValidationSeverity::Warning | AnimationValidationSeverity::Error)
        {
            self.set_inspector_status(Some(formatted));
        }
    }

    pub(super) fn drain_animation_validation_events(&mut self) -> Vec<AnimationValidationEvent> {
        self.with_editor_ui_state_mut(|state| std::mem::take(&mut state.pending_animation_validation_events))
    }
}
