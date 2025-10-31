use anyhow::{Context, Result};
use kestrel_engine::assets::skeletal;
use std::path::Path;

/// Smoke test for the skeletal importer using the slime rig fixture once authored.
#[test]
#[ignore = "Requires fixtures/gltf/skeletons/slime_rig.gltf to be added"]
fn import_slime_rig_fixture() -> Result<()> {
    let path = Path::new("fixtures/gltf/skeletons/slime_rig.gltf");
    anyhow::ensure!(path.exists(), "Skeletal fixture missing at {}", path.display());

    let import = skeletal::load_skeleton_from_gltf(path)
        .with_context(|| format!("Failed to load {}", path.display()))?;

    anyhow::ensure!(!import.skeleton.joints.is_empty(), "Fixture should contain at least one joint");
    anyhow::ensure!(!import.clips.is_empty(), "Fixture should contain at least one animation clip");

    Ok(())
}
