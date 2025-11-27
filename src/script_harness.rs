use std::collections::HashMap;
use std::fs::File;
use std::path::Path;

use anyhow::{Context, Result};
use bevy_ecs::prelude::Entity;
use glam::{Vec2, Vec4};
use pollster::block_on;
use serde::{Deserialize, Serialize};

use crate::assets::AssetManager;
use crate::config::WindowConfig;
use crate::ecs::{EcsWorld, Tint, Transform, Velocity};
use crate::events::GameEvent;
use crate::environment::EnvironmentRegistry;
use crate::input::Input;
use crate::material_registry::MaterialRegistry;
use crate::mesh_registry::MeshRegistry;
use crate::plugins::{CapabilityTrackerHandle, EnginePlugin, FeatureRegistryHandle, PluginContext};
use crate::renderer::Renderer;
use crate::scripts::{ScriptBehaviour, ScriptCommand, ScriptHandle, ScriptPlugin};
use crate::time::Time;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HarnessFixture {
    #[serde(default = "default_main_script")]
    pub main_script: String,
    #[serde(default = "default_steps")]
    pub steps: usize,
    #[serde(default = "default_dt")]
    pub dt: f32,
    #[serde(default = "default_seed")]
    pub deterministic_seed: Option<u64>,
    pub behaviours: Vec<FixtureBehaviour>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FixtureBehaviour {
    pub path: String,
    #[serde(default)]
    pub persist_state: bool,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub translation: Option<[f32; 2]>,
    #[serde(default)]
    pub scale: Option<[f32; 2]>,
    #[serde(default)]
    pub rotation: Option<f32>,
    #[serde(default)]
    pub velocity: Option<[f32; 2]>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HarnessOutput {
    pub steps: usize,
    pub dt: f32,
    pub behaviours: Vec<String>,
    pub results: Vec<StepResult>,
    pub final_entities: Vec<EntitySummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepResult {
    pub step: usize,
    pub logs: Vec<String>,
    pub commands: Vec<CommandSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandSummary {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<ScriptHandle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entity: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub atlas: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefab: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<[f32; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EntitySummary {
    pub entity: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub translation: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub velocity: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tint: Option<[f32; 4]>,
}

pub fn run_fixture(fixture: &HarnessFixture) -> Result<HarnessOutput> {
    let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
    let mut ecs = EcsWorld::new();
    let mut assets = AssetManager::new();
    let mut input = Input::new();
    let mut material_registry = MaterialRegistry::new();
    let mut mesh_registry = MeshRegistry::new(&mut material_registry);
    let mut environment_registry = EnvironmentRegistry::new();
    let time = Time::new();

    let mut plugin = ScriptPlugin::new(&fixture.main_script);
    if let Some(seed) = fixture.deterministic_seed {
        plugin.enable_deterministic_mode(seed);
    }

    let mut labels: HashMap<Entity, String> = HashMap::new();
    for (idx, behaviour) in fixture.behaviours.iter().enumerate() {
        let mut transform = Transform::default();
        if let Some([x, y]) = behaviour.translation {
            transform.translation = Vec2::new(x, y);
        }
        if let Some([sx, sy]) = behaviour.scale {
            transform.scale = Vec2::new(sx, sy);
        }
        if let Some(rot) = behaviour.rotation {
            transform.rotation = rot;
        }
        let mut entity_builder = ecs.world.spawn((transform, ScriptBehaviour::with_persistence(
            behaviour.path.clone(),
            behaviour.persist_state,
        )));
        if let Some([vx, vy]) = behaviour.velocity {
            entity_builder.insert(Velocity(Vec2::new(vx, vy)));
        }
        let entity = entity_builder.id();
        let label = behaviour
            .name
            .clone()
            .unwrap_or_else(|| format!("entity{idx}"));
        labels.insert(entity, label);
    }

    let feature_registry = FeatureRegistryHandle::isolated();
    let capability_tracker = CapabilityTrackerHandle::isolated();
    let mut handle_map: HashMap<ScriptHandle, Entity> = HashMap::new();
    let mut results = Vec::with_capacity(fixture.steps);

    for step in 0..fixture.steps {
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
        plugin.update(&mut ctx, fixture.dt).with_context(|| format!("running step {step}"))?;
        let logs = plugin.take_logs();
        let commands = plugin.take_commands();
        let summaries = commands.iter().map(summarize_command).collect::<Vec<_>>();
        apply_commands(&commands, &mut ecs, &mut handle_map);
        results.push(StepResult { step, logs, commands: summaries });
    }

    let mut final_entities = collect_entities(&mut ecs, &labels);
    final_entities.sort_by(|a, b| {
        a.name
            .as_ref()
            .map(|n| n.as_str())
            .cmp(&b.name.as_ref().map(|n| n.as_str()))
            .then_with(|| a.entity.cmp(&b.entity))
    });

    let behaviours = fixture.behaviours.iter().map(|b| b.path.clone()).collect();
    Ok(HarnessOutput { steps: fixture.steps, dt: fixture.dt, behaviours, results, final_entities })
}

pub fn load_fixture<P: AsRef<Path>>(path: P) -> Result<HarnessFixture> {
    let file = File::open(path.as_ref()).with_context(|| format!("opening fixture '{}'", path.as_ref().display()))?;
    Ok(serde_json::from_reader(file).with_context(|| "parsing fixture JSON")?)
}

fn collect_entities(ecs: &mut EcsWorld, labels: &HashMap<Entity, String>) -> Vec<EntitySummary> {
    let mut query = ecs.world.query::<(Entity, Option<&Transform>, Option<&Velocity>, Option<&Tint>)>();
    let mut out = Vec::new();
    for (entity, transform, velocity, tint) in query.iter(&ecs.world) {
        let Some(t) = transform else { continue };
        out.push(EntitySummary {
            entity: entity.to_bits(),
            name: labels.get(&entity).cloned(),
            translation: [t.translation.x, t.translation.y],
            rotation: t.rotation,
            scale: [t.scale.x, t.scale.y],
            velocity: velocity.map(|v| [v.0.x, v.0.y]),
            tint: tint.map(|tint| [tint.0.x, tint.0.y, tint.0.z, tint.0.w]),
        });
    }
    out
}

fn apply_commands(commands: &[ScriptCommand], ecs: &mut EcsWorld, handles: &mut HashMap<ScriptHandle, Entity>) {
    for cmd in commands {
        match cmd {
            ScriptCommand::Spawn { handle, position, scale, velocity, .. } => {
                let entity = ecs
                    .world
                    .spawn((Transform { translation: *position, rotation: 0.0, scale: Vec2::splat(*scale) },
                            Velocity(*velocity)))
                    .id();
                handles.insert(*handle, entity);
            }
            ScriptCommand::SetPosition { handle, position } => {
                if let Some(entity) = handles.get(handle).copied() {
                    set_position(ecs, entity, *position);
                }
            }
            ScriptCommand::SetRotation { handle, rotation } => {
                if let Some(entity) = handles.get(handle).copied() {
                    set_rotation(ecs, entity, *rotation);
                }
            }
            ScriptCommand::SetScale { handle, scale } => {
                if let Some(entity) = handles.get(handle).copied() {
                    set_scale(ecs, entity, *scale);
                }
            }
            ScriptCommand::SetVelocity { handle, velocity } => {
                if let Some(entity) = handles.get(handle).copied() {
                    set_velocity(ecs, entity, *velocity);
                }
            }
            ScriptCommand::SetTint { handle, tint } => {
                if let Some(entity) = handles.get(handle).copied() {
                    set_tint(ecs, entity, *tint);
                }
            }
            ScriptCommand::Despawn { handle } => {
                if let Some(entity) = handles.remove(handle) {
                    let _ = ecs.world.despawn(entity);
                }
            }
            ScriptCommand::EntitySetPosition { entity, position } => set_position(ecs, *entity, *position),
            ScriptCommand::EntitySetRotation { entity, rotation } => set_rotation(ecs, *entity, *rotation),
            ScriptCommand::EntitySetScale { entity, scale } => set_scale(ecs, *entity, *scale),
            ScriptCommand::EntitySetVelocity { entity, velocity } => set_velocity(ecs, *entity, *velocity),
            ScriptCommand::EntitySetTint { entity, tint } => set_tint(ecs, *entity, *tint),
            ScriptCommand::EntityDespawn { entity } => {
                let _ = ecs.world.despawn(*entity);
            }
            _ => {}
        }
    }
    handles.retain(|_, entity| ecs.world.get_entity(*entity).is_ok());
}

fn set_position(ecs: &mut EcsWorld, entity: Entity, position: Vec2) {
    ensure_transform(ecs, entity);
    if let Some(mut t) = ecs.world.get_mut::<Transform>(entity) {
        t.translation = position;
    }
}

fn set_rotation(ecs: &mut EcsWorld, entity: Entity, rotation: f32) {
    ensure_transform(ecs, entity);
    if let Some(mut t) = ecs.world.get_mut::<Transform>(entity) {
        t.rotation = rotation;
    }
}

fn set_scale(ecs: &mut EcsWorld, entity: Entity, scale: Vec2) {
    ensure_transform(ecs, entity);
    if let Some(mut t) = ecs.world.get_mut::<Transform>(entity) {
        t.scale = scale;
    }
}

fn set_velocity(ecs: &mut EcsWorld, entity: Entity, velocity: Vec2) {
    if ecs.world.get::<Velocity>(entity).is_none() {
        if let Ok(mut e) = ecs.world.get_entity_mut(entity) {
            e.insert(Velocity(velocity));
            return;
        }
    }
    if let Some(mut v) = ecs.world.get_mut::<Velocity>(entity) {
        v.0 = velocity;
    }
}

fn set_tint(ecs: &mut EcsWorld, entity: Entity, tint: Option<Vec4>) {
    match tint {
        Some(color) => {
            if ecs.world.get::<Tint>(entity).is_none() {
                if let Ok(mut e) = ecs.world.get_entity_mut(entity) {
                    e.insert(Tint(color));
                    return;
                }
            }
            if let Some(mut t) = ecs.world.get_mut::<Tint>(entity) {
                t.0 = color;
            }
        }
        None => {
            if let Ok(mut e) = ecs.world.get_entity_mut(entity) {
                e.remove::<Tint>();
            }
        }
    }
}

fn ensure_transform(ecs: &mut EcsWorld, entity: Entity) {
    if ecs.world.get::<Transform>(entity).is_none() {
        if let Ok(mut e) = ecs.world.get_entity_mut(entity) {
            e.insert(Transform::default());
        }
    }
}

fn summarize_command(cmd: &ScriptCommand) -> CommandSummary {
    use ScriptCommand::*;
    #[allow(unreachable_patterns)]
    match cmd {
        Spawn { handle, atlas, region, position, scale, velocity } => CommandSummary {
            kind: "spawn".into(),
            handle: Some(*handle),
            entity: None,
            atlas: Some(atlas.clone()),
            region: Some(region.clone()),
            template: None,
            prefab: None,
            position: Some([position.x, position.y]),
            scale: Some([*scale, *scale]),
            rotation: None,
            velocity: Some([velocity.x, velocity.y]),
            tint: None,
            details: None,
        },
        SpawnPrefab { handle, path } => CommandSummary {
            kind: "spawn_prefab".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: Some(path.clone()),
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        SpawnTemplate { handle, template } => CommandSummary {
            kind: "spawn_template".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: Some(template.clone()),
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        SetPosition { handle, position } => CommandSummary {
            kind: "set_position".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: Some([position.x, position.y]),
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        SetRotation { handle, rotation } => CommandSummary {
            kind: "set_rotation".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: Some(*rotation),
            velocity: None,
            tint: None,
            details: None,
        },
        SetScale { handle, scale } => CommandSummary {
            kind: "set_scale".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: Some([scale.x, scale.y]),
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        SetVelocity { handle, velocity } => CommandSummary {
            kind: "set_velocity".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: Some([velocity.x, velocity.y]),
            tint: None,
            details: None,
        },
        SetTint { handle, tint } => CommandSummary {
            kind: "set_tint".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: tint.map(|t| [t.x, t.y, t.z, t.w]),
            details: None,
        },
        SetSpriteRegion { handle, region } => CommandSummary {
            kind: "set_sprite_region".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: Some(region.clone()),
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        Despawn { handle } => CommandSummary {
            kind: "despawn".into(),
            handle: Some(*handle),
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        EntitySetPosition { entity, position } => CommandSummary {
            kind: "entity_set_position".into(),
            handle: None,
            entity: Some(entity.to_bits()),
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: Some([position.x, position.y]),
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        EntitySetRotation { entity, rotation } => CommandSummary {
            kind: "entity_set_rotation".into(),
            handle: None,
            entity: Some(entity.to_bits()),
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: Some(*rotation),
            velocity: None,
            tint: None,
            details: None,
        },
        EntitySetScale { entity, scale } => CommandSummary {
            kind: "entity_set_scale".into(),
            handle: None,
            entity: Some(entity.to_bits()),
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: Some([scale.x, scale.y]),
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        EntitySetTint { entity, tint } => CommandSummary {
            kind: "entity_set_tint".into(),
            handle: None,
            entity: Some(entity.to_bits()),
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: tint.map(|t| [t.x, t.y, t.z, t.w]),
            details: None,
        },
        EntitySetVelocity { entity, velocity } => CommandSummary {
            kind: "entity_set_velocity".into(),
            handle: None,
            entity: Some(entity.to_bits()),
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: Some([velocity.x, velocity.y]),
            tint: None,
            details: None,
        },
        EntityDespawn { entity } => CommandSummary {
            kind: "entity_despawn".into(),
            handle: None,
            entity: Some(entity.to_bits()),
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        SetAutoSpawnRate { rate } => CommandSummary {
            kind: "set_auto_spawn_rate".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: Some(format!("{rate}")),
        },
        SetSpawnPerPress { count } => CommandSummary {
            kind: "set_spawn_per_press".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: Some(format!("{count}")),
        },
        SetEmitterRate { rate } => CommandSummary {
            kind: "set_emitter_rate".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: Some(format!("{rate}")),
        },
        SetEmitterSpread { spread } => CommandSummary {
            kind: "set_emitter_spread".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: Some(format!("{spread}")),
        },
        SetEmitterSpeed { speed } => CommandSummary {
            kind: "set_emitter_speed".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: Some(format!("{speed}")),
        },
        SetEmitterLifetime { lifetime } => CommandSummary {
            kind: "set_emitter_lifetime".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: Some(format!("{lifetime}")),
        },
        SetEmitterStartColor { color } => CommandSummary {
            kind: "set_emitter_start_color".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: Some([color.x, color.y, color.z, color.w]),
            details: None,
        },
        SetEmitterEndColor { color } => CommandSummary {
            kind: "set_emitter_end_color".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: Some([color.x, color.y, color.z, color.w]),
            details: None,
        },
        SetEmitterStartSize { size } => CommandSummary {
            kind: "set_emitter_start_size".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: Some([*size, *size]),
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        SetEmitterEndSize { size } => CommandSummary {
            kind: "set_emitter_end_size".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: Some([*size, *size]),
            rotation: None,
            velocity: None,
            tint: None,
            details: None,
        },
        _ => CommandSummary {
            kind: "unsupported".into(),
            handle: None,
            entity: None,
            atlas: None,
            region: None,
            template: None,
            prefab: None,
            position: None,
            scale: None,
            rotation: None,
            velocity: None,
            tint: None,
            details: Some(format!("{cmd:?}")),
        },
    }
}

fn push_event_bridge(_: &mut EcsWorld, _: GameEvent) {}

fn default_dt() -> f32 {
    0.016
}

fn default_steps() -> usize {
    3
}

fn default_main_script() -> String {
    "assets/scripts/main.rhai".to_string()
}

fn default_seed() -> Option<u64> {
    Some(1)
}
