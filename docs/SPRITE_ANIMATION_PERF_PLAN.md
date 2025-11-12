# Sprite Animation Performance Guard Plan

## 1. Mission & Success Criteria
- Keep the sprite timeline path at or below **0.200 ms for 10 000 animators** in release builds.
- Surface slow-path usage (var-dt, ping-pong, event-heavy clips) in-editor so asset changes stay honest.
- Fail CI whenever regressions push `sprite_timelines_mean_ms > 0.2005`.
- Provide a repeatable perf matrix (baseline/SoA/fixed-point/SIMD) with archived CSVs plus README notes.

---

## 2. Phase 1 — Hot Loop Hygiene (Kernel Audit)

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 1.1 Remove `%` / `/` from the inner loop | Inspect `advance_animation_fast_loop(_slot)` for modulo/divide ops; precompute reciprocal frame counts and rely on multiply-adds. Keep ping-pong flips as a direction bit toggled only at wrap points. | Perf inspection (cargo-asm/perf) shows no `%`/`/` in hot functions. |
| 1.2 Floor-delta fast path | Confirm constant-delta mode uses integer/fixed-point accumulators with `time_left -= dt` loops. | Bench trace shows zero `rem_euclid` or float divides when const-dt is active. |
| 1.3 Ping-pong isolation | Keep ping-pong animators in a dedicated bucket or treat direction toggles only on boundary events. | Fast bucket occupancy ≥ 99 % on reference scenes. |
| 1.4 SIMD verification | Build benches with `-C target-cpu=native` + ThinLTO; inspect disassembly to verify vector width. Optionally surface lane-utilization stats in HUD. | Saved disassembly snippet plus HUD counter verifying vector lanes fire. |

---

## 3. Phase 2 — Instrumentation & HUD

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 2.1 Runtime counters | Track per-frame counts: `const_dt`, `var_dt`, `ping_pong`, `events_heavy`, `%slow = (var_dt + ping_pong)/total`. Reset every frame; no allocations. | Telemetry resource exposes counters; unit test exercises reset logic. |
| 2.2 Stats panel wiring | Extend the Stats sidebar to show the counters, `%slow`, and AnimationTime scale. Highlight when `%slow > 1%`. | Screenshot/doc snippet demonstrating the readout. |
| 2.3 Bench harness output | Update `animation_targets_measure` to log/export the counters per run. | `target/animation_targets_report.json` includes new fields and CI artifacts capture them. |
| 2.4 GPU upload timing split | Separate “evaluation” vs “palette upload” timings so HUD shows CPU vs GPU budgets. | Stats panel renders both numbers with threshold coloring. |

---

## 4. Phase 3 — Toolchain & CI Gates

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 3.1 Perf guard action | GitHub Action step runs the bench and fails if `sprite_timelines_mean_ms > 0.2005` or `%slow > 1%`. Threshold lives behind env var for ratcheting. | CI badge flips red on synthetic regression; README references action. |
| 3.2 Bench PGO profile | Capture PGO data for the animation bench target and bake into bench profile (opt-in for shipping builds). | `cargo test --profile bench-pgo ...` instructions documented; measured delta recorded. |
| 3.3 Fixed-point default | Keep `sprite_anim_fixed_point` enabled in production profiles while allowing opt-out for diagnostics. | Cargo profiles updated; documentation explains the default. |

---

## 5. Phase 4 — Asset & Importer Guardrails

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 4.1 Timeline drift lint | During Aseprite import, detect “uniform” timelines that drift > 1 tick and warn (CLI + inspector toast). | Importer emits warning; unit test feeds noisy data and expects lint. |
| 4.2 Event batching | Buffer per-animator events during evaluation and flush in one linear pass after the hot loop. | Bench shows zero extra branching; event regression tests still pass. |
| 4.3 Clip metadata | Store “fast-path eligible” metadata on clips and surface it in the inspector. | Inspector shows eligibility flag; docs updated. |

---

## 6. Phase 5 — Runtime Stability & Risk Tests

| Risk | Mitigation | Validation |
| --- | --- | --- |
| Animator re-bucketing churn | Maintain per-bucket slabs with swap-remove semantics when clips/flags change. | Stress test toggling clips randomly; frame-time variance stays flat. |
| SIMD tail drift | Add unit test running `len % lane_width` animators through SIMD + scalar paths and comparing outputs. | New test under `tests/` ensures parity. |
| Warmup & denormals | Keep warmup frames in benches; enforce FTZ/DAZ via compiler flags. | Bench logs FTZ status; docs explain rationale. |
| Frame-event spikes | Rate-limit per-frame event emission; log when cap hits. | Counter + warning when rate limit triggers; tests cover multi-event frames. |

---

## 7. Phase 6 — Bench Matrix & Documentation

| Task | Details | Exit Criteria |
| --- | --- | --- |
| 6.1 Bench matrix automation | Script 3-run sweeps for baseline, SoA fast path, fixed-point, SIMD. Archive CSVs under `perf/` or `artifacts/animation/`. | Latest CSVs committed; artifact workflow uploads them. |
| 6.2 README perf section | Add “Sprite Animation Perf” section summarizing target/budget, HUD counters, bench instructions, and charts. | README updated with table/chart references. |
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

