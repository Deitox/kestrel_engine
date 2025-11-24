use kestrel_engine::scripts::ScriptHost;
use kestrel_engine::assets::AssetManager;

#[test]
fn main_script_compiles() {
    let mut host = ScriptHost::new("assets/scripts/main.rhai");
    let assets = AssetManager::new();
    host.force_reload(Some(&assets)).expect("main.rhai should compile");
}
