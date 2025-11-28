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
- **Status:** [x] Read-only ECS snapshots, asset readbacks, and watchdog surfacing landed
- **Goal:** Provide a concrete plan for Milestone 13's stretch goal: isolating untrusted plugins.
- **Scope:** Define capability metadata in `config/plugins.json`, gate `PluginContext` APIs by declared capabilities, log capability grants in `PluginStatus`, and design an out-of-process host for `trust = "isolated"` entries.
- **Implementation Notes:**
    - **Capability taxonomy:** implemented enum-based capability metadata (`renderer`, `ecs`, `assets`, `input`, `scripts`, `analytics`, `time`, `events`) plus manifest validation so third-party entries must explicitly opt into what they touch.
    - **API gating:** `PluginContext` enforces capability checks on every mutating accessor and emits `CapabilityError`s (plus analytics events) whenever an untrusted plugin asks for something it was not granted. Tests cover the gating path so regressions are caught in CI.
    - **Telemetry + surfacing:** capability grants/violations are visible in both the plugin manifest UI (per-plugin rows show trusted caps + violation counts) and the analytics Stats panel, which now lists recent capability violations, watchdog trips, and asset readback activity.
    - **Pipe-based RPC harness:** `plugin_rpc.rs` defines the bincode/framed protocol shared by the engine and `kestrel_plugin_host`. The host loads the target cdylib, services `build/update/fixed_update/on_events/shutdown` requests over anonymous pipes, and mirrors plugin errors back to the editor UI.
    - **Isolated proxy integration:** `PluginManager` launches `kestrel_plugin_host` whenever `"trust": "isolated"` is set, captures emitted events from the host, and re-injects them into the editor ECS so `ctx.emit_event`/`ctx.emit_script_message` work for isolated plugins. Watchdog timers tear down hung hosts and surface the failure inline.
    - **Developer tooling:** `isolated_plugin_cli` loads a manifest entry, runs a headless update loop, prints emitted events, issues RPC entity-info probes, and drives read-only ECS/entity iteration plus asset readbacks for automation/fuzzing outside the editor.
    - **Read-only ECS snapshots:** `PluginRpc::ReadComponents` and `PluginRpc::IterEntities` expose Sprite/Transform/Hierarchy/etc. snapshots with cursor-based pagination so tooling can inspect isolated worlds without fabricating events. The manager tracks a per-plugin history that the UI renders via collapsible “Read-only ECS” drawers.
    - **Asset readbacks + watchdog UX:** `PluginRpc::AssetReadback` supports `AtlasMeta`, `AtlasBinary`, and `BlobRange` payloads with manifest-scoped filters, throttling, and caching. Watchdog kills record timestamp/reason/last RPC, show badges in the plugin panel, and feed analytics (`plugin_watchdog_tripped`) so Ops can triage spikes quickly.
    - **Telemetry integration test + CI gate:** `cargo test --test plugins isolated_plugin_telemetry_pipeline` boots the isolated host, forces capability violations, and performs blob readbacks to ensure the analytics surface reflects capability, watchdog, and asset events. `.github/workflows/animation-bench.yml` now runs this test so regressions in the sandboxed path are caught automatically.
    - **Asset transport + docs:** Responses include a `content_type` hint, byte length, and in the metadata case a JSON blob described by `docs/plugins/atlas_meta_schema.json`. `docs/plugins/asset_readbacks.md` documents the format plus TypeScript/Rust helper structs, while the Stats panel lists the latest readbacks for auditing.
    - **Asset cache + throttling:** A shared `IsolatedAssetCache` avoids reloading hot assets, and `AssetReadbackBudget` (8 requests/4 MB per 16 ms window) returns structured `RateLimited` errors. The Analytics plugin captures every request (plugin id, payload kind, target, byte count, cache hit, duration) for investigations.
    - **PluginStatus UX wiring:** The manifest UI renders inline `ECS`/`Asset` badges, collapsible ECS history, per-plugin asset metrics, retry controls, and the watchdog log drawer. Artists can reload cdylibs, clear alerts, and inspect analytics without leaving the editor.
    - **CLI + automation hooks:** `isolated_plugin_cli --watchdog-ms`, `--asset-readback kind=value`, and `--fail-on-throttle` simulate watchdog trips and asset fetches in CI. Scripts can assert on the emitted analytics breadcrumbs to gate pipelines.
    - **Security considerations:** Asset readbacks respect manifest filters, are rate limited, and emit analytics events capturing plugin id, asset id, byte count, and duration. Watchdog trips include the offending RPC so suspicious or abusive behavior can be audited quickly.
