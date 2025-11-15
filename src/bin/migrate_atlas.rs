use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const TARGET_VERSION: u32 = 2;

fn main() {
    if let Err(err) = run() {
        eprintln!("[migrate_atlas] error: {err:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut check_only = false;
    let mut inputs = Vec::new();
    let mut show_help = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--help" | "-h" => show_help = true,
            "--check" => check_only = true,
            other => inputs.push(other.to_string()),
        }
    }
    if show_help || inputs.is_empty() {
        print_usage();
        if inputs.is_empty() {
            return Ok(());
        }
        return Ok(());
    }
    let targets = collect_targets(&inputs)?;
    if targets.is_empty() {
        return Err(anyhow!("no atlas JSON files found in provided paths"));
    }
    let total = targets.len();
    let mut updated = 0_usize;
    for path in &targets {
        let changed = migrate_file(path.as_path(), check_only)
            .with_context(|| format!("failed to migrate '{}'", path.display()))?;
        if changed {
            if check_only {
                println!("Would update {}", path.display());
            } else {
                println!("Updated {}", path.display());
            }
            updated += 1;
        } else {
            println!("No changes needed {}", path.display());
        }
    }
    let verb = if check_only { "would change" } else { "updated" };
    println!("Processed {} files ({} {})", total, updated, verb);
    if check_only && updated > 0 {
        return Err(anyhow!(
            "{updated} file(s) require migration; rerun without --check to rewrite assets."
        ));
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "migrate_atlas

Usage:
  migrate_atlas [--check] <path> [<path>...]

Each <path> may be a JSON file or directory. Directories are walked recursively
and atlas documents are rewritten in place with the latest schema patches.
Use --check to verify cleanliness without modifying files (CI safe).
"
    );
}

fn collect_targets(inputs: &[String]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for input in inputs {
        let path = PathBuf::from(input);
        if !path.exists() {
            return Err(anyhow!("path '{}' does not exist", input));
        }
        if path.is_file() {
            if should_consider(&path) {
                add_target(path, &mut seen, &mut files)?;
            } else {
                eprintln!(
                    "[migrate_atlas] skipping '{}' (unsupported extension)",
                    path.display()
                );
            }
        } else if path.is_dir() {
            walk_dir(&path, &mut seen, &mut files)
                .with_context(|| format!("failed to enumerate directory '{}'", path.display()))?;
        } else {
            return Err(anyhow!("path '{}' is neither file nor directory", input));
        }
    }
    Ok(files)
}

fn walk_dir(dir: &Path, seen: &mut HashSet<PathBuf>, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, seen, files)?;
        } else if should_consider(&path) {
            add_target(path, seen, files)?;
        }
    }
    Ok(())
}

fn add_target(path: PathBuf, seen: &mut HashSet<PathBuf>, files: &mut Vec<PathBuf>) -> Result<()> {
    let normalized = normalize_path(&path)?;
    if seen.insert(normalized.clone()) {
        files.push(normalized);
    }
    Ok(())
}

fn should_consider(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
}

fn normalize_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        Ok(fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()))
    } else if let Some(parent) = path.parent() {
        let parent = fs::canonicalize(parent)
            .with_context(|| format!("failed to canonicalize '{}'", parent.display()))?;
        Ok(parent.join(path.file_name().unwrap_or_default()))
    } else {
        Ok(path.to_path_buf())
    }
}

fn migrate_file(path: &Path, check_only: bool) -> Result<bool> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read '{}'", path.display()))?;
    let mut doc: AtlasDocument =
        serde_json::from_str(&contents).with_context(|| "file does not match atlas schema")?;
    let mut changed = false;
    if doc.version.unwrap_or(1) < TARGET_VERSION {
        doc.version = Some(TARGET_VERSION);
        changed = true;
    }
    for (name, timeline) in doc.animations.iter_mut() {
        if migrate_timeline(name, timeline) {
            changed = true;
        }
    }
    if changed && !check_only {
        let serialized =
            serde_json::to_string_pretty(&doc).context("failed to serialize migrated atlas")?;
        fs::write(path, format!("{serialized}\n"))
            .with_context(|| format!("failed to write '{}'", path.display()))?;
    }
    Ok(changed)
}

fn migrate_timeline(name: &str, timeline: &mut TimelineDocument) -> bool {
    let mut changed = false;
    let (mode, looped) = canonical_loop_mode(timeline.loop_mode.as_deref(), timeline.looped);
    if timeline.loop_mode.as_deref() != Some(mode.as_str()) {
        timeline.loop_mode = Some(mode.as_str().to_string());
        changed = true;
    }
    if timeline.looped != looped {
        timeline.looped = looped;
        changed = true;
    }
    if sanitize_events(name, timeline) {
        changed = true;
    }
    if sanitize_frames(timeline) {
        changed = true;
    }
    changed
}

fn canonical_loop_mode(raw: Option<&str>, looped: bool) -> (LoopMode, bool) {
    if let Some(value) = raw {
        if let Some(mode) = LoopMode::parse(value) {
            return (mode, mode.looped());
        }
    }
    if looped {
        (LoopMode::Loop, true)
    } else {
        (LoopMode::OnceStop, false)
    }
}

fn sanitize_events(name: &str, timeline: &mut TimelineDocument) -> bool {
    if timeline.events.is_empty() {
        return false;
    }
    let frame_count = timeline.frames.len();
    let mut filtered = Vec::with_capacity(timeline.events.len());
    let mut seen = HashSet::new();
    let mut changed = false;
    for event in &timeline.events {
        if event.frame >= frame_count {
            eprintln!(
                "[migrate_atlas] dropping timeline event '{}' in '{}' referencing missing frame {}",
                event.name, name, event.frame
            );
            changed = true;
            continue;
        }
        let key = (event.frame, event.name.clone());
        if seen.insert(key) {
            filtered.push(event.clone());
        } else {
            changed = true;
        }
    }
    if filtered.len() != timeline.events.len() {
        timeline.events = filtered;
        changed = true;
    }
    changed
}

fn sanitize_frames(timeline: &mut TimelineDocument) -> bool {
    let mut changed = false;
    for frame in &mut timeline.frames {
        if frame.duration_ms == 0 {
            frame.duration_ms = 1;
            changed = true;
        }
    }
    changed
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AtlasDocument {
    #[serde(default)]
    version: Option<u32>,
    image: String,
    width: u32,
    height: u32,
    regions: BTreeMap<String, RectDocument>,
    #[serde(default)]
    animations: BTreeMap<String, TimelineDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RectDocument {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimelineDocument {
    #[serde(default)]
    looped: bool,
    #[serde(default)]
    loop_mode: Option<String>,
    #[serde(default)]
    frames: Vec<TimelineFrameDocument>,
    #[serde(default)]
    events: Vec<TimelineEventDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimelineFrameDocument {
    #[serde(default)]
    name: Option<String>,
    region: String,
    #[serde(default = "default_duration_ms")]
    duration_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TimelineEventDocument {
    frame: usize,
    name: String,
}

const fn default_duration_ms() -> u32 {
    100
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopMode {
    Loop,
    OnceHold,
    OnceStop,
    PingPong,
}

impl LoopMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "loop" => Some(Self::Loop),
            "once_hold" | "oncehold" => Some(Self::OnceHold),
            "once_stop" | "oncestop" | "once" => Some(Self::OnceStop),
            "pingpong" | "ping_pong" => Some(Self::PingPong),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            LoopMode::Loop => "loop",
            LoopMode::OnceHold => "once_hold",
            LoopMode::OnceStop => "once_stop",
            LoopMode::PingPong => "pingpong",
        }
    }

    fn looped(self) -> bool {
        matches!(self, LoopMode::Loop | LoopMode::PingPong)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn canonical_mode_falls_back_when_missing() {
        let (mode, looped) = canonical_loop_mode(None, false);
        assert_eq!(mode, LoopMode::OnceStop);
        assert!(!looped);
    }

    #[test]
    fn canonical_mode_normalizes_text() {
        let (mode, looped) = canonical_loop_mode(Some("PING_PONG"), true);
        assert_eq!(mode, LoopMode::PingPong);
        assert!(looped);
    }

    #[test]
    fn sanitize_events_drops_invalid_frames() {
        let mut timeline = TimelineDocument {
            looped: true,
            loop_mode: Some("loop".to_string()),
            frames: vec![
                TimelineFrameDocument {
                    name: None,
                    region: "a".to_string(),
                    duration_ms: 100,
                },
                TimelineFrameDocument {
                    name: None,
                    region: "b".to_string(),
                    duration_ms: 100,
                },
            ],
            events: vec![
                TimelineEventDocument {
                    frame: 0,
                    name: "footstep".to_string(),
                },
                TimelineEventDocument {
                    frame: 4,
                    name: "bad".to_string(),
                },
            ],
        };
        assert!(sanitize_events("demo", &mut timeline));
        assert_eq!(timeline.events.len(), 1);
        assert_eq!(timeline.events[0].name, "footstep");
    }

    #[test]
    fn sanitize_frames_clamps_zero_duration() {
        let mut timeline = TimelineDocument {
            looped: true,
            loop_mode: Some("loop".to_string()),
            frames: vec![
                TimelineFrameDocument {
                    name: None,
                    region: "a".to_string(),
                    duration_ms: 0,
                },
                TimelineFrameDocument {
                    name: None,
                    region: "b".to_string(),
                    duration_ms: 50,
                },
            ],
            events: Vec::new(),
        };
        assert!(sanitize_frames(&mut timeline));
        assert_eq!(timeline.frames[0].duration_ms, 1);
        assert_eq!(timeline.frames[1].duration_ms, 50);
    }

    #[test]
    fn check_mode_detects_needed_changes_without_writing() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            r#"{{
    "image": "atlas.png",
    "width": 4,
    "height": 4,
    "regions": {{
        "a": {{ "x": 0, "y": 0, "w": 2, "h": 2 }}
    }},
    "animations": {{
        "idle": {{
            "looped": true,
            "frames": [
                {{ "region": "a", "duration_ms": 0 }}
            ],
            "events": [
                {{ "frame": 2, "name": "bad" }}
            ]
        }}
    }}
}}"#
        )
        .unwrap();
        let path = file.path();
        let contents_before = fs::read_to_string(path).unwrap();
        assert!(migrate_file(path, true).unwrap());
        let contents_after_check = fs::read_to_string(path).unwrap();
        assert_eq!(contents_before, contents_after_check, "check mode must not rewrite files");
        assert!(migrate_file(path, false).unwrap());
        let contents_after_write = fs::read_to_string(path).unwrap();
        assert!(contents_after_write.contains("\"loop_mode\""));
        assert!(contents_after_write.contains("\"duration_ms\": 1"));
        assert!(!contents_after_write.contains("\"frame\": 2"));
    }
}
