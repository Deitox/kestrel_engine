use anyhow::{anyhow, Context, Result};
use glam::{Quat, Vec3};
use kestrel_engine::assets::skeletal::{self, SkeletonImport};
use kestrel_engine::assets::AssetManager;
use kestrel_engine::ecs::{BoneTransforms, EcsWorld};
use std::path::Path;

#[test]
fn import_slime_rig_fixture() -> Result<()> {
    let path = Path::new("fixtures/gltf/skeletons/slime_rig.gltf");
    anyhow::ensure!(path.exists(), "Fixture missing at {}", path.display());

    let SkeletonImport { skeleton, clips } = skeletal::load_skeleton_from_gltf(path)
        .with_context(|| format!("Failed to load {}", path.display()))?;

    assert_eq!(skeleton.name.as_ref(), "slime_skeleton");
    assert_eq!(skeleton.joints.len(), 2);
    assert_eq!(skeleton.roots.len(), 1);
    assert_eq!(skeleton.roots[0], 0);

    let bone0 = &skeleton.joints[0];
    assert_eq!(bone0.name.as_ref(), "bone_0");
    // Node index 1's parent is root node (index 0 in GLTF, not a joint), importer maps it to None.
    assert_eq!(bone0.parent, None);

    let bone1 = &skeleton.joints[1];
    assert_eq!(bone1.name.as_ref(), "bone_1");
    assert_eq!(bone1.parent, Some(0));

    assert_eq!(clips.len(), 1);
    let clip = &clips[0];
    assert_eq!(clip.name.as_ref(), "breath");
    assert!((clip.duration - 1.0).abs() < 1e-3);
    assert_eq!(clip.channels.len(), 2);

    let mut channels = clip.channels.as_ref().to_vec();
    channels.sort_by_key(|curve| curve.joint_index);

    let curve0 = &channels[0];
    assert_eq!(curve0.joint_index, 0);
    let track0 = curve0.translation.as_ref().expect("joint 0 translation track");
    let keys0 = track0.keyframes.as_ref();
    assert_eq!(keys0.len(), 2);
    approx_vec3(keys0[0].value, Vec3::new(0.0, 1.0, 0.0));
    approx_vec3(keys0[1].value, Vec3::new(0.0, 1.1, 0.0));
    assert!(curve0.rotation.is_none());
    assert!(curve0.scale.is_none());

    let curve1 = &channels[1];
    assert_eq!(curve1.joint_index, 1);
    let track1 = curve1.translation.as_ref().expect("joint 1 translation track");
    let keys1 = track1.keyframes.as_ref();
    assert_eq!(keys1.len(), 2);
    approx_vec3(keys1[0].value, Vec3::new(0.0, 2.0, 0.0));
    approx_vec3(keys1[1].value, Vec3::new(0.0, 2.2, 0.0));
    let rot_track = curve1.rotation.as_ref().expect("joint 1 rotation track");
    let rot_keys = rot_track.keyframes.as_ref();
    assert_eq!(rot_keys.len(), 2);
    approx_quat(rot_keys[0].value, Quat::IDENTITY);
    approx_quat(rot_keys[1].value, Quat::from_axis_angle(Vec3::Z, std::f32::consts::FRAC_PI_2));
    let scale_track = curve1.scale.as_ref().expect("joint 1 scale track");
    let scale_keys = scale_track.keyframes.as_ref();
    assert_eq!(scale_keys.len(), 2);
    approx_vec3(scale_keys[0].value, Vec3::new(1.0, 1.0, 1.0));
    approx_vec3(scale_keys[1].value, Vec3::new(1.1, 0.9, 1.0));

    Ok(())
}

fn approx_vec3(actual: Vec3, expected: Vec3) {
    assert!((actual - expected).length() < 1e-4, "expected {expected:?}, got {actual:?}");
}

fn approx_quat(actual: Quat, expected: Quat) {
    let dot = actual.normalize().dot(expected.normalize()).abs();
    assert!(dot > 1.0 - 1e-4, "expected {expected:?}, got {actual:?}");
}

#[test]
fn ecs_world_skeleton_helpers_manage_instances() -> Result<()> {
    let mut assets = AssetManager::new();
    assets
        .retain_skeleton("slime", Some("fixtures/gltf/skeletons/slime_rig.gltf"))
        .context("retain slime skeleton")?;

    let mut ecs = EcsWorld::new();
    let entity = ecs.world.spawn_empty().id();

    assert!(ecs.set_skeleton(entity, &assets, "slime"), "attach skeleton");
    assert!(
        ecs.world.get::<BoneTransforms>(entity).is_some(),
        "bone transforms should be present after attaching skeleton"
    );

    let info = ecs.entity_info(entity).ok_or_else(|| anyhow!("entity info unavailable"))?;
    let skeleton = info.skeleton.ok_or_else(|| anyhow!("skeleton info missing"))?;
    assert_eq!(skeleton.skeleton_key, "slime");
    assert!(skeleton.has_bone_transforms);
    assert_eq!(skeleton.joint_count, skeleton.palette_joint_count);

    let clip_keys = assets
        .skeletal_clip_keys_for("slime")
        .ok_or_else(|| anyhow!("clip keys missing for slime"))?;
    let clip_key = clip_keys.first().ok_or_else(|| anyhow!("no clip entries for slime"))?.clone();
    assert!(ecs.set_skeleton_clip(entity, &assets, &clip_key), "assign skeletal clip");

    assert!(ecs.set_skeleton_clip_time(entity, 0.5), "seek skeletal clip");
    let info = ecs.entity_info(entity).ok_or_else(|| anyhow!("entity info unavailable after seek"))?;
    let skeleton = info.skeleton.ok_or_else(|| anyhow!("skeleton info missing after seek"))?;
    let clip = skeleton.clip.ok_or_else(|| anyhow!("clip info missing after seek"))?;
    assert_eq!(clip.clip_key, clip_key);
    assert!(
        (clip.time - 0.5).abs() < 1e-3,
        "expected clip time near 0.5, got {}",
        clip.time
    );

    assert!(ecs.reset_skeleton_pose(entity), "reset skeletal pose");
    let reset = ecs.entity_info(entity).ok_or_else(|| anyhow!("entity info missing after reset"))?;
    let reset_clip =
        reset.skeleton.and_then(|s| s.clip).ok_or_else(|| anyhow!("clip info missing after reset"))?;
    assert!(
        reset_clip.time.abs() < 1e-4,
        "expected clip time near 0 after reset, got {}",
        reset_clip.time
    );

    assert!(ecs.clear_skeleton(entity), "clear skeleton components");
    assert!(
        ecs.world.get::<BoneTransforms>(entity).is_none(),
        "bone transforms should be removed after clearing skeleton"
    );
    let cleared = ecs.entity_info(entity).ok_or_else(|| anyhow!("entity info missing after clear"))?;
    assert!(cleared.skeleton.is_none(), "skeleton info should be cleared");

    Ok(())
}
