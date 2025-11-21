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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpriteGuardrailMode {
    Off,
    Warn,
    Clamp,
    Strict,
}

impl SpriteGuardrailMode {
    pub fn label(self) -> &'static str {
        match self {
            SpriteGuardrailMode::Off => "Off",
            SpriteGuardrailMode::Warn => "Warn",
            SpriteGuardrailMode::Clamp => "Clamp & Zoom",
            SpriteGuardrailMode::Strict => "Strict (hide sprites)",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct EditorConfig {
    #[serde(default = "EditorConfig::default_zoom_min")]
    pub camera_zoom_min: f32,
    #[serde(default = "EditorConfig::default_zoom_max")]
    pub camera_zoom_max: f32,
    #[serde(default = "EditorConfig::default_sprite_guard_max_pixels")]
    pub sprite_guard_max_pixels: f32,
    #[serde(default = "EditorConfig::default_guardrail_mode")]
    pub sprite_guardrail_mode: SpriteGuardrailMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MeshHashAlgorithm {
    #[default]
    Blake3,
    Metadata,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MeshConfig {
    #[serde(default)]
    pub hash_algorithm: MeshHashAlgorithm,
    #[serde(default = "MeshConfig::default_cache_limit")]
    pub hash_cache_limit: usize,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self { hash_algorithm: MeshHashAlgorithm::default(), hash_cache_limit: Self::default_cache_limit() }
    }
}

impl MeshConfig {
    const fn default_cache_limit() -> usize {
        512
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShadowConfig {
    #[serde(default = "ShadowConfig::default_cascade_count")]
    pub cascade_count: u32,
    #[serde(default = "ShadowConfig::default_resolution")]
    pub resolution: u32,
    #[serde(default = "ShadowConfig::default_split_lambda")]
    pub split_lambda: f32,
    #[serde(default = "ShadowConfig::default_pcf_radius")]
    pub pcf_radius: f32,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct AppConfig {
    pub window: WindowConfig,
    #[serde(default)]
    pub particles: ParticleConfig,
    #[serde(default)]
    pub mesh: MeshConfig,
    #[serde(default)]
    pub shadow: ShadowConfig,
    #[serde(default)]
    pub editor: EditorConfig,
}

#[derive(Debug, Clone, Default)]
pub struct AppConfigOverrides {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub vsync: Option<bool>,
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

impl EditorConfig {
    const fn default_zoom_min() -> f32 {
        0.25
    }

    const fn default_zoom_max() -> f32 {
        5.0
    }

    const fn default_sprite_guard_max_pixels() -> f32 {
        2048.0
    }

    fn default_guardrail_mode() -> SpriteGuardrailMode {
        SpriteGuardrailMode::Warn
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            camera_zoom_min: Self::default_zoom_min(),
            camera_zoom_max: Self::default_zoom_max(),
            sprite_guard_max_pixels: Self::default_sprite_guard_max_pixels(),
            sprite_guardrail_mode: Self::default_guardrail_mode(),
        }
    }
}

impl ShadowConfig {
    const fn default_cascade_count() -> u32 {
        4
    }

    const fn default_resolution() -> u32 {
        2048
    }

    const fn default_split_lambda() -> f32 {
        0.6
    }

    const fn default_pcf_radius() -> f32 {
        1.25
    }
}

impl Default for ShadowConfig {
    fn default() -> Self {
        Self {
            cascade_count: Self::default_cascade_count(),
            resolution: Self::default_resolution(),
            split_lambda: Self::default_split_lambda(),
            pcf_radius: Self::default_pcf_radius(),
        }
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

    pub fn apply_overrides(&mut self, overrides: &AppConfigOverrides) {
        if let Some(width) = overrides.width {
            self.window.width = width;
        }
        if let Some(height) = overrides.height {
            self.window.height = height;
        }
        if let Some(vsync) = overrides.vsync {
            self.window.vsync = vsync;
        }
    }
}

impl AppConfigOverrides {
    pub fn is_empty(&self) -> bool {
        self.width.is_none() && self.height.is_none() && self.vsync.is_none()
    }

    pub fn applied_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.width.is_some() {
            fields.push("width");
        }
        if self.height.is_some() {
            fields.push("height");
        }
        if self.vsync.is_some() {
            fields.push("vsync");
        }
        fields
    }
}
