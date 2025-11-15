use anyhow::{anyhow, Context, Result};
use kestrel_engine::animation_validation::{
    AnimationValidationEvent, AnimationValidationSeverity, AnimationValidator,
};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;

fn main() {
    match run() {
        Ok(summary) => {
            if summary.errors > 0 {
                process::exit(2);
            }
        }
        Err(err) => {
            eprintln!("animation_check error: {err:?}");
            process::exit(1);
        }
    }
}

#[derive(Default)]
struct RunSummary {
    checked: usize,
    warnings: usize,
    errors: usize,
}

fn run() -> Result<RunSummary> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return Ok(RunSummary::default());
    }
    let targets = collect_targets(&args)?;
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
            match event.severity {
                AnimationValidationSeverity::Warning => summary.warnings += 1,
                AnimationValidationSeverity::Error => summary.errors += 1,
                AnimationValidationSeverity::Info => {}
            }
        }
    }
    println!("Checked {} assets ({} warnings, {} errors)", summary.checked, summary.warnings, summary.errors);
    Ok(summary)
}

fn print_usage() {
    eprintln!(
        "Animation Check

Usage:
  animation_check <path> [<path>...]

Each <path> may be a file or directory. Directories are walked recursively
and JSON/GLTF files are validated.
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
    match path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_lowercase()) {
        Some(ext) if matches!(ext.as_str(), "json" | "gltf" | "glb" | "clip") => true,
        _ => false,
    }
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
    let severity = match event.severity {
        AnimationValidationSeverity::Info => "INFO",
        AnimationValidationSeverity::Warning => "WARN",
        AnimationValidationSeverity::Error => "ERROR",
    };
    println!("[{severity}] {} - {}", event.path.display(), event.message);
}
