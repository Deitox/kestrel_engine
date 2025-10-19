use crate::events::GameEvent;
use std::collections::VecDeque;

pub struct AudioManager {
    enabled: bool,
    capacity: usize,
    triggers: VecDeque<String>,
}

impl AudioManager {
    pub fn new(capacity: usize) -> Self {
        Self { enabled: true, capacity: capacity.max(1), triggers: VecDeque::new() }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn clear(&mut self) {
        self.triggers.clear();
    }

    pub fn recent_triggers(&self) -> impl ExactSizeIterator<Item = &String> {
        self.triggers.iter()
    }

    pub fn handle_event(&mut self, event: &GameEvent) {
        if !self.enabled {
            return;
        }
        let label = match event {
            GameEvent::SpriteSpawned { atlas, region, .. } => Some(format!("spawn:{}:{}", atlas, region)),
            GameEvent::EntityDespawned { .. } => Some(String::from("despawn")),
            GameEvent::Collision2d { .. } => Some(String::from("collision")),
            GameEvent::ScriptMessage { .. } => None,
        };
        if let Some(label) = label {
            self.push_trigger(label);
        }
    }

    fn push_trigger(&mut self, trigger: String) {
        if self.triggers.len() == self.capacity {
            self.triggers.pop_front();
        }
        self.triggers.push_back(trigger);
    }
}
