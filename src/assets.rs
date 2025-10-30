use crate::ecs::{SpriteAnimationFrame, SpriteAnimationLoopMode};
use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;

pub struct AssetManager {
    atlases: HashMap<String, TextureAtlas>,
    sampler: Option<wgpu::Sampler>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    texture_cache: HashMap<String, (wgpu::TextureView, (u32, u32))>,
    atlas_sources: HashMap<String, String>,
    atlas_refs: HashMap<String, usize>,
}

#[derive(Clone)]
pub struct TextureAtlas {
    pub image_key: String,
    pub width: u32,
    pub height: u32,
    pub regions: HashMap<Arc<str>, AtlasRegion>,
    pub animations: HashMap<String, SpriteTimeline>,
}

#[derive(Clone)]
pub struct AtlasRegion {
    pub id: u16,
    pub rect: Rect,
    pub uv: [f32; 4],
}

#[derive(Clone)]
pub struct SpriteTimeline {
    pub name: Arc<str>,
    pub looped: bool,
    pub loop_mode: SpriteAnimationLoopMode,
    pub frames: Arc<[SpriteAnimationFrame]>,
    pub durations: Arc<[f32]>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}
#[derive(Deserialize)]
struct AtlasFile {
    image: String,
    width: u32,
    height: u32,
    regions: HashMap<String, Rect>,
    #[serde(default)]
    animations: HashMap<String, AtlasTimelineFile>,
}

#[derive(Debug, Deserialize)]
struct AtlasTimelineFile {
    #[serde(default)]
    frames: Vec<AtlasTimelineFrameFile>,
    #[serde(default = "default_timeline_loop")]
    looped: bool,
    #[serde(default)]
    loop_mode: Option<String>,
    #[serde(default)]
    events: Vec<AtlasTimelineEventFile>,
}

#[derive(Debug, Deserialize)]
struct AtlasTimelineFrameFile {
    #[serde(default)]
    name: Option<String>,
    region: String,
    #[serde(default = "default_frame_duration_ms")]
    duration_ms: u32,
}

#[derive(Debug, Deserialize)]
struct AtlasTimelineEventFile {
    frame: usize,
    name: String,
}

const fn default_timeline_loop() -> bool {
    true
}

const fn default_frame_duration_ms() -> u32 {
    100
}

impl AssetManager {
    pub fn new() -> Self {
        Self {
            atlases: HashMap::new(),
            sampler: None,
            device: None,
            queue: None,
            texture_cache: HashMap::new(),
            atlas_sources: HashMap::new(),
            atlas_refs: HashMap::new(),
        }
    }
    pub fn set_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.device = Some(device.clone());
        self.queue = Some(queue.clone());
        self.sampler = Some(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Default Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        }));
    }
    pub fn default_sampler(&self) -> &wgpu::Sampler {
        self.sampler.as_ref().expect("sampler")
    }
    pub fn load_atlas(&mut self, key: &str, json_path: &str) -> Result<()> {
        self.load_atlas_internal(key, json_path)?;
        Ok(())
    }
    fn load_atlas_internal(&mut self, key: &str, json_path: &str) -> Result<()> {
        let bytes = fs::read(json_path)?;
        let af: AtlasFile = serde_json::from_slice(&bytes)?;
        let mut regions = HashMap::new();
        for (index, (name, rect)) in af.regions.into_iter().enumerate() {
            let id =
                u16::try_from(index).map_err(|_| anyhow!("Atlas '{key}' has more than 65535 regions"))?;
            let name_arc: Arc<str> = Arc::from(name);
            let uv = [
                rect.x as f32 / af.width as f32,
                rect.y as f32 / af.height as f32,
                (rect.x + rect.w) as f32 / af.width as f32,
                (rect.y + rect.h) as f32 / af.height as f32,
            ];
            regions.insert(Arc::clone(&name_arc), AtlasRegion { id, rect, uv });
        }
        let animations = Self::parse_timelines(key, &regions, af.animations);
        let atlas = TextureAtlas {
            image_key: af.image.clone(),
            width: af.width,
            height: af.height,
            regions,
            animations,
        };
        self.atlases.insert(key.to_string(), atlas);
        self.atlas_sources.insert(key.to_string(), json_path.to_string());
        Ok(())
    }

    fn parse_timelines(
        atlas_key: &str,
        regions: &HashMap<Arc<str>, AtlasRegion>,
        raw: HashMap<String, AtlasTimelineFile>,
    ) -> HashMap<String, SpriteTimeline> {
        let mut animations = HashMap::new();
        for (timeline_key, mut data) in raw {
            let mut frames = Vec::new();
            let mut durations = Vec::new();
            let mut event_map: HashMap<usize, Vec<String>> = HashMap::new();
            for event in data.events.drain(..) {
                event_map.entry(event.frame).or_default().push(event.name);
            }
            for (frame_index, frame) in data.frames.into_iter().enumerate() {
                let Some((region_key, region_info)) = regions.get_key_value(frame.region.as_str()) else {
                    eprintln!(
                        "[assets] atlas '{atlas_key}': timeline '{timeline_key}' references unknown region '{}', skipping frame.",
                        frame.region
                    );
                    continue;
                };
                let frame_name_arc =
                    frame.name.map(Arc::<str>::from).unwrap_or_else(|| Arc::clone(region_key));
                let duration = (frame.duration_ms.max(1) as f32) / 1000.0;
                let event_names = event_map.remove(&frame_index).unwrap_or_default();
                let events: Vec<Arc<str>> =
                    event_names.into_iter().map(|name| Arc::<str>::from(name)).collect();
                frames.push(SpriteAnimationFrame {
                    name: frame_name_arc,
                    region: Arc::clone(region_key),
                    region_id: region_info.id,
                    duration,
                    uv: region_info.uv,
                    events: Arc::from(events),
                });
                durations.push(duration);
            }
            if frames.is_empty() {
                eprintln!(
                    "[assets] atlas '{atlas_key}': timeline '{timeline_key}' has no valid frames, ignoring."
                );
                continue;
            }
            let mode_str = data.loop_mode.clone().unwrap_or_else(|| {
                if data.looped {
                    "loop".to_string()
                } else {
                    "once_stop".to_string()
                }
            });
            let mode_enum = SpriteAnimationLoopMode::from_str(&mode_str);
            let looped = mode_enum.looped();
            for (frame, names) in event_map {
                eprintln!(
                    "[assets] atlas '{atlas_key}': timeline '{timeline_key}' has events {:?} referencing missing frame index {}.",
                    names, frame
                );
            }
            let timeline_arc = Arc::<str>::from(timeline_key.clone());
            animations.insert(
                timeline_key.clone(),
                SpriteTimeline {
                    name: timeline_arc,
                    looped,
                    loop_mode: mode_enum,
                    frames: Arc::from(frames),
                    durations: Arc::from(durations),
                },
            );
        }
        animations
    }
    pub fn retain_atlas(&mut self, key: &str, json_path: Option<&str>) -> Result<()> {
        if self.atlases.contains_key(key) {
            *self.atlas_refs.entry(key.to_string()).or_insert(0) += 1;
            if let Some(path) = json_path {
                self.atlas_sources.insert(key.to_string(), path.to_string());
            }
            return Ok(());
        }
        let path_owned = if let Some(path) = json_path {
            path.to_string()
        } else if let Some(stored) = self.atlas_sources.get(key) {
            stored.clone()
        } else {
            return Err(anyhow!("Atlas '{key}' is not loaded and no JSON path provided to retain it."));
        };
        self.load_atlas_internal(key, &path_owned)?;
        self.atlas_sources.insert(key.to_string(), path_owned);
        self.atlas_refs.insert(key.to_string(), 1);
        Ok(())
    }
    pub fn atlas_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.atlases.keys().cloned().collect();
        keys.sort();
        keys
    }
    pub fn release_atlas(&mut self, key: &str) -> bool {
        if let Some(count) = self.atlas_refs.get_mut(key) {
            if *count > 0 {
                *count -= 1;
                if *count == 0 {
                    self.atlas_refs.remove(key);
                    if let Some(atlas) = self.atlases.remove(key) {
                        let image_path = format!("assets/images/{}", atlas.image_key);
                        self.texture_cache.remove(&image_path);
                    }
                    self.atlas_sources.remove(key);
                }
                return true;
            }
        }
        false
    }
    pub fn atlas_ref_count(&self, key: &str) -> usize {
        self.atlas_refs.get(key).copied().unwrap_or(0)
    }
    pub fn atlas_texture_view(&mut self, key: &str) -> Result<wgpu::TextureView> {
        self.load_or_reload_view(key, false)
    }
    fn load_or_reload_view(&mut self, key: &str, force: bool) -> Result<wgpu::TextureView> {
        let atlas = self.atlases.get(key).ok_or_else(|| anyhow!("atlas '{key}' not loaded"))?;
        let image_path = format!("assets/images/{}", atlas.image_key);
        if !force {
            if let Some((view, _)) = self.texture_cache.get(&image_path) {
                return Ok(view.clone());
            }
        }
        let dev = self.device.as_ref().ok_or_else(|| anyhow!("GPU device not initialized"))?;
        let q = self.queue.as_ref().ok_or_else(|| anyhow!("GPU queue not initialized"))?;
        let bytes = std::fs::read(&image_path)?;
        let img = image::load_from_memory(&bytes)?.to_rgba8();
        let (w, h) = img.dimensions();
        let rgba = img.into_raw();
        let texture = dev.create_texture(&wgpu::TextureDescriptor {
            label: Some("Atlas Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        q.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.texture_cache.insert(image_path, (view.clone(), (w, h)));
        Ok(view)
    }
    pub fn atlas_region_uv(&self, atlas_key: &str, region: &str) -> Result<[f32; 4]> {
        let atlas = self.atlases.get(atlas_key).ok_or_else(|| anyhow!("atlas '{atlas_key}' not loaded"))?;
        let (_, info) = atlas
            .regions
            .get_key_value(region)
            .ok_or_else(|| anyhow!("region '{region}' not found in atlas '{atlas_key}'"))?;
        Ok(info.uv)
    }
    pub fn atlas_region_exists(&self, atlas_key: &str, region: &str) -> bool {
        self.atlases.get(atlas_key).and_then(|atlas| atlas.regions.get(region)).is_some()
    }
    pub fn atlas_region_info(&self, atlas_key: &str, region: &str) -> Option<(&Arc<str>, &AtlasRegion)> {
        self.atlases.get(atlas_key).and_then(|atlas| atlas.regions.get_key_value(region))
    }
    pub fn atlas_region_names(&self, atlas_key: &str) -> Vec<String> {
        self.atlases
            .get(atlas_key)
            .map(|atlas| {
                let mut names: Vec<String> =
                    atlas.regions.keys().map(|name| name.as_ref().to_string()).collect();
                names.sort();
                names
            })
            .unwrap_or_default()
    }
    pub fn atlas_timeline(&self, atlas_key: &str, name: &str) -> Option<&SpriteTimeline> {
        self.atlases.get(atlas_key).and_then(|atlas| atlas.animations.get(name))
    }
    pub fn atlas_timeline_names(&self, atlas_key: &str) -> Vec<String> {
        self.atlases
            .get(atlas_key)
            .map(|atlas| atlas.animations.keys().cloned().collect())
            .unwrap_or_default()
    }
    pub fn has_atlas(&self, key: &str) -> bool {
        self.atlases.contains_key(key)
    }
    pub fn atlas_source(&self, key: &str) -> Option<&str> {
        self.atlas_sources.get(key).map(|s| s.as_str())
    }

    pub fn atlas_sources(&self) -> Vec<(String, String)> {
        self.atlas_sources.iter().map(|(key, path)| (key.clone(), path.clone())).collect()
    }

    pub fn reload_atlas(&mut self, key: &str) -> Result<()> {
        let source = self
            .atlas_sources
            .get(key)
            .cloned()
            .ok_or_else(|| anyhow!("Atlas '{key}' has no recorded source; cannot hot-reload"))?;

        let previous_image = self.atlases.get(key).map(|atlas| format!("assets/images/{}", atlas.image_key));

        self.load_atlas_internal(key, &source)?;

        if let Some(image_path) = previous_image {
            self.texture_cache.remove(&image_path);
        }
        if let Some(current) = self.atlases.get(key) {
            let image_path = format!("assets/images/{}", current.image_key);
            self.texture_cache.remove(&image_path);
            if self.device.is_some() {
                if let Err(err) = self.load_or_reload_view(key, true) {
                    eprintln!("[assets] Warning: failed to refresh GPU texture for atlas '{key}': {err}");
                }
            }
        }
        Ok(())
    }
}
