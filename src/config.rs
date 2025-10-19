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
pub struct AppConfig {
    pub window: WindowConfig,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { title: "Kestrel Engine".to_string(), width: 1280, height: 720, vsync: true, fullscreen: false }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { window: WindowConfig::default() }
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
