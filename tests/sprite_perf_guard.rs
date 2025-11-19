use kestrel_engine::sprite_perf_guard::{check_report, BenchThresholds};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from("tests/fixtures").join(name)
}

#[test]
fn sprite_perf_guard_accepts_healthy_metrics() {
    let path = fixture("sprite_perf_pass.json");
    let thresholds = BenchThresholds::new(0.300, 0.300, 0.01);
    let metrics = check_report(path, "sprite_timelines", thresholds, true).expect("guard should pass");
    assert!(metrics.mean_ms <= thresholds.mean_ms);
    assert!(metrics.max_ms <= thresholds.max_ms);
    assert!(metrics.slow_ratio.unwrap() <= thresholds.slow_ratio);
}

#[test]
fn sprite_perf_guard_flags_regressions() {
    let path = fixture("sprite_perf_fail.json");
    let thresholds = BenchThresholds::new(0.300, 0.300, 0.01);
    let err = check_report(path, "sprite_timelines", thresholds, true).expect_err("guard must fail");
    assert!(err.to_string().contains("case 'sprite_timelines' mean"), "unexpected error message: {err}");
}
