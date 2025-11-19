# Animation Workflows

## Purpose
- Document the end-to-end steps for authoring animation content.
- Capture examples for sprite timelines, transform clips, skeletal rigs, and graph configuration.
- Track prerequisites, tooling commands, and troubleshooting tips as features mature.

## End-to-End Authoring Tutorial
1. **Prep the workspace**
   - From a clean checkout run `cargo fetch --locked` and `cargo check --bin kestrel_engine --bin animation_check` so the editor binary, CLIs, and watcher plumbing are ready before touching assets.
   - Skim `docs/animation_sample_content.md` for the entity/asset names referenced below; all steps point at those fixtures so CI can replay the workflow exactly.
2. **Convert sprite source to an atlas**
   - Export the slime sheet from Aseprite using the prerequisites listed later in this file, then run `cargo run --bin aseprite_to_atlas -- fixtures/aseprite/slime_idle.json assets/images/slime_idle_atlas.json --atlas-key slime --events-file fixtures/aseprite/slime_idle_events.json`.
   - Inspect the CLI output for dropped tags or duplicate frame warnings, then commit the generated JSON under `assets/images/` so watchers and CI can consume it.
3. **Validate atlases, clips, graphs, and rigs**
   - Run `cargo run --bin animation_check assets/images/slime_idle_atlas.json assets/animations/clips/slime_idle.json assets/animations/graphs/slime_idle_graph.json assets/animations/skeletal/slime_rig.gltf --fail-on-warn` to exercise schema validators across every asset type touched by this tutorial.
   - If the atlas schema changed recently, follow up with `cargo run --bin migrate_atlas -- assets/images/slime_idle_atlas.json --check`; CI uses the same CLI with `--fix` when migrations land.
4. **Load the showcase scene and bind content**
   - Launch the editor via `cargo run` and choose **File + Open Scene → assets/scenes/animation_showcase.json**.
   - Select `sprite_timeline_demo`, click **Load & Assign**, enter `slime` for the atlas key plus `assets/images/slime_idle_atlas.json` for the path, and pick the `idle` timeline. The inspector log prints `Sprite atlas set to slime`, and playback starts immediately.
   - Toggle **Stats + Viewport Overlays** and confirm the Sprite Eval/Pack/Upload HUD rows reflect the new animator load. The same overlay now surfaces a **Lighting Budget** card whenever clustered point lights exceed the 256-light budget, so you can spot blown lighting caps while iterating on animation scenes without digging through logs.
5. **Author the transform clip**
   - With `transform_clip_demo` selected, open the Keyframe Editor and tweak a translation key (or press **Insert Key at Scrub**). Saving updates `assets/animations/clips/slime_idle.json`, triggers the watcher, and reruns validators without duplicate reloads.
   - Run `cargo test animation_clip transform_clip` afterwards to lock in the edits against the golden interpolation suite.
6. **Exercise the skeletal fixture**
   - Load `assets/scenes/skeletal_showcase.json` so `skeletal_demo` plays `slime::breath`.
   - In the inspector assign (or confirm) the `slime` skeleton, scrub the clip, and watch the Skeletal Eval + Palette Upload HUD rows for timing regressions.
   - From a terminal run `cargo test --test skeletal_import` to reverify GLTF skeleton/clip extraction after edits.
7. **Record deterministic captures and perf baselines**
   - Run `python scripts/capture_animation_samples.py animation_showcase` and `python scripts/capture_animation_samples.py skeletal_showcase` so `artifacts/scene_captures/*.json` stay in sync with the authored content.
   - Execute `cargo test --release --features anim_stats animation_targets_measure -- --ignored --exact --nocapture` to refresh `target/animation_targets_report.json`, then archive the `sprite_perf` / `animation_budget` blocks with the updated assets for reviewers.

## Sprite Atlas Timelines
- **Authoring prerequisites:** export sprite sheets from Aseprite using `File > Export Sprite Sheet` with `Layout: Packed`, `JSON Data` enabled (Array format), and frame tags for every animation you intend to drive.
- **Run importer CLI:** `cargo run --bin aseprite_to_atlas -- <input.json> <output.json> [--atlas-key name] [--default-loop-mode loop|once_hold|once_stop|pingpong] [--reverse-loop-mode loop|once_hold|once_stop|pingpong]`
  - Converts the Aseprite JSON into an atlas definition containing `regions` and `animations` compatible with `assets/images/atlas.json`.
  - Use `--atlas-key` to override the default atlas identifier (`main`) when targeting alternative atlases.
  - Use loop-mode flags to map Aseprite tag directions to engine loop semantics (e.g., `--default-loop-mode once_hold` for UI bursts, `--reverse-loop-mode once_stop` for exit animations).
  - Attach per-frame events with `--events-file events.json`, where the JSON maps timeline names to `{ "frame": <0-based index>, "name": "event" }` entries; these emit `SpriteAnimationEvent` records when frames become active.
  - Sample fixtures live in `fixtures/aseprite/`; run the CLI against `slime_idle.json` (paired with `slime_idle_events.json`) to validate the workflow end-to-end. The generated atlas defaults to the `slime` key and exposes regions `slime_idle_0`, `slime_idle_1`, `slime_attack_0..2`, and `slime_hit`.
  - Importer lint now surfaces “uniform dt drift” when most frames in a loop share the same duration but a few frames stray by ≥1ms. Watch the CLI output for `lint(info|warn)` entries and review the exported `lint[]` entries in the atlas JSON; fix noisy clips or commit the lint metadata alongside the atlas so CI/inspectors can track intentional drift.
- **Perf guard & CI hook:** After editing sprite content, refresh the release-profile bench via `python scripts/sprite_bench.py --profile bench-release --runs 1` followed by `cargo run --bin sprite_perf_guard -- --report target/animation_targets_report.json`. This records the Sprite Eval/Pack/Upload metrics described in `docs/SPRITE_ANIMATION_PERF_PLAN.md` and fails when `sprite_timelines` mean/max exceed the ≤0.300 ms budget or `%slow` > 1%. CI runs the same commands so local runs stay in lockstep with the guard.
  - Inside the editor, use the Sprite inspector’s **Atlas** dropdown and `Load & Assign` button to switch entities to any loaded atlas or import a new `*.json` on the fly—no manual scene edits required. A quick slime test looks like this:
    1. In **Load atlas**, type `slime` for the key and `assets/images/slime_idle_atlas.json` for the path, then press **Load & Assign**. The atlas dropdown will flip from `main` to `slime`.
    2. Set the **Region** field to one of the exported frames (e.g., `slime_idle_0`) so the sprite displays the slime sheet. The Region dropdown will autocomplete after the atlas loads.
    3. Pick a **Timeline** (`idle`, `attack`, or `hit`) to play the matching animation; the inspector scrubber and event preview controls work as usual.
    4. The inspector status line confirms the change (e.g., `Sprite atlas set to slime`). To revert, select `main` from the Atlas dropdown or load any other atlas JSON.
- **Hot-reload verification:** place the generated JSON alongside project content, launch the editor, and modify the source file -- look for `Hot reloaded atlas '<key>'` in the console. Sprite timelines pin the active frame by name, scale the in-progress time to the new duration, and preserve play direction so authoring edits do not cause visible pops.
- **Inspector controls:** The Sprite section exposes a scrub slider, `<`/`>` nudge buttons, per-frame duration and elapsed readouts, and an optional **Preview events** toggle that logs any events declared on the currently selected frame while scrubbing. When overlays are enabled, the viewport HUD also flags animation and lighting overruns in real time so tuning a timeline immediately reveals downstream perf/lighting pressure.
- **Phase controls:** Configure per-entity `Start Offset`, toggle `Randomize Start` to deterministically de-sync large crowds, and assign an optional `Group` tag in the Entity Inspector; group tags feed the global `AnimationTime` resource for per-collection speed scaling.
- **Troubleshooting:**
  - Duplicate frame names surface descriptive errors; rename frames in Aseprite or adjust export settings.
  - Invalid tag ranges log the offending indices; confirm tag start/end frames in the tag dialog.
  - If hot reload does not trigger, ensure the atlas is retained in-scene (`Scene > Atlas refs`) and the watcher path matches the edited file.

## Animation HUD & Perf Counters
- Open **Stats → Sprite Animation Perf** in the editor to inspect runtime telemetry. The panel lists fast/slow bucket counts, Δt mix (ar_dt vs const_dt), ping-pong/event-heavy animator totals, emitted/coalesced event counts, and modulo/division fallbacks. Values update every frame without allocations, so you can leave the panel open while iterating.
- The viewport HUD (toggle **Stats → Viewport Overlays**) mirrors the roadmap budgets with color-coded bars:
  - **Sprite Eval/Pack/Upload**: CPU time for sys_drive_sprite_animations, sys_apply_sprite_frame_states, and the GPU sprite pass (budgets 0.30 / 0.05 / 0.10 ms).
  - **Transform Clips**: CPU time for sys_drive_transform_clips, plus active clip counts (target ≤ 0.40 ms for 2 000 clips).
  - **Skeletal Eval**: CPU time for sys_drive_skeletal_clips, rig counts, and total bones (target ≤ 1.20 ms for 1 000 bones).
  - **Palette Uploads**: GPU joint palette staging cost and upload frequency (target ≤ 0.50 ms).
  Bars stay green under budget, flip amber between 80%–100%, and red once the budget is exceeded. Labels include the current animator/clip/bone totals so perf investigations start with concrete data.
- Need raw samples? Call sprite_anim_perf_history() from the REPL or test harness to fetch the ring buffer, or sprite_anim_perf_sample() to read the latest frame. Both helpers live on EcsWorld. Transform/skeletal timings surface via EcsWorld::system_timings() summaries if you need historical values outside the HUD.

## Automation & Validation
- **Watchers:** `assets/images/*.json` (atlases), `assets/animations/clips/**/*.json`, `assets/animations/graphs/**/*.json`, and `assets/animations/skeletal/**/*.json` are observed automatically inside the editor. Saving a file reloads the asset, reruns schema + semantic validators, and surfaces results both in the inspector banner and the Stats panel’s “Animation Validation Alerts”. Keep assets under `assets/animations/` so canonical paths are recorded; the inspector status line confirms each reload (e.g., `Reloaded clip 'slime_bob' from …`).
- **Lighting warnings:** The same overlay toggle powers a **Lighting Budget** card that appears whenever the clustered-light culler has to drop point lights (more than 256 visible). Use it alongside the Stats panel’s “Light Culling” section when you’re animating scenes with particle-driven emissives or scripted light spawns so you notice budget pressure before QA does.
- **State preservation:** Clip saves preserve scrub/play state. Skeletal GLTF reloads restore each entity’s active clip, time, playing flag, speed, and group tag so iteration never forces you to re-seed rigs manually.
- **Graph workflow:** Animation graph JSON reimports immediately. Even before graph runtime features ship, this keeps authored graphs validated and ready for CLI/CI enforcement.
- **CLI validation:** Use the same pipeline headlessly via `animation_check`:
  ```shell
  cargo run --bin animation_check -- assets/animations
  ```
  Provide files and/or directories. The command walks subdirectories, filters supported extensions (`.json`, `.clip`, `.gltf`, `.glb`), prints `INFO/WARN/ERROR` lines, and exits with code `2` when blocking errors exist. CI will run this exact command after animation asset changes.
- **Atlas migrations:** Normalize sprite atlases with `migrate_atlas` whenever loop mode semantics or schema versions change:
  ```shell
  cargo run --bin migrate_atlas -- assets/images
  ```
  Add `--check` to keep runs read-only (CI safe) while still reporting which files would have been touched. The CLI rewrites each atlas JSON in place, injecting canonical `loop_mode` strings (based on the old `looped` flag), clamping zero-duration frames to 1 ms, trimming duplicate/out-of-range timeline events, and bumping the root `version` to `2`. Point it at individual files or directories; unsupported JSON files are skipped with a warning so you can run it across entire content roots.
- **Troubleshooting:** Validation events include absolute paths and severity in the console plus analytics log. If a watched folder fails to register, confirm it exists (missing directories are skipped silently) or run `animation_check` against the problematic path to get detailed diagnostics.

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
    - Prefab exports now embed mesh and material source paths for any assets currently loaded in the editor, so instantiating a prefab in a fresh session will automatically reload the referenced GLTF/material definitions rather than depending on another scene to keep them alive.
    - Performance budget for this milestone is <= 0.40 ms CPU for 2 000 active clips; trigger `cargo test --release animation_targets_measure -- --ignored --nocapture` when you need a fresh measurement. Use `python scripts/capture_sprite_perf.py --label clips_baseline --runs 3` to gather both the averaged bench artefacts and the anim_stats per-step log/JSON pair for later comparison.

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
- `cargo test --release animation_targets_measure -- --ignored --nocapture` reports the <= 1.20 ms CPU budget for 1 000 bones and writes `target/animation_targets_report.json`. For multi-run captures (plus anim_stats traces), prefer `python scripts/capture_sprite_perf.py --label skeletal_baseline --runs 3` so the aggregated stats land in `perf/`.
  - Frame-by-frame palette uploads are covered by the GPU timer described below; keep `Mesh` and `Shadow` passes <= 0.50 ms combined.
- **Troubleshooting**
  - Missing skin weights trigger importer warnings; re-export the mesh with normalized weight channels.
  - If a clip fails to play, ensure all referenced joints exist in the imported skeleton and that interpolation modes are either `LINEAR` or `STEP`.
  - Runtime truncation logs only once per unique joint count. Use that signal to reauthor the rig or split meshes across multiple drawables.

## Animation Graph Authoring
- _Stub section - document state machine graphs, parameter wiring, and scripting hooks when available._

## Troubleshooting & Scripting Best Practices
- **Watcher reload gaps:** If an edit fails to appear in the editor, open `Scene > Atlas refs` (sprite timelines) or the clip dependency list to confirm the asset is retained. Watchers only fire for live references, so keep sample entities or bootstrap scripts retaining every atlas/clip/rig you plan to edit.
- **CLI validation noise:** Run `cargo run --bin animation_check <paths> --fail-on-warn --report-stats` when triaging schema errors; with `--report-stats` the CLI mirrors every validation event as JSON (plus a final summary object) so you can jump straight to the exact track, joint, or tag that failed validation. For migrations, prefer `cargo run --bin migrate_atlas -- <files> --check` locally and `--fix` when you need to rewrite legacy atlases.
- **Keyframe/editor drift:** Errors like `Clip keyframe time cannot be negative` surface the offending key index. Use the Keyframe Editor scrubber to jump there, or patch the JSON manually and rerun `cargo test transform_clip` before relaunching the editor.
- **Scripting hooks:** Retain assets during startup (`AssetManager::retain_clip/retain_skeleton`), drive playback through `AnimationCommands`, and group crowds with `AnimationTime::set_group_speed`. Mirroring the editor’s wiring guarantees watcher reloads hit every runtime path.
- **Perf regressions:** The **Stats + Sprite Animation Perf** panel shows whether animators are in the fast loop or the event-heavy path. If numbers spike, rerun `python scripts/capture_sprite_perf.py --label repro --runs 3` and inspect the emitted anim_stats JSON to pinpoint the regressing system.

## Testing Checklist
- Hot-reload sanity steps.
- Golden playback verification.
- Benchmark invocation (`cargo test --release --features anim_stats animation_targets_measure -- --ignored --exact --nocapture`).
- GPU timing capture (`Export GPU CSV` in the Stats panel, see below).

## anim_stats Profiling
- Enable the counters with `--features anim_stats` when running either profiling harness:  
  - `cargo test --release --features anim_stats --test animation_profile animation_profile_snapshot -- --ignored --exact --nocapture` (per-frame traces).  
  - `cargo test --release --features anim_stats animation_targets_measure -- --ignored --exact --nocapture` (roadmap checkpoints).
- Sprite metrics now expose hot-path coverage: `fast_bucket_entities`, `general_bucket_entities`, their frame counts, plus `frame_apply_count` so we can match GPU uploads to actual hot-loop work. `capture_sprite_perf.py` stores these stats alongside the run metadata each time it invokes `animation_profile_snapshot`.
- `animation_profile` logs per-step sprite counters in the format `sprite(fast=… event=… plain=…)` and transform clip counters `transform(adv=… zero=… skipped=… loop_resume=… zero_duration=…)`, aligned with the heaviest timing samples so you can spot whether spikes come from the fast-loop path or event/paused branches.
- `animation_targets_measure` captures the roadmap checkpoints, printing PASS/WARN summaries and writing per-case timing data to `target/animation_targets_report.json` for CI dashboards. The JSON now ships with `{mean, median, p95, p99}` timing stats, rich metadata (profile/LTO/CPU/rustc/features/commit), and a `sprite_perf` block mirroring the HUD counters so you can diff slow-path mix inside CI.
- Call `reset_sprite_animation_stats()` / `reset_transform_clip_stats()` between custom runs if you are inspecting counters from scripts or the REPL; the helpers are re-exported under `kestrel_engine::ecs::*`.

## GPU Timing Capture
- Open the **Stats** panel in the editor and scroll to **GPU Timings**. Hardware must expose timestamp queries; unsupported adapters show a warning.
- Allow a few frames of gameplay so the ring buffer accumulates samples. The panel lists the latest durations alongside running averages for `Shadow`, `Mesh`, `Sprite`, and full-frame passes.
- Click **Export GPU CSV** to dump the history to `target/gpu_timings.csv`. The export contains `frame,label,duration_ms` rows and is ideal for checking that mesh + shadow uploads remain within the 0.50 ms budget after skinning changes.
- If the export fails, inspect the status string directly under the button; file-system errors (e.g., read-only builds) are reported there.

## Change Log
- 2025-11-16: Added End-to-End Authoring Tutorial, Troubleshooting & Scripting Best Practices, and CI/perf capture guidance referencing the sample scenes.
- 2025-11-15: Documented animation HUD budgets, watcher behavior, and the animation_check CLI validation workflow.
- 2025-10-28: Added Aseprite importer workflow and CLI usage.
- 2025-11-02: Documented animation phase controls and AnimationTime integration.
- 2025-11-08: Added inspector playback controls, event preview details, and hot-reload continuity guarantees.
- 2025-11-12: Expanded skeletal workflow authoring steps and inspector guidance.

