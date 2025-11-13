# Engine Polish Targets

## Automated GPU Baselines & Alerts
- **Status:** [x] Completed via `cargo run --bin gpu_baseline` harness + `scripts/ci/run_gpu_baseline.ps1` and the GitHub Action step that uploads diffs.
- **Goal:** Catch regressions in Shadow/Mesh/Sprite passes automatically now that analytics exposes GPU timings.
- **Scope:** Deterministic headless runner loads the canned scene, renders N frames, writes `perf/gpu_baseline.json`, and compares to a checked-in baseline with per-pass tolerances; CI invokes it and fails on drift.
- **Implementation Notes:**
  - `GpuTimingAccumulator` serializes pass summaries and the `gpu_baseline` binary reuses the full scene-loading path so sprites/meshes match editor behavior.
  - `scripts/ci/run_gpu_baseline.ps1` wraps the binary for CI; `.github/workflows/animation-bench.yml` runs it after perf suite and publishes the artifact.
  - Future updates refresh `perf/gpu_baseline.json` to tighten tolerances as optimizations land.

## Cascaded Shadow Refinement
- **Status:** [ ] Pending
- **Goal:** Close Milestone 12's "advanced lighting" bullet by exposing cascade controls and adding PCF filtering for distant cascades.
- **Scope:** Promote cascade configuration (count, splits, resolution, PCF radius) into `SceneShadowData` / `config/app.json`, update the lighting panel to edit those values, and implement adaptive split computation per camera.
- **Implementation Notes:**
  - Replace the fixed `DEFAULT_CASCADE_SPLITS` with a function that blends uniform/logarithmic splits so near cascades gain precision while far ranges stretch smoothly.
  - Extend `ShadowUniform` and `sample_shadow` to carry per-cascade texel sizes + PCF radii, then use comparison samplers to suppress shimmer.
  - Mark shadow resources dirty whenever the cascade config changes so the renderer reshapes its texture array and updates bind groups automatically.

## Sprite Zoom Guardrails
- **Status:** [ ] Pending
- **Goal:** Prevent the editor from rasterizing enormous quads when zooming too far into sprites.
- **Scope:** Expose camera zoom limits in config/editor UI, detect when sprites exceed a target on-screen size, and either warn or auto-clamp zoom; optionally add an LOD path for oversize sprites.
- **Implementation Notes:**
  - Surface zoom-limit controls near the camera stats readout and persist overrides per scene so preview files remember safe ranges.
  - During sprite batching, compute each instance's screen footprint using `Camera2D::world_rect_to_screen_bounds`; emit warnings and clamp zoom when footprints exceed a configurable threshold.
  - Consider follow-up LOD logic that downscales or temporarily disables over-threshold sprites when `guardrails=Strict` to keep perf predictable.

## Build & Benchmark Harness
- **Status:** [x] Completed via `scripts/run_perf_suite.py` and the expanded `.github/workflows/animation-bench.yml` step that publishes the suite artifact.
- **Goal:** Deliver Milestone 14's "one command" perf suite by orchestrating `sprite_bench.py --runs 3` and `capture_sprite_perf.py`.
- **Scope:** Provide `scripts/run_perf_suite.py` that forwards shared CLI knobs, runs both helpers, captures their artifacts, and emits a combined JSON summary with optional baseline diffing.
- **Implementation Notes:**
  - Supports shared knobs such as `--sprite-baseline`, `--count`, `--steps`, and existing bench/capture overrides so CI/local workflows do not need to touch the original helpers.
  - The workflow now executes `python scripts/run_perf_suite.py --label ci_perf_suite ...` after the animation targets measurement and uploads the resulting `perf/ci_perf_suite_*` artifacts.
  - Existing helper scripts remain standalone; the suite simply coordinates them for CI and local automation.

## Plugin Sandboxing Roadmap
- **Status:** [ ] Pending
- **Goal:** Provide a concrete plan for Milestone 13's stretch goal: isolating untrusted plugins.
- **Scope:** Define capability metadata in `config/plugins.json`, gate `PluginContext` APIs by declared capabilities, log capability grants in `PluginStatus`, and design an out-of-process host for `trust = "isolated"` entries.
- **Implementation Notes:**
  - Map capabilities to the existing context helpers (Renderer, ECS, Assets, Input, Scripts) and default to "full trust" for built-ins while allowing downgraded access for third-party entries.
  - For real isolation, spin an auxiliary host executable that loads the plugin DLL, mirrors the `EnginePlugin` trait via IPC, and proxies only the approved capability calls so untrusted code never touches engine memory directly.
  - Extend analytics to record per-plugin CPU time/capability violations and add regression tests that ensure denied capabilities return clear errors both in-process and through the sandbox host.
