use crate::renderer::GpuPassTiming;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Default)]
struct PassSamples {
    samples: Vec<f32>,
    max_ms: f32,
}

impl PassSamples {
    fn record(&mut self, value: f32) {
        self.samples.push(value);
        if value > self.max_ms {
            self.max_ms = value;
        }
    }

    fn latest(&self) -> Option<f32> {
        self.samples.last().copied()
    }

    fn mean(&self) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        let sum: f32 = self.samples.iter().copied().sum();
        Some(sum / self.samples.len() as f32)
    }
}

/// Collects GPU pass timings across multiple frames and produces a serialized snapshot.
#[derive(Debug, Default)]
pub struct GpuTimingAccumulator {
    frame_count: usize,
    passes: BTreeMap<&'static str, PassSamples>,
}

impl GpuTimingAccumulator {
    /// Records the pass durations emitted for a single frame.
    pub fn record_frame(&mut self, timings: &[GpuPassTiming]) {
        if timings.is_empty() {
            return;
        }
        self.frame_count += 1;
        for timing in timings {
            self.passes.entry(timing.label).or_default().record(timing.duration_ms);
        }
    }

    /// Creates a serializable snapshot using the provided metadata.
    pub fn snapshot(
        &self,
        label: impl Into<String>,
        timestamp: impl Into<String>,
        commit: impl Into<String>,
    ) -> GpuBaselineSnapshot {
        let mut passes: Vec<GpuPassSnapshot> = self
            .passes
            .iter()
            .filter_map(|(label, samples)| {
                let latest = samples.latest()?;
                let mean = samples.mean()?;
                Some(GpuPassSnapshot {
                    label: label.to_string(),
                    latest_ms: latest,
                    average_ms: mean,
                    max_ms: samples.max_ms,
                    sample_count: samples.samples.len(),
                })
            })
            .collect();
        passes.sort_by(|a, b| a.label.cmp(&b.label));
        GpuBaselineSnapshot {
            label: label.into(),
            timestamp: timestamp.into(),
            commit: commit.into(),
            frame_count: self.frame_count,
            passes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuBaselineSnapshot {
    pub label: String,
    pub timestamp: String,
    pub commit: String,
    pub frame_count: usize,
    pub passes: Vec<GpuPassSnapshot>,
}

impl GpuBaselineSnapshot {
    pub fn write_to_path(&self, path: impl AsRef<Path>) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        fs::write(path, format!("{json}\n"))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let bytes = fs::read(path)?;
        let snapshot = serde_json::from_slice(&bytes)?;
        Ok(snapshot)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BaselinePassStatus {
    Matched,
    MissingInCurrent,
    MissingInBaseline,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuPassSnapshot {
    pub label: String,
    pub latest_ms: f32,
    pub average_ms: f32,
    pub max_ms: f32,
    pub sample_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuBaselineDelta {
    pub label: String,
    pub baseline_avg_ms: f32,
    pub current_avg_ms: f32,
    pub delta_ms: f32,
    pub allowed_drift_ms: f32,
    pub within_tolerance: bool,
    pub status: BaselinePassStatus,
}

/// Compares two snapshots and returns per-pass deltas against the supplied tolerances.
pub fn compare_baselines(
    baseline: &GpuBaselineSnapshot,
    current: &GpuBaselineSnapshot,
    tolerances: &HashMap<String, f32>,
    default_tolerance_ms: f32,
) -> Result<Vec<GpuBaselineDelta>> {
    if baseline.passes.is_empty() {
        return Err(anyhow!("Baseline snapshot contains no GPU pass samples"));
    }
    if current.passes.is_empty() {
        return Err(anyhow!("Current snapshot contains no GPU pass samples"));
    }
    let mut baseline_map: HashMap<&str, &GpuPassSnapshot> = HashMap::new();
    for entry in &baseline.passes {
        baseline_map.insert(entry.label.as_str(), entry);
    }
    let mut matched_baseline = HashSet::new();
    let mut deltas = Vec::new();
    for curr in &current.passes {
        if let Some(base) = baseline_map.get(curr.label.as_str()) {
            matched_baseline.insert(curr.label.as_str());
            let allowed = tolerances.get(curr.label.as_str()).copied().unwrap_or(default_tolerance_ms);
            let delta = curr.average_ms - base.average_ms;
            deltas.push(GpuBaselineDelta {
                label: curr.label.clone(),
                baseline_avg_ms: base.average_ms,
                current_avg_ms: curr.average_ms,
                delta_ms: delta,
                allowed_drift_ms: allowed,
                within_tolerance: delta <= allowed,
                status: BaselinePassStatus::Matched,
            });
        } else {
            deltas.push(GpuBaselineDelta {
                label: curr.label.clone(),
                baseline_avg_ms: 0.0,
                current_avg_ms: curr.average_ms,
                delta_ms: curr.average_ms,
                allowed_drift_ms: 0.0,
                within_tolerance: false,
                status: BaselinePassStatus::MissingInBaseline,
            });
        }
    }
    for base in &baseline.passes {
        if matched_baseline.contains(base.label.as_str()) {
            continue;
        }
        deltas.push(GpuBaselineDelta {
            label: base.label.clone(),
            baseline_avg_ms: base.average_ms,
            current_avg_ms: 0.0,
            delta_ms: base.average_ms,
            allowed_drift_ms: 0.0,
            within_tolerance: false,
            status: BaselinePassStatus::MissingInCurrent,
        });
    }
    deltas.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(deltas)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn timing(label: &'static str, duration_ms: f32) -> GpuPassTiming {
        GpuPassTiming { label, duration_ms }
    }

    #[test]
    fn accumulator_computes_means() {
        let mut acc = GpuTimingAccumulator::default();
        acc.record_frame(&[timing("Sprite pass", 1.0), timing("Mesh pass", 0.5)]);
        acc.record_frame(&[timing("Sprite pass", 2.0), timing("Mesh pass", 0.25)]);
        let snapshot = acc.snapshot("label", "ts", "commit");
        assert_eq!(snapshot.frame_count, 2);
        assert_eq!(snapshot.passes.len(), 2);
        let sprite = snapshot.passes.iter().find(|p| p.label == "Sprite pass").unwrap();
        assert!((sprite.average_ms - 1.5).abs() < f32::EPSILON);
        assert_eq!(sprite.sample_count, 2);
        assert_eq!(sprite.max_ms, 2.0);
    }

    #[test]
    fn compare_baseline_reports_deltas() {
        let baseline = GpuBaselineSnapshot {
            label: "base".into(),
            timestamp: "t0".into(),
            commit: "abc".into(),
            frame_count: 1,
            passes: vec![
                GpuPassSnapshot {
                    label: "Sprite pass".into(),
                    latest_ms: 1.0,
                    average_ms: 1.0,
                    max_ms: 1.0,
                    sample_count: 1,
                },
                GpuPassSnapshot {
                    label: "Mesh pass".into(),
                    latest_ms: 0.5,
                    average_ms: 0.5,
                    max_ms: 0.5,
                    sample_count: 1,
                },
            ],
        };
        let current = GpuBaselineSnapshot {
            label: "cur".into(),
            timestamp: "t1".into(),
            commit: "def".into(),
            frame_count: 1,
            passes: vec![
                GpuPassSnapshot {
                    label: "Sprite pass".into(),
                    latest_ms: 1.2,
                    average_ms: 1.2,
                    max_ms: 1.2,
                    sample_count: 1,
                },
                GpuPassSnapshot {
                    label: "Mesh pass".into(),
                    latest_ms: 0.6,
                    average_ms: 0.6,
                    max_ms: 0.6,
                    sample_count: 1,
                },
            ],
        };
        let tolerances = HashMap::from([("Sprite pass".to_string(), 0.3)]);
        let deltas = compare_baselines(&baseline, &current, &tolerances, 0.2).unwrap();
        assert_eq!(deltas.len(), 2);
        let sprite = deltas.iter().find(|d| d.label == "Sprite pass").unwrap();
        assert!((sprite.delta_ms - 0.2).abs() < f32::EPSILON);
        assert!(sprite.within_tolerance);
        assert_eq!(sprite.status, BaselinePassStatus::Matched);
        let mesh = deltas.iter().find(|d| d.label == "Mesh pass").unwrap();
        assert!((mesh.delta_ms - 0.1).abs() < f32::EPSILON);
        assert_eq!(mesh.allowed_drift_ms, 0.2);
        assert_eq!(mesh.status, BaselinePassStatus::Matched);
    }

    #[test]
    fn compare_baseline_reports_missing_and_extra_passes() {
        let baseline = GpuBaselineSnapshot {
            label: "base".into(),
            timestamp: "t0".into(),
            commit: "abc".into(),
            frame_count: 1,
            passes: vec![
                GpuPassSnapshot {
                    label: "Sprite pass".into(),
                    latest_ms: 1.0,
                    average_ms: 1.0,
                    max_ms: 1.0,
                    sample_count: 1,
                },
                GpuPassSnapshot {
                    label: "Mesh pass".into(),
                    latest_ms: 0.5,
                    average_ms: 0.5,
                    max_ms: 0.5,
                    sample_count: 1,
                },
            ],
        };
        let current = GpuBaselineSnapshot {
            label: "cur".into(),
            timestamp: "t1".into(),
            commit: "def".into(),
            frame_count: 1,
            passes: vec![
                GpuPassSnapshot {
                    label: "Sprite pass".into(),
                    latest_ms: 1.2,
                    average_ms: 1.2,
                    max_ms: 1.2,
                    sample_count: 1,
                },
                GpuPassSnapshot {
                    label: "Lighting pass".into(),
                    latest_ms: 0.7,
                    average_ms: 0.7,
                    max_ms: 0.7,
                    sample_count: 1,
                },
            ],
        };
        let deltas = compare_baselines(&baseline, &current, &HashMap::new(), 0.2).unwrap();
        assert_eq!(deltas.len(), 3);
        let sprite = deltas.iter().find(|d| d.label == "Sprite pass").unwrap();
        assert_eq!(sprite.status, BaselinePassStatus::Matched);
        let mesh = deltas.iter().find(|d| d.label == "Mesh pass").unwrap();
        assert_eq!(mesh.status, BaselinePassStatus::MissingInCurrent);
        assert!(!mesh.within_tolerance);
        assert_eq!(mesh.allowed_drift_ms, 0.0);
        let lighting = deltas.iter().find(|d| d.label == "Lighting pass").unwrap();
        assert_eq!(lighting.status, BaselinePassStatus::MissingInBaseline);
        assert_eq!(lighting.baseline_avg_ms, 0.0);
        assert_eq!(lighting.current_avg_ms, 0.7);
        assert!(!lighting.within_tolerance);
    }
}
