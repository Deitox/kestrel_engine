use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
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
    ///
    /// Milestone 5 will implement real validators; for now this returns a placeholder
    /// info event so watcher plumbing can be verified without panicking.
    pub fn validate_path(path: &Path) -> Vec<AnimationValidationEvent> {
        vec![AnimationValidationEvent {
            severity: AnimationValidationSeverity::Info,
            path: path.to_path_buf(),
            message: "Animation validation stub pending implementation".to_string(),
        }]
    }
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
    fn validator_returns_placeholder_event() {
        let events = AnimationValidator::validate_path(Path::new("foo/bar.clip"));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].severity, AnimationValidationSeverity::Info);
    }
}
