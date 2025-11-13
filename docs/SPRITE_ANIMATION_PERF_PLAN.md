# Sprite Animation Performance Guard Plan

## 1. Mission & Success Criteria
- Keep the sprite timeline path at or below **0.200 ms for 10 000 animators** in release builds; enforce `p95(sprite_timelines_ms) ≤ 0.205 ms` with 240 measured frames (after a 16-frame warmup).
- Capture and gate on `{mean, median, p95, p99}` plus metadata `{warmup_frames, measured_frames, rustc_version, target_cpu, lto_mode}` in every bench artifact.
- Surface slow-path usage (var-dt, ping-pong, event-heavy clips) in-editor so asset changes stay honest, with no allocations/logging inside the hot loop.
- Fail CI whenever perf metrics exceed thresholds and archive CSV/JSON artifacts per run; include commit SHA + feature flags.
- Provide a repeatable perf matrix (baseline/SoA/fixed-point/SIMD) with archived CSVs plus README notes.

---

## 2. Phase 1 — Hot Loop Hygiene (Kernel Audit)

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 1.1 Remove `%` / `/` from the inner loop | Inspect `advance_animation_fast_loop(_slot)` for modulo/divide ops; precompute reciprocal frame counts and rely on multiply-adds. Keep ping-pong flips as a direction bit toggled only at wrap points; add a regression trap that fails if a release bench build still contains `rem`/`div` in the symbol. | Perf inspection (cargo-asm/perf) plus automated test show no `%`/`/` in hot functions. |
| 1.2 Floor-delta fast path | Confirm constant-delta mode uses integer/fixed-point accumulators with `time_left -= dt` loops. | Bench trace shows zero `rem_euclid` or float divides when const-dt is active. |
| 1.3 Ping-pong isolation | Keep ping-pong animators in a dedicated bucket or treat direction toggles only on boundary events. | Fast bucket occupancy ≥ 99 % on reference scenes. |
| 1.4 SIMD verification | Build benches with `-C target-cpu=native` + ThinLTO; inspect disassembly to verify vector width. Optionally surface lane-utilization stats in HUD. | Saved disassembly snippet plus HUD counter verifying vector lanes fire. |

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
| 3.1 Perf guard action | GitHub Action runs the bench, records `{mean, median, p95, p99}` + Eval/Pack/Upload. If thresholds fail, rerun once and keep the better sample; after two failures block the build. Emit warnings when mean regresses >5% vs last green commit. Always upload artifacts and link them in CI logs. Gate on Eval `p95 ≤ 0.205 ms`, warn when Pack/Upload exceed budgets. | CI badge flips red on regression; logs show artifact links and trend comparisons. |
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

- [ ] Bench logs ≤ 0.200 ms mean/max, `%slow ≤ 1%`.  
- [ ] HUD counters accurate and highlighted on breaches.  
- [ ] CI gate fails on simulated regression.  
- [ ] Importer emits drift lint on synthetic noisy data.  
- [ ] SIMD tail + event flood tests pass.  
- [ ] README/docs updated with HUD + perf guidance.
