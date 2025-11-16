use crate::assets::skeletal;
use crate::assets::{
    parse_animation_clip_bytes, parse_animation_graph_bytes, parse_texture_atlas_bytes, AnimationClip,
    AnimationGraphAsset, TextureAtlasParseResult,
};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationValidationSeverity {
    Info,
    Warning,
    Error,
}

impl fmt::Display for AnimationValidationSeverity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnimationValidationSeverity::Info => write!(f, "info"),
            AnimationValidationSeverity::Warning => write!(f, "warning"),
            AnimationValidationSeverity::Error => write!(f, "error"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnimationValidationEvent {
    pub severity: AnimationValidationSeverity,
    pub path: PathBuf,
    pub message: String,
}

pub struct AnimationValidator;

impl AnimationValidator {
    /// Validate the asset at `path` and return any validation events.
    pub fn validate_path(path: &Path) -> Vec<AnimationValidationEvent> {
        if !path.exists() {
            return vec![Self::event(
                path,
                AnimationValidationSeverity::Warning,
                "File not found (it may have been removed).",
            )];
        }
        let ext = path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase());
        match ext.as_deref() {
            Some("clip") => return Self::validate_clip_from_path(path),
            Some("gltf") | Some("glb") => return Self::validate_skeletal_asset(path),
            _ => {}
        }
        if ext.as_deref() == Some("json") {
            let bytes = match fs::read(path) {
                Ok(bytes) => bytes,
                Err(err) => {
                    return vec![Self::event(
                        path,
                        AnimationValidationSeverity::Error,
                        format!("Failed to read JSON: {err}"),
                    )];
                }
            };
            return match Self::classify_json_asset(path, &bytes) {
                JsonAssetKind::Atlas => Self::validate_atlas_bytes(path, &bytes),
                JsonAssetKind::Clip => Self::validate_clip_bytes(path, &bytes),
                JsonAssetKind::Graph => Self::validate_graph_bytes(path, &bytes),
                JsonAssetKind::Unknown => {
                    vec![Self::event(
                        path,
                        AnimationValidationSeverity::Info,
                        "No validators available for this JSON file.",
                    )]
                }
            };
        }
        vec![Self::event(
            path,
            AnimationValidationSeverity::Info,
            "No validators available for this file type.",
        )]
    }

    fn validate_clip_from_path(path: &Path) -> Vec<AnimationValidationEvent> {
        match fs::read(path) {
            Ok(bytes) => Self::validate_clip_bytes(path, &bytes),
            Err(err) => vec![Self::event(
                path,
                AnimationValidationSeverity::Error,
                format!("Failed to read clip: {err}"),
            )],
        }
    }

    fn validate_clip_bytes(path: &Path, bytes: &[u8]) -> Vec<AnimationValidationEvent> {
        let key_hint = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("animation_clip");
        let source_label = path.display().to_string();
        match parse_animation_clip_bytes(bytes, key_hint, &source_label) {
            Ok(clip) => Self::clip_success_events(path, &clip),
            Err(err) => vec![Self::event(path, AnimationValidationSeverity::Error, format!("{err}"))],
        }
    }

    fn validate_graph_bytes(path: &Path, bytes: &[u8]) -> Vec<AnimationValidationEvent> {
        let key_hint = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("animation_graph");
        let source_label = path.display().to_string();
        match parse_animation_graph_bytes(bytes, key_hint, &source_label) {
            Ok(graph) => Self::graph_success_events(path, &graph),
            Err(err) => vec![Self::event(
                path,
                AnimationValidationSeverity::Error,
                format!("Failed to parse animation graph: {err}"),
            )],
        }
    }

    fn validate_atlas_bytes(path: &Path, bytes: &[u8]) -> Vec<AnimationValidationEvent> {
        let key_hint = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("texture_atlas");
        let source_label = path.display().to_string();
        match parse_texture_atlas_bytes(bytes, key_hint, &source_label) {
            Ok(TextureAtlasParseResult { atlas, diagnostics }) => {
                let mut events = Vec::new();
                let region_count = atlas.regions.len();
                let timeline_count = atlas.animations.len();
                let image_label = atlas.image_path.display().to_string();
                events.push(Self::event(
                    path,
                    AnimationValidationSeverity::Info,
                    format!(
                        "Parsed atlas '{key_hint}' with {region_count} region{} and {timeline_count} timeline{} (image: {image_label}).",
                        if region_count == 1 { "" } else { "s" },
                        if timeline_count == 1 { "" } else { "s" }
                    ),
                ));
                for warning in diagnostics.warnings {
                    events.push(Self::event(path, AnimationValidationSeverity::Warning, warning));
                }
                events
            }
            Err(err) => vec![Self::event(
                path,
                AnimationValidationSeverity::Error,
                format!("Failed to parse texture atlas: {err}"),
            )],
        }
    }

    fn validate_skeletal_asset(path: &Path) -> Vec<AnimationValidationEvent> {
        match skeletal::load_skeleton_from_gltf(path) {
            Ok(import) => {
                let mut events = Vec::new();
                let joint_count = import.skeleton.joints.len();
                if joint_count == 0 {
                    events.push(Self::event(
                        path,
                        AnimationValidationSeverity::Error,
                        "Skeleton contains zero joints.",
                    ));
                }
                if joint_count > 256 {
                    events.push(Self::event(
                        path,
                        AnimationValidationSeverity::Warning,
                        format!(
                            "Skeleton has {} joints which exceeds the 256-joint palette limit; expect GPU splits.",
                            joint_count
                        ),
                    ));
                }
                if import.clips.is_empty() {
                    events.push(Self::event(
                        path,
                        AnimationValidationSeverity::Warning,
                        "Skeleton import did not produce any clips.",
                    ));
                } else {
                    for clip in &import.clips {
                        if clip.duration <= 0.0 {
                            events.push(Self::event(
                                path,
                                AnimationValidationSeverity::Warning,
                                format!("Clip '{}' has zero duration.", clip.name),
                            ));
                        }
                    }
                }
                if !has_error(&events) {
                    let clip_count = import.clips.len();
                    events.push(Self::event(
                        path,
                        AnimationValidationSeverity::Info,
                        format!(
                            "Skeleton '{}' OK: {} joints, {} clip(s).",
                            import.skeleton.name, joint_count, clip_count
                        ),
                    ));
                }
                events
            }
            Err(err) => vec![Self::event(
                path,
                AnimationValidationSeverity::Error,
                format!("Failed to import skeleton: {err}"),
            )],
        }
    }

    fn clip_success_events(path: &Path, clip: &AnimationClip) -> Vec<AnimationValidationEvent> {
        let mut events = Vec::new();
        if clip.translation.is_none()
            && clip.rotation.is_none()
            && clip.scale.is_none()
            && clip.tint.is_none()
        {
            events.push(Self::event(
                path,
                AnimationValidationSeverity::Warning,
                format!("Clip '{}' does not define any tracks.", clip.name),
            ));
        }
        if clip.duration <= 0.0 {
            events.push(Self::event(
                path,
                AnimationValidationSeverity::Warning,
                "Clip duration is zero; ensure at least one keyframe has time > 0.",
            ));
        }
        let summary = Self::track_summary(clip);
        events.push(Self::event(
            path,
            AnimationValidationSeverity::Info,
            format!("Clip '{}' OK: duration {:.3}s, tracks {summary}", clip.name, clip.duration),
        ));
        events
    }

    fn graph_success_events(path: &Path, graph: &AnimationGraphAsset) -> Vec<AnimationValidationEvent> {
        let mut events = Vec::new();
        let mut state_names = HashSet::new();
        let mut duplicate_states = Vec::new();
        for state in graph.states.iter() {
            let name = state.name.as_ref();
            if name.trim().is_empty() {
                events.push(Self::event(
                    path,
                    AnimationValidationSeverity::Error,
                    "Animation graph state names cannot be empty.",
                ));
            }
            if !state_names.insert(name.to_string()) {
                duplicate_states.push(name.to_string());
            }
        }
        if !duplicate_states.is_empty() {
            events.push(Self::event(
                path,
                AnimationValidationSeverity::Error,
                format!("Animation graph has duplicate states: {}", duplicate_states.join(", ")),
            ));
        }
        let entry_state = graph.entry_state.as_ref();
        if !state_names.contains(entry_state) {
            events.push(Self::event(
                path,
                AnimationValidationSeverity::Error,
                format!("Entry state '{entry_state}' is not defined in the graph."),
            ));
        }
        let mut parameter_names = HashSet::new();
        for parameter in graph.parameters.iter() {
            let name = parameter.name.as_ref();
            if !parameter_names.insert(name.to_string()) {
                events.push(Self::event(
                    path,
                    AnimationValidationSeverity::Error,
                    format!("Duplicate parameter '{name}' detected."),
                ));
            }
        }
        for transition in graph.transitions.iter() {
            let from = transition.from.as_ref();
            let to = transition.to.as_ref();
            if !state_names.contains(from) {
                events.push(Self::event(
                    path,
                    AnimationValidationSeverity::Error,
                    format!("Transition references unknown 'from' state '{from}'."),
                ));
            }
            if !state_names.contains(to) {
                events.push(Self::event(
                    path,
                    AnimationValidationSeverity::Error,
                    format!("Transition references unknown 'to' state '{to}'."),
                ));
            }
            if from == to {
                events.push(Self::event(
                    path,
                    AnimationValidationSeverity::Warning,
                    format!("Transition from '{from}' to itself detected; confirm this is intentional."),
                ));
            }
        }
        for state in graph.states.iter() {
            let clip_empty = state.clip.as_deref().map(|clip| clip.trim().is_empty()).unwrap_or(true);
            if clip_empty {
                events.push(Self::event(
                    path,
                    AnimationValidationSeverity::Warning,
                    format!("State '{}' does not reference a clip.", state.name),
                ));
            }
        }
        if graph.transitions.is_empty() && graph.states.len() > 1 {
            events.push(Self::event(
                path,
                AnimationValidationSeverity::Warning,
                "Graph has multiple states but no transitions; states other than the entry will never be reached.",
            ));
        }
        if !has_error(&events) {
            events.push(Self::event(
                path,
                AnimationValidationSeverity::Info,
                format!(
                    "Graph '{}' OK: {} states, {} transitions, entry '{}'.",
                    graph.name.as_ref(),
                    graph.states.len(),
                    graph.transitions.len(),
                    entry_state
                ),
            ));
        }
        events
    }

    fn track_summary(clip: &AnimationClip) -> String {
        let mut segments = Vec::new();
        if let Some(track) = clip.translation.as_ref() {
            segments.push(format!("translation ({} keys)", track.keyframes.len()));
        }
        if let Some(track) = clip.rotation.as_ref() {
            segments.push(format!("rotation ({} keys)", track.keyframes.len()));
        }
        if let Some(track) = clip.scale.as_ref() {
            segments.push(format!("scale ({} keys)", track.keyframes.len()));
        }
        if let Some(track) = clip.tint.as_ref() {
            segments.push(format!("tint ({} keys)", track.keyframes.len()));
        }
        if segments.is_empty() {
            "no tracks authored".to_string()
        } else {
            segments.join(", ")
        }
    }

    fn classify_json_asset(path: &Path, bytes: &[u8]) -> JsonAssetKind {
        if path_contains_segment(path, "images") || path_contains_segment(path, "atlases") {
            if looks_like_atlas_json(bytes) {
                return JsonAssetKind::Atlas;
            }
        }
        if path_contains_segment(path, "graphs") {
            return JsonAssetKind::Graph;
        }
        if path_contains_segment(path, "clips") {
            return JsonAssetKind::Clip;
        }
        if looks_like_atlas_json(bytes) {
            return JsonAssetKind::Atlas;
        }
        if looks_like_clip_json(bytes) {
            return JsonAssetKind::Clip;
        }
        if looks_like_graph_json(bytes) {
            return JsonAssetKind::Graph;
        }
        JsonAssetKind::Unknown
    }

    fn event(
        path: &Path,
        severity: AnimationValidationSeverity,
        message: impl Into<String>,
    ) -> AnimationValidationEvent {
        AnimationValidationEvent { severity, path: path.to_path_buf(), message: message.into() }
    }
}

fn has_error(events: &[AnimationValidationEvent]) -> bool {
    events.iter().any(|event| matches!(event.severity, AnimationValidationSeverity::Error))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonAssetKind {
    Atlas,
    Clip,
    Graph,
    Unknown,
}

fn path_contains_segment(path: &Path, needle: &str) -> bool {
    let needle = needle.to_ascii_lowercase();
    path.iter().any(|component| component.to_string_lossy().eq_ignore_ascii_case(&needle))
}

fn looks_like_clip_json(bytes: &[u8]) -> bool {
    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(bytes) {
        return map.get("tracks").map(|tracks| tracks.is_object()).unwrap_or(false);
    }
    false
}

fn looks_like_graph_json(bytes: &[u8]) -> bool {
    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(bytes) {
        let states_ok = map.get("states").map(|states| states.is_array()).unwrap_or(false);
        let transitions_ok =
            map.get("transitions").map(|transitions| transitions.is_array()).unwrap_or(false);
        return states_ok && transitions_ok;
    }
    false
}

fn looks_like_atlas_json(bytes: &[u8]) -> bool {
    if let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(bytes) {
        let image_ok = map.get("image").map(|value| value.is_string()).unwrap_or(false);
        let regions_ok = map.get("regions").map(|regions| regions.is_object()).unwrap_or(false);
        let width_ok = map.get("width").map(|value| value.is_number()).unwrap_or(false);
        let height_ok = map.get("height").map(|value| value.is_number()).unwrap_or(false);
        return image_ok && regions_ok && width_ok && height_ok;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::Builder;

    #[test]
    fn severity_display_formats() {
        assert_eq!(AnimationValidationSeverity::Info.to_string(), "info");
        assert_eq!(AnimationValidationSeverity::Warning.to_string(), "warning");
        assert_eq!(AnimationValidationSeverity::Error.to_string(), "error");
    }

    #[test]
    fn validator_reports_missing_file() {
        let events = AnimationValidator::validate_path(Path::new("foo/bar.clip"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].severity, AnimationValidationSeverity::Warning);
        assert!(events[0].message.contains("not found"));
    }

    #[test]
    fn validator_succeeds_on_fixture_clip() {
        let path = Path::new("fixtures/animation_clips/slime_bob.json");
        let events = AnimationValidator::validate_path(path);
        assert!(!events.is_empty());
        assert!(events.iter().any(|event| event.severity == AnimationValidationSeverity::Info));
    }

    #[test]
    fn validator_accepts_graph_file() {
        let mut file = Builder::new().suffix(".json").tempfile().unwrap();
        writeln!(
            file,
            r#"{{
                "version": 1,
                "name": "slime_graph",
                "entry_state": "Idle",
                "states": [
                    {{"name": "Idle", "clip": "idle_clip"}},
                    {{"name": "Attack", "clip": "attack_clip"}}
                ],
                "transitions": [
                    {{"from": "Idle", "to": "Attack"}},
                    {{"from": "Attack", "to": "Idle"}}
                ]
            }}"#
        )
        .unwrap();
        let events = AnimationValidator::validate_path(file.path());
        assert!(events.iter().any(|event| event.severity == AnimationValidationSeverity::Info));
    }

    #[test]
    fn validator_reports_graph_error_for_unknown_state() {
        let mut file = Builder::new().suffix(".json").tempfile().unwrap();
        writeln!(
            file,
            r#"{{
                "version": 1,
                "states": [{{"name": "Idle", "clip": "idle_clip"}}],
                "transitions": [{{"from": "Idle", "to": "Missing"}}]
            }}"#
        )
        .unwrap();
        let events = AnimationValidator::validate_path(file.path());
        assert!(events.iter().any(|event| event.severity == AnimationValidationSeverity::Error));
    }

    #[test]
    fn validator_accepts_skeletal_asset() {
        let path = Path::new("fixtures/gltf/skeletons/slime_rig.gltf");
        assert!(path.exists(), "Missing skeletal fixture at {}", path.display());
        let events = AnimationValidator::validate_path(path);
        assert!(events.iter().any(|event| event.severity == AnimationValidationSeverity::Info));
    }

    #[test]
    fn validator_accepts_atlas_asset() {
        let path = Path::new("assets/images/atlas.json");
        assert!(path.exists(), "Missing atlas fixture at {}", path.display());
        let events = AnimationValidator::validate_path(path);
        assert!(
            events.iter().any(|event| event.severity == AnimationValidationSeverity::Info),
            "expected info event for atlas"
        );
        assert!(
            !events.iter().any(|event| event.severity == AnimationValidationSeverity::Error),
            "atlas validation should not emit errors"
        );
    }
}
