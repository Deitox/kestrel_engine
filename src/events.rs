use bevy_ecs::prelude::{Entity, Resource};
use std::fmt;

#[derive(Debug, Clone)]
pub enum GameEvent {
    SpriteSpawned { entity: Entity, atlas: String, region: String },
    EntityDespawned { entity: Entity },
    Collision2d { a: Entity, b: Entity },
    ScriptMessage { message: String },
}

impl GameEvent {
    pub fn describes_collision_between(a: Entity, b: Entity) -> Self {
        let (first, second) = if a.index() <= b.index() { (a, b) } else { (b, a) };
        GameEvent::Collision2d { a: first, b: second }
    }
}

impl fmt::Display for GameEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GameEvent::SpriteSpawned { entity, atlas, region } => {
                write!(f, "SpriteSpawned entity={} atlas={} region={}", entity.index(), atlas, region)
            }
            GameEvent::EntityDespawned { entity } => {
                write!(f, "EntityDespawned entity={}", entity.index())
            }
            GameEvent::Collision2d { a, b } => {
                write!(f, "Collision2d a={} b={}", a.index(), b.index())
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
