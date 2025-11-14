# Animation Milestone 5 Execution Plan

This plan breaks Milestone 5 (“Tooling, Automation, and Analytics”) into concrete, ordered tasks so the team can execute the roadmap from `docs/ANIMATION_SYSTEM_ROADMAP.md` without ambiguity.

## Goals
- Deliver the editor keyframe tooling, automation hooks, analytics overlays, and documentation promised in Milestone 5.
- Keep animation authors in the loop via hot reload + tutorials, and ensure CI validates every new surface.
- Provide exit criteria for each workstream plus integration checkpoints that match the roadmap’s acceptance tests.

## Workstreams & Tasks

### 1. Keyframe Editor UX
1. Define UI/UX spec for the layer list, per-track key display, and interactive controls (`docs/keyframe_editor_spec.md`).
2. Implement the editor panel (render tracks, select/add/move/delete Step + Linear keys, live scrubbing).
3. Persist per-track selection state and expose undo/redo hooks.
4. Write focused UI automation tests (egui test harness or snapshot comparisons).
5. Update analytics logging to capture editor usage events (optional stretch).

**Exit:** Artists can open the panel, edit keys in real time, and see playback respond immediately without errors.

### 2. Asset Watchers & Validators
1. Extend the existing atlas/clip watchers to observe transform clips and animation graphs.
2. Reuse importer infrastructure to trigger schema validators on change.
3. Surface validation errors in inspector + EventBus, with actionable messages.
4. Add regression tests for watcher reload + validation flows (simulate file edits).

**Exit:** Saving a clip/graph file reloads the asset, runs validators, and reports failures deterministically.

### 3. CLI Utilities & Automation
1. Ship `animation_check` (schema + perf validator) that can target directories or manifests.
2. Ship `migrate_atlas` helper to bump schema versions / fix known issues.
3. Add roadmap checkpoint harness wiring so `animation_targets_measure` emits JSON perf captures on demand.
4. Document CLI usage and integrate into CI smoke tests.

**Exit:** CI/bots can run the new commands, and docs tell authors when to use each tool.

### 4. Analytics Overlay & HUD Counters
1. Instrument animation evaluation cost, animator/bone counts, and budget thresholds inside AnalyticsPlugin.
2. Render those counters in the in-editor HUD/status bar with color coding when budgets are exceeded.
3. Add GPU palette upload timing to the overlay as described in the roadmap.
4. Record analytics samples into perf artifacts for CI trend tracking.

**Exit:** HUD shows live metrics, turns yellow/red when limits breach, and logs are archived for regression review.

### 5. Sample Content & Fixtures
1. Build/curate scenes showcasing sprite timelines, transform tracks, skeletal rigs, and graph-driven characters.
2. Ensure each scene has deterministic capture scripts (for docs + tests).
3. Add automated scene load tests to prevent regressions in the demo content.

**Exit:** Samples live under `assets/` (or fixtures) and are referenced by docs + automated tests.

### 6. Documentation & Tutorials
1. Expand `docs/animation_workflows.md` with end-to-end authoring tutorial covering new tooling.
2. Add troubleshooting and scripting best practices sections.
3. Cross-link README + roadmap with the new plan, tools, and CI expectations.

**Exit:** Following the tutorial from a clean checkout reproduces the milestone deliverables.

## Execution Order
1. **Keyframe Editor** – unlocks UX, informs later documentation.
2. **Asset Watchers/Validators** – ensures tooling feedback loop works while UI stabilizes.
3. **CLI Utilities** – required for automation + CI gating.
4. **Analytics Overlay** – depends on instrumentation from earlier work.
5. **Sample Content** – showcases previous features and feeds docs/tests.
6. **Documentation** – final polish referencing all new systems.

## Integration & Verification
- Add targeted tests per workstream (UI snapshots, watcher reload tests, CLI command tests, HUD metric assertions, scene load/perf scripts).
- Update `.github/workflows/animation-bench.yml` to call the new CLIs + capture HUD metrics once those tasks land.
- Extend `docs/SPRITE_ANIMATION_PERF_PLAN.md` to reference HUD counters and the new CI gates.

## Milestone Exit Checklist
- [ ] Keyframe editor panel functional & tested.
- [ ] Asset watchers validate + reload clips/graphs with inspector surfacing.
- [ ] `animation_check` and `migrate_atlas` CLIs documented and running in CI.
- [ ] Analytics HUD shows CPU/GPU animation metrics with budget thresholds.
- [ ] Sample scenes/scripts checked in with automated verification.
- [ ] Tutorials + docs updated, referencing the above tooling and CI expectations.

### Read-only Panel Status
- [x] Panel surfaces live track summaries (sprite timelines + transform clips).
- [ ] Expose per-key editing interactions.
- [ ] Persist edits via watchers/validators.

