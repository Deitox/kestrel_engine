# Sprite Animation Performance Guard Plan

## 1. Mission & Success Criteria
- Keep the sprite timeline path at or below **0.300 ms for 10,000 animators** in release builds; enforce `p95(sprite_timelines_ms) <= 0.305 ms` with 240 measured frames (after a 16-frame warmup).
- Capture and gate on `{mean, median, p95, p99}` plus metadata `{warmup_frames, measured_frames, samples_per_case, dt, profile, lto_mode, rustc_version, target_cpu, feature_flags, commit_sha}` in every bench artifact (already emitted by `animation_targets_report.json`).
- Surface slow-path usage (var-dt, ping-pong, event-heavy clips) in-editor so asset changes stay honest, with no allocations/logging inside the hot loop.
- Fail CI whenever perf metrics exceed thresholds and archive CSV/JSON artifacts per run; include commit SHA + feature flags.
- Provide a repeatable perf matrix (baseline/SoA/fixed-point/SIMD) with archived CSVs plus README notes.

---

## 2. Phase 1 — Hot Loop Hygiene (Kernel Audit)

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 2.1 Runtime counters | ? `SpriteAnimPerfTelemetry` records per-frame counts for `var_dt`, `const_dt`, `ping_pong`, `events_heavy`, `%slow`, modulo/div fallbacks, SIMD lane mix, and emitted/coalesced events without allocating in the hot loop. | Resource lives on `World`; tests/benches consume it via `sprite_anim_perf_history()` / `sprite_anim_perf_sample()`. |
| 2.2 Stats panel wiring | ? The editor?s **Stats ? Sprite Animation Perf** block shows the counters, highlights `%slow > 1%` or tail-scalar >5% streaks, and adds Eval/Pack/Upload progress bars fed by profiler/GPU timers. | Screenshot/doc snippet demonstrating the readout and warning states. |
| 2.3 Bench harness output | ? `animation_targets_measure` now emits percentile stats + `{warmup_frames, measured_frames, samples_per_case, dt, profile, lto_mode, rustc_version, target_cpu, feature_flags, commit_sha}` metadata plus a `sprite_perf` summary per case. | `target/animation_targets_report.json` carries the new envelope for CI. |
| 2.4 CPU/GPU split | ? Eval (hot loop), Pack (SoA?AoS), and Upload (GPU sprite pass) timings are surfaced in the HUD and captured by the profiler/telemetry mix. | Stats panel bars + JSON output show all three stages for gating. |


---

## 3. Phase 2 — Instrumentation & HUD

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 2.1 Runtime counters | Track per-frame counts: `const_dt`, `var_dt`, `ping_pong`, `events_heavy`, `%slow`, `mod_or_div_calls`, SIMD `lanes_8/lanes_4/tail_scalar`, `events_emitted`, `events_coalesced`. Maintain zero allocations/logging in the hot loop by writing into a ring buffer flushed post-frame. | Telemetry resource exposes counters; unit tests exercise reset logic and ring-buffer flushing. |
| 2.2 Stats panel wiring | Extend the Stats sidebar/HUD to show counters, `%slow`, AnimationTime scale, and highlight orange when `%slow > 1%` or `tail_scalar > 5%` for >60 consecutive frames. Include color-coded Eval/Pack/Upload timing bars. | Screenshot/doc snippet demonstrating the readout and warning states. |
| 2.3 Bench harness output | Update `animation_targets_measure` to log/export the counters, percentile stats, warmup frame count, and CPU/compiler metadata per run. Always upload CSV/JSON artifacts with commit SHA + feature flags. | `target/animation_targets_report.json` includes new fields; CI artifacts capture them and link from logs. |
| 2.4 CPU/GPU split | Separate “Eval” (hot loop), “Pack” (SoA→AoS), and “Upload” timings so HUD and benches show individual budget consumption. Gate primarily on Eval while warning when Pack/Upload grow >10 % week-over-week. | Stats panel renders three timings with thresholds; bench output includes them. |

---

## 4. Phase 3 — Toolchain & CI Gates

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 3.1 Perf guard action | GitHub Action runs the bench, records `{mean, median, p95, p99}` + Eval/Pack/Upload. If thresholds fail, rerun once and keep the better sample; after two failures block the build. Emit warnings when mean regresses >5% vs last green commit. Always upload artifacts and link them in CI logs. Gate on Eval `p95 ≤ 0.305 ms`, warn when Pack/Upload exceed budgets. | CI badge flips red on regression; logs show artifact links and trend comparisons. |
| 3.2 Bench PGO profile | Capture PGO data for the animation bench target and bake into the **bench profile only** (`cargo test --profile bench-pgo ...`). Shipping/release builds can opt out but record the active mode in bench metadata. | Instructions documented; measured delta recorded plus metadata field showing `lto_mode`/`pgo`. |
| 3.3 Fixed-point default | Keep `sprite_anim_fixed_point` enabled in production profiles while allowing opt-out for diagnostics. | Cargo profiles updated; documentation explains the default. |

---

## 5. Phase 4 — Asset & Importer Guardrails

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 4.1 Timeline drift lint | During Aseprite import, detect “uniform” timelines that drift > 1 tick and warn (CLI + inspector toast). Include line/frame numbers in messages and persist lint severity (`info` ≤1 tick, `warn` otherwise). | Importer emits structured lint entries; unit test feeds noisy data and expects severity + locations. |
| 4.2 Event batching | Buffer per-animator events during evaluation and flush in one linear pass after the hot loop to maintain zero allocations. Provide `events_emitted` vs `events_coalesced` counters. | Bench shows zero extra branching; event regression tests still pass. |
| 4.3 Clip metadata | Persist `fast_path_eligible` boolean per clip (uniform dt, non-ping-pong) and surface it in the inspector/HUD. | Inspector shows eligibility flag; docs updated; lint ensures flag accuracy. |

---

## 6. Phase 5 — Runtime Stability & Risk Tests

| Risk | Mitigation | Validation |
| --- | --- | --- |
| Animator re-bucketing churn | Maintain per-bucket slabs with swap-remove semantics when clips/flags change. | Stress test toggling clips randomly; frame-time variance stays flat. |
| SIMD/scalar parity | Add property test with randomized timelines/delta sequences (including multi-frame jumps, exact boundary wraps, ping-pong flips) to compare SIMD vs scalar outputs over K steps. | New exhaustive test ensures parity within tolerance. |
| Warmup & denormals | Keep warmup frames in benches; enforce FTZ/DAZ via compiler flags. | Bench logs FTZ status; docs explain rationale. |
| Frame-event spikes | Rate-limit per-frame event emission; log when cap hits. | Counter + warning when rate limit triggers; tests cover multi-event frames. |
| Allocator/heap churn | Use pre-reserved slabs for animator state; validate via tests that no reallocations occur during updates. | Unit test asserts allocator counters remain unchanged during long runs. |
| Hidden `%` regression | Add unit test/script that inspects release disassembly for `rem`/`div` in hot symbols and fails if found. | Automated test runs as part of bench suite. |
| Asset drift trap | Add fixture asset with subtle dt jitter and assert importer emits lint with correct severity/line numbers. | Regression test under importer suite. |

---

## 7. Phase 6 — Bench Matrix & Documentation

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 6.1 Bench matrix automation | Script **three-run** sweeps for baseline, SoA fast path, fixed-point, SIMD; store `commit_sha`, `feature_flags`, and metadata headers in the CSVs archived under `perf/` or artifacts. | Latest CSVs committed; artifact workflow uploads them with metadata. |
| 6.2 README perf section | Add “Sprite Animation Perf” section summarizing target budgets (Eval/Pack/Upload), HUD counters, bench methodology (warmup, percentiles), and throughput charts (2k/5k/10k/20k animators). | README updated with tables/charts and budget split. |
| 6.3 Docs update | Expand `docs/animation_workflows.md` with HUD interpretation, importer lint notes, and CI troubleshooting. | Documentation PR merged; screenshots captured. |

---

## 8. Suggested Timeline

1. **Week 1:** Phase 1 audit + SIMD validation.  
2. **Week 2:** Phase 2 instrumentation + HUD.  
3. **Week 3:** CI guard + importer lint.  
4. **Week 4:** Event buffering, risk tests, bench automation.  
5. **Week 5:** Documentation + final perf matrix publication.

---

## 9. Ownership & Dependencies

- Runtime/perf work: animation systems owners.  
- Editor HUD & tooling: UI/tools engineer.  
- CI/automation: DevOps (GitHub Actions + artifact hosting).  
- Importer/docs: content pipeline owner.  
- Requires existing bench harness, Stats panel, and importer CLI.

---

## 10. Verification Checklist

- [x] Bench logs ≤ 0.300 ms mean/max, `%slow ≤ 1%`. (`python scripts/sprite_bench.py --label sprite_perf_guard --runs 1` followed by `cargo run --bin sprite_perf_guard -- --report target/animation_targets_report.json` records the latest run and enforces the threshold locally.)  
- [x] HUD counters accurate and highlighted on breaches. (`src/app/editor_ui.rs:799` warns when `%slow > 1%` for 60 frames and highlights SIM D tail ratios >5%, while `docs/animation_workflows.md:29` walks authors through using the Sprite Animation Perf HUD.)
- [x] CI gate fails on simulated regression. (`cargo run --bin sprite_perf_guard -- --report target/animation_targets_report.json` enforces the thresholds, while `cargo test sprite_perf_guard` feeds the guard with fixtures to simulate a regression and proves the check fails when metrics drift.)  
- [ ] Importer emits drift lint on synthetic noisy data.  
- [ ] SIMD tail + event flood tests pass.  
- [x] README/docs updated with HUD + perf guidance (`README.md:62`, `docs/animation_workflows.md:29` describe the Stats → Sprite Animation Perf HUD, warning colors, and perf workflow tips).
