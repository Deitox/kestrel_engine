use bevy_ecs::prelude::{Entity, Resource};
use std::fmt;

#[derive(Debug, Clone)]
pub enum GameEvent {
    SpriteSpawned { entity: Entity, atlas: String, region: String },
    SpriteAnimationEvent { entity: Entity, timeline: String, event: String },
    EntityDespawned { entity: Entity },
    CollisionStarted { a: Entity, b: Entity },
    CollisionEnded { a: Entity, b: Entity },
    CollisionForce { a: Entity, b: Entity, force: f32 },
    ScriptMessage { message: String },
}

impl GameEvent {
    fn ordered_pair(a: Entity, b: Entity) -> (Entity, Entity) {
        let (first, second) = if a.index() <= b.index() { (a, b) } else { (b, a) };
        (first, second)
    }

    pub fn collision_started(a: Entity, b: Entity) -> Self {
        let (a, b) = Self::ordered_pair(a, b);
        GameEvent::CollisionStarted { a, b }
    }

    pub fn collision_ended(a: Entity, b: Entity) -> Self {
        let (a, b) = Self::ordered_pair(a, b);
        GameEvent::CollisionEnded { a, b }
    }

    pub fn collision_force(a: Entity, b: Entity, force: f32) -> Self {
        let (a, b) = Self::ordered_pair(a, b);
        GameEvent::CollisionForce { a, b, force }
    }
}

impl fmt::Display for GameEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GameEvent::SpriteSpawned { entity, atlas, region } => {
                write!(f, "SpriteSpawned entity={} atlas={} region={}", entity.index(), atlas, region)
            }
            GameEvent::SpriteAnimationEvent { entity, timeline, event } => {
                write!(
                    f,
                    "SpriteAnimationEvent entity={} timeline={} event={}",
                    entity.index(),
                    timeline,
                    event
                )
            }
            GameEvent::EntityDespawned { entity } => {
                write!(f, "EntityDespawned entity={}", entity.index())
            }
            GameEvent::CollisionStarted { a, b } => {
                write!(f, "CollisionStarted a={} b={}", a.index(), b.index())
            }
            GameEvent::CollisionEnded { a, b } => {
                write!(f, "CollisionEnded a={} b={}", a.index(), b.index())
            }
            GameEvent::CollisionForce { a, b, force } => {
                write!(f, "CollisionForce a={} b={} force={:.3}", a.index(), b.index(), force)
            }
            GameEvent::ScriptMessage { message } => write!(f, "ScriptMessage {message}"),
        }
    }
}

#[derive(Default, Resource)]
pub struct EventBus {
    events: Vec<GameEvent>,
}

impl EventBus {
    pub fn push(&mut self, event: GameEvent) {
        self.events.push(event);
    }

    pub fn drain(&mut self) -> Vec<GameEvent> {
        self.events.drain(..).collect()
    }
}
