# Animation Workflows (Stub)

## Purpose
- Document the end-to-end steps for authoring animation content.
- Capture examples for sprite timelines, transform clips, skeletal rigs, and graph configuration.
- Track prerequisites, tooling commands, and troubleshooting tips as features mature.

## Sprite Atlas Timelines
- **Export from Aseprite:** _TODO – describe required sprite sheet + JSON settings._
- **Run importer CLI:** `cargo run --bin aseprite_to_atlas -- <input.json> <output.json>`
- **Verify in editor:** _TODO – outline inspector checks, hot-reload workflow, and analytics counters._
- **Troubleshooting:** _TODO – common errors, schema version mismatches, logging hints._

## Transform & Property Clips
- _Stub section – fill in once Milestone 2 clip format lands._

## Skeletal Animation Pipeline
- _Stub section – detail GLTF requirements, importer CLI, and validation steps in Milestone 3._

## Animation Graph Authoring
- _Stub section – document state machine graphs, parameter wiring, and scripting hooks when available._

## Testing Checklist
- Hot-reload sanity steps.
- Golden playback verification.
- Benchmark invocation (`cargo test -- --ignored animation_bench_stub`).

## Change Log
- _Populate with updates as documentation evolves alongside roadmap milestones._
