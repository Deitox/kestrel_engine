use bevy_ecs::prelude::Entity;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::{EcsWorld, Transform};
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::events::GameEvent;
use kestrel_engine::input::Input;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::plugins::{
    CapabilityTrackerHandle, EnginePlugin, FeatureRegistryHandle, PluginContext,
};
use kestrel_engine::renderer::Renderer;
use kestrel_engine::scripts::{ScriptBehaviour, ScriptCommand, ScriptPlugin};
use kestrel_engine::time::Time;
use glam::Vec4;
use pollster::block_on;
use std::io::Write;
use tempfile::NamedTempFile;

fn write_script(contents: &str) -> NamedTempFile {
    let mut temp = NamedTempFile::new().expect("temp script");
    write!(temp, "{contents}").expect("write script");
    temp
}

fn push_event_bridge(ecs: &mut EcsWorld, event: GameEvent) {
    ecs.push_event(event);
}

#[test]
fn script_behaviour_roundtrips_and_resets_instance_id() {
    let mut world = EcsWorld::new();
    world.world.spawn((
        Transform::default(),
        ScriptBehaviour { script_path: "assets/scripts/spinner.rhai".to_string(), instance_id: 42 },
    ));
    let assets = AssetManager::new();

    let scene = world.export_scene(&assets);
    assert_eq!(scene.entities.len(), 1, "expected single entity in exported scene");
    let saved_script_path = scene
        .entities
        .first()
        .and_then(|entity| entity.script.as_ref())
        .map(|data| data.script_path.as_str())
        .expect("export should serialize script data");
    assert_eq!(saved_script_path, "assets/scripts/spinner.rhai");

    let mut loaded_world = EcsWorld::new();
    loaded_world.load_scene(&scene, &assets).expect("scene load should succeed");
    let mut query = loaded_world.world.query::<&ScriptBehaviour>();
    let behaviour = query
        .iter(&loaded_world.world)
        .next()
        .expect("loaded world should contain ScriptBehaviour");
    assert_eq!(behaviour.script_path.as_str(), "assets/scripts/spinner.rhai");
    assert_eq!(behaviour.instance_id, 0, "instance id should reset during load");
}

#[test]
fn behaviours_create_instances_and_run_lifecycle() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) {
                world.log("ready:" + entity.to_string());
            }
            fn process(world, entity, dt) {
                world.log("process:" + dt.to_string());
            }
        "#,
    );
    let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();

    let mut plugin = ScriptPlugin::new(main_script.path());
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();

    let entity = ecs
        .world
        .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())))
        .id();

    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    let mut ready_logs = 0usize;
    let mut process_logs = 0usize;
    for _ in 0..2 {
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
            feature_registry.clone(),
            None,
            capability_tracker.clone(),
        );
        plugin.update(&mut ctx, 0.016).expect("script update should succeed");
        let logs = plugin.take_logs();
        ready_logs += logs.iter().filter(|line| line.contains("ready:")).count();
        process_logs += logs.iter().filter(|line| line.contains("process:")).count();
    }

    let behaviour = ecs
        .world
        .get::<ScriptBehaviour>(entity)
        .expect("behaviour should stay attached");
    assert_ne!(behaviour.instance_id, 0, "instance id should be assigned after update");
    assert_eq!(behaviour.script_path, behaviour_path);
    assert_eq!(ready_logs, 1, "ready should run exactly once");
    assert!(
        process_logs >= 2,
        "process should run on each update call (saw {process_logs})"
    );
}

#[test]
fn behaviours_run_physics_process_on_fixed_update() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) { world.log("ready:" + entity.to_string()); }
            fn physics_process(world, entity, dt) { world.log("physics:" + dt.to_string()); }
        "#,
    );
    let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();

    let mut plugin = ScriptPlugin::new(main_script.path());
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();

    ecs.world.spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())));

    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    let mut ready_logs = 0usize;
    let mut physics_logs = 0usize;
    for _ in 0..2 {
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
            feature_registry.clone(),
            None,
            capability_tracker.clone(),
        );
        plugin.fixed_update(&mut ctx, 0.02).expect("fixed update should succeed");
        let logs = plugin.take_logs();
        ready_logs += logs.iter().filter(|line| line.contains("ready:")).count();
        physics_logs += logs.iter().filter(|line| line.contains("physics:")).count();
    }

    assert_eq!(ready_logs, 1, "ready should run once before physics loop");
    assert!(physics_logs >= 2, "physics_process should run each fixed_update (saw {physics_logs})");
}

#[test]
fn behaviour_errors_stop_further_calls() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) { world.log("ready"); }
            fn process(world, entity, dt) { call_unknown(); }
        "#,
    );
    let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();

    let mut plugin = ScriptPlugin::new(main_script.path());
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();

    ecs.world.spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())));

    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    // First update: ready runs, process errors.
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
            feature_registry.clone(),
            None,
            capability_tracker.clone(),
        );
        plugin.update(&mut ctx, 0.016).expect("update should not panic on script error");
        let logs = plugin.take_logs();
        assert_eq!(logs.iter().filter(|l| l.contains("ready")).count(), 1, "ready should run once");
        assert!(
            plugin.last_error().is_some(),
            "script error should surface in last_error after failing process"
        );
    }

    // Second update: errored instance should skip further calls.
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
            feature_registry.clone(),
            None,
            capability_tracker.clone(),
        );
        plugin.update(&mut ctx, 0.016).expect("update should tolerate existing error");
        let logs = plugin.take_logs();
        assert!(
            logs.is_empty(),
            "errored instance should skip callbacks; expected no further logs, got {logs:?}"
        );
    }
}

#[test]
fn asset_behaviours_run_without_global_state_errors() {
    let main_script_path = "assets/scripts/main.rhai";
    let behaviour_paths = ["assets/scripts/blinker.rhai", "assets/scripts/wanderer.rhai", "assets/scripts/spinner.rhai"];

    let mut plugin = ScriptPlugin::new(main_script_path);
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();
    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    for path in behaviour_paths {
        ecs.world.spawn((Transform::default(), ScriptBehaviour::new(path.to_string())));
    }

    // Drive update twice to allow ready + process to run.
    for _ in 0..2 {
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
            feature_registry.clone(),
            None,
            capability_tracker.clone(),
        );
        plugin.update(&mut ctx, 0.016).expect("script update should succeed");
        let _ = plugin.take_logs();
    }

    assert!(
        plugin.last_error().is_none(),
        "Asset behaviours should run without global state errors, got {:?}",
        plugin.last_error()
    );
}
#[test]
fn behaviours_enqueue_entity_tint_commands() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) { world.entity_set_tint(entity, 0.1, 0.2, 0.3, 0.4); }
            fn process(world, entity, dt) { world.entity_clear_tint(entity); }
        "#,
    );
    let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();

    let mut plugin = ScriptPlugin::new(main_script.path());
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();

    let entity = ecs
        .world
        .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())))
        .id();

    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

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
            feature_registry.clone(),
            None,
            capability_tracker.clone(),
        );
        plugin.update(&mut ctx, 0.016).expect("script update should succeed");
    }

    let tint_commands: Vec<(Entity, Option<Vec4>)> = plugin
        .take_commands()
        .into_iter()
        .filter_map(|cmd| match cmd {
            ScriptCommand::EntitySetTint { entity, tint } => Some((entity, tint)),
            _ => None,
        })
        .collect();
    assert_eq!(tint_commands.len(), 2, "expected tint set and clear commands");
    assert!(
        tint_commands.iter().any(|(target, tint)| {
            *target == entity
                && matches!(tint, Some(color) if (color.x - 0.1).abs() < 1e-4 && (color.y - 0.2).abs() < 1e-4
                    && (color.z - 0.3).abs() < 1e-4 && (color.w - 0.4).abs() < 1e-4)
        }),
        "expected tint set command for entity {entity:?}, got {tint_commands:?}"
    );
    assert!(
        tint_commands.iter().any(|(target, tint)| *target == entity && tint.is_none()),
        "expected tint clear command for entity {entity:?}, got {tint_commands:?}"
    );
}
