# Animation Workflows

## Purpose
- Document the end-to-end steps for authoring animation content.
- Capture examples for sprite timelines, transform clips, skeletal rigs, and graph configuration.
- Track prerequisites, tooling commands, and troubleshooting tips as features mature.

## Sprite Atlas Timelines
- **Authoring prerequisites:** export sprite sheets from Aseprite using `File > Export Sprite Sheet` with `Layout: Packed`, `JSON Data` enabled (Array format), and frame tags for every animation you intend to drive.
- **Run importer CLI:** `cargo run --bin aseprite_to_atlas -- <input.json> <output.json> [--atlas-key name]`
  - Converts the Aseprite JSON into an atlas definition containing `regions` and `animations` compatible with `assets/images/atlas.json`.
  - Use `--atlas-key` to override the default atlas identifier (`main`) when targeting alternative atlases.
- **Hot-reload verification:** place the generated JSON alongside project content, launch the editor, and modify the source file—look for `Hot reloaded atlas '<key>'` in the console while entities keep their current frame.
- **Troubleshooting:**
  - Duplicate frame names surface descriptive errors; rename frames in Aseprite or adjust export settings.
  - Invalid tag ranges log the offending indices; confirm tag start/end frames in the tag dialog.
  - If hot reload does not trigger, ensure the atlas is retained in-scene (`Scene > Atlas refs`) and the watcher path matches the edited file.

## Transform & Property Clips
- _Stub section — fill in once Milestone 2 clip format lands._

## Skeletal Animation Pipeline
- _Stub section — detail GLTF requirements, importer CLI, and validation steps in Milestone 3._

## Animation Graph Authoring
- _Stub section — document state machine graphs, parameter wiring, and scripting hooks when available._

## Testing Checklist
- Hot-reload sanity steps.
- Golden playback verification.
- Benchmark invocation (`cargo test -- --ignored animation_bench_run`).

## Change Log
- 2025-10-28: Added Aseprite importer workflow and CLI usage.
