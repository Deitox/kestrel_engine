use crate::events::GameEvent;
use bevy_ecs::entity::Entity;
use bincode::Options;
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
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum PluginHostResponse {
    Ok,
    Error(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum RpcGameEvent {
    SpriteSpawned { entity: RpcEntity, atlas: String, region: String },
    SpriteAnimationEvent { entity: RpcEntity, timeline: String, event: String },
    EntityDespawned { entity: RpcEntity },
    CollisionStarted { a: RpcEntity, b: RpcEntity },
    CollisionEnded { a: RpcEntity, b: RpcEntity },
    CollisionForce { a: RpcEntity, b: RpcEntity, force: f32 },
    ScriptMessage { message: String },
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct RpcEntity {
    bits: u64,
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
            GameEvent::SpriteSpawned { entity, atlas, region } => {
                RpcGameEvent::SpriteSpawned { entity: entity.into(), atlas, region }
            }
            GameEvent::SpriteAnimationEvent { entity, timeline, event } => {
                RpcGameEvent::SpriteAnimationEvent {
                    entity: entity.into(),
                    timeline: timeline.as_ref().to_string(),
                    event: event.as_ref().to_string(),
                }
            }
            GameEvent::EntityDespawned { entity } => RpcGameEvent::EntityDespawned { entity: entity.into() },
            GameEvent::CollisionStarted { a, b } => {
                RpcGameEvent::CollisionStarted { a: a.into(), b: b.into() }
            }
            GameEvent::CollisionEnded { a, b } => RpcGameEvent::CollisionEnded { a: a.into(), b: b.into() },
            GameEvent::CollisionForce { a, b, force } => {
                RpcGameEvent::CollisionForce { a: a.into(), b: b.into(), force }
            }
            GameEvent::ScriptMessage { message } => RpcGameEvent::ScriptMessage { message },
        }
    }
}

impl From<RpcGameEvent> for GameEvent {
    fn from(event: RpcGameEvent) -> Self {
        match event {
            RpcGameEvent::SpriteSpawned { entity, atlas, region } => {
                GameEvent::SpriteSpawned { entity: entity.into(), atlas, region }
            }
            RpcGameEvent::SpriteAnimationEvent { entity, timeline, event } => {
                GameEvent::SpriteAnimationEvent {
                    entity: entity.into(),
                    timeline: Arc::<str>::from(timeline),
                    event: Arc::<str>::from(event),
                }
            }
            RpcGameEvent::EntityDespawned { entity } => GameEvent::EntityDespawned { entity: entity.into() },
            RpcGameEvent::CollisionStarted { a, b } => {
                GameEvent::CollisionStarted { a: a.into(), b: b.into() }
            }
            RpcGameEvent::CollisionEnded { a, b } => GameEvent::CollisionEnded { a: a.into(), b: b.into() },
            RpcGameEvent::CollisionForce { a, b, force } => {
                GameEvent::CollisionForce { a: a.into(), b: b.into(), force }
            }
            RpcGameEvent::ScriptMessage { message } => GameEvent::ScriptMessage { message },
        }
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
}
