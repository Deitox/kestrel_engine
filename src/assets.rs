use anyhow::{anyhow, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

pub struct AssetManager {
    atlases: HashMap<String, TextureAtlas>,
    sampler: Option<wgpu::Sampler>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    texture_cache: HashMap<String, (wgpu::TextureView, (u32, u32))>,
}

#[derive(Clone)]
pub struct TextureAtlas {
    pub image_key: String,
    pub width: u32,
    pub height: u32,
    pub regions: HashMap<String, Rect>,
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
}

impl AssetManager {
    pub fn new() -> Self {
        Self {
            atlases: HashMap::new(),
            sampler: None,
            device: None,
            queue: None,
            texture_cache: HashMap::new(),
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
        let bytes = fs::read(json_path)?;
        let af: AtlasFile = serde_json::from_slice(&bytes)?;
        let atlas = TextureAtlas {
            image_key: af.image.clone(),
            width: af.width,
            height: af.height,
            regions: af.regions,
        };
        self.atlases.insert(key.to_string(), atlas);
        Ok(())
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
}
