//! CLI tool for converting Aseprite JSON exports into the engine's atlas timeline schema.
//!
//! Usage:
//! ```bash
//! cargo run --bin aseprite_to_atlas -- <input.json> <output.json> [--atlas-key main]
//! ```

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

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
    looped: bool,
}

#[derive(Debug)]
struct TimelineFrame {
    region: String,
    duration_ms: u32,
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

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--atlas-key" => {
                atlas_key = args.next().ok_or_else(|| anyhow!("--atlas-key requires a value"))?;
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
    let timelines = build_timelines(&ase)?;

    let atlas_json = json!({
        "image": ase.meta.image,
        "width": determine_width(&ase.frames)?,
        "height": determine_height(&ase.frames)?,
        "regions": regions,
        "animations": timelines_to_json(&timelines),
        "atlas_key": atlas_key,
    });

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

fn print_usage() {
    println!("Usage: aseprite_to_atlas <input.json> <output.json> [--atlas-key name]");
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

fn build_timelines(ase: &AsepriteFile) -> Result<Vec<Timeline>> {
    if ase.meta.frame_tags.is_empty() {
        let mut frames = Vec::new();
        for frame in &ase.frames {
            frames.push(TimelineFrame { region: frame.filename.clone(), duration_ms: frame.duration.max(1) });
        }
        let timeline = Timeline { name: "default".to_string(), frames, looped: true };
        return Ok(vec![timeline]);
    }

    let mut timelines = Vec::new();
    for tag in &ase.meta.frame_tags {
        let mut frames = Vec::new();
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
        for index in from..=to {
            let frame = &ase.frames[index];
            frames.push(TimelineFrame { region: frame.filename.clone(), duration_ms: frame.duration.max(1) });
        }
        let looped = match tag.direction.as_deref() {
            Some("pingpong") => true,
            Some("reverse") => false,
            _ => true,
        };
        timelines.push(Timeline { name: tag.name.clone(), frames, looped });
    }
    Ok(timelines)
}

fn timelines_to_json(timelines: &[Timeline]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
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
        map.insert(
            timeline.name.clone(),
            json!({
                "looped": timeline.looped,
                "frames": frames_json,
            }),
        );
    }
    serde_json::Value::Object(map)
}
