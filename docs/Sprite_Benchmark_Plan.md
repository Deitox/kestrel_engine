
# Sprite Animation Hot-Path Optimization — **plan.md**

**Target:** ≤ **0.200 ms** for **10,000** sprite animators (≈ ≤20 ns/anim) in the `animation_targets_measure` harness.  
**Baseline:** ~**0.288 ms** (≈28.8 ns/anim).  
**Gap to close:** ~**0.088 ms** (~31% improvement).

---

## Guiding Principles

- **Data-oriented first.** Favor SoA and contiguous memory over object graphs.
- **One tight kernel.** Minimize branches and indirection in the inner loop.
- **Remove divides/mods.** Replace with multiplies by reciprocals and conditional wraps.
- **Precompute everything.** Shift work to load time and pre-bake tables.
- **Measure relentlessly.** Lock the bench harness and compare apples-to-apples.

---

## Success Criteria (Acceptance)

1. `sprite_timelines`: **≤ 0.200 ms** @ **10,000** animators on the bench machine.
2. No regressions to `transform_clips` and `skeletal_clips` beyond ±3%.
3. CPU perf improvement is **consistent across ≥3 consecutive runs** (discard first warmup).
4. Memory increase for animator state ≤ **+10%** and no additional heap churn per frame.
5. GPU upload path remains **one contiguous write** per frame for sprites (or better).

---

## Constraints & Assumptions

- Rust stable toolchain acceptable; SIMD may use `std::simd` (stable) or a crate fallback.
- Engine-wide settings can be tuned for the bench (LTO, PGO) provided gameplay builds stay reasonable.
- Sprite clips mostly **constant frame durations**; variable-frame clips supported in a slow path.
- The bench harness is authoritative: `cargo test --release animation_targets_measure -- --ignored --nocapture`.

---

## Phase 0 – Verify & Lock Measurement

**Goal:** Reproducible, noise-reduced numbers.

- [x] Capture the baseline (sprite bench + anim_stats per-step stats) with the helper script so we only store light artefacts:
  ```bash
  python scripts/capture_sprite_perf.py --label before_phase0 --runs 3
  ```
  This writes `perf/before_phase0.txt` / `.json` plus `perf/before_phase0_profile.{log,json}` for the anim_stats breakdown.
- [ ] Pin CPU governor / disable turbo if needed (Windows: High performance power plan).
- [x] Use fixed env for bench (the script above sets these automatically; manual invocation still documented for reference):
  ```powershell
  $env:ANIMATION_PROFILE_COUNT=10000
  $env:ANIMATION_PROFILE_STEPS=240
  $env:ANIMATION_PROFILE_WARMUP=16
  $env:ANIMATION_PROFILE_DT=0.016666667
  cargo test --release animation_targets_measure -- --ignored --nocapture
  ```
- [ ] When investigating regressions, capture per-step stats with anim counters enabled:
  ```powershell
  $env:ANIMATION_PROFILE_COUNT=10000
  $env:ANIMATION_PROFILE_STEPS=240
  $env:ANIMATION_PROFILE_WARMUP=16
  $env:ANIMATION_PROFILE_DT=0.016666667
  $env:ANIMATION_PROFILE_TARGET_SYSTEM="sys_drive_sprite_animations"
  cargo test --release --features anim_stats animation_profile_snapshot -- --ignored --nocapture
  ```
  The profile harness prints sprite/transform call mixes so we can see how much work hits the fast loop versus the general path.

- [x] Capture **3+ runs** and record mean & stddev before/after each phase (the script emits the per-run table automatically).

Artifacts: `perf/before_phase0.txt`, `perf/before_phase0.json`

---

## Phase 1 — ECS Specialization & Instrumentation (current work)

### 1.1 Maintain a Fast-Loop Animator Tag

**Why:** `SpriteAnimation::fast_loop` already identifies const-duration, no-event timelines. A dedicated marker component keeps that set materialized so the driver can skip per-entity branching.

**Implementation path:**
- [x] Introduce `FastSpriteAnimator` and add `sys_flag_fast_sprite_animators` that reacts to added/changed `SpriteAnimation` components.
- [x] Register the tagging system ahead of `sys_drive_sprite_animations` so the marker is up to date every frame.
- [x] Add optional debug counters (inspector or log) that report how many animators sit in the fast bucket vs. general bucket to catch regressions quickly (`anim_stats` now exposes `fast_bucket_entities`, `general_bucket_entities`, and `frame_apply_count` so HUD/CLI tooling can highlight regressions).

**Checkpoint:** `animation_profile_snapshot` should show two distinct system queries with the fast bucket representing the bulk of animators in sprite-only scenes.

---

### 1.2 Split `sys_drive_sprite_animations` by bucket

**Why:** The hot path was dominated by per-entity checks before even reaching `advance_animation_fast_loop`. Running two queries—one restricted to `FastSpriteAnimator`, one for everything else—cuts those branches and keeps cache footprints smaller.

**Implementation path:**
- [x] Add `drive_fast_single/drive_fast_fixed` that assume looped/no-event clips and never touch `EventBus`.
- [x] Gate the legacy logic behind `drive_general_single/drive_general_fixed` so events, ping-pong, and terminal behaviors remain intact without polluting the fast kernel.
- [ ] Mirror the same separation inside renderer stats/analytics so HUD overlays can highlight fast vs. general costs.

**Checkpoint:** `animation_targets_measure` should now report decreased time in `sys_drive_sprite_animations` without affecting event delivery or ping-pong coverage.

---

### 1.3 Profiling hooks & anim stats

**Why:** We already have `anim_stats` plus `tests/animation_profile.rs`; the plan relies on those tools to prove improvements and guard against regressions.

**Tasks:**
- [x] Document the workflow in README/docs (bench command is listed above; keep examples up to date) – `python scripts/capture_sprite_perf.py --label <phase> --runs 3` is now the canonical path for capturing both the averaged bench numbers and the anim_stats per-step logs.
- [ ] Capture before/after CSVs from `animation_profile_snapshot` with `ANIMATION_PROFILE_TARGET_SYSTEM="sys_drive_sprite_animations"` so the bucket split’s impact is visible.
- [ ] Add a lightweight CI check (or scheduled job) that runs the profile with `anim_stats` enabled at least once per milestone.

**Checkpoint:** Each phase delivers a short report summarizing fast/event/plain call deltas plus per-step timing variance.

---

### 1.4 Sprite frame apply queue hygiene

**Why:** The new driver only enqueues sprites whose frame actually changed; we need to keep that invariant rigid so renderer upload cost stays bounded.

**Tasks:**
- [x] Audit `SpriteFrameApplyQueue` consumers to ensure no duplicate entries and to confirm removed entities clear pending updates (queue writes are now deduplicated per-entity and verified via unit tests).
- [x] Add a debug assertion/test that verifies `frame_updates` stays empty when `animation.fast_loop` absorbed a zero-delta advance (the driver asserts the queue is drained before each update and the new test exercises the fast-path no-op).
- [x] Consider logging (behind `anim_stats`) how many sprites were applied per frame to correlate with GPU upload batches (`frame_apply_count` is exported alongside the fast/general bucket stats).

**Checkpoint:** `animation_profile_snapshot` should show the queue drain matching the number of animators that reported a frame change, and GPU traces should continue to show a single write per material batch.

---

### 1.5 Build/test parity

**Why:** The bench harness must mirror shipping settings. Today `profile.release` already uses `opt-level=3`, ThinLTO, and `codegen-units=8`, while `profile.bench` overrides them (opt=2, no LTO, 16 units), skewing results.

**Tasks:**
- [x] Align `profile.bench` with `profile.release` (copy ThinLTO, opt=3, codegen-units=8) so `cargo test --profile bench` matches local perf expectations (our `Cargo.toml` already inherits `release`, so no further work needed).
- [ ] Consider a dedicated `profile.animation` that mirrors CI release builds (or simply force `--release`) if bench builds remain divergent.
- [x] Re-document the required flags in this plan/README after the profile change lands (README + plan now point at the helper script + command line).

**Checkpoint:** Running `cargo test --release animation_targets_measure -- --ignored --nocapture` and `cargo test --profile bench …` should produce comparable numbers (≤5% skew).

---

## Phase 2 — Medium Lifts (another 10–20% possible)

_Pre-req:_ Phase 1 metrics must confirm that the fast bucket handles the majority of animators and that `animation_targets_measure` is stable. These items are data-layout heavy and should be feature-gated (`sprite_anim_soa`, etc.) so we can fall back quickly if editor tooling or hot-reload scenarios surface regressions.

### 2.1 Fixed-point time counters

**Why:** Cheaper arithmetic and simpler SIMD, fewer casts.

**Format:** `u32` 16.16 or 24.8 (choose based on needed range/precision).

**Sketch:**
```rust
const FP_SHIFT: u32 = 16;
const FP_ONE:   u32 = 1 << FP_SHIFT;

fn to_fp(x: f32) -> u32 { (x * FP_ONE as f32) as u32 }
fn from_fp(x: u32) -> f32 { (x as f32) / (FP_ONE as f32) }

accum_fp[i] += dt_fp;
let step = ((accum_fp[i] * inv_dt_fp[i]) >> FP_SHIFT) - ((prev_fp[i] * inv_dt_fp[i]) >> FP_SHIFT);
frame_idx[i] += step;
accum_fp[i] -= step * frame_dt_fp[i];     // equivalent to fmod
```
**Tasks:**
- [ ] Add feature flag: `sprite_anim_fixed_point` for A/B testing.
- [ ] Convert only the hot loop first; leave public API as f32.

**Checkpoint:** Bench both float vs fixed; keep the better for your target CPU.

---

### 2.2 4–8-wide SIMD kernel (const-dt bucket)

**Why:** SoA enables vectorized step; the const-dt path is branch-light.

**Sketch (conceptual, using `std::simd`):**
```rust
use core::simd::{Simd, Mask};

type F = Simd<f32, 8>;
type I = Simd<u32, 8>;

let a0 = F::from_slice(&accum_time[i..i+8]);
let inv = F::from_slice(&inv_dt[i..i+8]);
let a1 = a0 + F::splat(dt);
let s0 = (a0 * inv).floor();
let s1 = (a1 * inv).floor();
let step: I = (s1 - s0).cast();          // 0 or more

let mut idx = I::from_slice(&frame_idx[i..i+8]) + step;
let fc  = I::from_slice(&frame_count[i..i+8]).cast();
let wrap_mask: Mask<i32, 8> = idx.simd_ge(fc);
idx = idx - wrap_mask.to_int().select(fc, I::splat(0));

// fmod via subtracting whole frames
let a1_mod = a1 - (s1 / inv);            // a1 - floor(a1*inv)/inv
a1_mod.as_array().iter().enumerate().for_each(|(k,&v)| accum_time[i+k]=v);
idx.as_array().iter().enumerate().for_each(|(k,&v)| frame_idx[i+k]=v);
```
**Tasks:**
- [ ] Implement SIMD for the const-dt bucket only.
- [ ] Fallback to scalar when `len % 8 != 0`.
- [ ] Validate correctness with unit tests (edge wraps, ping-pong loops).

**Checkpoint:** Bench — expect ~1.2–1.5× speedup for inner loop when memory layout is clean.

---

### 2.3 Frame-cursor prefetch & next-dt cache (var-dt)

- Keep `next_dt[]` per animator to avoid re-reading `frame_dt[frame_idx+1]` on the next frame.
- Optional: software prefetch (platform-dependent; weigh complexity).

**Checkpoint:** Bench on clips with many variable frames.

---

### 2.4 Single write-combined GPU upload

- Produce a contiguous `Vec<PodSprite>` once per frame.
- Perform exactly **one** `queue.write_buffer()` for sprites (or batched by material if required).

**Tasks:**
- [ ] Transcode SoA→AoS at the end of update in a linear pass.
- [ ] Ensure buffer usage flags permit write-combine.

**Checkpoint:** GPU timing should not regress; CPU time unchanged or better.

---

## Phase 3 — Deep Refactors (use if still above target)

### 3.1 Segment-slope incremental sampling

**Idea:** For any interpolated track (if introduced), precompute slope per segment at load time. In the update loop, do `value += slope * dt` when within a segment, avoiding a second sampling pass.

**Tasks:**
- [ ] Extend clip import to produce `slope[]` per segment.
- [ ] Bend cursor-advance code to return both index and current value in one pass.

### 3.2 Workload bucketing & kernel specialization

- Bucket by clip type/length to minimize branching in kernels.
- Process buckets in stable order frame-to-frame to keep caches warm.

### 3.3 PGO (+ optional BOLT)

- Build the bench with PGO; run the harness; rebuild with the generated profile to improve layout & inlining.
- Optional: apply BOLT to the binary (advanced; measure gains).

---

## Bench Protocol & Reporting

1. **Warmup:** `ANIMATION_PROFILE_WARMUP=16`
2. **3 measured runs** per phase. Record: mean, stddev, commit hash, flags. `python scripts/capture_sprite_perf.py --label after_phase1 --runs 3` runs both the sprite bench sweep and `animation_profile_snapshot` so every phase gets averaged totals plus per-step anim_stats logs/JSON.
3. **Artifact files:**
   - `perf/before_phase0.{txt,json}` + `perf/before_phase0_profile.{log,json}`
   - `perf/after_phase1.{txt,json}` + `perf/after_phase1_profile.{log,json}`
   - `perf/after_phase2.{txt,json}` + `perf/after_phase2_profile.{log,json}`
   - `perf/final.{txt,json}` + `perf/final_profile.{log,json}`
4. **Graph (optional):** simple CSV and plot of ms vs animators (can be derived from the JSON summaries if needed).

---

## Validation & Regression Tests

- [ ] Unit tests for wrap-around: exact boundary (`dt == frame_dt`), multi-frame jumps, long `dt` spikes.
- [ ] Loop modes: clamp, loop, ping-pong (bidirectional increment/decrement correctness).
- [ ] Mixed buckets: random distribution of const/var clips.
- [ ] Fast bucket tagging: regression tests guaranteeing `FastSpriteAnimator` disappears as soon as events or ping-pong behavior is introduced.
- [ ] SpriteFrameApplyQueue stays empty when no frames change (unit test + optional `debug_assert!`).
- [ ] SoA<->AoS transcode correctness once Phase 2 feature flags are enabled (visual spot-check via a minimal sample scene).
---

## Rollback & Safety

- Fast bucket specialization (Phase 1) is the new default path; keep unit tests in place so we can detect regressions immediately.
- Gate Phase 2/3 experiments behind explicit features:
  - `sprite_anim_soa`
  - `sprite_anim_fixed_point`
  - `sprite_anim_simd`
- If a feature regresses visuals or perf on specific hardware, disable it while keeping the others so engineers can continue iterating.

---

## Task Checklist (TL;DR)

- [x] Phase 0: stabilize bench & capture baseline.
- [x] 1.1 Maintain `FastSpriteAnimator` tag + validation tests.
- [x] 1.2 Split `sys_drive_sprite_animations` into fast/general buckets.
- [x] 1.3 Document/run the anim-stats profiling workflow each time perf work lands.
- [x] 1.4 Track SpriteFrameApplyQueue churn (tests + optional counters).
- [x] 1.5 Align `profile.bench` with `profile.release` (opt3 + ThinLTO + `codegen-units=8`).
- [ ] 2.1 SoA animator storage (feature-gated).
- [ ] 2.2 Fixed-point or SIMD kernels (decide based on SoA results).
- [ ] 2.3 Prefetch/next-dt cache for var-dt.
- [ ] 2.4 Single write-combined GPU upload.
- [ ] 3.x Deep refactors if needed: slope sampling, workload bucketing, PGO/BOLT.
- [ ] Final bench & sign-off against acceptance criteria.

---

## Notes & Tips

- Bench both scalar-float and fixed-point+SIMD; keep the faster variant for your target CPUs.
- If `frame_count` is uniform for a big bucket, hoist it into a local constant to aid vectorization and reduce loads.
- Keep per-anim flags compact (bitfields) to reduce memory traffic.
- Consider `SmallVec`/static arrays for tiny, common clips to stay L1-resident.

---

**Owner:** `animation/runtime`  
**Reviewers:** `engine-core`, `rendering`, `perf`  
**Status:** Draft — ready to implement Phase 1


