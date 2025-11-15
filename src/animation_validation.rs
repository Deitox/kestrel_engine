use crate::assets::{parse_animation_clip_bytes, AnimationClip};
use serde_json::Value;
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
            Some("gltf") | Some("glb") => {
                return vec![Self::event(
                    path,
                    AnimationValidationSeverity::Info,
                    "Skeletal asset validation pending implementation.",
                )];
            }
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
                JsonAssetKind::Clip => Self::validate_clip_bytes(path, &bytes),
                JsonAssetKind::Graph => vec![Self::event(
                    path,
                    AnimationValidationSeverity::Info,
                    "Animation graph validation pending implementation.",
                )],
                JsonAssetKind::Unknown => vec![Self::event(
                    path,
                    AnimationValidationSeverity::Info,
                    "No validators available for this JSON file.",
                )],
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
        let key_hint = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("animation_clip");
        let source_label = path.display().to_string();
        match parse_animation_clip_bytes(bytes, key_hint, &source_label) {
            Ok(clip) => Self::clip_success_events(path, &clip),
            Err(err) => vec![Self::event(
                path,
                AnimationValidationSeverity::Error,
                format!("{err}"),
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
            format!(
                "Clip '{}' OK: duration {:.3}s, tracks {summary}",
                clip.name, clip.duration
            ),
        ));
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
        if path_contains_segment(path, "graphs") {
            return JsonAssetKind::Graph;
        }
        if path_contains_segment(path, "clips") {
            return JsonAssetKind::Clip;
        }
        if looks_like_clip_json(bytes) {
            JsonAssetKind::Clip
        } else {
            JsonAssetKind::Unknown
        }
    }

    fn event(path: &Path, severity: AnimationValidationSeverity, message: impl Into<String>) -> AnimationValidationEvent {
        AnimationValidationEvent { severity, path: path.to_path_buf(), message: message.into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JsonAssetKind {
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

#[cfg(test)]
mod tests {
    use super::*;
 
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
}
