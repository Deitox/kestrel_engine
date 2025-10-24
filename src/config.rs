use anyhow::{Context, Result};
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct WindowConfig {
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub vsync: bool,
    pub fullscreen: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ParticleConfig {
    #[serde(default = "ParticleConfig::default_max_spawn_per_frame")]
    pub max_spawn_per_frame: u32,
    #[serde(default = "ParticleConfig::default_max_total")]
    pub max_total: u32,
    #[serde(default = "ParticleConfig::default_max_emitter_backlog")]
    pub max_emitter_backlog: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub window: WindowConfig,
    #[serde(default)]
    pub particles: ParticleConfig,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { title: "Kestrel Engine".to_string(), width: 1280, height: 720, vsync: true, fullscreen: false }
    }
}

impl ParticleConfig {
    const fn default_max_spawn_per_frame() -> u32 {
        256
    }

    const fn default_max_total() -> u32 {
        2_000
    }

    fn default_max_emitter_backlog() -> f32 {
        64.0
    }
}

impl Default for ParticleConfig {
    fn default() -> Self {
        Self {
            max_spawn_per_frame: Self::default_max_spawn_per_frame(),
            max_total: Self::default_max_total(),
            max_emitter_backlog: Self::default_max_emitter_backlog(),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { window: WindowConfig::default(), particles: ParticleConfig::default() }
    }
}

impl AppConfig {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes =
            fs::read(path).with_context(|| format!("Failed to read config file {}", path.display()))?;
        let cfg = serde_json::from_slice(&bytes)
            .with_context(|| format!("Failed to parse config file {}", path.display()))?;
        Ok(cfg)
    }

    pub fn load_or_default(path: impl AsRef<Path>) -> Self {
        match Self::load(path) {
            Ok(cfg) => cfg,
            Err(err) => {
                eprintln!("Config load error: {err:?}. Falling back to defaults.");
                Self::default()
            }
        }
    }
}
