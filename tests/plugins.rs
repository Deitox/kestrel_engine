use anyhow::Result;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::events::GameEvent;
use kestrel_engine::input::Input;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::plugins::{EnginePlugin, PluginContext, PluginManager};
use kestrel_engine::renderer::Renderer;
use kestrel_engine::time::Time;
use pollster::block_on;
use std::any::Any;

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
