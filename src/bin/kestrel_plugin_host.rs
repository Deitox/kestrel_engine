use anyhow::{anyhow, bail, Context, Result};
use kestrel_engine::assets::AssetManager;
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::EcsWorld;
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::events::GameEvent;
use kestrel_engine::input::Input;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::plugin_rpc::{recv_frame, send_frame, PluginHostRequest, PluginHostResponse};
use kestrel_engine::plugins::{
    CapabilityTrackerHandle, EnginePlugin, FeatureRegistryHandle, PluginContext, PluginEntryFn,
    ENGINE_PLUGIN_API_VERSION, PLUGIN_ENTRY_SYMBOL,
};
use kestrel_engine::renderer::Renderer;
use kestrel_engine::time::Time;
use libloading::Library;
use pollster::block_on;
use std::env;
use std::io::{self, BufReader, BufWriter};
use std::path::PathBuf;
use std::time::Duration;

fn main() {
    if let Err(err) = run() {
        eprintln!("[isolated-host] error: {err:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let opts = HostOptions::parse()?;
    let service = PluginHostService::new(opts)?;
    service.run()
}

struct HostOptions {
    plugin_path: PathBuf,
    plugin_name: String,
    capabilities: Vec<String>,
}

impl HostOptions {
    fn parse() -> Result<Self> {
        let mut plugin_path = None;
        let mut plugin_name = "<unknown>".to_string();
        let mut capabilities = Vec::new();
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--plugin" => {
                    plugin_path = args.next();
                }
                "--name" => {
                    if let Some(name) = args.next() {
                        plugin_name = name;
                    }
                }
                "--cap" => {
                    if let Some(cap) = args.next() {
                        capabilities.push(cap);
                    }
                }
                _ => {}
            }
        }
        let plugin_path =
            plugin_path.map(PathBuf::from).ok_or_else(|| anyhow!("--plugin argument missing"))?;
        Ok(Self { plugin_path, plugin_name, capabilities })
    }
}

struct PluginHostService {
    plugin: Box<dyn EnginePlugin>,
    _library: Library,
    engine: EngineState,
    opts: HostOptions,
}

impl PluginHostService {
    fn new(opts: HostOptions) -> Result<Self> {
        let library = unsafe {
            Library::new(&opts.plugin_path)
                .with_context(|| format!("loading plugin '{}'", opts.plugin_path.display()))?
        };
        let entry_fn = unsafe {
            library.get::<PluginEntryFn>(PLUGIN_ENTRY_SYMBOL).with_context(|| {
                format!(
                    "resolving '{symbol}' in plugin '{}'",
                    symbol = "kestrel_plugin_entry",
                    opts.plugin_path.display()
                )
            })?
        };
        let export = unsafe { entry_fn() };
        if export.api_version != ENGINE_PLUGIN_API_VERSION {
            bail!(
                "api mismatch: plugin targets v{}, engine exports v{}",
                export.api_version,
                ENGINE_PLUGIN_API_VERSION
            );
        }
        let handle = unsafe { (export.create)() };
        if handle.is_null() {
            bail!("plugin returned null handle");
        }
        let plugin = unsafe { handle.into_box() };
        Ok(Self { plugin, _library: library, engine: EngineState::new(), opts })
    }

    fn run(mut self) -> Result<()> {
        eprintln!(
            "[isolated-host] running '{}' from '{}' (caps={:?})",
            self.opts.plugin_name,
            self.opts.plugin_path.display(),
            self.opts.capabilities
        );
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut reader = BufReader::new(stdin.lock());
        let mut writer = BufWriter::new(stdout.lock());
        loop {
            let request = match recv_frame::<_, PluginHostRequest>(&mut reader) {
                Ok(req) => req,
                Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(err) => {
                    eprintln!("[isolated-host] failed to read request: {err:?}");
                    break;
                }
            };
            let (response, should_shutdown) = self.handle_request(request);
            if let Err(err) = send_frame(&mut writer, &response) {
                eprintln!("[isolated-host] failed to send response: {err:?}");
                break;
            }
            if should_shutdown {
                break;
            }
        }
        eprintln!("[isolated-host] shutting down '{}'", self.opts.plugin_name);
        Ok(())
    }

    fn handle_request(&mut self, request: PluginHostRequest) -> (PluginHostResponse, bool) {
        let mut shutdown = false;
        let result = match request {
            PluginHostRequest::Build => self.engine.with_context(|ctx| self.plugin.build(ctx)),
            PluginHostRequest::Update { dt } => {
                self.engine.set_delta(dt);
                self.engine.with_context(|ctx| self.plugin.update(ctx, dt))
            }
            PluginHostRequest::FixedUpdate { dt } => {
                self.engine.set_delta(dt);
                self.engine.with_context(|ctx| self.plugin.fixed_update(ctx, dt))
            }
            PluginHostRequest::OnEvents { events } => {
                let events: Vec<GameEvent> = events.into_iter().map(Into::into).collect();
                self.engine.with_context(|ctx| self.plugin.on_events(ctx, &events))
            }
            PluginHostRequest::Shutdown => {
                shutdown = true;
                self.engine.with_context(|ctx| self.plugin.shutdown(ctx))
            }
        };
        let response = match result {
            Ok(()) => PluginHostResponse::Ok,
            Err(err) => {
                eprintln!("[isolated-host] plugin call failed: {err:?}");
                PluginHostResponse::Error(err.to_string())
            }
        };
        (response, shutdown)
    }
}

struct EngineState {
    renderer: Renderer,
    ecs: EcsWorld,
    assets: AssetManager,
    input: Input,
    material_registry: MaterialRegistry,
    mesh_registry: MeshRegistry,
    environment_registry: EnvironmentRegistry,
    time: Time,
    feature_registry: FeatureRegistryHandle,
    capability_tracker: CapabilityTrackerHandle,
}

impl EngineState {
    fn new() -> Self {
        let mut material_registry = MaterialRegistry::new();
        let mesh_registry = MeshRegistry::new(&mut material_registry);
        Self {
            renderer: block_on(Renderer::new(&WindowConfig::default())),
            ecs: EcsWorld::new(),
            assets: AssetManager::new(),
            input: Input::new(),
            material_registry,
            mesh_registry,
            environment_registry: EnvironmentRegistry::new(),
            time: Time::new(),
            feature_registry: FeatureRegistryHandle::isolated(),
            capability_tracker: CapabilityTrackerHandle::isolated(),
        }
    }

    fn set_delta(&mut self, dt: f32) {
        self.time.delta = Duration::from_secs_f32(dt.max(0.0));
    }

    fn with_context<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut PluginContext<'_>) -> Result<()>,
    {
        let mut ctx = PluginContext::new(
            &mut self.renderer,
            &mut self.ecs,
            &mut self.assets,
            &mut self.input,
            &mut self.material_registry,
            &mut self.mesh_registry,
            &mut self.environment_registry,
            &self.time,
            isolated_emit_event,
            self.feature_registry.clone(),
            None,
            self.capability_tracker.clone(),
        );
        f(&mut ctx)
    }
}

fn isolated_emit_event(ecs: &mut EcsWorld, event: GameEvent) {
    ecs.push_event(event);
}
