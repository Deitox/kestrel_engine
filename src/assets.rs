use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

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
    pub regions: HashMap<String, Rect>,
    pub animations: HashMap<String, SpriteTimeline>,
}

#[derive(Clone, Debug)]
pub struct SpriteTimeline {
    pub name: String,
    pub looped: bool,
    pub frames: Vec<SpriteTimelineFrame>,
}

#[derive(Clone, Debug)]
pub struct SpriteTimelineFrame {
    pub region: String,
    pub duration: f32,
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
}

#[derive(Debug, Deserialize)]
struct AtlasTimelineFrameFile {
    region: String,
    #[serde(default = "default_frame_duration_ms")]
    duration_ms: u32,
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
        let regions = af.regions;
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
        regions: &HashMap<String, Rect>,
        raw: HashMap<String, AtlasTimelineFile>,
    ) -> HashMap<String, SpriteTimeline> {
        let mut animations = HashMap::new();
        for (timeline_name, data) in raw {
            let mut frames = Vec::new();
            for frame in data.frames {
                if !regions.contains_key(&frame.region) {
                    eprintln!(
                        "[assets] atlas '{atlas_key}': timeline '{timeline_name}' references unknown region '{}', skipping frame.",
                        frame.region
                    );
                    continue;
                }
                let duration = (frame.duration_ms.max(1) as f32) / 1000.0;
                frames.push(SpriteTimelineFrame { region: frame.region, duration });
            }
            if frames.is_empty() {
                eprintln!(
                    "[assets] atlas '{atlas_key}': timeline '{timeline_name}' has no valid frames, ignoring."
                );
                continue;
            }
            animations.insert(
                timeline_name.clone(),
                SpriteTimeline { name: timeline_name, looped: data.looped, frames },
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
        let r = atlas
            .regions
            .get(region)
            .ok_or_else(|| anyhow!("region '{region}' not found in atlas '{atlas_key}'"))?;
        let u0 = r.x as f32 / atlas.width as f32;
        let v0 = r.y as f32 / atlas.height as f32;
        let u1 = (r.x + r.w) as f32 / atlas.width as f32;
        let v1 = (r.y + r.h) as f32 / atlas.height as f32;
        Ok([u0, v0, u1, v1])
    }
    pub fn atlas_region_exists(&self, atlas_key: &str, region: &str) -> bool {
        self.atlases.get(atlas_key).and_then(|atlas| atlas.regions.get(region)).is_some()
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

    /// Stub hook for future atlas hot-reload integration.
    /// Accepts a list of atlas sources and logs the intent without installing watchers yet.
    pub fn watch_atlas_sources_stub<'a, I, P>(&self, paths: I)
    where
        I: IntoIterator<Item = &'a P>,
        P: AsRef<Path> + 'a,
    {
        let joined =
            paths.into_iter().map(|p| p.as_ref().display().to_string()).collect::<Vec<_>>().join(", ");
        if joined.is_empty() {
            println!("[assets] atlas hot-reload stub: no sources registered.");
        } else {
            println!(
                "[assets] atlas hot-reload stub registered for: {joined}. \
                 File watching will be implemented in Milestone 1."
            );
        }
    }
}
