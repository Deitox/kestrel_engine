use anyhow::{Context, Result};
use glam::Vec3;
use kestrel_engine::assets::skeletal::{self, SkeletonImport};
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
    assert!(curve1.rotation.is_none());
    assert!(curve1.scale.is_none());

    Ok(())
}

fn approx_vec3(actual: Vec3, expected: Vec3) {
    assert!((actual - expected).length() < 1e-4, "expected {expected:?}, got {actual:?}");
}
