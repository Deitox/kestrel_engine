use crate::events::GameEvent;
use rodio::source::{SineWave, Source};
use rodio::{OutputStream, OutputStreamHandle, Sink};
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
            GameEvent::Collision2d { .. } => String::from("collision"),
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
        let frequency_hz = if label.starts_with("spawn") {
            440.0
        } else if label == "despawn" {
            330.0
        } else if label == "collision" {
            560.0
        } else {
            return;
        };
        if let Ok(sink) = Sink::try_new(handle) {
            let source = SineWave::new(frequency_hz).take_duration(Duration::from_millis(140)).amplify(0.18);
            sink.append(source);
            sink.detach();
        }
    }
}
