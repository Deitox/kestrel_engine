use anyhow::{anyhow, Context, Result};
use kestrel_engine::animation_validation::{
    AnimationValidationEvent, AnimationValidationSeverity, AnimationValidator,
};
use serde::Serialize;
use serde_json::json;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

fn main() {
    match run() {
        Ok(result) => {
            if result.summary.errors > 0 || (result.fail_on_warn && result.summary.warnings > 0) {
                process::exit(2);
            }
        }
        Err(err) => {
            eprintln!("animation_check error: {err:?}");
            process::exit(1);
        }
    }
}

#[derive(Default, Serialize)]
struct RunSummary {
    checked: usize,
    warnings: usize,
    errors: usize,
}

struct RunResult {
    summary: RunSummary,
    fail_on_warn: bool,
}

struct CliOptions {
    fail_on_warn: bool,
    report_stats: bool,
    show_help: bool,
    targets: Vec<String>,
}

fn run() -> Result<RunResult> {
    let args: Vec<String> = env::args().skip(1).collect();
    let options = parse_cli_args(&args)?;
    if options.show_help {
        print_usage();
        return Ok(RunResult { summary: RunSummary::default(), fail_on_warn: options.fail_on_warn });
    }
    if options.targets.is_empty() {
        return Err(anyhow!("no animation assets found in provided paths"));
    }
    let targets = collect_targets(&options.targets)?;
    if targets.is_empty() {
        return Err(anyhow!("no animation assets found in provided paths"));
    }
    let mut summary = RunSummary::default();
    for path in targets {
        summary.checked += 1;
        let events = AnimationValidator::validate_path(&path);
        if events.is_empty() {
            println!("OK {}", path.display());
            continue;
        }
        for event in events {
            report_event(&event);
            if options.report_stats {
                report_event_json(&event);
            }
            match event.severity {
                AnimationValidationSeverity::Warning => summary.warnings += 1,
                AnimationValidationSeverity::Error => summary.errors += 1,
                AnimationValidationSeverity::Info => {}
            }
        }
    }
    println!("Checked {} assets ({} warnings, {} errors)", summary.checked, summary.warnings, summary.errors);
    if options.report_stats {
        report_summary_json(&summary);
    }
    Ok(RunResult { summary, fail_on_warn: options.fail_on_warn })
}

fn print_usage() {
    eprintln!(
        "Animation Check

Usage:
  animation_check [--fail-on-warn] <path> [<path>...]

Each <path> may be a file or directory. Directories are walked recursively
and JSON/GLTF files are validated. Use --fail-on-warn to treat warnings
as errors (exit code 2).
"
    );
}

fn parse_cli_args(args: &[String]) -> Result<CliOptions> {
    let mut options =
        CliOptions { fail_on_warn: false, report_stats: false, show_help: false, targets: Vec::new() };
    for arg in args {
        match arg.as_str() {
            "--fail-on-warn" => options.fail_on_warn = true,
            "--report-stats" => options.report_stats = true,
            "--help" | "-h" => options.show_help = true,
            _ if arg.starts_with("--") => {
                return Err(anyhow!("unknown flag '{arg}'"));
            }
            _ => options.targets.push(arg.clone()),
        }
    }
    Ok(options)
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
            if should_validate(&path) {
                add_target(path, &mut seen, &mut files)?;
            } else {
                eprintln!("[animation_check] skipping '{}' (unsupported extension)", path.display());
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
        } else if should_validate(&path) {
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

fn should_validate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_lowercase()),
        Some(ext) if matches!(ext.as_str(), "json" | "gltf" | "glb" | "clip")
    )
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

fn report_event(event: &AnimationValidationEvent) {
    let severity = severity_label(event.severity, true);
    println!("[{severity}] {} - {}", event.path.display(), event.message);
}

fn report_event_json(event: &AnimationValidationEvent) {
    let severity = severity_label(event.severity, false);
    let json_value = json!({
        "severity": severity,
        "path": event.path.display().to_string(),
        "message": event.message,
    });
    println!("{json_value}");
}

fn report_summary_json(summary: &RunSummary) {
    let json_value = json!({
        "summary": {
            "checked": summary.checked,
            "warnings": summary.warnings,
            "errors": summary.errors,
        }
    });
    println!("{json_value}");
}

fn severity_label(severity: AnimationValidationSeverity, uppercase: bool) -> &'static str {
    match (severity, uppercase) {
        (AnimationValidationSeverity::Info, true) => "INFO",
        (AnimationValidationSeverity::Warning, true) => "WARN",
        (AnimationValidationSeverity::Error, true) => "ERROR",
        (AnimationValidationSeverity::Info, false) => "info",
        (AnimationValidationSeverity::Warning, false) => "warning",
        (AnimationValidationSeverity::Error, false) => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_handles_fail_on_warn() {
        let args = vec!["--fail-on-warn".to_string(), "foo".to_string()];
        let opts = parse_cli_args(&args).expect("parse args");
        assert!(opts.fail_on_warn);
        assert!(!opts.report_stats);
        assert_eq!(opts.targets, vec!["foo".to_string()]);
        assert!(!opts.show_help);
    }

    #[test]
    fn parse_args_handles_report_stats() {
        let args = vec!["--report-stats".to_string(), "clip.clip".to_string()];
        let opts = parse_cli_args(&args).expect("parse args");
        assert!(opts.report_stats);
        assert!(!opts.fail_on_warn);
        assert_eq!(opts.targets, vec!["clip.clip".to_string()]);
    }

    #[test]
    fn parse_args_errors_on_unknown_flag() {
        let args = vec!["--unknown".to_string()];
        assert!(parse_cli_args(&args).is_err());
    }
}
