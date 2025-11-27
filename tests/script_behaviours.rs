use bevy_ecs::prelude::Entity;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::{EcsWorld, SceneEntityTag, Transform};
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
use kestrel_engine::scene::SceneEntityId;
use kestrel_engine::time::Time;
use serde_json::Value as JsonValue;
use glam::Vec4;
use pollster::block_on;
use std::fs;
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
        ScriptBehaviour {
            script_path: "assets/scripts/spinner.rhai".to_string(),
            instance_id: 42,
            persist_state: false,
            mute_errors: false,
        },
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
fn compile_errors_flag_entity_and_clear_after_fix() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let bad_behaviour = write_script(
        r#"
            fn ready(world, entity) { world.log("ready"); }
            fn process(world, entity, dt) { let =; }
        "#,
    );
    let behaviour_path = bad_behaviour.path().to_string_lossy().into_owned();

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

    // First update: compile fails, instance is not bound but entity is marked errored.
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
        plugin.update(&mut ctx, 0.016).expect("update should surface compile error");
        let _ = plugin.take_logs();
    }

    let behaviour = ecs
        .world
        .get::<ScriptBehaviour>(entity)
        .expect("behaviour should remain attached after compile error");
    assert_eq!(behaviour.instance_id, 0, "instance should not bind on compile error");
    assert!(
        plugin.last_error().is_some(),
        "compile error should populate last_error"
    );
    assert!(
        plugin.entity_has_errored_instance(entity),
        "entity should be marked errored when compile fails"
    );

    // Fix the script and ensure the error clears and the instance binds.
    fs::write(
        bad_behaviour.path(),
        r#"
            fn ready(world, entity) { world.log("ready-fixed"); }
            fn process(world, entity, dt) { world.log("process-fixed"); }
        "#,
    )
    .expect("rewrite behaviour script");

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
            feature_registry,
            None,
            capability_tracker,
        );
        plugin.update(&mut ctx, 0.016).expect("update should succeed after fixing script");
        let _ = plugin.take_logs();
    }

    let behaviour = ecs
        .world
        .get::<ScriptBehaviour>(entity)
        .expect("behaviour should remain attached after reload");
    assert_ne!(behaviour.instance_id, 0, "instance should bind after successful compile");
    assert!(
        !plugin.entity_has_errored_instance(entity),
        "entity error marker should clear after script is fixed"
    );
    assert!(plugin.last_error().is_none(), "last_error should clear after successful reload");
}

#[test]
fn runtime_errors_include_call_stacks() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn inner(world, entity) { let x = 1 / 0; }
            fn outer(world, entity) { inner(world, entity); }
            fn ready(world, entity) { }
            fn process(world, entity, dt) { outer(world, entity); }
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
    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    ecs.world.spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())));

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
        feature_registry,
        None,
        capability_tracker,
    );
    plugin.update(&mut ctx, 0.016).expect("update should surface runtime error");
    let _ = plugin.take_logs();
    let err = plugin.last_error().expect("error should be recorded with call stack");
    assert!(err.contains("Call stack:"), "call stack missing from error: {err}");
    assert!(err.contains("inner"), "expected inner() in call stack: {err}");
    assert!(err.contains("outer"), "expected outer() in call stack: {err}");
}

#[test]
fn muted_instances_suppress_global_errors() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) { }
            fn process(world, entity, dt) { let crash = 1 / 0; }
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
    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    let mut behaviour = ScriptBehaviour::new(behaviour_path.clone());
    behaviour.mute_errors = true;
    let entity = ecs.world.spawn((Transform::default(), behaviour)).id();

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
        feature_registry,
        None,
        capability_tracker,
    );
    plugin.update(&mut ctx, 0.016).expect("update should tolerate muted errors");
    let _ = plugin.take_logs();
    assert!(
        plugin.last_error().is_none(),
        "muted instances should not surface global errors, got {:?}",
        plugin.last_error()
    );
    assert!(
        plugin.entity_has_errored_instance(entity),
        "muted instance should still be marked errored"
    );
}

#[test]
fn persisted_state_roundtrips_through_scene_export_and_load() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) {
                world.state_set("count", 7);
                world.state_set("label", "hello");
            }
            fn process(world, entity, dt) { }
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
    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    ecs.world.spawn((
        Transform::default(),
        SceneEntityTag::new(SceneEntityId::new()),
        ScriptBehaviour::with_persistence(behaviour_path.clone(), true),
    ));

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
        plugin.update(&mut ctx, 0.016).expect("update should run ready");
        let _ = plugin.take_logs();
    }

    let scene = ecs.export_scene(&assets);
    let script_data = scene.entities.first().and_then(|e| e.script.as_ref()).expect("script data present");
    let persisted = script_data.persisted_state.as_ref().expect("persisted state serialized");
    assert_eq!(persisted["count"], JsonValue::from(7));
    assert_eq!(persisted["label"], JsonValue::from("hello"));

    let mut loaded = EcsWorld::new();
    loaded.load_scene(&scene, &assets).expect("scene reload");
    let mut query = loaded.world.query::<(&ScriptBehaviour, Option<&kestrel_engine::scripts::ScriptPersistedState>)>();
    let (behaviour, persisted_component) =
        query.iter(&loaded.world).next().expect("loaded script entity exists");
    assert!(behaviour.persist_state, "persist_state should round-trip");
    let persisted = persisted_component.expect("persisted component attached");
    assert_eq!(persisted.0["count"], JsonValue::from(7));
    assert_eq!(persisted.0["label"], JsonValue::from("hello"));
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

#[test]
fn behaviours_enqueue_entity_transform_commands() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) {
                world.entity_set_position(entity, 1.0, 2.0);
                world.entity_set_rotation(entity, 0.5);
                world.entity_set_scale(entity, 2.0, 3.0);
                world.entity_set_velocity(entity, 4.0, 5.0);
            }
            fn process(world, entity, dt) {
                world.entity_despawn(entity);
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

    let mut saw_position = None;
    let mut saw_rotation = None;
    let mut saw_scale = None;
    let mut saw_velocity = None;
    let mut saw_despawn = false;
    for cmd in plugin.take_commands() {
        match cmd {
            ScriptCommand::EntitySetPosition { entity: target, position } if target == entity => {
                saw_position = Some(position);
            }
            ScriptCommand::EntitySetRotation { entity: target, rotation } if target == entity => {
                saw_rotation = Some(rotation);
            }
            ScriptCommand::EntitySetScale { entity: target, scale } if target == entity => {
                saw_scale = Some(scale);
            }
            ScriptCommand::EntitySetVelocity { entity: target, velocity } if target == entity => {
                saw_velocity = Some(velocity);
            }
            ScriptCommand::EntityDespawn { entity: target } if target == entity => {
                saw_despawn = true;
            }
            _ => {}
        }
    }

    assert_eq!(saw_position, Some(glam::Vec2::new(1.0, 2.0)));
    assert_eq!(saw_rotation, Some(0.5));
    assert_eq!(saw_scale, Some(glam::Vec2::new(2.0, 3.0)));
    assert_eq!(saw_velocity, Some(glam::Vec2::new(4.0, 5.0)));
    assert!(saw_despawn, "expected entity_despawn command");
}

#[test]
fn behaviours_respect_pause_and_step_once() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            let calls = 0;
            fn ready(world, entity) { world.log("ready"); }
            fn process(world, entity, dt) { world.log("process"); }
        "#,
    );
    let behaviour_path = behaviour_script.path().to_string_lossy().into_owned();

    let mut plugin = ScriptPlugin::new(main_script.path());
    plugin.set_paused(true);
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

    // Paused without step: nothing should run and instance remains unbound.
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
        plugin.update(&mut ctx, 0.016).expect("paused update should succeed");
        let logs = plugin.take_logs();
        assert!(logs.is_empty(), "paused update should not run scripts");
        let behaviour = ecs
            .world
            .get::<ScriptBehaviour>(entity)
            .expect("behaviour should exist");
        assert_eq!(behaviour.instance_id, 0, "paused update should not create instance");
    }

    // Single step: ready + process should run once and instance should bind.
    plugin.step_once();
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
        plugin.update(&mut ctx, 0.016).expect("step update should succeed");
        let logs = plugin.take_logs();
        assert_eq!(
            logs.iter().filter(|l| l.contains("ready")).count(),
            1,
            "ready should run during step"
        );
        assert!(
            logs.iter().any(|l| l.contains("process")),
            "process should run during step"
        );
        let behaviour = ecs
            .world
            .get::<ScriptBehaviour>(entity)
            .expect("behaviour should exist");
        assert_ne!(behaviour.instance_id, 0, "step update should create instance");
    }

    // Stay paused without another step: nothing new should run.
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
            feature_registry,
            None,
            capability_tracker,
        );
        plugin.update(&mut ctx, 0.016).expect("paused update should succeed");
        let logs = plugin.take_logs();
        assert!(logs.is_empty(), "paused update after step should not run scripts");
    }
}

#[test]
fn instances_are_pruned_when_entities_change() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) { world.log("ready"); }
            fn process(world, entity, dt) { }
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
    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    let mut run_update = |plugin: &mut ScriptPlugin, ecs: &mut EcsWorld| {
        let mut ctx = PluginContext::new(
            &mut renderer,
            ecs,
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
    };

    let entity = ecs
        .world
        .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())))
        .id();
    run_update(&mut plugin, &mut ecs);
    assert_eq!(plugin.instance_count_for_test(), 1, "instance should be created after update");

    assert!(ecs.world.despawn(entity), "entity should despawn");
    run_update(&mut plugin, &mut ecs);
    assert_eq!(plugin.instance_count_for_test(), 0, "instance should be pruned after despawn");

    let entity = ecs
        .world
        .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())))
        .id();
    run_update(&mut plugin, &mut ecs);
    assert_eq!(plugin.instance_count_for_test(), 1, "instance should be recreated for new entity");

    ecs.world.entity_mut(entity).remove::<ScriptBehaviour>();
    run_update(&mut plugin, &mut ecs);
    assert_eq!(plugin.instance_count_for_test(), 0, "instance should be pruned after component removal");
}

#[test]
fn exit_is_invoked_on_cleanup() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) { world.log("ready"); }
            fn process(world, entity, dt) { }
            fn exit(world, entity) { world.log("exit:" + entity.to_string()); }
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
    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    let mut run_update = |plugin: &mut ScriptPlugin, ecs: &mut EcsWorld| {
        let mut ctx = PluginContext::new(
            &mut renderer,
            ecs,
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
        plugin.take_logs()
    };

    let entity = ecs
        .world
        .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())))
        .id();
    let _ = run_update(&mut plugin, &mut ecs); // bind instance

    assert!(ecs.world.despawn(entity), "entity should despawn");
    let logs = run_update(&mut plugin, &mut ecs);
    assert!(
        logs.iter().any(|l| l.contains("exit:")),
        "expected exit log after cleanup, got {logs:?}"
    );
    assert_eq!(plugin.instance_count_for_test(), 0, "instance should be removed after exit");
}

#[test]
fn cleanup_runs_while_paused() {
    let main_script = write_script(
        r#"
            fn init(world) { }
            fn update(world, dt) { }
        "#,
    );
    let behaviour_script = write_script(
        r#"
            fn ready(world, entity) { world.log("ready"); }
            fn process(world, entity, dt) { }
            fn exit(world, entity) { world.log("exit:" + entity.to_string()); }
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
    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();

    let entity = ecs
        .world
        .spawn((Transform::default(), ScriptBehaviour::new(behaviour_path.clone())))
        .id();

    // Bind the instance while unpaused.
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
        plugin.update(&mut ctx, 0.016).expect("initial update should bind instance");
        let _ = plugin.take_logs();
    }

    plugin.set_paused(true);
    assert!(ecs.world.despawn(entity), "entity should despawn");

    // While paused, cleanup should still prune the instance and call exit.
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
            feature_registry,
            None,
            capability_tracker,
        );
        plugin.update(&mut ctx, 0.016).expect("paused update should still cleanup instances");
        let logs = plugin.take_logs();
        assert!(
            logs.iter().any(|l| l.contains("exit:")),
            "expected exit log while paused cleanup ran, got {logs:?}"
        );
    }

    assert_eq!(
        plugin.instance_count_for_test(),
        0,
        "instance should be removed even when cleanup runs during pause"
    );
}
