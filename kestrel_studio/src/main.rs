use anyhow::{anyhow, Result};
use kestrel_engine::cli::CliOverrides;
use kestrel_studio::project::Project;
use kestrel_studio::run_with_project;
use std::env;
use std::path::PathBuf;

fn main() {
    let (project_path, cli_overrides) = match parse_args() {
        Ok(result) => result,
        Err(err) => {
            eprintln!("[cli] {err}");
            std::process::exit(2);
        }
    };
    let project = load_project(project_path);
    Project::record_recent(project.root());
    if let Err(err) = pollster::block_on(run_with_project(project, cli_overrides)) {
        eprintln!("Application error: {err:?}");
    }
}

fn parse_args() -> Result<(Option<PathBuf>, kestrel_engine::config::AppConfigOverrides)> {
    let mut project_path: Option<PathBuf> = None;
    let mut passthrough: Vec<String> = Vec::new();
    let mut args = env::args();
    if let Some(first) = args.next() {
        passthrough.push(first);
    }
    while let Some(flag) = args.next() {
        if flag == "--project" {
            let value = args.next().ok_or_else(|| anyhow!("Expected a value after --project"))?;
            project_path = Some(PathBuf::from(value));
            continue;
        }
        passthrough.push(flag.clone());
        if flag.starts_with("--") {
            if let Some(value) = args.next() {
                passthrough.push(value);
            } else {
                return Err(anyhow!("Missing value for flag '{flag}'"));
            }
        }
    }
    let cli_overrides = CliOverrides::parse(&passthrough)?.into_config_overrides();
    Ok((project_path, cli_overrides))
}

fn load_project(project_path: Option<PathBuf>) -> Project {
    if let Some(path) = project_path {
        match Project::load(&path) {
            Ok(project) => {
                println!("[project] Loaded {} ({})", project.describe(), path.display());
                return project;
            }
            Err(err) => {
                eprintln!("[project] Failed to load {}: {err}", path.display());
                std::process::exit(2);
            }
        }
    }
    if let Ok(env_path) = env::var("KESTREL_PROJECT") {
        let path = PathBuf::from(env_path);
        match Project::load(&path) {
            Ok(project) => {
                println!("[project] Loaded {} ({}) from KESTREL_PROJECT", project.describe(), path.display());
                return project;
            }
            Err(err) => eprintln!("[project] Failed to load KESTREL_PROJECT {}: {err}", path.display()),
        }
    }
    if let Some(path) = Project::load_recent() {
        match Project::load(&path) {
            Ok(project) => {
                println!("[project] Loaded recent project {} ({})", project.describe(), path.display());
                return project;
            }
            Err(err) => eprintln!("[project] Failed to load recent project {}: {err}", path.display()),
        }
    }
    match Project::default() {
        Ok(project) => {
            println!(
                "[project] No manifest supplied; using default layout at {}",
                project.assets_root().display()
            );
            project
        }
        Err(err) => {
            eprintln!("[project] Failed to initialize default project: {err}");
            std::process::exit(2);
        }
    }
}
