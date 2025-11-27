use std::env;
use std::fs::{self, File};
use std::path::PathBuf;

use anyhow::{anyhow, bail, Context, Result};
use kestrel_engine::script_harness::{load_fixture, HarnessOutput};
use kestrel_engine::script_harness::run_fixture;

fn main() {
    if let Err(err) = run_cli() {
        eprintln!("[script-harness] error: {err:?}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let opts = parse_args()?;
    let fixture = load_fixture(&opts.fixture)?;
    let output = run_fixture(&fixture)?;

    if let Some(path) = &opts.write_output {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating output directory '{}'", parent.display()))?;
            }
        }
        let file = File::create(path)
            .with_context(|| format!("writing harness output to '{}'", path.display()))?;
        serde_json::to_writer_pretty(file, &output).with_context(|| "serializing harness output")?;
        println!("[script-harness] wrote {}", path.display());
    }

    if let Some(path) = &opts.check_golden {
        let file = File::open(path)
            .with_context(|| format!("opening golden file '{}'", path.display()))?;
        let expected: HarnessOutput =
            serde_json::from_reader(file).with_context(|| "parsing golden JSON")?;
        if expected != output {
            bail!(
                "golden mismatch for {} (use --write-output to refresh):\nexpected: {}\nactual:   {}",
                opts.fixture.display(),
                serde_json::to_string(&expected).unwrap_or_default(),
                serde_json::to_string(&output).unwrap_or_default(),
            );
        }
        println!("[script-harness] matched golden {}", path.display());
    } else if opts.write_output.is_none() {
        serde_json::to_writer_pretty(std::io::stdout(), &output)?;
        println!();
    }

    Ok(())
}

struct CliOptions {
    fixture: PathBuf,
    write_output: Option<PathBuf>,
    check_golden: Option<PathBuf>,
}

fn parse_args() -> Result<CliOptions> {
    let mut fixture = None;
    let mut write_output = None;
    let mut check_golden = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fixture" | "-f" => fixture = args.next().map(PathBuf::from),
            "--write-output" | "-o" => write_output = args.next().map(PathBuf::from),
            "--golden" | "-g" => check_golden = args.next().map(PathBuf::from),
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                return Err(anyhow!("unknown argument '{other}'"));
            }
        }
    }
    let Some(fixture) = fixture else { return Err(anyhow!("--fixture <path> is required")); };
    Ok(CliOptions { fixture, write_output, check_golden })
}

fn print_help() {
    println!("Usage: script_harness --fixture <path> [--golden <path>] [--write-output <path>]");
    println!("  -f, --fixture        Path to a harness fixture JSON file");
    println!("  -g, --golden         Optional golden output file to compare against");
    println!("  -o, --write-output   Optional path to write the actual output JSON");
}
