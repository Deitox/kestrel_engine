# Animation Sample Content

Milestone 5 requires a deterministic scene that exercises the animation stack (sprite timelines, transform clips, atlas migrations) so the new tooling, HUD counters, and CLIs have something concrete to validate. The assets below ship inside the repository so both the editor and CI workflows can load/verify them without extra setup.

## Scene: `assets/scenes/animation_showcase.json`

| Entity Name            | Purpose |
|------------------------|---------|
| `sprite_timeline_demo` | Loads the `slime_idle` sprite timeline (from `assets/images/slime_idle_atlas.json`) and loops it at 1× speed. This is the canonical target for hot-reload demos: edit the atlas JSON or re-import from Aseprite and watch the inspector + HUD update live. |
| `transform_clip_demo`  | References the `slime_idle` transform clip (`assets/animations/clips/slime_idle.json`). Use the Keyframe Editor to tweak keys, hit “Insert Key at Scrub”, and the editor will persist the clip, reload it, and run validators immediately. |
| `palette_upload_probe` | Static sprite that keeps the GPU palette upload counter non-zero so the HUD always shows the palette bar, even in a simple scene. |

### Loading the scene

1. Launch the editor (`cargo run`) and choose **File → Open Scene… → `assets/scenes/animation_showcase.json`**.
2. The dependencies section already retains the atlases/clips, so the animation panel lists both demo tracks immediately.
3. Toggle the Animation HUD under **Stats → Viewport Overlays** to see the metrics update while editing. When the scene pushes more than 256 point lights through the clustered-light system, the overlay adds a **Lighting Budget** card so you know exactly when lights are being culled.

### Suggested workflows

- **Atlas / sprite timeline iteration:** Open `assets/images/slime_idle_atlas.json` in your editor, change a keyframe duration, and hit save. The watcher reloads the atlas, `animation_check` emits INFO lines in the console, and `sprite_timeline_demo` reflects the change live.
- **Transform clip authoring:** Select `transform_clip_demo`, open the Keyframe Editor, and edit the translation/scale keys. Saving adds a new entry to `assets/animations/clips/slime_idle.json`, suppresses duplicate watcher events, reloads the clip, and re-runs validators so the inspector banner always shows the latest status.
- **HUD / analytics validation:** With the scene loaded, hit play and observe the HUD entries for Sprite Eval/Pack/Upload plus Transform/Skeletal/Palette rows. These numbers now mirror the `animation_budget` section emitted in `target/animation_targets_report.json`, and the Lighting Budget card pops up if the scene exceeds the point-light cap so you can trim emitters or adjust scripts before baking captures.

### Deterministic capture

- Run `python scripts/capture_animation_samples.py animation_showcase` to emit `artifacts/scene_captures/animation_showcase_capture.json`. The script wraps the `scene_capture` helper so every entity/dependency/metadata field is serialized in a deterministic order.
- `tests/animation_showcase_scene.rs` loads `animation_showcase.json`, generates a fresh summary via the same code path, and asserts it matches the capture. If it drifts, re-run the script above and inspect the diff.

## Scene: `assets/scenes/skeletal_showcase.json`

This minimal scene keeps the skeletal fixture wired up so the HUD, watchers, and validation tests always have an active rig.

| Entity Name        | Purpose |
|--------------------|---------|
| `skeletal_demo`    | Attaches the `slime` skeleton (`assets/animations/skeletal/slime_rig.gltf`) and plays the `slime::breath` clip on loop. Use the inspector to pause, scrub, or swap clips while verifying the HUD’s Skeletal Eval rows and the asset watcher output. |

### Deterministic capture

- Run `python scripts/capture_animation_samples.py skeletal_showcase` to refresh `artifacts/scene_captures/skeletal_showcase_capture.json`.
- `cargo test animation_showcase_scene` now verifies both the animation and skeletal scene captures, so CI will flag any drift in the serialized skeleton data.

## Asset quick reference

| Asset | Path | Notes |
|-------|------|-------|
| Sprite atlas | `assets/images/slime_idle_atlas.json` | Contains `idle`, `attack`, and `hit` timelines used for watcher/CLI demos. |
| Transform clip | `assets/animations/clips/slime_idle.json` | Multi-channel clip wired to the transform clip entity; default target for Keyframe Editor tests. |
| Animation graph | `assets/animations/graphs/slime_idle_graph.json` | Placeholder graph used by scripting tests; run `animation_check assets/animations` to validate changes. |
| Skeletal fixture | `assets/animations/skeletal/slime_rig.gltf` | Exercised by `animation_check` and perf harness; future scenes can reference it once skeletal authoring lands in the editor. |

Use these assets when writing docs/tutorials so the paths match what ships in the repo and CI can replicate every step verbatim.
