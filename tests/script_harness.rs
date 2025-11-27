use std::fs::File;
use std::path::Path;

use kestrel_engine::script_harness::{load_fixture, HarnessOutput, run_fixture};

#[test]
fn counter_fixture_matches_golden() {
    assert_fixture_matches("tests/fixtures/script_harness/counter.json", "tests/fixtures/script_harness/counter.golden.json");
}

#[test]
fn entity_commands_fixture_matches_golden() {
    assert_fixture_matches(
        "tests/fixtures/script_harness/entity_commands.json",
        "tests/fixtures/script_harness/entity_commands.golden.json",
    );
}

#[test]
fn spawn_commands_fixture_matches_golden() {
    assert_fixture_matches(
        "tests/fixtures/script_harness/spawn_commands.json",
        "tests/fixtures/script_harness/spawn_commands.golden.json",
    );
}

#[test]
fn event_bus_fixture_matches_golden() {
    assert_fixture_matches(
        "tests/fixtures/script_harness/event_bus.json",
        "tests/fixtures/script_harness/event_bus.golden.json",
    );
}

fn assert_fixture_matches(fixture_path: &str, golden_path: &str) {
    let fixture = load_fixture(fixture_path).expect("load fixture");
    let output = run_fixture(&fixture).expect("run fixture");
    let golden_file = File::open(Path::new(golden_path)).expect("open golden");
    let golden: HarnessOutput = serde_json::from_reader(golden_file).expect("parse golden");
    assert_eq!(output, golden, "fixture {} diverged from golden {}", fixture_path, golden_path);
}
