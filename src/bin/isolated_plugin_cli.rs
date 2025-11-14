use anyhow::{anyhow, Context, Result};
use bevy_ecs::prelude::Entity;
use kestrel_engine::assets::AssetManager;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::events::GameEvent;
use kestrel_engine::input::Input;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::plugin_rpc::RpcAssetReadbackPayload;
use kestrel_engine::plugins::{CapabilityTrackerHandle, FeatureRegistryHandle, PluginContext, PluginManager};
use kestrel_engine::renderer::Renderer;
use kestrel_engine::time::Time;
use pollster::block_on;
use std::env;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() {
    if let Err(err) = run_cli() {
        eprintln!("[isolated-cli] error: {err:?}");
        std::process::exit(1);
    }
}

fn run_cli() -> Result<()> {
    let opts = CliOptions::parse()?;
    let manifest =
        PluginManager::load_manifest(&opts.manifest)?.ok_or_else(|| anyhow!("manifest missing"))?;

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
        let feature_handle = manager.feature_handle();
        let capability_handle = manager.capability_tracker_handle();
        let mut ctx = make_context(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            feature_handle,
            capability_handle,
        );
        manager
            .load_dynamic_from_manifest(&manifest, &mut ctx)
            .with_context(|| format!("loading plugin '{}'", opts.plugin))?;
    }

    let watchdog_limit = opts.watchdog_ms.map(Duration::from_millis);
    for step in 0..opts.steps {
        let step_start = Instant::now();
        {
            let feature_handle = manager.feature_handle();
            let capability_handle = manager.capability_tracker_handle();
            let mut ctx = make_context(
                &mut renderer,
                &mut ecs,
                &mut assets,
                &mut input,
                &mut material_registry,
                &mut mesh_registry,
                &mut environment_registry,
                &time,
                feature_handle,
                capability_handle,
            );
            manager.update(&mut ctx, opts.dt);
        }
        drain_and_print_events(step, ecs.drain_events());
        if let Some(limit) = watchdog_limit {
            let elapsed = step_start.elapsed();
            if elapsed > limit {
                return Err(anyhow!(format!(
                    "watchdog tripped after {:.2} ms (limit {} ms)",
                    elapsed.as_secs_f64() * 1000.0,
                    limit.as_millis()
                )));
            }
        }
    }

    if let Some(bits) = opts.entity_bits {
        let entity = Entity::from_bits(bits);
        match manager.query_isolated_entity_info(&opts.plugin, entity)? {
            Some(info) => {
                println!(
                    "[isolated-cli] entity {} scene={} translation=({:.2},{:.2}) sprite={:?}",
                    info.entity.index(),
                    info.scene_id,
                    info.translation[0],
                    info.translation[1],
                    info.sprite
                        .as_ref()
                        .map(|sprite| format!("{}::{}", sprite.atlas, sprite.region))
                        .unwrap_or_else(|| "none".to_string())
                );
            }
            None => println!("[isolated-cli] entity {} has no snapshot in isolated host", entity.index()),
        }
    }

    if !opts.asset_readbacks.is_empty() {
        for payload in &opts.asset_readbacks {
            match manager.asset_readback(&opts.plugin, payload.clone()) {
                Ok(response) => {
                    println!(
                        "[isolated-cli] asset readback {} -> {} ({})",
                        format_asset_payload(payload),
                        format!("{} bytes", response.byte_length),
                        response.content_type
                    );
                }
                Err(err) => {
                    if err.to_string().contains("asset readback budget exceeded") && !opts.fail_on_throttle {
                        eprintln!("[isolated-cli] asset readback throttled: {err}");
                    } else {
                        return Err(err);
                    }
                }
            }
        }
    }

    {
        let feature_handle = manager.feature_handle();
        let capability_handle = manager.capability_tracker_handle();
        let mut ctx = make_context(
            &mut renderer,
            &mut ecs,
            &mut assets,
            &mut input,
            &mut material_registry,
            &mut mesh_registry,
            &mut environment_registry,
            &time,
            feature_handle,
            capability_handle,
        );
        manager.shutdown(&mut ctx);
    }
    Ok(())
}

fn drain_and_print_events(step: usize, events: Vec<GameEvent>) {
    if events.is_empty() {
        return;
    }
    for event in events {
        println!("[isolated-cli][step {step}] event: {event}");
    }
}

fn push_event_bridge(ecs: &mut EcsWorld, event: GameEvent) {
    ecs.push_event(event);
}

#[allow(clippy::too_many_arguments)]
fn make_context<'a>(
    renderer: &'a mut Renderer,
    ecs: &'a mut EcsWorld,
    assets: &'a mut AssetManager,
    input: &'a mut Input,
    material_registry: &'a mut MaterialRegistry,
    mesh_registry: &'a mut MeshRegistry,
    environment_registry: &'a mut EnvironmentRegistry,
    time: &'a Time,
    feature_handle: FeatureRegistryHandle,
    capability_handle: CapabilityTrackerHandle,
) -> PluginContext<'a> {
    PluginContext::new(
        renderer,
        ecs,
        assets,
        input,
        material_registry,
        mesh_registry,
        environment_registry,
        time,
        push_event_bridge,
        feature_handle,
        None,
        capability_handle,
    )
}

struct CliOptions {
    manifest: PathBuf,
    plugin: String,
    steps: usize,
    dt: f32,
    entity_bits: Option<u64>,
    watchdog_ms: Option<u64>,
    asset_readbacks: Vec<RpcAssetReadbackPayload>,
    fail_on_throttle: bool,
}

impl CliOptions {
    fn parse() -> Result<Self> {
        let mut manifest = None;
        let mut plugin = None;
        let mut steps = 1usize;
        let mut dt = 0.016;
        let mut entity_bits = None;
        let mut watchdog_ms = None;
        let mut asset_readbacks = Vec::new();
        let mut fail_on_throttle = false;
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--manifest" => {
                    let value = args.next().ok_or_else(|| anyhow!("--manifest requires a path"))?;
                    manifest = Some(PathBuf::from(value));
                }
                "--plugin" => {
                    plugin = Some(args.next().ok_or_else(|| anyhow!("--plugin requires a name"))?);
                }
                "--steps" => {
                    let value = args.next().ok_or_else(|| anyhow!("--steps requires a value"))?;
                    steps = value.parse().context("--steps must be an integer")?;
                }
                "--dt" => {
                    let value = args.next().ok_or_else(|| anyhow!("--dt requires a value"))?;
                    dt = value.parse().context("--dt must be a number")?;
                }
                "--entity-bits" => {
                    let value = args.next().ok_or_else(|| anyhow!("--entity-bits requires a value"))?;
                    entity_bits = Some(value.parse().context("--entity-bits must be u64")?);
                }
                "--watchdog-ms" => {
                    let value = args.next().ok_or_else(|| anyhow!("--watchdog-ms requires a value"))?;
                    watchdog_ms = Some(value.parse().context("--watchdog-ms must be an integer")?);
                }
                "--asset-readback" => {
                    let value = args.next().ok_or_else(|| anyhow!("--asset-readback requires a value"))?;
                    asset_readbacks.push(parse_asset_readback_spec(&value)?);
                }
                "--fail-on-throttle" => {
                    fail_on_throttle = true;
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(anyhow!("unknown argument '{other}'")),
            }
        }
        let manifest = manifest.ok_or_else(|| anyhow!("--manifest is required"))?;
        let plugin = plugin.ok_or_else(|| anyhow!("--plugin is required"))?;
        Ok(Self {
            manifest,
            plugin,
            steps,
            dt,
            entity_bits,
            watchdog_ms,
            asset_readbacks,
            fail_on_throttle,
        })
    }
}

fn print_usage() {
    eprintln!(
        "Usage: isolated_plugin_cli --manifest <path> --plugin <name> [--steps N] [--dt DT] [--entity-bits ID] [--watchdog-ms MS] [--asset-readback kind=value] [--fail-on-throttle]"
    );
}

fn parse_asset_readback_spec(spec: &str) -> Result<RpcAssetReadbackPayload> {
    let (kind, value) = spec
        .split_once('=')
        .ok_or_else(|| anyhow!("asset readback spec must use kind=value syntax"))?;
    match kind {
        "atlas-meta" | "atlas_meta" => Ok(RpcAssetReadbackPayload::AtlasMeta { atlas_id: value.to_string() }),
        "atlas-binary" | "atlas_binary" => {
            Ok(RpcAssetReadbackPayload::AtlasBinary { atlas_id: value.to_string() })
        }
        "blob" => {
            let mut parts = value.split(',');
            let blob_id = parts.next().ok_or_else(|| anyhow!("blob spec requires id,offset,length"))?;
            let offset = parts
                .next()
                .ok_or_else(|| anyhow!("blob spec missing offset"))?
                .parse()
                .context("blob offset must be u64")?;
            let length = parts
                .next()
                .ok_or_else(|| anyhow!("blob spec missing length"))?
                .parse()
                .context("blob length must be u64")?;
            Ok(RpcAssetReadbackPayload::BlobRange {
                blob_id: blob_id.to_string(),
                offset,
                length,
            })
        }
        _ => Err(anyhow!("unknown asset readback kind '{kind}'")),
    }
}

fn format_asset_payload(payload: &RpcAssetReadbackPayload) -> String {
    match payload {
        RpcAssetReadbackPayload::AtlasMeta { atlas_id } => format!("atlas-meta({atlas_id})"),
        RpcAssetReadbackPayload::AtlasBinary { atlas_id } => format!("atlas-binary({atlas_id})"),
        RpcAssetReadbackPayload::BlobRange { blob_id, offset, length } => {
            format!("blob({blob_id}, offset={offset}, length={length})")
        }
    }
}
