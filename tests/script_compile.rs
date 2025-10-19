use kestrel_engine::scripts::ScriptHost;

#[test]
fn main_script_compiles() {
    let mut host = ScriptHost::new("assets/scripts/main.rhai");
    host.force_reload().expect("main.rhai should compile");
}
