use anyhow::{Context, Result};
use kestrel_engine::sprite_perf_guard::{check_report, BenchThresholds};
use std::env;
use std::path::PathBuf;

struct Args {
    report: PathBuf,
    case: String,
    thresholds: BenchThresholds,
    require_sprite_perf: bool,
}

fn usage() {
    eprintln!(
        "\
Usage: sprite_perf_guard [--report <path>] [--case <label>] \\
       [--mean-threshold <ms>] [--max-threshold <ms>] [--slow-threshold <ratio>] \\
       [--allow-missing-sprite-perf]

Defaults:
  --report target/animation_targets_report.json
  --case sprite_timelines
  --mean-threshold 0.300
  --max-threshold 0.300
  --slow-threshold 0.01 (1%)
"
    );
}

fn parse_args() -> Result<Args> {
    let mut report = PathBuf::from("target/animation_targets_report.json");
    let mut case = "sprite_timelines".to_string();
    let mut mean_threshold = 0.300;
    let mut max_threshold = 0.300;
    let mut slow_threshold = 0.01;
    let mut require_sprite_perf = true;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--report" => {
                let value = args.next().context("--report requires a path")?;
                report = PathBuf::from(value);
            }
            "--case" => {
                case = args.next().context("--case requires a label")?;
            }
            "--mean-threshold" => {
                let value = args.next().context("--mean-threshold requires a value")?;
                mean_threshold = value.parse().context("invalid --mean-threshold")?;
            }
            "--max-threshold" => {
                let value = args.next().context("--max-threshold requires a value")?;
                max_threshold = value.parse().context("invalid --max-threshold")?;
            }
            "--slow-threshold" => {
                let value = args.next().context("--slow-threshold requires a value")?;
                slow_threshold = value.parse().context("invalid --slow-threshold")?;
            }
            "--allow-missing-sprite-perf" => {
                require_sprite_perf = false;
            }
            "--help" | "-h" => {
                usage();
                std::process::exit(0);
            }
            other => {
                return Err(anyhow::anyhow!("unknown argument '{other}'"));
            }
        }
    }
    Ok(Args {
        report,
        case,
        thresholds: BenchThresholds::new(mean_threshold, max_threshold, slow_threshold),
        require_sprite_perf,
    })
}

fn main() {
    if let Err(err) = run_guard() {
        eprintln!("[sprite_perf_guard] {err:?}");
        std::process::exit(1);
    }
}

fn run_guard() -> Result<()> {
    let args = parse_args()?;
    let metrics = check_report(&args.report, &args.case, args.thresholds, args.require_sprite_perf)?;
    println!(
        "[sprite_perf_guard] case '{}' OK: mean {:.3} ms, max {:.3} ms{}",
        args.case,
        metrics.mean_ms,
        metrics.max_ms,
        metrics.slow_ratio.map(|ratio| format!(", slow_ratio {:.2}%", ratio * 100.0)).unwrap_or_default()
    );
    Ok(())
}
