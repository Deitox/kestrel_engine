# Engine Polish Targets

## Automated GPU Baselines & Alerts
- **Goal:** Catch regressions in Shadow/Mesh/Sprite passes automatically now that analytics exposes GPU timings.
- **Scope:** Add a deterministic, ignored test that boots the engine headless, loads a canned scene, runs N frames, and writes `perf/gpu_baseline.json` with the same schema as the sprite bench summaries. Compare the samples against a checked-in baseline with configurable tolerances (e.g., Shadow/Mesh ±0.30 ms, Sprite ±0.20 ms) and fail when drift exceeds the envelope.
- **Implementation Notes:**
  - Reuse `Renderer::init_headless_for_test` + `take_gpu_timings` to avoid window/system noise and keep runs deterministic.
  - Use `EcsWorld::load_scene_from_path_with_dependencies` so atlases/materials load reliably, then pause particle/script systems during the capture window.
  - Serialize via the same helper that backs `export_gpu_timings_csv`, add a `scripts/ci/run_gpu_baseline.ps1` wrapper, and wire a GitHub workflow job so PRs upload the JSON artifact.

## Cascaded Shadow Refinement
- **Goal:** Close Milestone 12’s “advanced lighting” bullet by exposing cascade controls and adding PCF filtering for distant cascades.
- **Scope:** Promote cascade configuration (count, splits, resolution, PCF radius) into `SceneShadowData` / `config/app.json`, update the lighting panel to edit those values, and implement adaptive split computation per camera.
- **Implementation Notes:**
  - Replace the fixed `DEFAULT_CASCADE_SPLITS` with a function that blends uniform/logarithmic splits so near cascades gain precision while far ranges stretch smoothly.
  - Extend `ShadowUniform` and `sample_shadow` to carry per-cascade texel sizes + PCF radii, then use comparison samplers to suppress shimmer.
  - Mark shadow resources dirty whenever the cascade config changes so the renderer reshapes its texture array and updates bind groups automatically.

## Sprite Zoom Guardrails
- **Goal:** Prevent the editor from rasterizing enormous quads when zooming too far into sprites.
- **Scope:** Expose camera zoom limits in config/editor UI, detect when sprites exceed a target on-screen size, and either warn or auto-clamp zoom; optionally add an LOD path for oversize sprites.
- **Implementation Notes:**
  - Surface zoom-limit controls near the camera stats readout and persist overrides per scene so preview files remember safe ranges.
  - During sprite batching, compute each instance’s screen footprint using `Camera2D::world_rect_to_screen_bounds`; emit warnings and clamp zoom when footprints exceed a configurable threshold.
  - Consider follow-up LOD logic that downscales or temporarily disables over-threshold sprites when `guardrails=Strict` to keep perf predictable.

## Build & Benchmark Harness
- **Goal:** Deliver Milestone 14’s “one command” perf suite by orchestrating `sprite_bench.py --runs 3` and `capture_sprite_perf.py`.
- **Scope:** Add `scripts/run_perf_suite.py` (or PowerShell equivalent) that forwards shared CLI knobs, runs both helpers, captures their artifacts, and emits a combined JSON summary with optional baseline diffing.
- **Implementation Notes:**
  - Support `--baseline perf/before_phase0.json` and future GPU baselines so the suite exits non-zero on drift.
  - Update `.github/workflows/animation-bench.yml` to call the suite runner after the animation target measurement and upload the combined artifacts directory.
  - Keep the existing scripts standalone; the new suite simply coordinates them for CI/local automation.

## Plugin Sandboxing Roadmap
- **Goal:** Provide a concrete plan for Milestone 13’s stretch goal: isolating untrusted plugins.
- **Scope:** Define capability metadata in `config/plugins.json`, gate `PluginContext` APIs by declared capabilities, log capability grants in `PluginStatus`, and design an out-of-process host for `trust = "isolated"` entries.
- **Implementation Notes:**
  - Map capabilities to the existing context helpers (Renderer, ECS, Assets, Input, Scripts) and default to “full trust” for built-ins while allowing downgraded access for third-party entries.
  - For real isolation, spin an auxiliary host executable that loads the plugin DLL, mirrors the `EnginePlugin` trait via IPC, and proxies only the approved capability calls so untrusted code never touches engine memory directly.
  - Extend analytics to record per-plugin CPU time/capability violations and add regression tests that ensure denied capabilities return clear errors both in-process and through the sandbox host.
