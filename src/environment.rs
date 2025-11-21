use crate::renderer::Renderer;
use anyhow::{anyhow, Context, Result};
use glam::{Vec2, Vec3};
use half::f16;
use image::{DynamicImage, ImageReader};
use std::collections::HashMap;
use std::f32::consts::{PI, TAU};
use std::fs;
use std::path::Path;
use std::sync::Arc;

const DIFFUSE_RESOLUTION: u32 = 32;
const SPECULAR_BASE_RESOLUTION: u32 = 128;
const SPECULAR_MIP_COUNT: u32 = 6;
const BRDF_LUT_SIZE: u32 = 256;
const DIFFUSE_SAMPLE_COUNT: usize = 64;
const SPECULAR_SAMPLE_COUNT: usize = 128;
const BRDF_SAMPLE_COUNT: usize = 128;

pub struct EnvironmentRegistry {
    environments: HashMap<String, EnvironmentEntry>,
    default_key: String,
    sampler: Option<Arc<wgpu::Sampler>>,
    revision: u64,
}

struct EnvironmentEntry {
    definition: EnvironmentDefinition,
    maps: Option<EnvironmentMaps>,
    gpu: Option<Arc<EnvironmentGpu>>,
    ref_count: usize,
    permanent: bool,
}

#[derive(Clone)]
pub struct EnvironmentDefinition {
    key: String,
    label: String,
    source: Option<String>,
}

#[derive(Clone)]
struct EnvironmentMaps {
    diffuse: Cubemap,
    specular: PrefilteredCubemap,
    brdf: Lut2D,
}

#[derive(Clone)]
struct Cubemap {
    size: u32,
    faces: [Vec<f32>; 6],
}

#[derive(Clone)]
struct PrefilteredCubemap {
    base_size: u32,
    levels: Vec<CubemapLevel>,
}

#[derive(Clone)]
struct CubemapLevel {
    size: u32,
    faces: [Vec<f32>; 6],
}

#[derive(Clone)]
struct Lut2D {
    width: u32,
    height: u32,
    data: Vec<f32>,
}

#[derive(Clone)]
struct HdrImage {
    width: u32,
    height: u32,
    pixels: Vec<Vec3>,
}

pub struct EnvironmentGpu {
    _diffuse_texture: Arc<wgpu::Texture>,
    diffuse_view: Arc<wgpu::TextureView>,
    _specular_texture: Arc<wgpu::Texture>,
    specular_view: Arc<wgpu::TextureView>,
    _brdf_texture: Arc<wgpu::Texture>,
    brdf_view: Arc<wgpu::TextureView>,
    sampler: Arc<wgpu::Sampler>,
    specular_mip_count: u32,
}

impl EnvironmentRegistry {
    pub fn new() -> Self {
        let (default_definition, default_maps) = EnvironmentDefinition::generated_default();
        let default_key = default_definition.key().to_string();
        let mut registry = Self {
            environments: HashMap::new(),
            default_key: default_key.clone(),
            sampler: None,
            revision: 0,
        };
        registry.environments.insert(
            default_key,
            EnvironmentEntry {
                definition: default_definition,
                maps: Some(default_maps),
                gpu: None,
                ref_count: 1,
                permanent: true,
            },
        );
        registry.bump_revision();
        registry
    }

    pub fn load_directory<P: AsRef<Path>>(&mut self, dir: P) -> Result<Vec<String>> {
        let dir_path = dir.as_ref();
        if !dir_path.exists() {
            return Ok(Vec::new());
        }
        let mut loaded = Vec::new();
        let entries = fs::read_dir(dir_path)
            .with_context(|| format!("reading environment directory '{}'", dir_path.display()))?;
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let source_path = entry.path();
            if !is_supported_environment_file(&source_path) {
                continue;
            }
            let Some(key) = environment_key_from_path(&source_path) else {
                continue;
            };
            if self.environments.contains_key(&key) {
                continue;
            }
            let (definition, maps) =
                EnvironmentDefinition::from_path(key.clone(), source_path.to_string_lossy().into_owned())
                    .with_context(|| format!("processing environment '{}'", source_path.display()))?;
            self.environments.insert(
                key.clone(),
                EnvironmentEntry { definition, maps: Some(maps), gpu: None, ref_count: 0, permanent: false },
            );
            self.bump_revision();
            loaded.push(key);
        }
        Ok(loaded)
    }

    pub fn default_key(&self) -> &str {
        &self.default_key
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.environments.keys()
    }

    pub fn definition(&self, key: &str) -> Option<&EnvironmentDefinition> {
        self.environments.get(key).map(|entry| &entry.definition)
    }

    pub fn retain(&mut self, key: &str, source: Option<&str>) -> Result<()> {
        if let Some(entry) = self.environments.get_mut(key) {
            entry.ref_count = entry.ref_count.saturating_add(1);
            if let Some(path) = source {
                let new_source = path.to_string();
                if entry.definition.source() != Some(new_source.as_str()) {
                    entry.definition.set_source(Some(new_source));
                    entry.maps = None;
                    entry.gpu = None;
                }
            }
            return Ok(());
        }
        let path = source
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("Environment '{key}' not loaded and no source provided."))?;
        let (definition, maps) = EnvironmentDefinition::from_path(key.to_string(), path.clone())
            .with_context(|| format!("Failed to load environment '{key}' from {path}"))?;
        self.environments.insert(
            key.to_string(),
            EnvironmentEntry { definition, maps: Some(maps), gpu: None, ref_count: 1, permanent: false },
        );
        self.bump_revision();
        Ok(())
    }

    pub fn release(&mut self, key: &str) -> bool {
        if let Some(entry) = self.environments.get_mut(key) {
            if entry.permanent {
                return true;
            }
            if entry.ref_count > 0 {
                entry.ref_count -= 1;
            }
            if entry.ref_count == 0 {
                entry.gpu = None;
                entry.maps = None;
            }
            return true;
        }
        false
    }

    pub fn ref_count(&self, key: &str) -> Option<usize> {
        self.environments.get(key).map(|entry| entry.ref_count)
    }

    pub fn version(&self) -> u64 {
        self.revision
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub fn ensure_gpu(&mut self, key: &str, renderer: &mut Renderer) -> Result<Arc<EnvironmentGpu>> {
        let sampler = self.ensure_sampler(renderer)?;
        let gpu = {
            let entry =
                self.environments.get_mut(key).ok_or_else(|| anyhow!("Environment '{key}' not retained"))?;
            if let Some(gpu) = entry.gpu.as_ref() {
                return Ok(gpu.clone());
            }
            if entry.maps.is_none() {
                let source = entry
                    .definition
                    .source()
                    .ok_or_else(|| anyhow!("Environment '{key}' has no recorded source; cannot rebuild"))?
                    .to_string();
                let maps = EnvironmentMaps::from_path(&source)
                    .with_context(|| format!("Failed to reload environment '{key}' from {source}"))?;
                entry.maps = Some(maps);
            }
            let maps = entry.maps.as_ref().expect("environment maps should be initialized");
            let gpu = EnvironmentGpu::new(renderer, maps, sampler.clone())
                .with_context(|| format!("Failed to upload environment '{key}'"))?;
            let gpu = Arc::new(gpu);
            entry.gpu = Some(gpu.clone());
            gpu
        };
        Ok(gpu)
    }

    pub fn sampler(&mut self, renderer: &mut Renderer) -> Result<Arc<wgpu::Sampler>> {
        self.ensure_sampler(renderer)
    }

    fn ensure_sampler(&mut self, renderer: &Renderer) -> Result<Arc<wgpu::Sampler>> {
        if let Some(sampler) = self.sampler.as_ref() {
            return Ok(sampler.clone());
        }
        let device = renderer.device()?.clone();
        let sampler = Arc::new(device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Environment Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        }));
        self.sampler = Some(sampler.clone());
        Ok(sampler)
    }
}

impl EnvironmentDefinition {
    fn generated_default() -> (Self, EnvironmentMaps) {
        let image = generate_default_hdr();
        let maps = EnvironmentMaps::from_hdr(&image);
        (
            Self {
                key: "environment::default".to_string(),
                label: "Neutral Gradient".to_string(),
                source: None,
            },
            maps,
        )
    }

    fn from_path(key: String, path: String) -> Result<(Self, EnvironmentMaps)> {
        let maps = EnvironmentMaps::from_path(&path)?;
        let label = Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| key.clone());
        Ok((Self { key, label, source: Some(path) }, maps))
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    pub fn set_source(&mut self, source: Option<String>) {
        self.source = source;
    }
}

impl EnvironmentMaps {
    fn from_hdr(image: &HdrImage) -> Self {
        let diffuse = compute_diffuse_cubemap(image, DIFFUSE_RESOLUTION);
        let specular = compute_specular_cubemap(image, SPECULAR_BASE_RESOLUTION, SPECULAR_MIP_COUNT);
        let brdf = compute_brdf_lut(BRDF_LUT_SIZE);
        Self { diffuse, specular, brdf }
    }

    fn from_path(path: &str) -> Result<Self> {
        let image = load_hdr_image(path)?;
        Ok(Self::from_hdr(&image))
    }
}

fn f32_to_f16_bits(data: &[f32]) -> Vec<u16> {
    data.iter().map(|value| f16::from_f32(*value).to_bits()).collect()
}

impl EnvironmentGpu {
    fn new(renderer: &Renderer, maps: &EnvironmentMaps, sampler: Arc<wgpu::Sampler>) -> Result<Self> {
        let device = renderer.device()?.clone();
        let queue = renderer.queue()?.clone();

        let diffuse_texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Environment Diffuse Cube"),
            size: wgpu::Extent3d {
                width: maps.diffuse.size,
                height: maps.diffuse.size,
                depth_or_array_layers: 6,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        for face in 0..6 {
            let face_data = &maps.diffuse.faces[face];
            let face_half = f32_to_f16_bits(face_data);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &diffuse_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: face as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                bytemuck::cast_slice(&face_half),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some((maps.diffuse.size * 8) as u32),
                    rows_per_image: Some(maps.diffuse.size),
                },
                wgpu::Extent3d {
                    width: maps.diffuse.size,
                    height: maps.diffuse.size,
                    depth_or_array_layers: 1,
                },
            );
        }
        let diffuse_view = Arc::new(diffuse_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Environment Diffuse View"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        }));

        let mip_count = maps.specular.levels.len().max(1) as u32;
        let specular_texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Environment Specular Cube"),
            size: wgpu::Extent3d {
                width: maps.specular.base_size,
                height: maps.specular.base_size,
                depth_or_array_layers: 6,
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        for (level_idx, level) in maps.specular.levels.iter().enumerate() {
            for face in 0..6 {
                let face_half = f32_to_f16_bits(&level.faces[face]);
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &specular_texture,
                        mip_level: level_idx as u32,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: face as u32 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&face_half),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some((level.size * 8) as u32),
                        rows_per_image: Some(level.size),
                    },
                    wgpu::Extent3d { width: level.size, height: level.size, depth_or_array_layers: 1 },
                );
            }
        }
        let specular_view = Arc::new(specular_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Environment Specular View"),
            dimension: Some(wgpu::TextureViewDimension::Cube),
            base_mip_level: 0,
            mip_level_count: Some(mip_count),
            ..Default::default()
        }));

        let brdf_texture = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Environment BRDF LUT"),
            size: wgpu::Extent3d {
                width: maps.brdf.width,
                height: maps.brdf.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        }));
        let brdf_half = f32_to_f16_bits(&maps.brdf.data);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &brdf_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&brdf_half),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((maps.brdf.width * 8) as u32),
                rows_per_image: Some(maps.brdf.height),
            },
            wgpu::Extent3d { width: maps.brdf.width, height: maps.brdf.height, depth_or_array_layers: 1 },
        );
        let brdf_view = Arc::new(brdf_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Environment BRDF View"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            ..Default::default()
        }));

        Ok(Self {
            _diffuse_texture: diffuse_texture,
            diffuse_view,
            _specular_texture: specular_texture,
            specular_view,
            _brdf_texture: brdf_texture,
            brdf_view,
            sampler,
            specular_mip_count: mip_count,
        })
    }

    pub fn diffuse_view(&self) -> &wgpu::TextureView {
        self.diffuse_view.as_ref()
    }

    pub fn specular_view(&self) -> &wgpu::TextureView {
        self.specular_view.as_ref()
    }

    pub fn brdf_view(&self) -> &wgpu::TextureView {
        self.brdf_view.as_ref()
    }

    pub fn sampler(&self) -> &wgpu::Sampler {
        self.sampler.as_ref()
    }

    pub fn specular_mip_count(&self) -> u32 {
        self.specular_mip_count
    }
}

impl Default for EnvironmentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

fn is_supported_environment_file(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()).map(|s| s.to_ascii_lowercase()) {
        Some(ext) => matches!(ext.as_str(), "hdr" | "exr" | "png"),
        None => false,
    }
}

fn environment_key_from_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_string_lossy();
    let sanitized: String = stem
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_lowercase() } else { '_' })
        .collect();
    if sanitized.is_empty() {
        None
    } else {
        Some(format!("environment::{sanitized}"))
    }
}

fn load_hdr_image(path: &str) -> Result<HdrImage> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let dyn_img = reader.decode()?;
    convert_to_hdr(&dyn_img)
}

fn convert_to_hdr(image: &DynamicImage) -> Result<HdrImage> {
    let rgb = image.to_rgb32f();
    let width = rgb.width();
    let height = rgb.height();
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for pixel in rgb.pixels() {
        let [r, g, b] = pixel.0;
        pixels.push(Vec3::new(r, g, b));
    }
    Ok(HdrImage { width, height, pixels })
}

fn generate_default_hdr() -> HdrImage {
    let width = 256u32;
    let height = 128u32;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let v = y as f32 / (height - 1) as f32;
        for x in 0..width {
            let u = x as f32 / (width - 1) as f32;
            let horizon = (1.0 - (2.0 * (v - 0.5)).abs()).clamp(0.0, 1.0);
            let sky = Vec3::new(0.25, 0.35, 0.6) * (1.0 - v) + Vec3::new(0.65, 0.7, 0.9) * v;
            let sun_dir = Vec2::new(u - 0.2, v - 0.35);
            let sun = ((1.0 - sun_dir.length() * 6.0).max(0.0)).powf(12.0);
            let ground = Vec3::new(0.08, 0.07, 0.05) * (1.0 - horizon) + Vec3::new(0.2, 0.18, 0.16) * horizon;
            let mut color = sky * (0.6 + 0.4 * horizon) + ground * (1.0 - horizon);
            color += Vec3::new(1.0, 0.9, 0.75) * sun * 8.0;
            pixels.push(color);
        }
    }
    HdrImage { width, height, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn environment_key_sanitizes_names() {
        let path = PathBuf::from("Bright Sky 01.hdr");
        let key = environment_key_from_path(&path).expect("key");
        assert_eq!(key, "environment::bright_sky_01");
    }

    #[test]
    fn load_directory_registers_png_files() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("Studio.png");
        let mut img = RgbImage::new(4, 2);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = Rgb([(x as u8).saturating_mul(40), (y as u8).saturating_mul(80), 200]);
        }
        img.save(&path).expect("save png");

        let mut registry = EnvironmentRegistry::new();
        let added = registry.load_directory(dir.path()).expect("load directory");
        assert_eq!(added, vec!["environment::studio".to_string()]);
        assert!(registry.definition("environment::studio").is_some());
    }

    #[test]
    fn release_drops_cached_maps() {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("test_env.png");
        let mut img = RgbImage::new(2, 1);
        for (x, _, pixel) in img.enumerate_pixels_mut() {
            *pixel = Rgb([(x as u8).saturating_mul(80), 64, 200]);
        }
        img.save(&path).expect("save png");

        let mut registry = EnvironmentRegistry::new();
        let key = "environment::temp";
        let path_string = path.to_string_lossy().to_string();
        registry.retain(key, Some(path_string.as_str())).expect("retain environment");
        {
            let entry = registry.environments.get(key).expect("entry");
            assert_eq!(entry.ref_count, 1);
            assert!(entry.maps.is_some(), "maps should be cached after retain");
        }
        assert!(registry.release(key));
        {
            let entry = registry.environments.get(key).expect("entry");
            assert_eq!(entry.ref_count, 0);
            assert!(entry.maps.is_none(), "maps should be dropped when refcount reaches zero");
            assert!(entry.gpu.is_none(), "gpu resources cleared when refcount reaches zero");
        }
    }
}

fn compute_diffuse_cubemap(image: &HdrImage, size: u32) -> Cubemap {
    let mut faces: [Vec<f32>; 6] = [
        vec![0.0; (size * size * 4) as usize],
        vec![0.0; (size * size * 4) as usize],
        vec![0.0; (size * size * 4) as usize],
        vec![0.0; (size * size * 4) as usize],
        vec![0.0; (size * size * 4) as usize],
        vec![0.0; (size * size * 4) as usize],
    ];
    for face in 0..6 {
        let data = &mut faces[face];
        for y in 0..size {
            for x in 0..size {
                let dir = cubemap_direction(face, x, y, size);
                let mut result = Vec3::ZERO;
                let mut weight_sum = 0.0f32;
                for sample in 0..DIFFUSE_SAMPLE_COUNT {
                    let xi = hammersley(sample as u32, DIFFUSE_SAMPLE_COUNT as u32);
                    let sample_dir = cosine_sample_hemisphere(dir, xi);
                    let n_dot_l = dir.dot(sample_dir).max(0.0);
                    if n_dot_l > 0.0 {
                        result += sample_equirect(image, sample_dir) * n_dot_l;
                        weight_sum += n_dot_l;
                    }
                }
                if weight_sum > 0.0 {
                    result /= weight_sum;
                }
                let idx = ((y * size + x) * 4) as usize;
                data[idx] = result.x;
                data[idx + 1] = result.y;
                data[idx + 2] = result.z;
                data[idx + 3] = 1.0;
            }
        }
    }
    Cubemap { size, faces }
}

fn compute_specular_cubemap(image: &HdrImage, base_size: u32, mip_count: u32) -> PrefilteredCubemap {
    let mut levels = Vec::new();
    let max_level = mip_count.max(1);
    for mip in 0..max_level {
        let size = (base_size >> mip).max(1);
        let roughness = mip as f32 / (max_level as f32 - 1.0).max(1.0);
        let mut faces: [Vec<f32>; 6] = [
            vec![0.0; (size * size * 4) as usize],
            vec![0.0; (size * size * 4) as usize],
            vec![0.0; (size * size * 4) as usize],
            vec![0.0; (size * size * 4) as usize],
            vec![0.0; (size * size * 4) as usize],
            vec![0.0; (size * size * 4) as usize],
        ];
        for face in 0..6 {
            let data = &mut faces[face];
            for y in 0..size {
                for x in 0..size {
                    let r = cubemap_direction(face, x, y, size);
                    let mut color = Vec3::ZERO;
                    let mut weight_sum = 0.0f32;
                    for sample in 0..SPECULAR_SAMPLE_COUNT {
                        let xi = hammersley(sample as u32, SPECULAR_SAMPLE_COUNT as u32);
                        let h = importance_sample_ggx(r, xi, roughness);
                        let l = reflect(-r, h).normalize();
                        let n_dot_l = r.dot(l).max(0.0);
                        if n_dot_l > 0.0 {
                            let weight = n_dot_l;
                            color += sample_equirect(image, l) * weight;
                            weight_sum += weight;
                        }
                    }
                    if weight_sum > 0.0 {
                        color /= weight_sum;
                    }
                    let idx = ((y * size + x) * 4) as usize;
                    data[idx] = color.x;
                    data[idx + 1] = color.y;
                    data[idx + 2] = color.z;
                    data[idx + 3] = 1.0;
                }
            }
        }
        levels.push(CubemapLevel { size, faces });
    }
    PrefilteredCubemap { base_size, levels }
}

fn compute_brdf_lut(size: u32) -> Lut2D {
    let mut data = vec![0.0f32; (size * size * 4) as usize];
    for y in 0..size {
        let roughness = (y as f32 + 0.5) / size as f32;
        for x in 0..size {
            let n_dot_v = (x as f32 + 0.5) / size as f32;
            let (a, b) = integrate_brdf(n_dot_v, roughness);
            let idx = ((y * size + x) * 4) as usize;
            data[idx] = a;
            data[idx + 1] = b;
            data[idx + 2] = 0.0;
            data[idx + 3] = 1.0;
        }
    }
    Lut2D { width: size, height: size, data }
}

fn sample_equirect(image: &HdrImage, dir: Vec3) -> Vec3 {
    let d = dir.normalize();
    let theta = d.y.clamp(-1.0, 1.0).acos();
    let phi = d.z.atan2(d.x);
    let u = (phi + PI) / TAU;
    let v = theta / PI;
    let x = u * (image.width as f32 - 1.0);
    let y = v * (image.height as f32 - 1.0);
    let x0 = x.floor();
    let y0 = y.floor();
    let x1 = x0 + 1.0;
    let y1 = y0 + 1.0;
    let tx = x - x0;
    let ty = y - y0;

    let ix0 = x0.rem_euclid(image.width as f32) as u32;
    let ix1 = x1.rem_euclid(image.width as f32) as u32;
    let iy0 = y0.clamp(0.0, (image.height - 1) as f32) as u32;
    let iy1 = y1.clamp(0.0, (image.height - 1) as f32) as u32;

    let c00 = image.pixel(ix0, iy0);
    let c10 = image.pixel(ix1, iy0);
    let c01 = image.pixel(ix0, iy1);
    let c11 = image.pixel(ix1, iy1);

    let c0 = c00 * (1.0 - tx) + c10 * tx;
    let c1 = c01 * (1.0 - tx) + c11 * tx;
    c0 * (1.0 - ty) + c1 * ty
}

fn cubemap_direction(face: usize, x: u32, y: u32, size: u32) -> Vec3 {
    let a = (2.0 * (x as f32 + 0.5) / size as f32) - 1.0;
    let b = (2.0 * (y as f32 + 0.5) / size as f32) - 1.0;
    match face {
        0 => Vec3::new(1.0, -b, -a),
        1 => Vec3::new(-1.0, -b, a),
        2 => Vec3::new(a, 1.0, b),
        3 => Vec3::new(a, -1.0, -b),
        4 => Vec3::new(a, -b, 1.0),
        _ => Vec3::new(-a, -b, -1.0),
    }
    .normalize()
}

fn cosine_sample_hemisphere(normal: Vec3, xi: Vec2) -> Vec3 {
    let r = xi.x.sqrt();
    let theta = TAU * xi.y;
    let x = r * theta.cos();
    let y = r * theta.sin();
    let z = (1.0 - xi.x).sqrt();
    tangent_to_world(normal, Vec3::new(x, y, z))
}

fn importance_sample_ggx(normal: Vec3, xi: Vec2, roughness: f32) -> Vec3 {
    let a = roughness.max(0.001);
    let phi = TAU * xi.x;
    let cos_theta = ((1.0 - xi.y) / (1.0 + (a * a - 1.0) * xi.y)).sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    let h = Vec3::new(phi.cos() * sin_theta, phi.sin() * sin_theta, cos_theta);
    tangent_to_world(normal, h)
}

fn tangent_to_world(normal: Vec3, vec: Vec3) -> Vec3 {
    let up = if normal.z.abs() < 0.999 { Vec3::Z } else { Vec3::X };
    let tangent = normal.cross(up).normalize();
    let bitangent = normal.cross(tangent);
    tangent * vec.x + bitangent * vec.y + normal * vec.z
}

fn reflect(v: Vec3, n: Vec3) -> Vec3 {
    v - 2.0 * v.dot(n) * n
}

fn hammersley(i: u32, n: u32) -> Vec2 {
    Vec2::new(i as f32 / n as f32, radical_inverse_vdc(i))
}

fn radical_inverse_vdc(bits: u32) -> f32 {
    let mut b = bits;
    b = (b << 16) | (b >> 16);
    b = ((b & 0x5555_5555) << 1) | ((b & 0xAAAA_AAAA) >> 1);
    b = ((b & 0x3333_3333) << 2) | ((b & 0xCCCC_CCCC) >> 2);
    b = ((b & 0x0F0F_0F0F) << 4) | ((b & 0xF0F0_F0F0) >> 4);
    b = ((b & 0x00FF_00FF) << 8) | ((b & 0xFF00_FF00) >> 8);
    (b as f32) * 2.328_306_4e-10
}

fn integrate_brdf(n_dot_v: f32, roughness: f32) -> (f32, f32) {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let v = Vec3::new((1.0 - n_dot_v * n_dot_v).sqrt(), 0.0, n_dot_v);
    let mut a = 0.0f32;
    let mut b = 0.0f32;
    for i in 0..BRDF_SAMPLE_COUNT {
        let xi = hammersley(i as u32, BRDF_SAMPLE_COUNT as u32);
        let h = importance_sample_ggx(normal, xi, roughness);
        let l = reflect(-v, h);
        let n_dot_l = l.z.max(0.0);
        let n_dot_h = h.z.max(0.0);
        let v_dot_h = v.dot(h).max(0.0);
        if n_dot_l > 0.0 {
            let g = geometry_smith(normal, v, l, roughness);
            let g_vis = (g * v_dot_h) / (n_dot_h * n_dot_v).max(1e-4);
            let fc = (1.0 - v_dot_h).powi(5);
            a += (1.0 - fc) * g_vis;
            b += fc * g_vis;
        }
    }
    let scale = 1.0 / BRDF_SAMPLE_COUNT as f32;
    (a * scale, b * scale)
}

fn geometry_smith(normal: Vec3, v: Vec3, l: Vec3, roughness: f32) -> f32 {
    let n_dot_v = normal.dot(v).max(0.0);
    let n_dot_l = normal.dot(l).max(0.0);
    geometry_schlick_ggx(n_dot_v, roughness) * geometry_schlick_ggx(n_dot_l, roughness)
}

fn geometry_schlick_ggx(n_dot_v: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) * 0.125;
    n_dot_v / (n_dot_v * (1.0 - k) + k)
}

impl HdrImage {
    fn pixel(&self, x: u32, y: u32) -> Vec3 {
        let idx = (y * self.width + x) as usize;
        self.pixels[idx]
    }
}
