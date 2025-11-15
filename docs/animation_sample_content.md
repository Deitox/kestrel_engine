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
3. Toggle the Animation HUD under **Stats → Viewport Overlays** to see the metrics update while editing.

### Suggested workflows

- **Atlas / sprite timeline iteration:** Open `assets/images/slime_idle_atlas.json` in your editor, change a keyframe duration, and hit save. The watcher reloads the atlas, `animation_check` emits INFO lines in the console, and `sprite_timeline_demo` reflects the change live.
- **Transform clip authoring:** Select `transform_clip_demo`, open the Keyframe Editor, and edit the translation/scale keys. Saving adds a new entry to `assets/animations/clips/slime_idle.json`, suppresses duplicate watcher events, reloads the clip, and re-runs validators so the inspector banner always shows the latest status.
- **HUD / analytics validation:** With the scene loaded, hit play and observe the HUD entries for Sprite Eval/Pack/Upload plus Transform/Skeletal/Palette rows. These numbers now mirror the `animation_budget` section emitted in `target/animation_targets_report.json`.

## Asset quick reference

| Asset | Path | Notes |
|-------|------|-------|
| Sprite atlas | `assets/images/slime_idle_atlas.json` | Contains `idle`, `attack`, and `hit` timelines used for watcher/CLI demos. |
| Transform clip | `assets/animations/clips/slime_idle.json` | Multi-channel clip wired to the transform clip entity; default target for Keyframe Editor tests. |
| Animation graph | `assets/animations/graphs/slime_idle_graph.json` | Placeholder graph used by scripting tests; run `animation_check assets/animations` to validate changes. |
| Skeletal fixture | `assets/animations/skeletal/slime_rig.gltf` | Exercised by `animation_check` and perf harness; future scenes can reference it once skeletal authoring lands in the editor. |

Use these assets when writing docs/tutorials so the paths match what ships in the repo and CI can replicate every step verbatim.
