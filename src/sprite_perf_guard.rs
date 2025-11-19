use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct BenchThresholds {
    pub mean_ms: f64,
    pub max_ms: f64,
    pub slow_ratio: f64,
}

impl BenchThresholds {
    pub const fn new(mean_ms: f64, max_ms: f64, slow_ratio: f64) -> Self {
        Self { mean_ms, max_ms, slow_ratio }
    }
}

#[derive(Debug, Deserialize)]
struct SpritePerfEnvelope {
    #[serde(default)]
    slow_ratio_p95: Option<f64>,
    #[serde(default)]
    slow_ratio_mean: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CaseSummary {
    #[serde(default)]
    mean_step_ms: Option<f64>,
    #[serde(default)]
    max_step_ms: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct CaseEntry {
    label: String,
    summary: CaseSummary,
    #[serde(default)]
    sprite_perf: Option<SpritePerfEnvelope>,
}

#[derive(Debug, Deserialize)]
struct ReportCases {
    #[serde(default)]
    cases: Vec<CaseEntry>,
}

#[derive(Debug, Deserialize)]
struct SystemEntry {
    label: String,
    #[serde(default)]
    mean_ms: f64,
    #[serde(default)]
    runs: Vec<f64>,
}

#[derive(Debug, Deserialize)]
struct ReportSystems {
    #[serde(default)]
    systems: Vec<SystemEntry>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct CaseMetrics {
    pub mean_ms: f64,
    pub max_ms: f64,
    pub slow_ratio: Option<f64>,
}

pub fn check_report<P: AsRef<Path>>(
    report_path: P,
    case_label: &str,
    thresholds: BenchThresholds,
    require_sprite_perf: bool,
) -> Result<CaseMetrics> {
    let path = report_path.as_ref();
    let contents = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let metrics = parse_cases(&contents, case_label)
        .or_else(|| parse_systems(&contents, case_label))
        .ok_or_else(|| {
            anyhow!(
                "case '{}' not found in {}. Expected either 'cases' or 'systems' payload.",
                case_label,
                path.display()
            )
        })?;
    if metrics.mean_ms > thresholds.mean_ms {
        return Err(anyhow!(
            "case '{}' mean {:.3} ms exceeded threshold {:.3} ms",
            case_label,
            metrics.mean_ms,
            thresholds.mean_ms
        ));
    }
    if metrics.max_ms > thresholds.max_ms {
        return Err(anyhow!(
            "case '{}' max {:.3} ms exceeded threshold {:.3} ms",
            case_label,
            metrics.max_ms,
            thresholds.max_ms
        ));
    }
    if require_sprite_perf {
        let slow_ratio =
            metrics.slow_ratio.ok_or_else(|| anyhow!("case '{}' missing sprite_perf metrics", case_label))?;
        if slow_ratio > thresholds.slow_ratio {
            return Err(anyhow!(
                "case '{}' slow ratio {:.3}% exceeded threshold {:.3}%",
                case_label,
                slow_ratio * 100.0,
                thresholds.slow_ratio * 100.0
            ));
        }
    }
    Ok(metrics)
}

fn parse_cases(contents: &str, case_label: &str) -> Option<CaseMetrics> {
    let payload: ReportCases = serde_json::from_str(contents).ok()?;
    for case in payload.cases {
        if case.label == case_label {
            let mean = case.summary.mean_step_ms?;
            let max = case.summary.max_step_ms?;
            let slow = case.sprite_perf.and_then(|perf| perf.slow_ratio_p95.or(perf.slow_ratio_mean));
            return Some(CaseMetrics { mean_ms: mean, max_ms: max, slow_ratio: slow });
        }
    }
    None
}

fn parse_systems(contents: &str, case_label: &str) -> Option<CaseMetrics> {
    let payload: ReportSystems = serde_json::from_str(contents).ok()?;
    for system in payload.systems {
        if system.label == case_label {
            let mut max = system.runs.iter().copied().fold(f64::MIN, f64::max);
            if !max.is_finite() {
                max = system.mean_ms;
            }
            return Some(CaseMetrics { mean_ms: system.mean_ms, max_ms: max, slow_ratio: None });
        }
    }
    None
}
