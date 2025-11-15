use kestrel_engine::scene_capture::{capture_scene_from_path, SceneCaptureSummary};
use std::fs::File;
use std::path::Path;

#[test]
fn animation_showcase_scene_matches_capture() {
    let capture_path = Path::new("artifacts/scene_captures/animation_showcase_capture.json");
    assert!(
        capture_path.exists(),
        "Expected {} to exist. Run scripts/capture_animation_samples.py animation_showcase first.",
        capture_path.display()
    );
    let reader = File::open(capture_path).expect("capture file readable");
    let expected: SceneCaptureSummary = serde_json::from_reader(reader).expect("capture file should parse");

    let scene_path = Path::new("assets/scenes/animation_showcase.json");
    let actual = capture_scene_from_path(scene_path).expect("scene capture summary should succeed");

    assert_eq!(
        actual, expected,
        "animation_showcase.json drifted. Re-run scripts/capture_animation_samples.py animation_showcase and inspect the changes."
    );
}
