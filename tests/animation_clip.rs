use kestrel_engine::assets::AssetManager;

#[test]
fn retain_clip_loads_fixture_tracks() {
    let mut assets = AssetManager::new();
    assets.retain_clip("slime", Some("fixtures/animation_clips/slime_bob.json")).expect("load clip fixture");

    let clip = assets.clip("slime").expect("clip present");
    assert_eq!(clip.name.as_ref(), "slime_bob");
    assert!(clip.looped, "fixture clip should be marked looping");
    assert!((clip.duration - 0.5).abs() < f32::EPSILON, "duration should match last keyframe time");

    let translation = clip.translation.as_ref().expect("translation track");
    assert_eq!(translation.keyframes.len(), 3, "translation track retains all keyframes");

    let rotation = clip.rotation.as_ref().expect("rotation track");
    assert_eq!(rotation.keyframes.len(), 2);
    assert!(
        (rotation.keyframes[1].value - 6.2831855).abs() < 1e-6,
        "rotation final value should match fixture"
    );

    let tint = clip.tint.as_ref().expect("tint track");
    let tint_end = tint.keyframes.last().unwrap().value;
    assert!(
        (tint_end.x - 0.6).abs() < 1e-6 && (tint_end.y - 0.9).abs() < 1e-6,
        "tint keyframes should preserve values"
    );
}
