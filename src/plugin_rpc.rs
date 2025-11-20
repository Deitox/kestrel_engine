use crate::events::{AudioEmitter, GameEvent};
use bevy_ecs::entity::Entity;
use bincode::Options;
use glam::Vec3;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::sync::Arc;

const FRAME_LEN_BYTES: usize = std::mem::size_of::<u32>();

#[derive(Debug, Serialize, Deserialize)]
pub enum PluginHostRequest {
    Build,
    Update { dt: f32 },
    FixedUpdate { dt: f32 },
    OnEvents { events: Vec<RpcGameEvent> },
    QueryEntityInfo { entity: RpcEntity },
    ReadComponents(RpcReadComponentsRequest),
    IterEntities(RpcIterEntitiesRequest),
    AssetReadback(RpcAssetReadbackRequest),
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PluginHostResponse {
    Ok { events: Vec<RpcGameEvent>, data: Option<RpcResponseData> },
    Error(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcAudioEmitter {
    pub position: [f32; 3],
    pub max_distance: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum RpcGameEvent {
    SpriteSpawned { entity: RpcEntity, atlas: String, region: String, audio: Option<RpcAudioEmitter> },
    SpriteAnimationEvent { entity: RpcEntity, timeline: String, event: String },
    EntityDespawned { entity: RpcEntity },
    CollisionStarted { a: RpcEntity, b: RpcEntity, audio: Option<RpcAudioEmitter> },
    CollisionEnded { a: RpcEntity, b: RpcEntity, audio: Option<RpcAudioEmitter> },
    CollisionForce { a: RpcEntity, b: RpcEntity, force: f32, audio: Option<RpcAudioEmitter> },
    ScriptMessage { message: String },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum RpcResponseData {
    EntityInfo(Option<RpcEntityInfo>),
    ReadComponents(RpcReadComponentsResponse),
    IterEntities(RpcIterEntitiesResponse),
    AssetReadback(RpcAssetReadbackResponse),
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct RpcEntity {
    bits: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcEntityInfo {
    pub entity: RpcEntity,
    pub scene_id: String,
    pub translation: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
    pub velocity: Option<[f32; 2]>,
    pub sprite: Option<RpcSpriteInfo>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RpcSpriteInfo {
    pub atlas: String,
    pub region: String,
}

pub type RpcRequestId = u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcReadComponentsRequest {
    pub request_id: RpcRequestId,
    pub entity: RpcEntity,
    pub components: Vec<RpcComponentKind>,
    pub format: RpcSnapshotFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcReadComponentsResponse {
    pub request_id: RpcRequestId,
    pub snapshot: Option<RpcEntitySnapshot>,
    pub missing_components: Vec<RpcComponentKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcIterEntitiesRequest {
    pub request_id: RpcRequestId,
    pub filter: RpcEntityFilter,
    pub cursor: Option<RpcIteratorCursor>,
    pub limit: u32,
    pub components: Vec<RpcComponentKind>,
    pub format: RpcSnapshotFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcIterEntitiesResponse {
    pub request_id: RpcRequestId,
    pub snapshots: Vec<RpcEntitySnapshot>,
    pub next_cursor: Option<RpcIteratorCursor>,
    pub exhausted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcIteratorCursor {
    pub last_entity_bits: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RpcEntityFilter {
    pub components: Vec<RpcComponentKind>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcEntitySnapshot {
    pub entity: RpcEntity,
    pub components: Vec<RpcComponentSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RpcComponentSnapshot {
    Transform2D(RpcTransformSnapshot),
    WorldTransform(RpcWorldTransformSnapshot),
    Sprite(RpcSpriteSnapshot),
    Hierarchy(RpcHierarchySnapshot),
    Velocity(RpcVelocitySnapshot),
    Tint(RpcTintSnapshot),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcTransformSnapshot {
    pub translation: [f32; 2],
    pub rotation: f32,
    pub scale: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcWorldTransformSnapshot {
    pub matrix: [[f32; 4]; 4],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcSpriteSnapshot {
    pub atlas: String,
    pub region: String,
    pub frame_index: Option<u32>,
    pub visible: bool,
    pub color: [f32; 4],
    pub flip_x: bool,
    pub flip_y: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcHierarchySnapshot {
    pub parent: Option<RpcEntity>,
    pub child_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcVelocitySnapshot {
    pub linear: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcTintSnapshot {
    pub color: [f32; 4],
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcComponentKind {
    Transform2D,
    WorldTransform,
    Sprite,
    Hierarchy,
    Velocity,
    Tint,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcSnapshotFormat {
    Full,
    Partial,
    Lite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcAssetReadbackRequest {
    pub request_id: RpcRequestId,
    pub payload: RpcAssetReadbackPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RpcAssetReadbackPayload {
    AtlasMeta { atlas_id: String },
    AtlasBinary { atlas_id: String },
    BlobRange { blob_id: String, offset: u64, length: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcAssetReadbackResponse {
    pub request_id: RpcRequestId,
    pub content_type: String,
    pub bytes: Vec<u8>,
    pub metadata_json: Option<String>,
    pub byte_length: u64,
}

impl From<Entity> for RpcEntity {
    fn from(entity: Entity) -> Self {
        Self { bits: entity.to_bits() }
    }
}

impl From<RpcEntity> for Entity {
    fn from(rpc: RpcEntity) -> Self {
        Entity::from_bits(rpc.bits)
    }
}

impl From<GameEvent> for RpcGameEvent {
    fn from(event: GameEvent) -> Self {
        match event {
            GameEvent::SpriteSpawned { entity, atlas, region, audio } => {
                RpcGameEvent::SpriteSpawned {
                    entity: entity.into(),
                    atlas,
                    region,
                    audio: audio.map(RpcAudioEmitter::from),
                }
            }
            GameEvent::SpriteAnimationEvent { entity, timeline, event } => {
                RpcGameEvent::SpriteAnimationEvent {
                    entity: entity.into(),
                    timeline: timeline.as_ref().to_string(),
                    event: event.as_ref().to_string(),
                }
            }
            GameEvent::EntityDespawned { entity } => RpcGameEvent::EntityDespawned { entity: entity.into() },
            GameEvent::CollisionStarted { a, b, audio } => {
                RpcGameEvent::CollisionStarted { a: a.into(), b: b.into(), audio: audio.map(RpcAudioEmitter::from) }
            }
            GameEvent::CollisionEnded { a, b, audio } => {
                RpcGameEvent::CollisionEnded { a: a.into(), b: b.into(), audio: audio.map(RpcAudioEmitter::from) }
            }
            GameEvent::CollisionForce { a, b, force, audio } => {
                RpcGameEvent::CollisionForce {
                    a: a.into(),
                    b: b.into(),
                    force,
                    audio: audio.map(RpcAudioEmitter::from),
                }
            }
            GameEvent::ScriptMessage { message } => RpcGameEvent::ScriptMessage { message },
        }
    }
}

impl From<RpcGameEvent> for GameEvent {
    fn from(event: RpcGameEvent) -> Self {
        match event {
            RpcGameEvent::SpriteSpawned { entity, atlas, region, audio } => {
                GameEvent::SpriteSpawned { entity: entity.into(), atlas, region, audio: audio.map(AudioEmitter::from) }
            }
            RpcGameEvent::SpriteAnimationEvent { entity, timeline, event } => {
                GameEvent::SpriteAnimationEvent {
                    entity: entity.into(),
                    timeline: Arc::<str>::from(timeline),
                    event: Arc::<str>::from(event),
                }
            }
            RpcGameEvent::EntityDespawned { entity } => GameEvent::EntityDespawned { entity: entity.into() },
            RpcGameEvent::CollisionStarted { a, b, audio } => {
                GameEvent::CollisionStarted { a: a.into(), b: b.into(), audio: audio.map(AudioEmitter::from) }
            }
            RpcGameEvent::CollisionEnded { a, b, audio } => {
                GameEvent::CollisionEnded { a: a.into(), b: b.into(), audio: audio.map(AudioEmitter::from) }
            }
            RpcGameEvent::CollisionForce { a, b, force, audio } => {
                GameEvent::CollisionForce { a: a.into(), b: b.into(), force, audio: audio.map(AudioEmitter::from) }
            }
            RpcGameEvent::ScriptMessage { message } => GameEvent::ScriptMessage { message },
        }
    }
}

impl From<AudioEmitter> for RpcAudioEmitter {
    fn from(value: AudioEmitter) -> Self {
        RpcAudioEmitter { position: value.position.to_array(), max_distance: value.max_distance }
    }
}

impl From<RpcAudioEmitter> for AudioEmitter {
    fn from(value: RpcAudioEmitter) -> Self {
        AudioEmitter { position: Vec3::from_array(value.position), max_distance: value.max_distance }
    }
}

pub fn send_frame<W, T>(writer: &mut W, value: &T) -> io::Result<()>
where
    W: Write,
    T: Serialize,
{
    let payload = bincode_options().serialize(value).map_err(to_io_error)?;
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "frame too large"))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

pub fn recv_frame<R, T>(reader: &mut R) -> io::Result<T>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut len_buf = [0u8; FRAME_LEN_BYTES];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    bincode_options().deserialize(&payload).map_err(to_io_error)
}

fn bincode_options() -> impl bincode::Options {
    bincode::DefaultOptions::new().with_fixint_encoding()
}

fn to_io_error(err: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn rpc_game_event_round_trip() {
        let source = GameEvent::ScriptMessage { message: "hello world".to_string() };
        let rpc: RpcGameEvent = source.clone().into();
        let restored: GameEvent = rpc.into();
        assert_eq!(format!("{source:?}"), format!("{restored:?}"));
    }

    #[test]
    fn framed_transport_round_trip() {
        let request = PluginHostRequest::Update { dt: 1.5 };
        let mut buffer = Vec::new();
        send_frame(&mut buffer, &request).expect("frame serialized");
        let mut cursor = Cursor::new(buffer);
        let decoded: PluginHostRequest = recv_frame(&mut cursor).expect("frame decoded without corruption");
        match decoded {
            PluginHostRequest::Update { dt } => assert!((dt - 1.5).abs() < f32::EPSILON),
            other => panic!("unexpected request decoded: {other:?}"),
        }
    }
    #[test]
    fn response_payload_serializes() {
        let response = PluginHostResponse::Ok {
            events: Vec::new(),
            data: Some(RpcResponseData::EntityInfo(Some(RpcEntityInfo {
                entity: RpcEntity { bits: 42 },
                scene_id: "demo".to_string(),
                translation: [1.0, 2.0],
                rotation: 0.5,
                scale: [1.0, 1.0],
                velocity: Some([0.0, 0.0]),
                sprite: Some(RpcSpriteInfo { atlas: "atlas".into(), region: "region".into() }),
            }))),
        };
        let mut buffer = Vec::new();
        send_frame(&mut buffer, &response).expect("response serialized");
        let mut cursor = std::io::Cursor::new(buffer);
        let decoded: PluginHostResponse =
            recv_frame(&mut cursor).expect("response decoded without corruption");
        match decoded {
            PluginHostResponse::Ok { data, .. } => match data {
                Some(RpcResponseData::EntityInfo(Some(info))) => {
                    assert_eq!(info.scene_id, "demo");
                }
                other => panic!("unexpected payload: {other:?}"),
            },
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn read_components_request_round_trip() {
        let request = PluginHostRequest::ReadComponents(RpcReadComponentsRequest {
            request_id: 7,
            entity: RpcEntity { bits: 99 },
            components: vec![RpcComponentKind::Transform2D, RpcComponentKind::Sprite],
            format: RpcSnapshotFormat::Full,
        });
        let mut buffer = Vec::new();
        send_frame(&mut buffer, &request).expect("request serialized");
        let mut cursor = Cursor::new(buffer);
        let decoded: PluginHostRequest = recv_frame(&mut cursor).expect("request decoded without corruption");
        match decoded {
            PluginHostRequest::ReadComponents(payload) => {
                assert_eq!(payload.request_id, 7);
                assert_eq!(payload.components.len(), 2);
            }
            other => panic!("unexpected request decoded: {other:?}"),
        }
    }

    #[test]
    fn asset_readback_response_round_trip() {
        let response = PluginHostResponse::Ok {
            events: Vec::new(),
            data: Some(RpcResponseData::AssetReadback(RpcAssetReadbackResponse {
                request_id: 88,
                content_type: "application/json".to_string(),
                bytes: vec![1, 2, 3, 4],
                metadata_json: Some("{\"frames\":1}".to_string()),
                byte_length: 4,
            })),
        };
        let mut buffer = Vec::new();
        send_frame(&mut buffer, &response).expect("response serialized");
        let mut cursor = std::io::Cursor::new(buffer);
        let decoded: PluginHostResponse =
            recv_frame(&mut cursor).expect("response decoded without corruption");
        match decoded {
            PluginHostResponse::Ok { data: Some(RpcResponseData::AssetReadback(payload)), .. } => {
                assert_eq!(payload.request_id, 88);
                assert_eq!(payload.byte_length, 4);
                assert_eq!(payload.bytes, vec![1, 2, 3, 4]);
            }
            other => panic!("unexpected response decoded: {other:?}"),
        }
    }
}
