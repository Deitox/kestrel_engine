# Aseprite Importer Fixtures

Sample exports used to validate the `aseprite_to_atlas` CLI as part of the animation system roadmap.

## Contents
- `slime_idle.json` — canonical packed-sheet export with three tagged timelines (`idle`, `attack`, `hit`).
- `slime_idle_events.json` — companion event definitions matching the `attack` timeline.

## Usage
Run the importer against the fixture to produce an atlas timeline JSON:

```bash
cargo run --bin aseprite_to_atlas -- fixtures\aseprite\slime_idle.json target\slime_idle_atlas.json ^
    --events-file fixtures\aseprite\slime_idle_events.json
```

The generated atlas can be dropped into `assets/images/` (or any watched path) to exercise hot-reload flows documented in `docs/animation_workflows.md`.
