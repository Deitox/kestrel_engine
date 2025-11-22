use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{
    animation_watch::{AnimationAssetKind, AnimationAssetWatcher},
    atlas_watch::normalize_path_for_watch,
    mesh_reload::run_mesh_reload_job,
    mesh_reload::{MeshReloadJob, MeshReloadRequest, MeshReloadResult},
    App,
};
use crate::assets::TextureAtlasDiagnostics;
use anyhow::Result;

const MAX_MESH_RELOADS_PER_FRAME: usize = 1;

impl App {
    pub fn hot_reload_atlas(&mut self, key: &str) -> Result<(usize, TextureAtlasDiagnostics)> {
        let diagnostics = self.assets.reload_atlas(key)?;
        self.invalidate_atlas_view(key);
        let refreshed = self.ecs.refresh_sprite_animations_for_atlas(key, &self.assets);
        Ok((refreshed, diagnostics))
    }

    pub(super) fn sync_atlas_hot_reload(&mut self) {
        let Some(watcher) = self.atlas_hot_reload.as_mut() else {
            return;
        };
        let mut desired = Vec::new();
        for (key, path) in self.assets.atlas_sources() {
            let path_buf = PathBuf::from(path);
            if let Some((original, normalized)) = normalize_path_for_watch(&path_buf) {
                desired.push((original, normalized, key));
            } else {
                eprintln!("[assets] skipping atlas '{key}' – unable to resolve path for watching");
            }
        }
        if let Err(err) = watcher.sync(&desired) {
            eprintln!("[assets] failed to sync atlas hot-reload watchers: {err}");
        }
    }

    pub(super) fn sync_animation_asset_watch_roots(&mut self) {
        let Some(watcher) = self.animation_asset_watcher.as_mut() else {
            self.animation_watch_roots_queue.clear();
            self.animation_watch_roots_pending.clear();
            self.animation_watch_roots_registered.clear();
            return;
        };
        while let Some((path, kind)) = self.animation_watch_roots_queue.pop() {
            let key = (path.clone(), kind);
            self.animation_watch_roots_pending.remove(&key);
            if !path.exists() {
                continue;
            }
            match watcher.watch_root(&path, kind) {
                Ok(()) => {
                    self.animation_watch_roots_registered.insert(key);
                }
                Err(err) => {
                    eprintln!(
                        "[animation] failed to watch {} directory {}: {err:?}",
                        kind.label(),
                        path.display()
                    );
                }
            }
        }
    }

    pub(super) fn seed_animation_watch_roots(&mut self) {
        for (_, source) in self.assets.clip_sources() {
            self.queue_animation_watch_root(Path::new(&source), AnimationAssetKind::Clip);
        }
        for (_, source) in self.assets.skeleton_sources() {
            self.queue_animation_watch_root(Path::new(&source), AnimationAssetKind::Skeletal);
        }
        for (_, source) in self.assets.animation_graph_sources() {
            self.queue_animation_watch_root(Path::new(&source), AnimationAssetKind::Graph);
        }
    }

    pub(super) fn queue_animation_watch_root(&mut self, path: &Path, kind: AnimationAssetKind) {
        let Some(root) = Self::watch_root_for_source(path) else {
            return;
        };
        if !root.exists() {
            return;
        }
        let normalized = Self::normalize_validation_path(&root);
        let key = (normalized, kind);
        if self.animation_watch_roots_registered.contains(&key)
            || self.animation_watch_roots_pending.contains(&key)
        {
            return;
        }
        self.animation_watch_roots_pending.insert(key.clone());
        self.animation_watch_roots_queue.push(key);
    }

    pub(super) fn watch_root_for_source(path: &Path) -> Option<PathBuf> {
        if path.is_dir() {
            Some(path.to_path_buf())
        } else if let Some(parent) = path.parent() {
            Some(parent.to_path_buf())
        } else {
            Some(path.to_path_buf())
        }
    }

    pub(super) fn init_animation_asset_watcher(asset_root: &Path) -> Option<AnimationAssetWatcher> {
        let mut watcher = match AnimationAssetWatcher::new() {
            Ok(watcher) => watcher,
            Err(err) => {
                eprintln!("[animation] asset watcher disabled: {err:?}");
                return None;
            }
        };
        let watch_roots = [
            (asset_root.join("animations/clips"), AnimationAssetKind::Clip),
            (asset_root.join("animations/graphs"), AnimationAssetKind::Graph),
            (asset_root.join("animations/skeletal"), AnimationAssetKind::Skeletal),
        ];
        for (path, kind) in watch_roots {
            let path = path.as_path();
            if !path.exists() {
                continue;
            }
            if let Err(err) = watcher.watch_root(path, kind) {
                eprintln!("[animation] failed to watch {} ({}): {err:?}", path.display(), kind.label())
            }
        }
        Some(watcher)
    }

    pub(super) fn process_animation_asset_watchers(&mut self) {
        self.dispatch_animation_reload_queue();
        self.drain_animation_reload_results();
        self.drain_animation_validation_results();
        self.sync_animation_asset_watch_roots();
        let Some(watcher) = self.animation_asset_watcher.as_mut() else {
            return;
        };
        let changes = watcher.drain_changes();
        if changes.is_empty() {
            return;
        }
        let mut dedup: HashSet<(PathBuf, AnimationAssetKind)> = HashSet::new();
        for change in changes {
            let normalized = Self::normalize_validation_path(&change.path);
            if !dedup.insert((normalized.clone(), change.kind)) {
                continue;
            }
            if let Some(mut request) = self.prepare_animation_reload_request(normalized, change.kind) {
                request.skip_validation = self.consume_validation_suppression(&request.path);
                self.enqueue_animation_reload(request);
            }
        }
        self.dispatch_animation_reload_queue();
        self.drain_animation_reload_results();
        self.drain_animation_validation_results();
    }

    pub(super) fn process_atlas_hot_reload_events(&mut self) {
        let keys = if let Some(watcher) = self.atlas_hot_reload.as_mut() {
            watcher.drain_keys()
        } else {
            Vec::new()
        };
        if keys.is_empty() {
            return;
        }
        let mut unique = keys;
        unique.sort();
        unique.dedup();
        for key in unique {
            match self.hot_reload_atlas(&key) {
                Ok((updated, diagnostics)) => {
                    println!(
                        "[assets] Hot reloaded atlas '{key}' ({updated} animation component{} refreshed)",
                        if updated == 1 { "" } else { "s" }
                    );
                    self.record_atlas_validation_results(&key, diagnostics);
                }
                Err(err) => {
                    eprintln!("[assets] Failed to hot reload atlas '{key}': {err}");
                }
            }
        }
    }

    pub(super) fn sync_mesh_hot_reload(&mut self) {
        let Some(watcher) = self.mesh_hot_reload.as_mut() else {
            return;
        };
        let mut desired: Vec<(PathBuf, PathBuf, String)> = Vec::new();
        for key in self.mesh_registry.keys() {
            if let Some(path) = self.mesh_registry.mesh_source(key) {
                if let Some((original, normalized)) = normalize_path_for_watch(path) {
                    desired.push((original, normalized, key.to_string()));
                }
            }
        }
        if let Err(err) = watcher.sync(&desired) {
            eprintln!("[mesh] mesh hot-reload sync failed: {err}");
        }
    }

    pub(super) fn process_mesh_hot_reload_events(&mut self) {
        let keys =
            if let Some(watcher) = self.mesh_hot_reload.as_mut() { watcher.drain_keys() } else { Vec::new() };
        let mut unique = keys;
        unique.sort();
        unique.dedup();
        for key in unique {
            if self.mesh_reload_inflight.contains(&key) {
                continue;
            }
            if self.mesh_hot_reload_pending_set.insert(key.clone()) {
                self.mesh_hot_reload_pending.push_back(key);
            }
        }
        self.dispatch_mesh_reload_jobs();
        self.drain_mesh_reload_results();
    }

    fn dispatch_mesh_reload_jobs(&mut self) {
        let mut submitted = 0usize;
        while submitted < MAX_MESH_RELOADS_PER_FRAME {
            let Some(key) = self.mesh_hot_reload_pending.pop_front() else { break };
            self.mesh_hot_reload_pending_set.remove(&key);
            if self.mesh_reload_inflight.contains(&key) {
                continue;
            }
            let source = self.mesh_registry.mesh_source(&key).and_then(|p| p.to_str().map(|s| s.to_string()));
            let path = match source {
                Some(path) => path,
                None => {
                    eprintln!("[mesh] Hot reload skipped for '{key}': no source path recorded");
                    continue;
                }
            };
            let request = MeshReloadRequest { key: key.clone(), path: PathBuf::from(&path) };
            let job = MeshReloadJob { request };
            if let Some(worker) = self.mesh_reload_worker.as_ref() {
                match worker.submit(job) {
                    Ok(()) => {
                        self.mesh_reload_inflight.insert(key);
                        submitted += 1;
                    }
                    Err(returned) => {
                        let returned_key = returned.request.key.clone();
                        if self.mesh_hot_reload_pending_set.insert(returned_key.clone()) {
                            self.mesh_hot_reload_pending.push_front(returned_key);
                        }
                        break;
                    }
                }
            } else {
                let result = run_mesh_reload_job(job);
                self.apply_mesh_reload_result(result);
                submitted += 1;
            }
        }
    }

    fn drain_mesh_reload_results(&mut self) {
        if let Some(worker) = self.mesh_reload_worker.as_ref() {
            for result in worker.drain() {
                self.apply_mesh_reload_result(result);
            }
        }
    }

    fn apply_mesh_reload_result(&mut self, result: MeshReloadResult) {
        self.mesh_reload_inflight.remove(&result.key);
        match result.data {
            Ok(import) => {
                let fingerprint = self.mesh_registry.fingerprint_for_path(&result.path);
                let outcome = self.mesh_registry.apply_import(
                    &result.key,
                    import,
                    result.path.clone(),
                    fingerprint,
                    &mut self.material_registry,
                );
                match outcome {
                    Ok(()) => println!("[mesh] Hot reloaded '{}' from {}", result.key, result.path.display()),
                    Err(err) => eprintln!("[mesh] Failed to apply reload for '{}': {err}", result.key),
                }
            }
            Err(err) => eprintln!(
                "[mesh] Reload failed for '{}': {} (from {})",
                result.key,
                err,
                result.path.display()
            ),
        }
    }
}
