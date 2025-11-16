use anyhow::Result;
use kestrel_engine::analytics::AnalyticsPlugin;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::events::GameEvent;
use kestrel_engine::input::Input;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::plugin_rpc::RpcAssetReadbackPayload;
use kestrel_engine::plugins::{
    apply_manifest_builtin_toggles, apply_manifest_dynamic_toggles, EnginePlugin, ManifestBuiltinToggle,
    ManifestDynamicToggle, PluginCapability, PluginContext, PluginManager, PluginState,
};
use kestrel_engine::renderer::Renderer;
use kestrel_engine::time::Time;
use pollster::block_on;
use serde_json::json;
use std::any::Any;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
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
struct PanickingPlugin {
    update_calls: usize,
}

impl EnginePlugin for PanickingPlugin {
    fn name(&self) -> &'static str {
        "panicker"
    }

    fn update(&mut self, _ctx: &mut PluginContext<'_>, _dt: f32) -> Result<()> {
        self.update_calls += 1;
        panic!("intentional panicking plugin");
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

#[derive(Default)]
struct RendererAccessPlugin;

impl EnginePlugin for RendererAccessPlugin {
    fn name(&self) -> &'static str {
        "renderer_access"
    }

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        ctx.renderer_mut()?.mark_shadow_settings_dirty();
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
struct UnauthorizedRendererPlugin;

impl EnginePlugin for UnauthorizedRendererPlugin {
    fn name(&self) -> &'static str {
        "unauthorized_renderer"
    }

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        let _ = ctx.renderer_mut();
        Ok(())
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, _dt: f32) -> Result<()> {
        let _ = ctx.renderer_mut();
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
            manager.capability_tracker_handle(),
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
            manager.capability_tracker_handle(),
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
            manager.capability_tracker_handle(),
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
            manager.capability_tracker_handle(),
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
            manager.capability_tracker_handle(),
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
            manager.capability_tracker_handle(),
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
fn capability_gating_blocks_unlisted_access() {
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();

    let err = {
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
            manager.capability_tracker_handle(),
        );
        manager
            .register_with_capabilities(
                Box::new(RendererAccessPlugin::default()),
                Vec::new(),
                vec![PluginCapability::Ecs],
                &mut ctx,
            )
            .expect_err("renderer capability should be required")
    };
    let message = format!("{err:?}");
    assert!(message.contains("renderer"), "error should mention missing renderer capability: {message}");
    let metrics = manager.capability_metrics();
    let log = metrics.get("renderer_access").expect("violation log exists");
    assert_eq!(log.count, 1, "violation count recorded");
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

#[test]
fn isolated_plugin_emits_script_message_via_rpc() {
    let plugin_path = build_example_dynamic_plugin();
    let manifest_dir = tempdir().expect("temp manifest dir");
    let manifest_path = manifest_dir.path().join("plugins.json");
    let manifest_json = json!({
        "disable_builtins": [],
        "plugins": [{
            "name": "example_dynamic",
            "path": plugin_path.to_string_lossy(),
            "enabled": true,
            "version": "0.1.0",
            "requires_features": [],
            "provides_features": [],
            "capabilities": ["renderer","ecs","assets","input","events","time"],
            "trust": "isolated"
        }]
    });
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest_json).unwrap())
        .expect("manifest written");
    let manifest =
        PluginManager::load_manifest(&manifest_path).expect("manifest read").expect("manifest present");

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
            manager.capability_tracker_handle(),
        );

        let loaded = manager.load_dynamic_from_manifest(&manifest, &mut ctx).expect("dynamic manifest loads");
        assert_eq!(loaded, vec!["example_dynamic"]);

        manager.update(&mut ctx, 1.1);
    }
    let events = ecs.drain_events();
    assert!(
        events.iter().any(
            |event| matches!(event, GameEvent::ScriptMessage { message } if message.contains("heartbeat"))
        ),
        "isolated plugin should emit a heartbeat script message, got {events:?}"
    );

    let unused_entity = ecs.world.spawn_empty().id();
    let query =
        manager.query_isolated_entity_info("example_dynamic", unused_entity).expect("query entity info");
    assert!(query.is_none(), "stub host should not report entity snapshots: {query:?}");

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
        manager.capability_tracker_handle(),
    );
    manager.shutdown(&mut ctx);
}

#[test]
fn capability_violations_emit_events() {
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();
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
        manager.capability_tracker_handle(),
    );

    manager
        .register_with_capabilities(
            Box::new(UnauthorizedRendererPlugin::default()),
            Vec::new(),
            vec![PluginCapability::Ecs, PluginCapability::Assets],
            &mut ctx,
        )
        .expect("registration succeeds");

    let initial_events = manager.drain_capability_events();
    assert!(
        initial_events.iter().any(|event| event.plugin == "unauthorized_renderer"),
        "build should log violation"
    );

    manager.update(&mut ctx, 0.016);
    let update_events = manager.drain_capability_events();
    assert_eq!(update_events.len(), 1, "exactly one violation emitted during update");
    let event = &update_events[0];
    assert_eq!(event.plugin, "unauthorized_renderer");
    assert!(matches!(event.capability, PluginCapability::Renderer));

    let metrics = manager.capability_metrics();
    let log = metrics.get("unauthorized_renderer").expect("metric present");
    assert_eq!(log.count, 2, "build + update should record two violations");
    assert!(log.last_timestamp.is_some(), "last timestamp captured");

    manager.shutdown(&mut ctx);
}

#[test]
fn isolated_asset_readback_roundtrip() {
    let plugin_path = build_example_dynamic_plugin();
    let manifest_dir = tempdir().expect("temp manifest dir");
    let manifest_path = manifest_dir.path().join("plugins.json");
    let manifest_json = json!({
        "disable_builtins": [],
        "plugins": [{
            "name": "example_dynamic",
            "path": plugin_path.to_string_lossy(),
            "enabled": true,
            "version": "0.1.0",
            "requires_features": [],
            "provides_features": [],
            "capabilities": ["renderer","ecs","assets","input","events","time"],
            "trust": "isolated"
        }]
    });
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest_json).unwrap())
        .expect("manifest written");
    let manifest =
        PluginManager::load_manifest(&manifest_path).expect("manifest read").expect("manifest present");

    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();
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
        manager.capability_tracker_handle(),
    );

    let loaded = manager.load_dynamic_from_manifest(&manifest, &mut ctx).expect("manifest loads");
    assert_eq!(loaded, vec!["example_dynamic"]);

    let blob_path = std::env::current_dir().expect("cwd").join("assets").join("images").join("atlas.png");
    assert!(blob_path.exists(), "atlas.png must exist for blob readback");
    let payload = RpcAssetReadbackPayload::BlobRange {
        blob_id: blob_path.to_string_lossy().to_string(),
        offset: 0,
        length: 64,
    };
    let response = manager.asset_readback("example_dynamic", payload).expect("blob readback succeeds");
    assert_eq!(response.content_type, "application/octet-stream");
    assert!(response.byte_length > 0);
    assert!(response.metadata_json.is_none(), "blob readback should not include metadata");

    manager.shutdown(&mut ctx);
}

#[test]
fn isolated_asset_readback_budget_is_enforced() {
    let plugin_path = build_example_dynamic_plugin();
    let manifest_dir = tempdir().expect("temp manifest dir");
    let manifest_path = manifest_dir.path().join("plugins.json");
    let manifest_json = json!({
        "disable_builtins": [],
        "plugins": [{
            "name": "example_dynamic",
            "path": plugin_path.to_string_lossy(),
            "enabled": true,
            "version": "0.1.0",
            "requires_features": [],
            "provides_features": [],
            "capabilities": ["renderer","ecs","assets","input","events","time"],
            "trust": "isolated"
        }]
    });
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest_json).unwrap())
        .expect("manifest written");
    let manifest =
        PluginManager::load_manifest(&manifest_path).expect("manifest read").expect("manifest present");

    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();
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
        manager.capability_tracker_handle(),
    );

    manager.load_dynamic_from_manifest(&manifest, &mut ctx).expect("manifest loads");

    let blob_path = std::env::current_dir().expect("cwd").join("assets").join("images").join("atlas.png");
    assert!(blob_path.exists(), "atlas.png must exist for blob readbacks");
    let blob_id = blob_path.to_string_lossy().to_string();

    for i in 0..8 {
        let payload =
            RpcAssetReadbackPayload::BlobRange { blob_id: blob_id.clone(), offset: i * 16, length: 8 };
        manager.asset_readback("example_dynamic", payload).expect("readback succeeds");
    }

    let err = manager
        .asset_readback(
            "example_dynamic",
            RpcAssetReadbackPayload::BlobRange { blob_id, offset: 1024, length: 16 },
        )
        .expect_err("budget should be exceeded");
    assert!(err.to_string().contains("budget"), "throttle error should mention budget: {err:?}");

    manager.shutdown(&mut ctx);
}

#[test]
fn isolated_plugin_telemetry_pipeline() {
    let plugin_path = build_example_dynamic_plugin();
    let manifest_dir = tempdir().expect("temp manifest dir");
    let manifest_path = manifest_dir.path().join("plugins.json");
    let manifest_json = json!({
        "disable_builtins": [],
        "plugins": [{
            "name": "example_dynamic",
            "path": plugin_path.to_string_lossy(),
            "enabled": true,
            "version": "0.1.0",
            "requires_features": [],
            "provides_features": [],
            "capabilities": ["ecs","assets","events","time"],
            "trust": "isolated"
        }]
    });
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest_json).unwrap())
        .expect("manifest written");
    let manifest =
        PluginManager::load_manifest(&manifest_path).expect("manifest read").expect("manifest present");

    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();
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
        manager.capability_tracker_handle(),
    );

    manager
        .register_with_capabilities(
            Box::new(UnauthorizedRendererPlugin::default()),
            Vec::new(),
            vec![PluginCapability::Ecs],
            &mut ctx,
        )
        .expect("builtin plugin registered");
    manager.load_dynamic_from_manifest(&manifest, &mut ctx).expect("manifest loads");
    manager.update(&mut ctx, 0.016);

    let capability_events = manager.drain_capability_events();
    assert!(
        capability_events.iter().any(|event| event.plugin == "unauthorized_renderer"),
        "capability events recorded for builtin plugin"
    );

    let blob_path = std::env::current_dir().expect("cwd").join("assets").join("images").join("atlas.png");
    assert!(blob_path.exists(), "atlas.png must exist for blob readbacks");
    let payload = RpcAssetReadbackPayload::BlobRange {
        blob_id: blob_path.to_string_lossy().to_string(),
        offset: 0,
        length: 128,
    };
    manager.asset_readback("example_dynamic", payload).expect("blob readback succeeds");
    let asset_events = manager.drain_asset_readback_events();
    assert!(
        asset_events.iter().any(|event| event.plugin == "example_dynamic"),
        "asset readback events recorded for isolated plugin"
    );

    let mut analytics = AnalyticsPlugin::default();
    analytics.record_plugin_capability_metrics(manager.capability_metrics());
    analytics.record_plugin_capability_events(capability_events.clone());
    analytics.record_plugin_asset_readbacks(asset_events.clone());

    let recorded = analytics.plugin_capability_events();
    assert_eq!(recorded.len(), capability_events.len());
    assert!(
        recorded.iter().any(|event| event.plugin == "unauthorized_renderer"),
        "analytics stored capability violation events"
    );

    let recent_readbacks = analytics.plugin_asset_readbacks();
    assert!(!recent_readbacks.is_empty(), "analytics stored recent asset readback events");

    manager.shutdown(&mut ctx);
}

#[test]
fn plugin_panic_marks_failure() {
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let mut manager = PluginManager::default();
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
        manager.capability_tracker_handle(),
    );

    manager.register(Box::new(PanickingPlugin::default()), &mut ctx).expect("register plugin");
    manager.update(&mut ctx, 0.016);
    {
        let plugin = manager.get::<PanickingPlugin>().expect("panicker present");
        assert_eq!(plugin.update_calls, 1, "panicking plugin should run exactly once");
    }
    manager.update(&mut ctx, 0.02);
    {
        let plugin = manager.get::<PanickingPlugin>().expect("panicker present");
        assert_eq!(plugin.update_calls, 1, "panicking plugin should not be scheduled after failure");
    }
    let status = manager
        .statuses()
        .iter()
        .find(|status| status.name == "panicker")
        .expect("status for panicker plugin");
    match &status.state {
        PluginState::Failed(reason) => {
            assert!(reason.contains("panicked"), "failure state should mention panic cause: {reason}");
        }
        other => panic!("expected failed status, got {other:?}"),
    }
    manager.shutdown(&mut ctx);
}

fn build_example_dynamic_plugin() -> PathBuf {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let plugin_dir = project_root.join("plugins").join("example_dynamic");
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let artifact = plugin_dir.join("target").join("debug").join(library_file_name("example_dynamic"));
    if !artifact.exists() {
        let status = Command::new(&cargo)
            .args(["build", "--offline"])
            .current_dir(&plugin_dir)
            .status()
            .expect("cargo build example_dynamic");
        assert!(status.success(), "building example_dynamic plugin failed");
        assert!(artifact.exists(), "example_dynamic plugin artifact missing at {}", artifact.display());
    }
    assert!(artifact.exists(), "example_dynamic plugin artifact missing at {}", artifact.display());
    artifact
}

fn library_file_name(name: &str) -> String {
    format!("{}{}{}", std::env::consts::DLL_PREFIX, name, std::env::consts::DLL_SUFFIX)
}
