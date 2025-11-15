# Keyframe Editor UX Specification

This document captures the interaction model and layout for the Milestone 5 keyframe editor so engineers and designers share a single source of truth before implementation begins.

## Objectives
- Provide an in-editor panel that lets authors inspect and edit animation tracks (sprite, transform, skeletal) with minimal context switching.
- Support both **Step** and **Linear** interpolation keys in Milestone 5, with a path to extend toward curves later.
- Ensure edits are undoable, hot-reload friendly, and reflected in the running scene without restarting playback.

## Target Users
| Persona | Needs |
|---------|-------|
| Animator / Technical Artist | Quickly tweak timing, add events, and preview without leaving the editor. |
| Gameplay Engineer | Inspect tracks tied to scripts, verify parameter changes, and debug timeline issues. |

## Panel Layout
1. **Header Bar**
   - Track selector (drop-down filtering by entity + component).
   - Playback controls (Play/Pause, Step Left/Right, Loop toggle).
   - Global time indicator (current time, total duration).
2. **Layer List (left column, resizable)**
   - Hierarchical list of tracks grouped by component (Sprite, Transform, Skeletal).
   - Icons for visibility/lock per layer.
   - Click selects the track (single) or toggles multi-select with modifiers.
3. **Timeline Canvas (right pane)**
   - Horizontal time axis with ruler + zoom controls (mouse wheel over axis).
   - Rows per selected track showing keys.
   - Keys rendered as diamonds (Linear) or squares (Step); selected keys highlighted.
   - Event markers displayed at the top of the track with tooltips.
4. **Inspector Drawer (bottom collapsible)**
   - Shows properties for the selected key(s): timestamp, value, interpolation type, tangents (future).
   - Supports multi-edit when multiple keys selected.

## Interactions
- **Selection**
  - Click key: select single.
  - Shift-click: range select within track.
  - Ctrl/Cmd-click: toggle selection membership.
- **Manipulation**
  - Drag key horizontally to change time (snaps to frame/time grid when holding Shift).
  - Drag vertically to reorder key priority (for track layering) when supported.
  - Copy/Paste keys via standard shortcuts (Ctrl+C/V).
  - Delete via Delete/Backspace or context menu.
- **Insertion**
  - Double-click on empty timeline area inserts a key at that timestamp using default interpolation.
  - Context menu (“Add Key...”) allows specifying value/interpolation at creation.
- **Scrubbing**
  - Dragging the playhead scrubber updates animation state live; scrubbing obeys AnimationTime scaling.
  - Holding Alt while scrubbing performs “preview” mode without committing state (for future extension).
- **Undo/Redo**
  - Integrated with existing editor undo stack; each edit operation enqueues a command object.

## Data & Persistence
- Editor interacts with runtime `AnimationClip`/`TransformClip` data via `AnimationLibrary` abstractions.
- Edits write to an in-memory document; save-on-edit persists to source asset and triggers the watcher pipeline.
- Dirty indicator appears in header when in-memory clip diverges from disk.

## Technical Notes
- Panel lives under `editor_ui::AnimationKeyframePanel` and ships enabled by default.
- Requires access to `AnimationTime`, selected entity context, and asset handles for clips/graphs.
- Rendering implemented via egui primitives; timeline virtualization required for >100 tracks.

## Open Questions
1. Do we support multi-track editing across different components simultaneously? (Initial scope: yes, but only for homogeneous key types.)
2. Should event markers be editable within the same panel or deferred to a separate pane? (Preferred: same panel for parity with roadmap.)
3. How do we reconcile live scrubbing with script-driven animation overrides? Needs coordination with scripting team.

## Next Steps
1. Wire up panel scaffolding + feature flag in `editor_ui.rs`.
2. Define data conversion layer between clip assets and editable key structures.
3. Build incremental functionality: selection -> manipulation -> insertion -> inspector editing -> undo/redo.
4. Add automated UI regression tests plus docs/screencasts once feature is interactive.
