use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{
    animation_watch::{AnimationAssetKind, AnimationAssetWatcher},
    atlas_watch::normalize_path_for_watch,
    App,
};

impl App {
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

    pub(super) fn init_animation_asset_watcher() -> Option<AnimationAssetWatcher> {
        let mut watcher = match AnimationAssetWatcher::new() {
            Ok(watcher) => watcher,
            Err(err) => {
                eprintln!("[animation] asset watcher disabled: {err:?}");
                return None;
            }
        };
        let watch_roots = [
            ("assets/animations/clips", AnimationAssetKind::Clip),
            ("assets/animations/graphs", AnimationAssetKind::Graph),
            ("assets/animations/skeletal", AnimationAssetKind::Skeletal),
        ];
        for (root, kind) in watch_roots {
            let path = Path::new(root);
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
}
