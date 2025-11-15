use anyhow::{anyhow, Context, Result};
use kestrel_engine::scene_capture::capture_scene_from_path;
use serde_json::to_writer_pretty;
use std::fs::File;
use std::io;
use std::path::PathBuf;

fn print_help() {
    eprintln!(
        "Usage: scene_capture --scene <path> [--out <path>]\n\n\
         Options:\n  --scene <path>   Scene JSON/kscene to summarize (required)\n  \
         --out <path>     Destination for the capture JSON (defaults to stdout)\n  \
         --compact        Emit minified JSON instead of pretty output\n  \
         -h, --help       Show this message"
    );
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut scene_path: Option<PathBuf> = None;
    let mut out_path: Option<PathBuf> = None;
    let mut pretty = true;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--scene" => {
                let value = args.next().context("--scene requires a path")?;
                scene_path = Some(PathBuf::from(value));
            }
            "--out" => {
                let value = args.next().context("--out requires a path")?;
                out_path = Some(PathBuf::from(value));
            }
            "--compact" => {
                pretty = false;
            }
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            other => {
                return Err(anyhow!("Unknown argument '{other}'. Use --help for usage."));
            }
        }
    }

    let scene_path = scene_path.ok_or_else(|| anyhow!("--scene is required"))?;
    let capture = capture_scene_from_path(&scene_path)
        .with_context(|| format!("Capturing scene {}", scene_path.display()))?;

    match out_path {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Creating directory {}", parent.display()))?;
            }
            let file = File::create(&path).with_context(|| format!("Creating {}", path.display()))?;
            if pretty {
                to_writer_pretty(file, &capture)?;
            } else {
                serde_json::to_writer(file, &capture)?;
            }
            println!("Wrote capture to {}", path.display());
        }
        None => {
            let stdout = io::stdout();
            let handle = stdout.lock();
            if pretty {
                to_writer_pretty(handle, &capture)?;
            } else {
                serde_json::to_writer(handle, &capture)?;
            }
        }
    }

    Ok(())
}
