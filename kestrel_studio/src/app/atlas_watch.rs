use anyhow::{anyhow, Result};
use notify::event::ModifyKind;
use notify::{Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

#[derive(Debug, Clone)]
pub(crate) struct AtlasWatchEntry {
    pub(crate) key: String,
    pub(crate) original: PathBuf,
}

pub(crate) struct AtlasHotReload {
    watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    watched: HashMap<PathBuf, AtlasWatchEntry>,
}

pub(crate) fn normalize_path_for_watch(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let absolute = if path.is_absolute() { path.to_path_buf() } else { env::current_dir().ok()?.join(path) };
    let canonical = fs::canonicalize(&absolute).unwrap_or_else(|_| absolute.clone());
    Some((absolute, canonical))
}

pub(crate) fn normalize_event_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else if let Ok(cwd) = env::current_dir() {
        let absolute = cwd.join(path);
        fs::canonicalize(&absolute).unwrap_or(absolute)
    } else {
        PathBuf::from(path)
    }
}

impl AtlasHotReload {
    pub(crate) fn new() -> Result<Self> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        if let Err(err) = watcher.configure(
            NotifyConfig::default()
                .with_compare_contents(true)
                .with_poll_interval(Duration::from_millis(250)),
        ) {
            eprintln!("[assets] atlas watcher configuration warning: {err}");
        }
        Ok(Self { watcher, rx, watched: HashMap::new() })
    }

    pub(crate) fn sync(&mut self, desired: &[(PathBuf, PathBuf, String)]) -> Result<()> {
        let mut desired_map: HashMap<PathBuf, (PathBuf, String)> = HashMap::new();
        for (original, normalized, key) in desired {
            desired_map.insert(normalized.clone(), (original.clone(), key.clone()));
        }
        for (normalized, (original, key)) in desired_map.iter() {
            match self.watched.get_mut(normalized) {
                Some(entry) => {
                    if entry.key != *key {
                        entry.key = key.clone();
                    }
                }
                None => {
                    self.watch_path(original.clone(), normalized.clone(), key.clone())
                        .map_err(|err| anyhow!("watch failed for '{}': {err}", original.display()))?;
                }
            }
        }
        let obsolete: Vec<PathBuf> =
            self.watched.keys().filter(|path| !desired_map.contains_key(*path)).cloned().collect();
        for normalized in obsolete {
            self.unwatch_path(&normalized)
                .map_err(|err| anyhow!("unwatch failed for '{}': {err}", normalized.display()))?;
        }
        Ok(())
    }

    pub(crate) fn drain_keys(&mut self) -> Vec<String> {
        let mut keys = Vec::new();
        while let Ok(res) = self.rx.try_recv() {
            match res {
                Ok(event) => {
                    if !Self::is_relevant(&event.kind) {
                        continue;
                    }
                    for path in event.paths {
                        if let Some(key) = self.resolve_path(&path) {
                            if !keys.contains(&key) {
                                keys.push(key);
                            }
                        }
                    }
                }
                Err(err) => eprintln!("[assets] Atlas watcher error: {err}"),
            }
        }
        keys
    }

    pub(crate) fn watch_path(
        &mut self,
        original: PathBuf,
        normalized: PathBuf,
        key: String,
    ) -> notify::Result<()> {
        self.watcher.watch(&original, RecursiveMode::NonRecursive)?;
        self.watched.insert(normalized, AtlasWatchEntry { key, original });
        Ok(())
    }

    pub(crate) fn unwatch_path(&mut self, normalized: &Path) -> notify::Result<()> {
        if let Some(entry) = self.watched.remove(normalized) {
            self.watcher.unwatch(&entry.original)?;
        }
        Ok(())
    }

    fn resolve_path(&self, path: &Path) -> Option<String> {
        let normalized = normalize_event_path(path);
        if let Some(entry) = self.watched.get(&normalized) {
            return Some(entry.key.clone());
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else if let Ok(cwd) = env::current_dir() {
            cwd.join(path)
        } else {
            PathBuf::from(path)
        };
        for entry in self.watched.values() {
            if entry.original == absolute {
                return Some(entry.key.clone());
            }
        }
        None
    }

    fn is_relevant(kind: &EventKind) -> bool {
        matches!(
            kind,
            EventKind::Modify(ModifyKind::Data(_))
                | EventKind::Modify(ModifyKind::Name(_))
                | EventKind::Modify(ModifyKind::Any)
                | EventKind::Create(_)
                | EventKind::Remove(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_paths() {
        let path = Path::new("assets/atlas.json");
        let (absolute, canonical) = normalize_path_for_watch(path).expect("path normalized");
        assert!(absolute.is_absolute(), "absolute path expected");
        assert!(canonical.is_absolute(), "canonical path expected");
    }

    #[test]
    fn normalize_event_path_handles_relative() {
        let relative = PathBuf::from("foo/bar.txt");
        let normalized = normalize_event_path(&relative);
        assert!(normalized.is_absolute(), "relative paths normalize to absolute");
    }
}
