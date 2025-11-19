//! CLI tool for converting Aseprite JSON exports into the engine's atlas timeline schema.
//!
//! Usage:
//! ```bash
//! cargo run --bin aseprite_to_atlas -- <input.json> <output.json> \
//!     [--atlas-key main] \
//!     [--default-loop-mode loop|once_hold|once_stop|pingpong] \
//!     [--reverse-loop-mode loop|once_hold|once_stop|pingpong]
//! ```

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct AsepriteFile {
    frames: Vec<AseFrame>,
    meta: AseMeta,
}

#[derive(Debug, Deserialize)]
struct AseFrame {
    filename: String,
    frame: AseRect,
    duration: u32,
}

#[derive(Debug, Deserialize)]
struct AseRect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Debug, Deserialize)]
struct AseMeta {
    image: String,
    #[serde(default, rename = "frameTags")]
    frame_tags: Vec<AseTag>,
}

#[derive(Debug, Deserialize)]
struct AseTag {
    name: String,
    from: u32,
    to: u32,
    #[serde(default)]
    direction: Option<String>,
}

#[derive(Debug)]
struct Timeline {
    name: String,
    frames: Vec<TimelineFrame>,
    mode: LoopMode,
    lints: Vec<TimelineLint>,
}

#[derive(Debug)]
struct TimelineFrame {
    region: String,
    duration_ms: u32,
    events: Vec<String>,
}

#[derive(Debug)]
struct TimelineLint {
    code: &'static str,
    severity: LintSeverity,
    message: String,
    timeline: String,
    reference_ms: u32,
    max_diff_ms: u32,
    frames: Vec<LintFrame>,
}

#[derive(Debug)]
struct LintFrame {
    index: usize,
    duration_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LintSeverity {
    Info,
    Warn,
}

impl LintSeverity {
    fn as_str(self) -> &'static str {
        match self {
            LintSeverity::Info => "info",
            LintSeverity::Warn => "warn",
        }
    }
}

#[derive(Debug, Deserialize)]
struct TimelineEventRecord {
    frame: usize,
    name: String,
}

#[derive(Debug, Clone, Copy)]
enum LoopMode {
    Loop,
    OnceHold,
    OnceStop,
    PingPong,
}

impl LoopMode {
    fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "loop" => Ok(Self::Loop),
            "once_hold" | "oncehold" => Ok(Self::OnceHold),
            "once_stop" | "oncestop" | "once" => Ok(Self::OnceStop),
            "pingpong" | "ping_pong" => Ok(Self::PingPong),
            other => Err(anyhow!("unknown loop mode '{other}' (expected loop|once_hold|once_stop|pingpong)")),
        }
    }

    fn looped(self) -> bool {
        matches!(self, LoopMode::Loop | LoopMode::PingPong)
    }
}

impl Display for LoopMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            LoopMode::Loop => write!(f, "loop"),
            LoopMode::OnceHold => write!(f, "once_hold"),
            LoopMode::OnceStop => write!(f, "once_stop"),
            LoopMode::PingPong => write!(f, "pingpong"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct LoopModeConfig {
    default_mode: LoopMode,
    reverse_mode: LoopMode,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("[aseprite_to_atlas] error: {err:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let input = args.next().ok_or_else(|| anyhow!("input JSON path required"))?;
    let output = args.next().ok_or_else(|| anyhow!("output JSON path required"))?;
    let mut atlas_key = "main".to_string();
    let mut default_mode = LoopMode::Loop;
    let mut reverse_mode = LoopMode::Loop;
    let mut events_file: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--atlas-key" => {
                atlas_key = args.next().ok_or_else(|| anyhow!("--atlas-key requires a value"))?;
            }
            "--default-loop-mode" => {
                let value = args.next().ok_or_else(|| anyhow!("--default-loop-mode requires a value"))?;
                default_mode = LoopMode::parse(&value)?;
            }
            "--reverse-loop-mode" => {
                let value = args.next().ok_or_else(|| anyhow!("--reverse-loop-mode requires a value"))?;
                reverse_mode = LoopMode::parse(&value)?;
            }
            "--events-file" => {
                let value = args.next().ok_or_else(|| anyhow!("--events-file requires a value"))?;
                events_file = Some(PathBuf::from(value));
            }
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            other => return Err(anyhow!("unknown argument '{other}'")),
        }
    }

    let input_path = PathBuf::from(&input);
    let output_path = PathBuf::from(&output);

    let data = fs::read_to_string(&input_path)
        .with_context(|| format!("reading Aseprite JSON {}", input_path.display()))?;
    let ase: AsepriteFile = serde_json::from_str(&data)
        .with_context(|| format!("parsing Aseprite JSON {}", input_path.display()))?;

    if ase.frames.is_empty() {
        return Err(anyhow!("Aseprite export contains no frames"));
    }

    let regions = build_regions(&ase)?;
    let loop_config = LoopModeConfig { default_mode, reverse_mode };
    let events_map = if let Some(path) = events_file { load_events_file(&path)? } else { HashMap::new() };
    let timelines = build_timelines(&ase, &loop_config, &events_map)?;

    let (animations_json, lint_json) = timelines_to_json(&timelines);
    let mut atlas_json = serde_json::Map::new();
    atlas_json.insert("image".to_string(), json!(ase.meta.image));
    atlas_json.insert("width".to_string(), json!(determine_width(&ase.frames)?));
    atlas_json.insert("height".to_string(), json!(determine_height(&ase.frames)?));
    atlas_json.insert("regions".to_string(), json!(regions));
    atlas_json.insert("animations".to_string(), animations_json);
    atlas_json.insert("atlas_key".to_string(), json!(atlas_key));
    if !lint_json.is_empty() {
        atlas_json.insert("lint".to_string(), serde_json::Value::Array(lint_json));
    }
    let atlas_json = serde_json::Value::Object(atlas_json);

    if let Some(dir) = output_path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating output directory {}", dir.display()))?;
    }
    fs::write(&output_path, serde_json::to_vec_pretty(&atlas_json)?)
        .with_context(|| format!("writing output {}", output_path.display()))?;

    println!(
        "[aseprite_to_atlas] Wrote atlas definition with {} regions and {} timelines to {}",
        regions.len(),
        timelines.len(),
        output_path.display()
    );
    Ok(())
}

fn load_events_file(path: &Path) -> Result<HashMap<String, Vec<TimelineEventRecord>>> {
    let data = fs::read_to_string(path).with_context(|| format!("reading events file {}", path.display()))?;
    let parsed: HashMap<String, Vec<TimelineEventRecord>> =
        serde_json::from_str(&data).with_context(|| format!("parsing events file {}", path.display()))?;
    Ok(parsed)
}

fn print_usage() {
    println!(
        "Usage: aseprite_to_atlas <input.json> <output.json> [--atlas-key name] \\\n    \
         [--default-loop-mode loop|once_hold|once_stop|pingpong] [--reverse-loop-mode loop|once_hold|once_stop|pingpong]"
    );
    println!("Converts an Aseprite JSON export into an atlas timeline definition.");
}

fn determine_width(frames: &[AseFrame]) -> Result<u32> {
    frames
        .iter()
        .map(|frame| frame.frame.x + frame.frame.w)
        .max()
        .ok_or_else(|| anyhow!("missing frame dimensions"))
}

fn determine_height(frames: &[AseFrame]) -> Result<u32> {
    frames
        .iter()
        .map(|frame| frame.frame.y + frame.frame.h)
        .max()
        .ok_or_else(|| anyhow!("missing frame dimensions"))
}

fn build_regions(ase: &AsepriteFile) -> Result<HashMap<String, serde_json::Value>> {
    let mut map = HashMap::new();
    for frame in &ase.frames {
        let key = frame.filename.clone();
        if map.contains_key(&key) {
            return Err(anyhow!("duplicate frame filename '{key}' in Aseprite export"));
        }
        map.insert(
            key,
            json!({
                "x": frame.frame.x,
                "y": frame.frame.y,
                "w": frame.frame.w,
                "h": frame.frame.h
            }),
        );
    }
    Ok(map)
}

fn build_timelines(
    ase: &AsepriteFile,
    config: &LoopModeConfig,
    events: &HashMap<String, Vec<TimelineEventRecord>>,
) -> Result<Vec<Timeline>> {
    if ase.meta.frame_tags.is_empty() {
        let mut frames = Vec::new();
        let mut event_map = collate_events(events.get("default"));
        for (index, frame) in ase.frames.iter().enumerate() {
            let frame_events = event_map.remove(&index).unwrap_or_default();
            frames.push(TimelineFrame {
                region: frame.filename.clone(),
                duration_ms: frame.duration.max(1),
                events: frame_events,
            });
        }
        for (frame, names) in event_map {
            eprintln!(
                "[aseprite_to_atlas] warning: events {:?} reference frame {} in 'default' timeline, but it does not exist.",
                names, frame
            );
        }
        let mut lint_entries = Vec::new();
        if let Some(lint) = detect_uniform_drift("default", &frames) {
            report_lint(&lint);
            lint_entries.push(lint);
        }
        let timeline =
            Timeline { name: "default".to_string(), frames, mode: config.default_mode, lints: lint_entries };
        return Ok(vec![timeline]);
    }

    let mut timelines = Vec::new();
    for tag in &ase.meta.frame_tags {
        let mut frames = Vec::new();
        let mut event_map = collate_events(events.get(&tag.name));
        let from = tag.from as usize;
        let to = tag.to as usize;
        if from >= ase.frames.len() || to >= ase.frames.len() || from > to {
            return Err(anyhow!(
                "frame tag '{}' has invalid range [{}..{}] for {} frames",
                tag.name,
                from,
                to,
                ase.frames.len()
            ));
        }
        for (local_index, frame_index) in (from..=to).enumerate() {
            let frame = &ase.frames[frame_index];
            let frame_events = event_map.remove(&local_index).unwrap_or_default();
            frames.push(TimelineFrame {
                region: frame.filename.clone(),
                duration_ms: frame.duration.max(1),
                events: frame_events,
            });
        }
        let mode = match tag.direction.as_deref().map(|s| s.to_ascii_lowercase()) {
            Some(direction) if direction == "pingpong" => LoopMode::PingPong,
            Some(direction) if direction == "reverse" => config.reverse_mode,
            _ => config.default_mode,
        };
        for (frame, names) in event_map {
            eprintln!(
                "[aseprite_to_atlas] warning: events {:?} reference frame {} in timeline '{}', but it does not exist.",
                names, frame, tag.name
            );
        }
        let mut lint_entries = Vec::new();
        if let Some(lint) = detect_uniform_drift(&tag.name, &frames) {
            report_lint(&lint);
            lint_entries.push(lint);
        }
        timelines.push(Timeline { name: tag.name.clone(), frames, mode, lints: lint_entries });
    }
    Ok(timelines)
}

fn collate_events(records: Option<&Vec<TimelineEventRecord>>) -> HashMap<usize, Vec<String>> {
    let mut map = HashMap::new();
    if let Some(entries) = records {
        for record in entries {
            map.entry(record.frame).or_insert_with(Vec::new).push(record.name.clone());
        }
    }
    map
}

fn detect_uniform_drift(name: &str, frames: &[TimelineFrame]) -> Option<TimelineLint> {
    if frames.len() < 2 {
        return None;
    }
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for frame in frames {
        *counts.entry(frame.duration_ms).or_insert(0) += 1;
    }
    let (reference, count) = counts.into_iter().max_by_key(|(_, count)| *count)?;
    let mut minimum_majority = frames.len() * 3 / 5;
    if minimum_majority == 0 {
        minimum_majority = 1;
    }
    if count < minimum_majority {
        return None;
    }
    let mut max_diff = 0_u32;
    let mut offenders = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let diff = reference.abs_diff(frame.duration_ms);
        if diff > 0 {
            max_diff = max_diff.max(diff);
            offenders.push(LintFrame { index, duration_ms: frame.duration_ms });
        }
    }
    if offenders.is_empty() {
        return None;
    }
    let severity = if max_diff <= 1 { LintSeverity::Info } else { LintSeverity::Warn };
    let message = format!(
        "timeline '{}' drifts {}ms from {}ms baseline across {} frames",
        name,
        max_diff,
        reference,
        offenders.len()
    );
    Some(TimelineLint {
        code: "uniform_dt_drift",
        severity,
        message,
        timeline: name.to_string(),
        reference_ms: reference,
        max_diff_ms: max_diff,
        frames: offenders,
    })
}

fn report_lint(lint: &TimelineLint) {
    let tier = lint.severity.as_str();
    eprintln!("[aseprite_to_atlas] lint({tier}): {}", lint.message);
}

fn timelines_to_json(timelines: &[Timeline]) -> (serde_json::Value, Vec<serde_json::Value>) {
    let mut map = serde_json::Map::new();
    let mut lint_entries = Vec::new();
    for timeline in timelines {
        let frames_json: Vec<serde_json::Value> = timeline
            .frames
            .iter()
            .map(|frame| {
                json!({
                    "region": frame.region,
                    "duration_ms": frame.duration_ms
                })
            })
            .collect();
        let events_json: Vec<serde_json::Value> = timeline
            .frames
            .iter()
            .enumerate()
            .flat_map(|(index, frame)| {
                frame.events.iter().map(move |event| json!({ "frame": index, "name": event }))
            })
            .collect();
        let mut timeline_json = serde_json::Map::new();
        timeline_json.insert("loop_mode".to_string(), json!(timeline.mode.to_string()));
        timeline_json.insert("looped".to_string(), json!(timeline.mode.looped()));
        timeline_json.insert("frames".to_string(), serde_json::Value::Array(frames_json));
        if !events_json.is_empty() {
            timeline_json.insert("events".to_string(), serde_json::Value::Array(events_json));
        }
        map.insert(timeline.name.clone(), serde_json::Value::Object(timeline_json));
        for lint in &timeline.lints {
            lint_entries.push(lint_to_json(lint));
        }
    }
    (serde_json::Value::Object(map), lint_entries)
}

fn lint_to_json(lint: &TimelineLint) -> serde_json::Value {
    let frames: Vec<serde_json::Value> = lint
        .frames
        .iter()
        .map(|frame| json!({ "frame": frame.index, "duration_ms": frame.duration_ms }))
        .collect();
    json!({
        "code": lint.code,
        "severity": lint.severity.as_str(),
        "timeline": lint.timeline,
        "message": lint.message,
        "reference_ms": lint.reference_ms,
        "max_diff_ms": lint.max_diff_ms,
        "frames": frames
    })
}
