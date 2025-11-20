use crate::events::{AudioEmitter, GameEvent};
use crate::plugins::{EnginePlugin, PluginContext};
use anyhow::Result;
use cpal::traits::{DeviceTrait, HostTrait};
use glam::Vec3;
use rodio::source::{SineWave, Source};
use rodio::{OutputStream, OutputStreamHandle, Sink, SpatialSink};
use std::any::Any;
use std::collections::VecDeque;
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct AudioListenerState {
    pub position: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
}

#[derive(Clone, Copy, Debug)]
pub struct AudioSpatialConfig {
    pub enabled: bool,
    pub min_distance: f32,
    pub max_distance: f32,
    pub pan_width: f32,
}

#[derive(Clone, Copy, Debug)]
struct SpatialParams {
    gain: f32,
    emitter: Vec3,
    left_ear: Vec3,
    right_ear: Vec3,
}

pub struct AudioManager {
    enabled: bool,
    capacity: usize,
    triggers: VecDeque<String>,
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    playback_available: bool,
    failed_playbacks: u32,
    last_error: Option<String>,
    device_name: Option<String>,
    sample_rate_hz: Option<u32>,
    listener: AudioListenerState,
    spatial: AudioSpatialConfig,
}

#[derive(Clone, Debug, Default)]
pub struct AudioHealthSnapshot {
    pub playback_available: bool,
    pub enabled: bool,
    pub failed_playbacks: u32,
    pub last_error: Option<String>,
    pub device_name: Option<String>,
    pub sample_rate_hz: Option<u32>,
}

#[derive(Clone, Debug, Default)]
struct AudioDeviceInfo {
    name: Option<String>,
    sample_rate_hz: Option<u32>,
}

impl AudioManager {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let device_info = AudioDeviceInfo::detect();
        let listener = AudioListenerState { position: Vec3::ZERO, forward: Vec3::new(0.0, 0.0, -1.0), up: Vec3::Y };
        let spatial = AudioSpatialConfig { enabled: true, min_distance: 0.1, max_distance: 25.0, pan_width: 10.0 };
        match OutputStream::try_default() {
            Ok((stream, handle)) => Self {
                enabled: false,
                capacity,
                triggers: VecDeque::new(),
                _stream: Some(stream),
                handle: Some(handle),
                playback_available: true,
                failed_playbacks: 0,
                last_error: None,
                device_name: device_info.name.clone(),
                sample_rate_hz: device_info.sample_rate_hz,
                listener,
                spatial,
            },
            Err(err) => {
                eprintln!(
                    "Audio output unavailable{}: {err}",
                    device_info.describe().map(|info| format!(" ({info})")).unwrap_or_default()
                );
                Self {
                    enabled: false,
                    capacity,
                    triggers: VecDeque::new(),
                    _stream: None,
                    handle: None,
                    playback_available: false,
                    failed_playbacks: 0,
                    last_error: Some(format!("Audio output unavailable: {err}")),
                    device_name: device_info.name,
                    sample_rate_hz: device_info.sample_rate_hz,
                    listener,
                    spatial,
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
        self.failed_playbacks = 0;
        self.last_error = None;
    }

    pub fn set_listener_state(&mut self, state: AudioListenerState) {
        self.listener = state;
    }

    pub fn spatial_config(&self) -> AudioSpatialConfig {
        self.spatial
    }

    pub fn set_spatial_config(&mut self, cfg: AudioSpatialConfig) {
        self.spatial = cfg;
    }

    pub fn recent_triggers(&self) -> impl ExactSizeIterator<Item = &String> {
        self.triggers.iter()
    }

    pub fn health_snapshot(&self) -> AudioHealthSnapshot {
        AudioHealthSnapshot {
            playback_available: self.playback_available,
            enabled: self.enabled,
            failed_playbacks: self.failed_playbacks,
            last_error: self.last_error.clone(),
            device_name: self.device_name.clone(),
            sample_rate_hz: self.sample_rate_hz,
        }
    }

    pub fn handle_event(&mut self, event: &GameEvent) {
        let (label, emitter, base_amp) = match event {
            GameEvent::SpriteSpawned { atlas, region, audio, .. } => {
                (format!("spawn:{}:{}", atlas, region), audio.as_ref(), 0.18)
            }
            GameEvent::EntityDespawned { .. } => (String::from("despawn"), None, 0.18),
            GameEvent::CollisionStarted { audio, .. } => (String::from("collision"), audio.as_ref(), 0.18),
            GameEvent::CollisionEnded { .. } => (String::from("collision_end"), None, 0.18),
            GameEvent::CollisionForce { force, audio, .. } => {
                let amplitude = (force / 2000.0).clamp(0.0, 1.0);
                (format!("collision_force:{force:.3}"), audio.as_ref(), 0.12 + amplitude * 0.2)
            }
            GameEvent::SpriteAnimationEvent { .. } => return,
            GameEvent::ScriptMessage { .. } => return,
        };
        self.push_trigger(label.clone());
        if self.enabled && self.playback_available {
            let spatial = emitter.and_then(|em| self.compute_spatial(em));
            let gain = spatial.as_ref().map(|s| s.gain).unwrap_or(1.0);
            self.play_label(&label, base_amp * gain, spatial);
        }
    }

    fn push_trigger(&mut self, trigger: String) {
        if self.triggers.len() == self.capacity {
            self.triggers.pop_front();
        }
        self.triggers.push_back(trigger);
    }

    fn play_label(&mut self, label: &str, gain: f32, spatial: Option<SpatialParams>) {
        let handle = match self.handle.as_ref() {
            Some(handle) => handle,
            None => {
                self.record_failure("Audio handle unavailable");
                return;
            }
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
        let amplitude = force_magnitude.map_or(0.18, |force| 0.12 + (force / 2000.0) * 0.2) * gain;
        if let Some(spatial) = spatial {
            if let Ok(sink) = SpatialSink::try_new(
                handle,
                spatial.emitter.to_array(),
                spatial.left_ear.to_array(),
                spatial.right_ear.to_array(),
            ) {
                let source =
                    SineWave::new(frequency_hz).take_duration(Duration::from_millis(140)).amplify(amplitude);
                sink.append(source);
                sink.detach();
                self.last_error = None;
                return;
            }
        }
        match Sink::try_new(handle) {
            Ok(sink) => {
                let source =
                    SineWave::new(frequency_hz).take_duration(Duration::from_millis(140)).amplify(amplitude);
                sink.append(source);
                sink.detach();
                self.last_error = None;
            }
            Err(err) => {
                self.record_failure(format!("Failed to create audio sink: {err}"));
            }
        }
    }

    fn record_failure(&mut self, message: impl Into<String>) {
        self.failed_playbacks = self.failed_playbacks.saturating_add(1);
        self.last_error = Some(message.into());
    }

    fn compute_spatial(&self, emitter: &AudioEmitter) -> Option<SpatialParams> {
        if !self.spatial.enabled {
            return None;
        }
        let right = self.listener.forward.cross(self.listener.up).normalize_or_zero();
        if right.length_squared() <= f32::EPSILON {
            return None;
        }
        let rel = emitter.position - self.listener.position;
        let distance = rel.length();
        let max_distance = self
            .spatial
            .max_distance
            .min(emitter.max_distance.max(self.spatial.min_distance + 0.001));
        let range = (max_distance - self.spatial.min_distance).max(0.001);
        let t = ((distance - self.spatial.min_distance) / range).clamp(0.0, 1.0);
        let gain = (1.0 - t).powi(2);
        let head_width = 0.3;
        let half = right * (head_width * 0.5);
        Some(SpatialParams {
            gain,
            emitter: rel,
            left_ear: -half,
            right_ear: half,
        })
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

    pub fn set_listener_state(&mut self, state: AudioListenerState) {
        self.manager.set_listener_state(state);
    }

    pub fn spatial_config(&self) -> AudioSpatialConfig {
        self.manager.spatial_config()
    }

    pub fn set_spatial_config(&mut self, cfg: AudioSpatialConfig) {
        self.manager.set_spatial_config(cfg);
    }

    pub fn recent_triggers(&self) -> impl ExactSizeIterator<Item = &String> {
        self.manager.recent_triggers()
    }

    pub fn health_snapshot(&self) -> AudioHealthSnapshot {
        self.manager.health_snapshot()
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

impl AudioDeviceInfo {
    fn detect() -> Self {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            return Self::default();
        };
        let name = device.name().ok();
        let sample_rate_hz = device.default_output_config().ok().map(|config| config.sample_rate().0);
        Self { name, sample_rate_hz }
    }

    fn describe(&self) -> Option<String> {
        match (self.name.as_deref(), self.sample_rate_hz) {
            (Some(name), Some(rate)) => Some(format!("{name} @ {rate} Hz")),
            (Some(name), None) => Some(name.to_string()),
            (None, Some(rate)) => Some(format!("{rate} Hz")),
            _ => None,
        }
    }
}
