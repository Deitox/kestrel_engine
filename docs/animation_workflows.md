# Animation Workflows

## Purpose
- Document the end-to-end steps for authoring animation content.
- Capture examples for sprite timelines, transform clips, skeletal rigs, and graph configuration.
- Track prerequisites, tooling commands, and troubleshooting tips as features mature.

## Sprite Atlas Timelines
- **Authoring prerequisites:** export sprite sheets from Aseprite using `File > Export Sprite Sheet` with `Layout: Packed`, `JSON Data` enabled (Array format), and frame tags for every animation you intend to drive.
- **Run importer CLI:** `cargo run --bin aseprite_to_atlas -- <input.json> <output.json> [--atlas-key name] [--default-loop-mode loop|once_hold|once_stop|pingpong] [--reverse-loop-mode loop|once_hold|once_stop|pingpong]`
  - Converts the Aseprite JSON into an atlas definition containing `regions` and `animations` compatible with `assets/images/atlas.json`.
  - Use `--atlas-key` to override the default atlas identifier (`main`) when targeting alternative atlases.
  - Use loop-mode flags to map Aseprite tag directions to engine loop semantics (e.g., `--default-loop-mode once_hold` for UI bursts, `--reverse-loop-mode once_stop` for exit animations).
  - Attach per-frame events with `--events-file events.json`, where the JSON maps timeline names to `{ "frame": <0-based index>, "name": "event" }` entries; these emit `SpriteAnimationEvent` records when frames become active.
  - Sample fixtures live in `fixtures/aseprite/`; run the CLI against `slime_idle.json` (paired with `slime_idle_events.json`) to validate the workflow end-to-end. The generated atlas defaults to the `slime` key and exposes regions `slime_idle_0`, `slime_idle_1`, `slime_attack_0..2`, and `slime_hit`.
  - Inside the editor, use the Sprite inspector’s **Atlas** dropdown and `Load & Assign` button to switch entities to any loaded atlas or import a new `*.json` on the fly—no manual scene edits required. A quick slime test looks like this:
    1. In **Load atlas**, type `slime` for the key and `assets/images/slime_idle_atlas.json` for the path, then press **Load & Assign**. The atlas dropdown will flip from `main` to `slime`.
    2. Set the **Region** field to one of the exported frames (e.g., `slime_idle_0`) so the sprite displays the slime sheet. The Region dropdown will autocomplete after the atlas loads.
    3. Pick a **Timeline** (`idle`, `attack`, or `hit`) to play the matching animation; the inspector scrubber and event preview controls work as usual.
    4. The inspector status line confirms the change (e.g., `Sprite atlas set to slime`). To revert, select `main` from the Atlas dropdown or load any other atlas JSON.
- **Hot-reload verification:** place the generated JSON alongside project content, launch the editor, and modify the source file -- look for `Hot reloaded atlas '<key>'` in the console. Sprite timelines pin the active frame by name, scale the in-progress time to the new duration, and preserve play direction so authoring edits do not cause visible pops.
- **Inspector controls:** The Sprite section exposes a scrub slider, `<`/`>` nudge buttons, per-frame duration and elapsed readouts, and an optional **Preview events** toggle that logs any events declared on the currently selected frame while scrubbing.
- **Phase controls:** Configure per-entity `Start Offset`, toggle `Randomize Start` to deterministically de-sync large crowds, and assign an optional `Group` tag in the Entity Inspector; group tags feed the global `AnimationTime` resource for per-collection speed scaling.
- **Troubleshooting:**
  - Duplicate frame names surface descriptive errors; rename frames in Aseprite or adjust export settings.
  - Invalid tag ranges log the offending indices; confirm tag start/end frames in the tag dialog.
  - If hot reload does not trigger, ensure the atlas is retained in-scene (`Scene > Atlas refs`) and the watcher path matches the edited file.

## Transform & Property Clips
- **Milestone status:** The `AnimationClip` loader and fixtures are live on `main`; playback systems, inspector controls, and ECS glue land across Milestone 2. Build clips now so content is ready as the runtime merges.
- **Authoring prerequisites:** Keep source files under `assets/animation_clips/` (any path is valid as long as you pass it to `AssetManager::retain_clip`); use schema `version >= 1`; express keyframe times in seconds and rotations in radians; values must be finite or the loader rejects the clip.
- **Clip schema overview:** Each track (`translation`, `rotation`, `scale`, `tint`) is optional. `interpolation` accepts `linear` or `step` (defaults to `linear`). Duplicate timestamps collapse to the last keyframe. Translation/scale use `[x, y]`, tint uses `[r, g, b, a]`, and all channels clamp to author-supplied ranges at runtime.
- **Template:** Reference `fixtures/animation_clips/slime_bob.json` for a working example:

```json
{
  "version": 1,
  "name": "slime_bob",
  "looped": true,
  "tracks": {
    "translation": {
      "interpolation": "linear",
      "keyframes": [
        { "time": 0.0, "value": [0.0, 0.0] },
        { "time": 0.25, "value": [0.0, 4.0] },
        { "time": 0.5, "value": [0.0, 0.0] }
      ]
    },
    "rotation": {
      "interpolation": "linear",
      "keyframes": [
        { "time": 0.0, "value": 0.0 },
        { "time": 0.5, "value": 6.2831855 }
      ]
    },
    "scale": {
      "interpolation": "step",
      "keyframes": [
        { "time": 0.0, "value": [1.0, 1.0] },
        { "time": 0.5, "value": [1.2, 0.8] }
      ]
    },
    "tint": {
      "interpolation": "linear",
      "keyframes": [
        { "time": 0.0, "value": [1.0, 1.0, 1.0, 1.0] },
        { "time": 0.5, "value": [0.6, 0.9, 1.0, 1.0] }
      ]
    }
  }
}
```
- **Create & iterate:**
  1. Copy the template (or use your DCC exporter) into `assets/animation_clips/<name>.json` and bump the `name`/`looped` fields.
  2. Author keyframes per track, keeping the last keyframe time equal to the intended clip length; insert exact duplicates when you need step changes.
  3. In the editor build that includes Milestone 2, assign the clip key (e.g., `slime_bob`) in the Transform/Property Clip inspector panel; this wires the entity's `ClipInstance` to `TransformTrackPlayer`/`PropertyTrackPlayer`.
  4. Use the scrubber to confirm interpolation, playback speed, and looping. Inspector track badges surface which channels are present.
- **Validation & troubleshooting:**
  - Run `cargo test animation_clip` after editing clips; it exercises loader invariants against `fixtures/animation_clips` and catches ordering/finite-value issues.
  - Loader errors such as `Clip keyframe time cannot be negative` or `Clip keyframe contains non-finite rotation value` point directly at the offending keyframe index.
  - If a clip fails to appear in the inspector list, confirm the asset was retained (`AssetManager::retain_clip`) and that the scene/prefab dependency points at the JSON path.
  - Performance budget for this milestone is <= 0.40 ms CPU for 2 000 active clips; once the transform track benchmark lands, trigger it from the `animation_bench` suite to track regressions.

## Skeletal Animation Pipeline
- _Stub section - detail GLTF requirements, importer CLI, and validation steps in Milestone 3._

## Animation Graph Authoring
- _Stub section - document state machine graphs, parameter wiring, and scripting hooks when available._

## Testing Checklist
- Hot-reload sanity steps.
- Golden playback verification.
- Benchmark invocation (`cargo test -- --ignored animation_bench_run`).

## Change Log
- 2025-10-28: Added Aseprite importer workflow and CLI usage.
- 2025-11-02: Documented animation phase controls and AnimationTime integration.
- 2025-11-08: Added inspector playback controls, event preview details, and hot-reload continuity guarantees.
