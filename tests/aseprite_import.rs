use std::fs;
use std::path::PathBuf;
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
