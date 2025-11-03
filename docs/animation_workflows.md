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
- **Inspector walkthrough:** Use the Transform/Property Clip panel to validate runtime behavior before wiring clips into gameplay.
  1. Select an entity that retains the clip (or assign it using the key field), then confirm the panel lists the clip duration, loop mode, and play state.
  2. Use the Play/Pause toggle, Loop switch, and Speed slider to preview playback at different rates; the elapsed time readout honors global and group scaling as well as fixed-step evaluation.
  3. Drag the scrubber or tap the `<`/`>` nudge buttons to inspect specific keyframes. Track badges highlight which channels (translation, rotation, scale, tint) carry authored keys.
  4. Watch the per-track value previews update while the clip plays. If nothing changes, verify the entity owns the expected `TransformTrackPlayer` or `PropertyTrackPlayer` components.
  5. Edit and save the source JSON to trigger hot reload. The inspector preserves the current normalized time and immediately reflects the new keyframe data for comparison.
  - **Validation & troubleshooting:**
    - Run `cargo test animation_clip` after editing clips; it exercises loader invariants against `fixtures/animation_clips` and catches ordering/finite-value issues.
    - Run `cargo test transform_clip` to execute the golden interpolation suite and determinism checks across playback scenarios.
    - Loader errors such as `Clip keyframe time cannot be negative` or `Clip keyframe contains non-finite rotation value` point directly at the offending keyframe index.
    - If a clip fails to appear in the inspector list, confirm the asset was retained (`AssetManager::retain_clip`) and that the scene/prefab dependency points at the JSON path.
    - Scene and prefab exports automatically record transform clip dependencies; when moving clip JSON files, update the retained path so round-trip loads can resolve them without manual fixes.
    - Performance budget for this milestone is <= 0.40 ms CPU for 2 000 active clips; once the transform track benchmark lands, trigger it from the `animation_bench` suite to track regressions.

## Skeletal Animation Pipeline
- **Authoring prerequisites**
  - Export GLTF 2.0 scenes with a single skin per character. Parent joints under a dedicated root and provide stable joint names.
  - Limit joint counts to 256 (hard cap enforced by `MAX_SKIN_JOINTS` in the WGSL shaders). Higher counts import successfully but are truncated and warn at runtime.
  - Include at least one animation clip per rig so importer coverage stays green. Translation values are authoring-space meters; rotations use unit quaternions.
- **Authoring flow**
  1. Freeze transforms on meshes and joints, zero inherited scales, and bake the intended rest pose before export; the importer records the baked matrices as the authoritative bind pose.
  2. Trim animation ranges to the exact frames you expect to ship, then resample to a consistent timestep (30 or 60 FPS) so clip data behaves deterministically in tests.
  3. Export as GLTF 2.0 with `Embed buffers` enabled (or keep the `.bin` beside the `.gltf`) and include both the skin and authored clips in the same file.
  4. Copy the export into `assets/characters/<rig>.gltf` (fixtures stay under `fixtures/gltf/skeletons/` for regression tests) and bump the rig name if you intend to load multiple variants side by side.
  5. Optionally author a minimal preview scene that references the skeleton and clip; it shortens validation loops when verifying inspector UX.
- **Importer usage**
  - Drop source files under `assets/characters/<rig>.gltf` (fixtures continue to live in `fixtures/gltf/skeletons/` for tests).
  - Run `cargo test --test skeletal_import` to confirm extraction of the skeleton hierarchy, inverse bind matrices, and animation curves. Extend the test with additional assertions whenever the GLTF changes.
  - The importer emits `SkeletonAsset`, `SkeletalClip`, and any mesh skin bindings in a single pass; use `AssetManager::retain_skeleton`/`retain_clip` from scripts or the scene loader to stage them.
- **Runtime wiring**
  1. **Skeleton instance:** Attach a `SkeletonInstance` component to an entity via the inspector (assign the skeleton key and desired default clip). `reset_to_rest_pose` seeds local/model matrices so meshes display immediately.
  2. **Pose output:** Add `BoneTransforms` for entities that need palette uploads. The animation system keeps the struct in sync after each clip evaluation.
  3. **Skinned mesh:** Tag renderable entities with `SkinMesh`, point the `skeleton_entity` at the host `SkeletonInstance`, and ensure the mesh asset was imported with skinning streams (joint indices + weights). When both components are present, the renderer now uploads palettes from the shared buffer pool each frame.
  4. **Animation control:** Drive clips via the existing animation commands (`play_clip`, `set_group`, etc.). The unit tests in `src/ecs/systems/animation.rs` (`cargo test slime_rig_pose`) provide a golden reference for pose evaluation and loop wrapping.
- **Inspector expectations**
  - The **Skeleton** panel lists skeleton key, joint count, active clip, normalized time, and playback state. Expect an inline warning if the rig exceeds 256 joints or when clips lack authored channels.
  - Clip selectors surface every retained `<skeleton>::<clip>` combo and update the pose view immediately while scrubbing; palette lengths reflect the active clip's joint coverage.
  - The **Skinning** panel manages `SkinMesh` bindings: add/remove the component, assign a skeleton by `SceneEntityId`, sync joint counts from the target rig, and manually override joint budgets when debugging stray meshes.
  - Mesh inspector entries show whether a `SkinMesh` resolved a palette; the renderer truncates excess joints but keeps the entity visible so you can spot mismatches quickly.
- **Benchmarks & validation**
  - `cargo test --release animation_bench_run -- --ignored --nocapture` generates `benchmarks/animation_skeletal_clips.csv`, tracking the <= 1.20 ms CPU budget for 1 000 bones.
  - Frame-by-frame palette uploads are covered by the GPU timer described below; keep `Mesh` and `Shadow` passes <= 0.50 ms combined.
- **Troubleshooting**
  - Missing skin weights trigger importer warnings; re-export the mesh with normalized weight channels.
  - If a clip fails to play, ensure all referenced joints exist in the imported skeleton and that interpolation modes are either `LINEAR` or `STEP`.
  - Runtime truncation logs only once per unique joint count. Use that signal to reauthor the rig or split meshes across multiple drawables.

## Animation Graph Authoring
- _Stub section - document state machine graphs, parameter wiring, and scripting hooks when available._

## Testing Checklist
- Hot-reload sanity steps.
- Golden playback verification.
- Benchmark invocation (`cargo test --release --features anim_stats animation_bench_run -- --ignored --exact --nocapture`).
- GPU timing capture (`Export GPU CSV` in the Stats panel, see below).

## anim_stats Profiling
- Enable the counters with `--features anim_stats` when running either profiling harness:  
  - `cargo test --release --features anim_stats --test animation_profile animation_profile_snapshot -- --ignored --exact --nocapture` (per-frame traces).  
  - `cargo test --release --features anim_stats animation_bench_run -- --ignored --exact --nocapture` (CI automation).
- `animation_profile` logs per-step sprite counters in the format `sprite(fast=… event=… plain=…)` and transform clip counters `transform(adv=… zero=… skipped=… loop_resume=… zero_duration=…)`, aligned with the heaviest timing samples so you can spot whether spikes come from the fast-loop path or event/paused branches.
- `animation_bench` emits the same counters averaged per step and appends them to the CSV outputs (`animation_sprite_timelines.csv`, `animation_transform_clips.csv`). CI dashboards can now ingest both timing and path mix data from a single artifact.
- Call `reset_sprite_animation_stats()` / `reset_transform_clip_stats()` between custom runs if you are inspecting counters from scripts or the REPL; the helpers are re-exported under `kestrel_engine::ecs::*`.

## GPU Timing Capture
- Open the **Stats** panel in the editor and scroll to **GPU Timings**. Hardware must expose timestamp queries; unsupported adapters show a warning.
- Allow a few frames of gameplay so the ring buffer accumulates samples. The panel lists the latest durations alongside running averages for `Shadow`, `Mesh`, `Sprite`, and full-frame passes.
- Click **Export GPU CSV** to dump the history to `target/gpu_timings.csv`. The export contains `frame,label,duration_ms` rows and is ideal for checking that mesh + shadow uploads remain within the 0.50 ms budget after skinning changes.
- If the export fails, inspect the status string directly under the button; file-system errors (e.g., read-only builds) are reported there.

## Change Log
- 2025-10-28: Added Aseprite importer workflow and CLI usage.
- 2025-11-02: Documented animation phase controls and AnimationTime integration.
- 2025-11-08: Added inspector playback controls, event preview details, and hot-reload continuity guarantees.
- 2025-11-12: Expanded skeletal workflow authoring steps and inspector guidance.

