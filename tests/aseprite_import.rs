use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn aseprite_to_atlas_generates_expected_json() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let input_path = temp_dir.path().join("input.json");
    let output_path = temp_dir.path().join("output.json");

    let aseprite_sample = r#"{
  "frames": [
    {
      "filename": "frame0",
      "frame": { "x": 0, "y": 0, "w": 32, "h": 32 },
      "duration": 100
    },
    {
      "filename": "frame1",
      "frame": { "x": 32, "y": 0, "w": 32, "h": 32 },
      "duration": 120
    }
  ],
  "meta": {
    "image": "demo.png",
    "frameTags": [
      { "name": "idle", "from": 0, "to": 1, "direction": "forward" }
    ]
  }
}"#;

    fs::write(&input_path, aseprite_sample).expect("write sample aseprite json");

    let exe = locate_binary("aseprite_to_atlas");
    let status =
        Command::new(exe).arg(&input_path).arg(&output_path).status().expect("run aseprite_to_atlas");

    assert!(status.success(), "aseprite_to_atlas did not exit successfully");

    let generated = fs::read_to_string(&output_path).expect("read generated atlas json");
    assert!(generated.contains("\"idle\""), "generated atlas should contain the timeline name");
    assert!(generated.contains("\"loop_mode\": \"loop\""), "default loop mode should be loop");
    assert!(generated.contains("\"duration_ms\": 100"), "frame duration should be preserved");
}

#[test]
fn aseprite_to_atlas_honors_loop_mode_flags() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let input_path = temp_dir.path().join("input.json");
    let output_path = temp_dir.path().join("output.json");

    let aseprite_sample = r#"{
  "frames": [
    {
      "filename": "frame0",
      "frame": { "x": 0, "y": 0, "w": 16, "h": 16 },
      "duration": 80
    }
  ],
  "meta": {
    "image": "single.png",
    "frameTags": []
  }
}"#;

    fs::write(&input_path, aseprite_sample).expect("write sample aseprite json");

    let exe = locate_binary("aseprite_to_atlas");
    let status = Command::new(&exe)
        .arg(&input_path)
        .arg(&output_path)
        .arg("--default-loop-mode")
        .arg("once_hold")
        .arg("--reverse-loop-mode")
        .arg("once_stop")
        .status()
        .expect("run aseprite_to_atlas");

    assert!(status.success(), "aseprite_to_atlas did not exit successfully");

    let generated = fs::read_to_string(&output_path).expect("read generated atlas json");
    assert!(
        generated.contains("\"loop_mode\": \"once_hold\""),
        "default loop mode flag should override timeline mode"
    );
    assert!(generated.contains("\"looped\": false"), "once_hold should mark timeline as non-looping");
}

#[test]
fn aseprite_to_atlas_applies_events_file() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let input_path = temp_dir.path().join("input.json");
    let output_path = temp_dir.path().join("output.json");
    let events_path = temp_dir.path().join("events.json");

    let aseprite_sample = r#"{
  "frames": [
    {
      "filename": "frame0",
      "frame": { "x": 0, "y": 0, "w": 16, "h": 16 },
      "duration": 100
    },
    {
      "filename": "frame1",
      "frame": { "x": 16, "y": 0, "w": 16, "h": 16 },
      "duration": 120
    }
  ],
  "meta": {
    "image": "demo.png",
    "frameTags": [
      { "name": "idle", "from": 0, "to": 1, "direction": "forward" }
    ]
  }
}"#;

    fs::write(&input_path, aseprite_sample).expect("write sample aseprite json");
    fs::write(&events_path, "{\"idle\": [{\"frame\": 1, \"name\": \"footstep\"}]}")
        .expect("write events json");

    let exe = locate_binary("aseprite_to_atlas");
    let status = Command::new(exe)
        .arg(&input_path)
        .arg(&output_path)
        .arg("--events-file")
        .arg(&events_path)
        .status()
        .expect("run aseprite_to_atlas");

    assert!(status.success(), "aseprite_to_atlas did not exit successfully with events file");

    let generated = fs::read_to_string(&output_path).expect("read generated atlas json");
    assert!(generated.contains("\"events\""), "atlas output should include events array");
    assert!(generated.contains("footstep"), "atlas output should include the specified event name");
}

#[test]
fn aseprite_to_atlas_processes_fixture_exports() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let output_path = temp_dir.path().join("output.json");

    let exe = locate_binary("aseprite_to_atlas");
    let status = Command::new(exe)
        .arg(Path::new("fixtures/aseprite/slime_idle.json"))
        .arg(&output_path)
        .arg("--events-file")
        .arg(Path::new("fixtures/aseprite/slime_idle_events.json"))
        .status()
        .expect("run aseprite_to_atlas with fixture input");

    assert!(status.success(), "aseprite_to_atlas did not exit successfully for fixture");

    let generated = fs::read_to_string(&output_path).expect("read generated atlas json");
    assert!(
        generated.contains("\"attack\""),
        "fixture atlas should include the attack timeline"
    );
    assert!(
        generated.contains("\"windup\""),
        "fixture atlas should retain event names from the events file"
    );
    assert!(
        generated.contains("\"image\": \"slime.png\""),
        "fixture atlas should propagate sprite sheet metadata"
    );
}

fn locate_binary(name: &str) -> PathBuf {
    if let Ok(path) = std::env::var(format!("CARGO_BIN_EXE_{name}")) {
        return PathBuf::from(path);
    }
    let mut path = std::env::current_exe().expect("current exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push(name);
    if cfg!(windows) {
        path.set_extension("exe");
    }
    assert!(path.exists(), "expected binary '{}' at {}, but it does not exist", name, path.display());
    path
}
