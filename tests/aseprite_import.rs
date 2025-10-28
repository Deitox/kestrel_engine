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
    assert!(generated.contains("\"duration_ms\": 100"), "frame duration should be preserved");
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
