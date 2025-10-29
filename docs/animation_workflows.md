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
- **Hot-reload verification:** place the generated JSON alongside project content, launch the editor, and modify the source file -- look for `Hot reloaded atlas '<key>'` in the console. Sprite timelines pin the active frame by name, scale the in-progress time to the new duration, and preserve play direction so authoring edits do not cause visible pops.
- **Inspector controls:** The Sprite section exposes a scrub slider, `<`/`>` nudge buttons, per-frame duration and elapsed readouts, and an optional **Preview events** toggle that logs any events declared on the currently selected frame while scrubbing.
- **Phase controls:** Configure per-entity `Start Offset`, toggle `Randomize Start` to deterministically de-sync large crowds, and assign an optional `Group` tag in the Entity Inspector; group tags feed the global `AnimationTime` resource for per-collection speed scaling.
- **Troubleshooting:**
  - Duplicate frame names surface descriptive errors; rename frames in Aseprite or adjust export settings.
  - Invalid tag ranges log the offending indices; confirm tag start/end frames in the tag dialog.
  - If hot reload does not trigger, ensure the atlas is retained in-scene (`Scene > Atlas refs`) and the watcher path matches the edited file.

## Transform & Property Clips
- _Stub section - fill in once Milestone 2 clip format lands._

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
