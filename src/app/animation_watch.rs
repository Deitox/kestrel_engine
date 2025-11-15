use anyhow::{Context, Result};
use notify::event::ModifyKind;
use notify::{Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::VecDeque;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationAssetKind {
    Clip,
    Graph,
    Skeletal,
}

impl AnimationAssetKind {
    pub fn label(self) -> &'static str {
        match self {
            AnimationAssetKind::Clip => "clip",
            AnimationAssetKind::Graph => "graph",
            AnimationAssetKind::Skeletal => "skeletal",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnimationAssetChange {
    pub path: PathBuf,
    pub kind: AnimationAssetKind,
}

pub struct AnimationAssetWatcher {
    watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    registrations: Vec<(PathBuf, AnimationAssetKind)>,
}

impl AnimationAssetWatcher {
    pub fn new() -> Result<Self> {
        let (tx, rx) = channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        watcher
            .configure(
                NotifyConfig::default()
                    .with_compare_contents(false)
                    .with_poll_interval(Duration::from_millis(300)),
            )
            .context("configure animation watcher")?;
        Ok(Self { watcher, rx, registrations: Vec::new() })
    }

    pub fn watch_root(&mut self, root: impl AsRef<Path>, kind: AnimationAssetKind) -> Result<()> {
        let root = root.as_ref();
        if !root.exists() {
            anyhow::bail!("path '{}' does not exist", root.display());
        }
        let normalized = normalize_watch_path(root);
        if self.registrations.iter().any(|(existing, _)| *existing == normalized) {
            return Ok(());
        }
        let mode = if normalized.is_dir() { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        self.watcher.watch(&normalized, mode).with_context(|| format!("watch {}", normalized.display()))?;
        self.registrations.push((normalized, kind));
        Ok(())
    }

    pub fn drain_changes(&mut self) -> Vec<AnimationAssetChange> {
        let mut changes = Vec::new();
        let mut backlog: VecDeque<notify::Result<Event>> = VecDeque::new();
        while let Ok(event) = self.rx.try_recv() {
            backlog.push_back(event);
        }
        while let Some(event) = backlog.pop_front() {
            match event {
                Ok(event) => {
                    if !Self::is_relevant(&event.kind) {
                        continue;
                    }
                    for path in event.paths {
                        if let Some(kind) = self.kind_for_path(&path) {
                            changes.push(AnimationAssetChange { path, kind });
                        }
                    }
                }
                Err(err) => eprintln!("[animation] asset watcher error: {err}"),
            }
        }
        changes
    }

    fn kind_for_path(&self, path: &Path) -> Option<AnimationAssetKind> {
        let normalized = normalize_watch_path(path);
        self.registrations.iter().find(|(root, _)| normalized.starts_with(root)).map(|(_, kind)| *kind)
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

fn normalize_watch_path(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    match fs::canonicalize(&absolute) {
        Ok(canonical) => canonical,
        Err(_) => {
            if let Some(parent) = absolute.parent() {
                if let Ok(parent_canon) = fs::canonicalize(parent) {
                    if let Some(name) = absolute.file_name() {
                        return parent_canon.join(name);
                    }
                    return parent_canon;
                }
            }
            absolute
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_label_matches_enum() {
        assert_eq!(AnimationAssetKind::Clip.label(), "clip");
        assert_eq!(AnimationAssetKind::Graph.label(), "graph");
        assert_eq!(AnimationAssetKind::Skeletal.label(), "skeletal");
    }
}
