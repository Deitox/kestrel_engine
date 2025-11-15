use kestrel_engine::scene_capture::{capture_scene_from_path, SceneCaptureSummary};
use std::fs::File;
use std::path::PathBuf;

fn assert_scene_matches_capture(name: &str) {
    let capture_path: PathBuf =
        ["artifacts", "scene_captures", &format!("{name}_capture.json")].iter().collect();
    assert!(
        capture_path.exists(),
        "Expected {} to exist. Run scripts/capture_animation_samples.py {name} first.",
        capture_path.display()
    );
    let reader = File::open(&capture_path).expect("capture file readable");
    let expected: SceneCaptureSummary = serde_json::from_reader(reader).expect("capture file should parse");

    let scene_path: PathBuf = ["assets", "scenes", &format!("{name}.json")].iter().collect();
    let actual = capture_scene_from_path(&scene_path).expect("scene capture summary should succeed");

    assert_eq!(
        actual, expected,
        "{name}.json drifted. Re-run scripts/capture_animation_samples.py {name} and inspect the changes."
    );
}

#[test]
fn animation_showcase_scene_matches_capture() {
    assert_scene_matches_capture("animation_showcase");
}

#[test]
fn skeletal_showcase_scene_matches_capture() {
    assert_scene_matches_capture("skeletal_showcase");
}
