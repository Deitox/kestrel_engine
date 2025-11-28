# Skeletal Importer Plan

## Goals
- Load GLTF skeleton data (skins + animations) into engine-managed assets.
- Provide authoring fixture (rig + clip) to validate importer and runtime.
- Keep importer deterministic, zero-allocation during runtime playback (preprocess).

## Target Asset Types
- `SkeletonAsset`
  - `name: Arc<str>`
  - `joints: Vec<SkeletonJoint>` (hierarchy and metadata)
  - `inverse_bind_matrices: Arc<[Mat4]>`
  - `root_joints: Vec<u32>`
- `SkinMeshBinding`
  - Mesh key reference
  - `skeleton: Arc<str>`
  - Per-vertex joint indices/weights buffer handles
- `SkeletalClip`
  - `name: Arc<str>`
  - `duration: f32`
  - `channels: Vec<JointCurve>` with rotation (Quat), translation (Vec3), scale (Vec3) tracks
  - `looped: bool`

`AssetManager` additions:
- HashMaps keyed by skeleton and clip ids with retain/release semantics.
- Source path tracking for hot-reload (GLTF -> skeleton/clip extraction snapshots).

## Import Pipeline
1. Use `gltf::import` to read document + buffers.
2. For each `skin`:
   - Collect joint node indices and parent relationships.
   - Resolve inverse bind matrices from accessor (fallback identity).
   - Compute rest pose transforms by sampling node local transform.
   - Emit `SkeletonJoint { name, parent, rest_local, rest_world, inverse_bind }`.
3. For each animation in document:
   - Iterate channels grouped by target node.
   - Convert translation/rotation/scale curves to per-joint keyframes (Vec3/Quat Vec3).
   - Normalize times to seconds, ensure ascending order, dedupe duplicates (reuse existing helpers where possible).
   - Skip unsupported interpolation (Cubic/Bezier) with warning; fallback to linear.
4. Emit `SkeletalClip` instances referencing skeleton id; store under `skeleton_name::clip_name` to avoid collisions.
5. For meshes using the skin:
   - Record binding metadata (`SkinMeshBinding`) to help renderer fetch joint palette and skin weights.

## Fixture Assets
- Location: `fixtures/gltf/skeletons/`
- Provide `slime_rig.gltf` with ~12 bones, one idle clip.
- Include exported animation (loop) to serve golden test reference.
- Add hashed JSON snapshot of expected skeleton/clips for regression.

## Integration Points
- `AssetManager::retain_skeleton`, `release_skeleton` similar to clip/atlas.
- CLI tool (optional) to dump skeleton summary for debugging.
- Tests under `tests/skeletal_import.rs` verifying importer output matches fixture (joint count, hierarchy, keyframe counts).

## Open Questions
- How to map GLTF node transforms into engine coordinate system (ensure consistent handedness with existing mesh loader).
- Do we bake quaternion handedness adjustments in importer or runtime evaluator?
- Level of support for multiple skins per GLTF (initially first skin only, warn for others?).

## Next Steps
- [x] Implement importer module (`src/assets/skeletal.rs`) with pure-data structs and loader entry points.
- [x] Wire into `AssetManager` (retain/load functions, hot-reload).
- [x] Author fixture GLTF + tests. *(fixtures/gltf/skeletons/slime_rig.gltf plus regression check now validate importer output.)*
- [x] Proceed with ECS component scaffolding (`SkeletonInstance`, etc.). *(Pose playback system drives `SkeletonInstance` + `BoneTransforms`; renderer now pools joint palettes for mesh and shadow passes.)*
- [x] Land golden pose tests for the slime rig fixture. *(`ecs::systems::animation` unit tests verify keyframes and loop wrapping.)*


