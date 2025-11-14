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
- **Status:** [x] Completed by threading cascade controls through `SceneShadowData`, `config/app.json`, and the editor lighting panel.
- **Goal:** Close Milestone 12's "advanced lighting" bullet by exposing cascade controls and adding PCF filtering for distant cascades.
- **Scope:** Promote cascade configuration (count, splits, resolution, PCF radius) into `SceneShadowData` / `config/app.json`, update the lighting panel to edit those values, and implement adaptive split computation per camera.
- **Implementation Notes:**
  - The renderer now blends uniform and logarithmic cascade splits per camera, uploads per-cascade texel sizes/PCF radii, and re-allocates the shadow map array whenever cascade counts or resolution change.
  - `ShadowUniform`/`sample_shadow` include comparison-sampler PCF filtering with per-cascade radii to suppress shimmer.
  - Scene metadata + config persist cascade knobs so CI/editor captures stay deterministic.

## Sprite Zoom Guardrails
- **Status:** [x] Completed with configurable zoom limits + sprite footprint checks.
- **Goal:** Prevent the editor from rasterizing enormous quads when zooming too far into sprites.
- **Scope:** Expose camera zoom limits in config/editor UI, detect when sprites exceed a target on-screen size, and either warn or auto-clamp zoom; optionally add an LOD path for oversize sprites.
- **Implementation Notes:**
  - Added `editor` config + UI sliders for min/max orthographic zoom, sprite guardrail thresholds, and guard modes (Off/Warn/Clamp/Strict) so artists can tune limits live.
  - Sprite batching now measures each instance's on-screen footprint via `Camera2D::world_rect_to_screen_bounds`; warn or auto-clamp zoom when pixels exceed the threshold, or hide offending sprites entirely when guardrails are `Strict`.
  - Guardrail feedback surfaces directly in the Camera pane so users understand when zoom clamping/culling just occurred.

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
