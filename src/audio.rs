use crate::events::GameEvent;
use crate::plugins::{EnginePlugin, PluginContext};
use anyhow::Result;
use rodio::source::{SineWave, Source};
use rodio::{OutputStream, OutputStreamHandle, Sink};
use std::any::Any;
use std::collections::VecDeque;
use std::time::Duration;

pub struct AudioManager {
    enabled: bool,
    capacity: usize,
    triggers: VecDeque<String>,
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    playback_available: bool,
}

impl AudioManager {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        match OutputStream::try_default() {
            Ok((stream, handle)) => Self {
                enabled: false,
                capacity,
                triggers: VecDeque::new(),
                _stream: Some(stream),
                handle: Some(handle),
                playback_available: true,
            },
            Err(err) => {
                eprintln!("Audio output unavailable: {err}");
                Self {
                    enabled: false,
                    capacity,
                    triggers: VecDeque::new(),
                    _stream: None,
                    handle: None,
                    playback_available: false,
                }
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn available(&self) -> bool {
        self.playback_available
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if !self.playback_available {
            self.enabled = false;
        } else {
            self.enabled = enabled;
        }
    }

    pub fn clear(&mut self) {
        self.triggers.clear();
    }

    pub fn recent_triggers(&self) -> impl ExactSizeIterator<Item = &String> {
        self.triggers.iter()
    }

    pub fn handle_event(&mut self, event: &GameEvent) {
        let label = match event {
            GameEvent::SpriteSpawned { atlas, region, .. } => format!("spawn:{}:{}", atlas, region),
            GameEvent::EntityDespawned { .. } => String::from("despawn"),
            GameEvent::CollisionStarted { .. } => String::from("collision"),
            GameEvent::CollisionEnded { .. } => String::from("collision_end"),
            GameEvent::CollisionForce { force, .. } => format!("collision_force:{force:.3}"),
            GameEvent::ScriptMessage { .. } => return,
        };
        self.push_trigger(label.clone());
        if self.enabled && self.playback_available {
            self.play_label(&label);
        }
    }

    fn push_trigger(&mut self, trigger: String) {
        if self.triggers.len() == self.capacity {
            self.triggers.pop_front();
        }
        self.triggers.push_back(trigger);
    }

    fn play_label(&mut self, label: &str) {
        let handle = match self.handle.as_ref() {
            Some(handle) => handle,
            None => return,
        };
        let mut force_magnitude = None;
        let frequency_hz = if label.starts_with("spawn") {
            440.0
        } else if label == "despawn" {
            330.0
        } else if label == "collision" {
            560.0
        } else if label == "collision_end" {
            280.0
        } else if let Some(force_str) = label.strip_prefix("collision_force:") {
            if let Ok(force) = force_str.parse::<f32>() {
                let clamped = force.clamp(0.0, 2000.0);
                force_magnitude = Some(clamped);
                360.0 + clamped * 0.12
            } else {
                return;
            }
        } else {
            return;
        };
        if let Ok(sink) = Sink::try_new(handle) {
            let amplitude = force_magnitude.map_or(0.18, |force| 0.12 + (force / 2000.0) * 0.2);
            let source =
                SineWave::new(frequency_hz).take_duration(Duration::from_millis(140)).amplify(amplitude);
            sink.append(source);
            sink.detach();
        }
    }
}

pub struct AudioPlugin {
    manager: AudioManager,
}

impl AudioPlugin {
    pub fn new(capacity: usize) -> Self {
        Self { manager: AudioManager::new(capacity) }
    }

    pub fn enabled(&self) -> bool {
        self.manager.enabled()
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.manager.set_enabled(enabled);
    }

    pub fn available(&self) -> bool {
        self.manager.available()
    }

    pub fn clear(&mut self) {
        self.manager.clear();
    }

    pub fn recent_triggers(&self) -> impl ExactSizeIterator<Item = &String> {
        self.manager.recent_triggers()
    }
}

impl EnginePlugin for AudioPlugin {
    fn name(&self) -> &'static str {
        "audio"
    }

    fn version(&self) -> &'static str {
        "1.0.0"
    }

    fn build(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        Ok(())
    }

    fn on_events(&mut self, _ctx: &mut PluginContext<'_>, events: &[GameEvent]) -> Result<()> {
        for event in events {
            self.manager.handle_event(event);
        }
        Ok(())
    }

    fn shutdown(&mut self, _ctx: &mut PluginContext<'_>) -> Result<()> {
        self.manager.clear();
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
