use kestrel_engine::input::{Input, InputEvent};
use std::io::Write;
use tempfile::NamedTempFile;
use winit::keyboard::{Key, NamedKey};

#[test]
fn remapped_spawn_controls_override_defaults() {
    let mut temp = NamedTempFile::new().expect("temp input config");
    write!(temp, r#"{{"bindings":{{"spawn_burst_small":["k"],"spawn_burst_large":["g"]}}}}"#)
        .expect("write remap config");

    let mut input = Input::from_config(temp.path());

    assert!(!input.take_space_pressed(), "no events yet");
    assert!(!input.take_b_pressed(), "no events yet");

    input.push(InputEvent::Key { key: Key::Character("k".into()), pressed: true });
    assert!(input.take_space_pressed(), "custom key triggers the small spawn action");

    input.push(InputEvent::Key { key: Key::Named(NamedKey::Space), pressed: true });
    assert!(!input.take_space_pressed(), "default key should no longer fire when remapped");

    input.push(InputEvent::Key { key: Key::Character("g".into()), pressed: true });
    assert!(input.take_b_pressed(), "custom key triggers the large spawn action");

    input.push(InputEvent::Key { key: Key::Character("b".into()), pressed: true });
    assert!(!input.take_b_pressed(), "original binding is ignored after remapping");
}
