use anyhow::{anyhow, bail, Context, Result};
use bevy_ecs::prelude::Entity;
use bevy_ecs::world::EntityRef;
use kestrel_engine::assets::{AssetManager, AtlasSnapshot};
use kestrel_engine::config::WindowConfig;
use kestrel_engine::ecs::{
    Children, EcsWorld, EntityInfo, Parent, SceneEntityTag, Sprite, SpriteAnimation, SpriteInfo, Tint,
    Transform, Velocity, WorldTransform,
};
use kestrel_engine::environment::EnvironmentRegistry;
use kestrel_engine::events::GameEvent;
use kestrel_engine::input::Input;
use kestrel_engine::material_registry::MaterialRegistry;
use kestrel_engine::mesh_registry::MeshRegistry;
use kestrel_engine::plugin_rpc::{
    recv_frame, send_frame, PluginHostRequest, PluginHostResponse, RpcAssetReadbackPayload,
    RpcAssetReadbackRequest, RpcAssetReadbackResponse, RpcCapabilityEvent, RpcComponentKind,
    RpcComponentSnapshot, RpcEntityFilter, RpcEntityInfo, RpcEntitySnapshot, RpcGameEvent,
    RpcHierarchySnapshot, RpcIterEntitiesRequest, RpcIterEntitiesResponse, RpcIteratorCursor,
    RpcReadComponentsRequest, RpcReadComponentsResponse, RpcResponseData, RpcSnapshotFormat,
    RpcSpriteInfo, RpcSpriteSnapshot, RpcTintSnapshot, RpcTransformSnapshot, RpcVelocitySnapshot,
    RpcWorldTransformSnapshot,
};
use kestrel_engine::plugins::{
    CapabilityTrackerHandle, CapabilityFlags, EnginePlugin, FeatureRegistryHandle, PluginCapability,
    PluginCapabilityEvent, PluginContext, PluginEntryFn, PluginTrust, ENGINE_PLUGIN_API_VERSION,
    PLUGIN_ENTRY_SYMBOL,
};
use kestrel_engine::renderer::Renderer;
use kestrel_engine::time::Time;
use libloading::Library;
use pollster::block_on;
use serde::Serialize;
use std::cell::Cell;
use std::env;
use std::fs;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::ptr;
use std::time::Duration;

thread_local! {
    static ACTIVE_ENGINE_STATE: Cell<*mut EngineState> = const { Cell::new(ptr::null_mut()) };
}

const MAX_BLOB_READ_BYTES: u64 = 16 * 1024 * 1024;

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
    capabilities: Vec<PluginCapability>,
    trust: PluginTrust,
}

impl HostOptions {
    fn parse() -> Result<Self> {
        let mut plugin_path = None;
        let mut plugin_name = "<unknown>".to_string();
        let mut capabilities = Vec::new();
        let mut trust = PluginTrust::Isolated;
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
                        capabilities.push(
                            PluginCapability::from_label(&cap)
                                .ok_or_else(|| anyhow!("unknown capability label '{cap}'"))?,
                        );
                    }
                }
                "--trust" => {
                    if let Some(mode) = args.next() {
                        trust = match mode.as_str() {
                            "isolated" => PluginTrust::Isolated,
                            "full" => PluginTrust::Full,
                            other => bail!("unknown trust mode '{other}'"),
                        };
                    }
                }
                _ => {}
            }
        }
        let plugin_path =
            plugin_path.map(PathBuf::from).ok_or_else(|| anyhow!("--plugin argument missing"))?;
        Ok(Self { plugin_path, plugin_name, capabilities, trust })
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
                    "resolving '{symbol}' in plugin '{path}'",
                    symbol = "kestrel_plugin_entry",
                    path = opts.plugin_path.display()
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
        Ok(Self { plugin, _library: library, engine: EngineState::new(&opts), opts })
    }

    fn ok_response(
        &mut self,
        events: Vec<RpcGameEvent>,
        data: Option<RpcResponseData>,
    ) -> PluginHostResponse {
        PluginHostResponse::Ok {
            events,
            capability_violations: self.capability_events(),
            data,
        }
    }

    fn error_response(&mut self, message: String) -> PluginHostResponse {
        PluginHostResponse::Error { message, capability_violations: self.capability_events() }
    }

    fn capability_events(&mut self) -> Vec<RpcCapabilityEvent> {
        self.engine
            .drain_capability_events()
            .into_iter()
            .map(|evt| RpcCapabilityEvent { capability: evt.capability })
            .collect()
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
            PluginHostRequest::QueryEntityInfo { entity } => {
                let info = self.engine.entity_info_snapshot(entity.into());
                let response = self.ok_response(Vec::new(), Some(RpcResponseData::EntityInfo(info)));
                return (response, false);
            }
            PluginHostRequest::ReadComponents(request) => {
                let payload = self.engine.read_components(request);
                let response =
                    self.ok_response(Vec::new(), Some(RpcResponseData::ReadComponents(payload)));
                return (response, false);
            }
            PluginHostRequest::IterEntities(request) => {
                let payload = self.engine.iter_entities(request);
                let response =
                    self.ok_response(Vec::new(), Some(RpcResponseData::IterEntities(payload)));
                return (response, false);
            }
            PluginHostRequest::AssetReadback(request) => match self.engine.asset_readback(request) {
                Ok(payload) => {
                    let response =
                        self.ok_response(Vec::new(), Some(RpcResponseData::AssetReadback(payload)));
                    return (response, false);
                }
                Err(err) => {
                    eprintln!("[isolated-host] asset readback failed: {err:?}");
                    return (self.error_response(err.to_string()), false);
                }
            },
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
        let captured_events = self.engine.drain_captured_events();
        let response = match result {
            Ok(()) => {
                self.ok_response(captured_events.into_iter().map(RpcGameEvent::from).collect(), None)
            }
            Err(err) => {
                eprintln!("[isolated-host] plugin call failed: {err:?}");
                self.error_response(err.to_string())
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
    pending_events: Vec<GameEvent>,
    plugin_name: String,
    capability_flags: CapabilityFlags,
    trust: PluginTrust,
}

impl EngineState {
    fn new(opts: &HostOptions) -> Self {
        let mut material_registry = MaterialRegistry::new();
        let mesh_registry = MeshRegistry::new(&mut material_registry);
        let capability_flags = CapabilityFlags::from(opts.capabilities.as_slice());
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
            pending_events: Vec::new(),
            plugin_name: opts.plugin_name.clone(),
            capability_flags,
            trust: opts.trust,
        }
    }

    fn set_delta(&mut self, dt: f32) {
        self.time.delta = Duration::from_secs_f32(dt.max(0.0));
    }

    fn with_context<F>(&mut self, f: F) -> Result<()>
    where
        F: FnOnce(&mut PluginContext<'_>) -> Result<()>,
    {
        self.with_active(|state| {
            let mut ctx = PluginContext::new(
                &mut state.renderer,
                &mut state.ecs,
                &mut state.assets,
                &mut state.input,
                &mut state.material_registry,
                &mut state.mesh_registry,
                &mut state.environment_registry,
                &state.time,
                isolated_emit_event,
                state.feature_registry.clone(),
                None,
                state.capability_tracker.clone(),
            );
            ctx.set_active_plugin(&state.plugin_name, state.capability_flags, state.trust);
            let result = f(&mut ctx);
            ctx.clear_active_plugin();
            result
        })
    }

    fn with_active<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        ACTIVE_ENGINE_STATE.with(|cell| {
            let prev = cell.replace(self as *mut Self);
            let result = f(self);
            cell.set(prev);
            result
        })
    }

    fn capture_event(event: GameEvent) {
        ACTIVE_ENGINE_STATE.with(|cell| {
            let ptr = cell.get();
            if let Some(state) = unsafe { ptr.as_mut() } {
                state.pending_events.push(event);
            }
        });
    }

    fn drain_captured_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.pending_events)
    }

    fn drain_capability_events(&mut self) -> Vec<PluginCapabilityEvent> {
        self.capability_tracker.drain_events()
    }

    fn entity_info_snapshot(&self, entity: Entity) -> Option<RpcEntityInfo> {
        let info = self.ecs.entity_info(entity)?;
        Some(entity_info_to_rpc(entity, info))
    }

    fn read_components(&self, request: RpcReadComponentsRequest) -> RpcReadComponentsResponse {
        let entity = Entity::from(request.entity);
        let (snapshot, missing) = self.entity_snapshot(entity, &request.components, request.format);
        RpcReadComponentsResponse { request_id: request.request_id, snapshot, missing_components: missing }
    }

    fn iter_entities(&self, request: RpcIterEntitiesRequest) -> RpcIterEntitiesResponse {
        let limit = request.limit.clamp(1, 512) as usize;
        let mut matched_entities = Vec::new();
        {
            let world = &self.ecs.world;
            for entity_ref in world.iter_entities() {
                if self.matches_filter(&entity_ref, &request.filter) {
                    matched_entities.push(entity_ref.id());
                }
            }
        }
        matched_entities.sort_by_key(|entity| entity.to_bits());
        let cursor_bits = request.cursor.map(|cursor| cursor.last_entity_bits);
        let mut collected = Vec::new();
        let mut last_bits = None;
        let mut remaining_after_limit = false;
        for entity in matched_entities {
            if let Some(bits) = cursor_bits {
                if entity.to_bits() <= bits {
                    continue;
                }
            }
            if collected.len() >= limit {
                remaining_after_limit = true;
                break;
            }
            let (snapshot, _) = self.entity_snapshot(entity, &request.components, request.format);
            if let Some(snapshot) = snapshot {
                last_bits = Some(entity.to_bits());
                collected.push(snapshot);
            }
        }
        let next_cursor = if remaining_after_limit {
            last_bits.map(|bits| RpcIteratorCursor { last_entity_bits: bits })
        } else {
            None
        };
        RpcIterEntitiesResponse {
            request_id: request.request_id,
            snapshots: collected,
            next_cursor,
            exhausted: !remaining_after_limit,
        }
    }

    fn asset_readback(&self, request: RpcAssetReadbackRequest) -> Result<RpcAssetReadbackResponse> {
        match request.payload {
            RpcAssetReadbackPayload::AtlasMeta { atlas_id } => {
                let snapshot = self
                    .assets
                    .atlas_snapshot(&atlas_id)
                    .ok_or_else(|| anyhow!("atlas '{atlas_id}' not loaded"))?;
                let json = serialize_atlas_metadata(&atlas_id, snapshot)?;
                let bytes = json.as_bytes().to_vec();
                let byte_length = bytes.len() as u64;
                Ok(RpcAssetReadbackResponse {
                    request_id: request.request_id,
                    content_type: "application/json".to_string(),
                    bytes,
                    metadata_json: Some(json),
                    byte_length,
                })
            }
            RpcAssetReadbackPayload::AtlasBinary { atlas_id } => {
                let snapshot = self
                    .assets
                    .atlas_snapshot(&atlas_id)
                    .ok_or_else(|| anyhow!("atlas '{atlas_id}' not loaded"))?;
                let image_path = snapshot
                    .image_path
                    .to_str()
                    .ok_or_else(|| anyhow!("atlas image path contains invalid UTF-8"))?;
                let path = self.sanitize_blob_path(image_path)?;
                let metadata = fs::metadata(&path)
                    .with_context(|| format!("reading metadata for atlas image '{}'", path.display()))?;
                let byte_length = metadata.len();
                if byte_length > MAX_BLOB_READ_BYTES {
                    bail!(
                        "atlas image '{}' exceeds readback cap of {} bytes ({} bytes)",
                        path.display(),
                        MAX_BLOB_READ_BYTES,
                        byte_length
                    );
                }
                let mut file =
                    fs::File::open(&path).with_context(|| format!("opening atlas image '{}'", path.display()))?;
                let mut bytes = Vec::with_capacity(byte_length as usize);
                file.read_to_end(&mut bytes)
                    .with_context(|| format!("reading atlas image '{}'", path.display()))?;
                let content_type = guess_content_type(&path);
                Ok(RpcAssetReadbackResponse {
                    request_id: request.request_id,
                    content_type: content_type.to_string(),
                    byte_length,
                    bytes,
                    metadata_json: None,
                })
            }
            RpcAssetReadbackPayload::BlobRange { blob_id, offset, length } => {
                let path = self.sanitize_blob_path(&blob_id)?;
                let metadata = fs::metadata(&path)
                    .with_context(|| format!("reading metadata for blob '{}'", path.display()))?;
                let blob_len = metadata.len();
                let start = offset.min(blob_len);
                let available = blob_len.saturating_sub(start);
                let requested = if length == 0 { available } else { length.min(available) };
                if requested > MAX_BLOB_READ_BYTES {
                    bail!(
                        "blob range request ({requested} bytes) exceeds cap of {MAX_BLOB_READ_BYTES} bytes"
                    );
                }
                let mut file = fs::File::open(&path)
                    .with_context(|| format!("opening blob '{}' for range read", path.display()))?;
                file.seek(SeekFrom::Start(start))
                    .with_context(|| format!("seeking blob '{}' to offset {}", path.display(), start))?;
                let reader = BufReader::new(file);
                let mut slice = Vec::with_capacity(requested as usize);
                reader
                    .take(requested)
                    .read_to_end(&mut slice)
                    .with_context(|| format!("reading blob '{}' range {}..{}", path.display(), start, start + requested))?;
                Ok(RpcAssetReadbackResponse {
                    request_id: request.request_id,
                    content_type: "application/octet-stream".to_string(),
                    byte_length: slice.len() as u64,
                    bytes: slice,
                    metadata_json: None,
                })
            }
        }
    }

    fn sanitize_blob_path(&self, blob_id: &str) -> Result<PathBuf> {
        let path = Path::new(blob_id);
        if path.is_absolute() {
            bail!("blob path must be relative");
        }
        if path.components().any(|c| matches!(c, Component::ParentDir)) {
            bail!("blob path cannot traverse upwards");
        }
        let root = env::current_dir().context("resolve host working directory")?;
        let root_canon = root.canonicalize().unwrap_or_else(|_| root.clone());
        let candidate = root.join(path);
        if let Ok(canon) = candidate.canonicalize() {
            if !canon.starts_with(&root_canon) {
                bail!("blob path escapes sandbox");
            }
            return Ok(canon);
        }
        if !candidate.starts_with(&root) && !candidate.starts_with(&root_canon) {
            bail!("blob path escapes sandbox");
        }
        Ok(candidate)
    }

    fn matches_filter(&self, entity_ref: &EntityRef<'_>, filter: &RpcEntityFilter) -> bool {
        if !filter.components.is_empty()
            && !filter.components.iter().all(|component| self.entity_has_component(entity_ref, *component))
        {
            return false;
        }
        if filter.tags.is_empty() {
            return true;
        }
        let tag = entity_ref.get::<SceneEntityTag>();
        if tag.is_none() {
            return false;
        }
        let scene_id = tag.unwrap().id.as_str();
        filter.tags.iter().any(|needle| needle == scene_id)
    }

    fn entity_has_component(&self, entity_ref: &EntityRef<'_>, kind: RpcComponentKind) -> bool {
        match kind {
            RpcComponentKind::Transform2D => entity_ref.contains::<Transform>(),
            RpcComponentKind::WorldTransform => entity_ref.contains::<WorldTransform>(),
            RpcComponentKind::Sprite => entity_ref.contains::<Sprite>(),
            RpcComponentKind::Hierarchy => {
                entity_ref.contains::<Parent>() || entity_ref.contains::<Children>()
            }
            RpcComponentKind::Velocity => entity_ref.contains::<Velocity>(),
            RpcComponentKind::Tint => entity_ref.contains::<Tint>(),
        }
    }

    fn entity_snapshot(
        &self,
        entity: Entity,
        components: &[RpcComponentKind],
        format: RpcSnapshotFormat,
    ) -> (Option<RpcEntitySnapshot>, Vec<RpcComponentKind>) {
        let _ = format;
        if !self.ecs.world.entities().contains(entity) {
            return (None, components.to_vec());
        }
        let mut snapshots = Vec::new();
        let mut missing = Vec::new();
        for kind in components {
            match kind {
                RpcComponentKind::Transform2D => {
                    if let Some(transform) = self.ecs.world.get::<Transform>(entity) {
                        snapshots.push(RpcComponentSnapshot::Transform2D(RpcTransformSnapshot {
                            translation: transform.translation.to_array(),
                            rotation: transform.rotation,
                            scale: transform.scale.to_array(),
                        }));
                    } else {
                        missing.push(*kind);
                    }
                }
                RpcComponentKind::WorldTransform => {
                    if let Some(world_transform) = self.ecs.world.get::<WorldTransform>(entity) {
                        snapshots.push(RpcComponentSnapshot::WorldTransform(RpcWorldTransformSnapshot {
                            matrix: world_transform.0.to_cols_array_2d(),
                        }));
                    } else {
                        missing.push(*kind);
                    }
                }
                RpcComponentKind::Sprite => {
                    if let Some(sprite) = self.ecs.world.get::<Sprite>(entity) {
                        let animation = self.ecs.world.get::<SpriteAnimation>(entity);
                        let frame_index = animation.map(|anim| anim.frame_index as u32);
                        let tint = self.ecs.world.get::<Tint>(entity).map(|t| t.0.to_array());
                        snapshots.push(RpcComponentSnapshot::Sprite(RpcSpriteSnapshot {
                            atlas: sprite.atlas_key.to_string(),
                            region: sprite.region.to_string(),
                            frame_index,
                            visible: true,
                            color: tint.unwrap_or([1.0, 1.0, 1.0, 1.0]),
                            flip_x: false,
                            flip_y: false,
                        }));
                    } else {
                        missing.push(*kind);
                    }
                }
                RpcComponentKind::Hierarchy => {
                    let parent = self.ecs.world.get::<Parent>(entity).map(|p| p.0.into());
                    let child_count = self
                        .ecs
                        .world
                        .get::<Children>(entity)
                        .map(|children| children.0.len() as u32)
                        .unwrap_or(0);
                    if parent.is_some() || child_count > 0 {
                        snapshots.push(RpcComponentSnapshot::Hierarchy(RpcHierarchySnapshot {
                            parent,
                            child_count,
                        }));
                    } else {
                        missing.push(*kind);
                    }
                }
                RpcComponentKind::Velocity => {
                    if let Some(velocity) = self.ecs.world.get::<Velocity>(entity) {
                        snapshots.push(RpcComponentSnapshot::Velocity(RpcVelocitySnapshot {
                            linear: velocity.0.to_array(),
                        }));
                    } else {
                        missing.push(*kind);
                    }
                }
                RpcComponentKind::Tint => {
                    if let Some(tint) = self.ecs.world.get::<Tint>(entity) {
                        snapshots.push(RpcComponentSnapshot::Tint(RpcTintSnapshot {
                            color: tint.0.to_array(),
                            visible: true,
                        }));
                    } else {
                        missing.push(*kind);
                    }
                }
            }
        }
        if snapshots.is_empty() {
            (None, missing)
        } else {
            (Some(RpcEntitySnapshot { entity: entity.into(), components: snapshots }), missing)
        }
    }
}

fn isolated_emit_event(ecs: &mut EcsWorld, event: GameEvent) {
    EngineState::capture_event(event.clone());
    ecs.push_event(event);
}

fn entity_info_to_rpc(entity: Entity, info: EntityInfo) -> RpcEntityInfo {
    RpcEntityInfo {
        entity: entity.into(),
        scene_id: info.scene_id.as_str().to_string(),
        translation: info.translation.to_array(),
        rotation: info.rotation,
        scale: info.scale.to_array(),
        velocity: info.velocity.map(|v| v.to_array()),
        sprite: info.sprite.map(sprite_info_to_rpc),
    }
}

fn sprite_info_to_rpc(info: SpriteInfo) -> RpcSpriteInfo {
    RpcSpriteInfo { atlas: info.atlas, region: info.region }
}

#[derive(Serialize)]
struct AtlasMetaRegionSnapshot {
    name: String,
    rect: [u32; 4],
    uv: [f32; 4],
    id: u16,
}

#[derive(Serialize)]
struct AtlasMetaAnimationFrameSnapshot {
    region: String,
    duration: f32,
    events: Vec<String>,
}

#[derive(Serialize)]
struct AtlasMetaAnimationSnapshot {
    name: String,
    looped: bool,
    loop_mode: String,
    frame_count: usize,
    frames: Vec<AtlasMetaAnimationFrameSnapshot>,
}

#[derive(Serialize)]
struct AtlasMetaDocument {
    atlas_id: String,
    width: u32,
    height: u32,
    image_path: String,
    regions: Vec<AtlasMetaRegionSnapshot>,
    animations: Vec<AtlasMetaAnimationSnapshot>,
}

fn serialize_atlas_metadata(atlas_id: &str, snapshot: AtlasSnapshot<'_>) -> Result<String> {
    let mut regions: Vec<AtlasMetaRegionSnapshot> = snapshot
        .regions
        .iter()
        .map(|(name, region)| AtlasMetaRegionSnapshot {
            name: name.as_ref().to_string(),
            rect: [region.rect.x, region.rect.y, region.rect.w, region.rect.h],
            uv: region.uv,
            id: region.id,
        })
        .collect();
    regions.sort_by(|a, b| a.name.cmp(&b.name));

    let mut animations: Vec<AtlasMetaAnimationSnapshot> = snapshot
        .animations
        .iter()
        .map(|(name, timeline)| AtlasMetaAnimationSnapshot {
            name: name.clone(),
            looped: timeline.looped,
            loop_mode: format!("{:?}", timeline.loop_mode),
            frame_count: timeline.frames.len(),
            frames: timeline
                .frames
                .iter()
                .map(|frame| AtlasMetaAnimationFrameSnapshot {
                    region: frame.region.as_ref().to_string(),
                    duration: frame.duration,
                    events: frame.events.iter().map(|evt| evt.as_ref().to_string()).collect(),
                })
                .collect(),
        })
        .collect();
    animations.sort_by(|a, b| a.name.cmp(&b.name));

    let doc = AtlasMetaDocument {
        atlas_id: atlas_id.to_string(),
        width: snapshot.width,
        height: snapshot.height,
        image_path: snapshot.image_path.display().to_string(),
        regions,
        animations,
    };
    Ok(serde_json::to_string(&doc)?)
}

fn guess_content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()) {
        Some(ext) if ext == "png" => "image/png",
        Some(ext) if ext == "jpg" || ext == "jpeg" => "image/jpeg",
        Some(ext) if ext == "json" => "application/json",
        _ => "application/octet-stream",
    }
}
