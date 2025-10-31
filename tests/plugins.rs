use anyhow::Result;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::events::GameEvent;
use kestrel_engine::input::Input;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::plugins::{
    apply_manifest_builtin_toggles, apply_manifest_dynamic_toggles, EnginePlugin, ManifestBuiltinToggle,
    ManifestDynamicToggle, PluginContext, PluginManager,
};
use kestrel_engine::renderer::Renderer;
use kestrel_engine::time::Time;
use pollster::block_on;
use std::any::Any;
use std::fs;
use tempfile::tempdir;

fn push_event_bridge(ecs: &mut EcsWorld, event: GameEvent) {
    ecs.push_event(event);
}

#[derive(Default)]
struct CountingPlugin {
    build_calls: usize,
    update_calls: usize,
    fixed_calls: usize,
    shutdown_calls: usize,
    update_dts: Vec<f32>,
    fixed_dts: Vec<f32>,
    event_batches: Vec<usize>,
}

impl EnginePlugin for CountingPlugin {
    fn name(&self) -> &'static str {
        "counting"
    }

    fn build(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        self.build_calls += 1;
        Ok(())
    }

    fn update(&mut self, _ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        self.update_calls += 1;
        self.update_dts.push(dt);
        Ok(())
    }

    fn fixed_update(&mut self, _ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        self.fixed_calls += 1;
        self.fixed_dts.push(dt);
        Ok(())
    }

    fn on_events(&mut self, _ctx: &mut PluginContext<'_>, events: &[GameEvent]) -> Result<()> {
        self.event_batches.push(events.len());
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        self.shutdown_calls += 1;
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[derive(Default)]
struct FeaturePublishingPlugin;

impl EnginePlugin for FeaturePublishingPlugin {
    fn name(&self) -> &'static str {
        "feature_publisher"
    }

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        ctx.features_mut().register("test.feature");
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[test]
fn plugins_receive_lifecycle_hooks() {
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();

    {
        let mut ctx = PluginContext::new(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            push_event_bridge,
            manager.feature_handle(),
            None,
        );
        manager
            .register(Box::new(CountingPlugin::default()), &mut ctx)
            .expect("plugin registration succeeds");
    }

    {
        let mut ctx = PluginContext::new(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            push_event_bridge,
            manager.feature_handle(),
            None,
        );
        manager.update(&mut ctx, 0.5);
    }

    {
        let mut ctx = PluginContext::new(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            push_event_bridge,
            manager.feature_handle(),
            None,
        );
        manager.fixed_update(&mut ctx, 1.0 / 60.0);
    }

    let events = vec![
        GameEvent::ScriptMessage { message: "hello".to_string() },
        GameEvent::ScriptMessage { message: "world".to_string() },
    ];
    {
        let mut ctx = PluginContext::new(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            push_event_bridge,
            manager.feature_handle(),
            None,
        );
        manager.handle_events(&mut ctx, &events);
    }

    {
        let mut ctx = PluginContext::new(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            push_event_bridge,
            manager.feature_handle(),
            None,
        );
        manager.shutdown(&mut ctx);
    }

    let plugin = manager.get::<CountingPlugin>().expect("plugin stored");
    assert_eq!(plugin.build_calls, 1);
    assert_eq!(plugin.update_calls, 1);
    assert_eq!(plugin.fixed_calls, 1);
    assert_eq!(plugin.shutdown_calls, 1);
    assert_eq!(plugin.update_dts, vec![0.5]);
    assert_eq!(plugin.event_batches, vec![2]);
}

#[test]
fn plugins_can_publish_features() {
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();

    {
        let mut ctx = PluginContext::new(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            push_event_bridge,
            manager.feature_handle(),
            None,
        );
        manager
            .register(Box::new(FeaturePublishingPlugin::default()), &mut ctx)
            .expect("feature plugin registers");
    }

    let features: Vec<String> = manager.feature_handle().borrow().all().cloned().collect();
    assert!(
        features.iter().any(|feature| feature == "test.feature"),
        "feature registry tracks plugin-provided capabilities"
    );
}

#[test]
fn manifest_toggle_updates_and_persists() {
    let dir = tempdir().expect("temp dir created");
    let manifest_path = dir.path().join("plugins.json");
    let manifest_json = r#"
{
  "disable_builtins": [],
  "plugins": [
    { "name": "alpha", "path": "alpha.dll", "enabled": true },
    { "name": "beta", "path": "beta.dll", "enabled": false }
  ]
}
"#;
    fs::write(&manifest_path, manifest_json).expect("manifest written");

    let mut manifest =
        PluginManager::load_manifest(&manifest_path).expect("manifest read").expect("manifest present");
    let toggles = vec![
        ManifestDynamicToggle { name: "alpha".to_string(), new_enabled: false },
        ManifestDynamicToggle { name: "beta".to_string(), new_enabled: true },
        ManifestDynamicToggle { name: "beta".to_string(), new_enabled: true },
    ];

    let outcome = apply_manifest_dynamic_toggles(&mut manifest, &toggles);
    assert!(outcome.changed, "changes are detected");
    assert_eq!(outcome.enabled, vec!["beta".to_string()], "beta enabled list");
    assert_eq!(outcome.disabled, vec!["alpha".to_string()], "alpha disabled list");
    assert!(outcome.missing.is_empty(), "no missing entries reported");

    let states: Vec<(String, bool)> =
        manifest.entries().iter().map(|entry| (entry.name.clone(), entry.enabled)).collect();
    assert_eq!(
        states,
        vec![("alpha".to_string(), false), ("beta".to_string(), true)],
        "manifest entries updated in-memory"
    );

    manifest.save().expect("manifest saved");
    let reloaded =
        PluginManager::load_manifest(&manifest_path).expect("reload ok").expect("manifest still present");
    let persisted: Vec<(String, bool)> =
        reloaded.entries().iter().map(|entry| (entry.name.clone(), entry.enabled)).collect();
    assert_eq!(
        persisted,
        vec![("alpha".to_string(), false), ("beta".to_string(), true)],
        "manifest persisted new states"
    );
}

#[test]
fn manifest_toggle_reports_missing_entries() {
    let dir = tempdir().expect("temp dir created");
    let manifest_path = dir.path().join("plugins.json");
    let manifest_json = r#"
{
  "disable_builtins": [],
  "plugins": [
    { "name": "alpha", "path": "alpha.dll", "enabled": true }
  ]
}
"#;
    fs::write(&manifest_path, manifest_json).expect("manifest written");

    let mut manifest =
        PluginManager::load_manifest(&manifest_path).expect("manifest read").expect("manifest present");
    let toggles = vec![ManifestDynamicToggle { name: "ghost".to_string(), new_enabled: true }];
    let outcome = apply_manifest_dynamic_toggles(&mut manifest, &toggles);
    assert!(!outcome.changed, "no changes applied");
    assert_eq!(outcome.enabled, Vec::<String>::new(), "no enabled entries reported");
    assert_eq!(outcome.disabled, Vec::<String>::new(), "no disabled entries reported");
    assert_eq!(outcome.missing, vec!["ghost".to_string()], "missing entry is reported");
}

#[test]
fn manifest_builtin_toggle_updates_disable_list() {
    let dir = tempdir().expect("temp dir created");
    let manifest_path = dir.path().join("plugins.json");
    let manifest_json = r#"
{
  "disable_builtins": ["audio"],
  "plugins": []
}
"#;
    fs::write(&manifest_path, manifest_json).expect("manifest written");

    let mut manifest =
        PluginManager::load_manifest(&manifest_path).expect("manifest read").expect("manifest present");

    let toggles = vec![
        ManifestBuiltinToggle { name: "audio".to_string(), disable: false },
        ManifestBuiltinToggle { name: "analytics".to_string(), disable: true },
    ];
    let outcome = apply_manifest_builtin_toggles(&mut manifest, &toggles);
    assert!(outcome.changed, "changes detected for built-ins");
    assert_eq!(outcome.enabled, vec!["audio".to_string()], "audio re-enabled");
    assert_eq!(outcome.disabled, vec!["analytics".to_string()], "analytics disabled");

    manifest.save().expect("manifest saved");
    let reloaded =
        PluginManager::load_manifest(&manifest_path).expect("reload ok").expect("manifest present");
    assert!(!reloaded.is_builtin_disabled("audio"), "audio should be removed from disable list");
    assert!(reloaded.is_builtin_disabled("analytics"), "analytics should be present in disable list");
}
