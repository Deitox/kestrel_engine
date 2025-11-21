use crate::mesh::{ImportedMaterial, ImportedTexture, MaterialTextureBinding};
use crate::renderer::Renderer;
use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    base_color_factor: [f32; 4],
    emissive_factor: [f32; 4],
    params: [f32; 4],        // metallic, roughness, normal_scale, padding
    texture_flags: [f32; 4], // base_color, metallic_roughness, normal, emissive
}

#[derive(Clone)]
pub struct MaterialDefinition {
    pub key: String,
    pub label: String,
    pub base_color_factor: [f32; 4],
    pub metallic_factor: f32,
    pub roughness_factor: f32,
    pub emissive_factor: [f32; 3],
    pub base_color_texture: Option<MaterialTextureBinding>,
    pub metallic_roughness_texture: Option<MaterialTextureBinding>,
    pub normal_texture: Option<MaterialTextureBinding>,
    pub emissive_texture: Option<MaterialTextureBinding>,
    pub source: Option<String>,
}

pub struct MaterialRegistry {
    materials: HashMap<String, MaterialEntry>,
    textures: HashMap<String, TextureEntry>,
    texture_material_refs: HashMap<String, usize>,
    default_material: String,
    default_textures: Option<DefaultTextures>,
    sampler: Option<Arc<wgpu::Sampler>>,
}

#[allow(dead_code)]
struct MaterialEntry {
    definition: MaterialDefinition,
    gpu: Option<Arc<MaterialGpu>>,
    ref_count: usize,
    permanent: bool,
}

struct TextureEntry {
    width: u32,
    height: u32,
    data: Vec<u8>,
    gpu_srgb: Option<Arc<GpuTexture>>,
    gpu_linear: Option<Arc<GpuTexture>>,
}

struct DefaultTextures {
    base_color: Arc<GpuTexture>,
    metallic_roughness: Arc<GpuTexture>,
    normal: Arc<GpuTexture>,
    emissive: Arc<GpuTexture>,
}

#[allow(dead_code)]
pub struct MaterialGpu {
    bind_group: Arc<wgpu::BindGroup>,
    uniform_buffer: Arc<wgpu::Buffer>,
    base_color: Arc<GpuTexture>,
    metallic_roughness: Arc<GpuTexture>,
    normal: Arc<GpuTexture>,
    emissive: Arc<GpuTexture>,
}

#[allow(dead_code)]
struct GpuTexture {
    texture: Arc<wgpu::Texture>,
    view: Arc<wgpu::TextureView>,
    srgb: bool,
}

impl MaterialRegistry {
    pub fn new() -> Self {
        let default_material = "material::default".to_string();
        let mut registry = Self {
            materials: HashMap::new(),
            textures: HashMap::new(),
            texture_material_refs: HashMap::new(),
            default_material: default_material.clone(),
            default_textures: None,
            sampler: None,
        };
        let default_definition = MaterialDefinition {
            key: default_material.clone(),
            label: "Default".to_string(),
            base_color_factor: [1.0, 1.0, 1.0, 1.0],
            metallic_factor: 0.0,
            roughness_factor: 1.0,
            emissive_factor: [0.0, 0.0, 0.0],
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            emissive_texture: None,
            source: None,
        };
        registry.materials.insert(
            default_material,
            MaterialEntry { definition: default_definition, gpu: None, ref_count: 0, permanent: true },
        );
        registry
    }

    pub fn register_gltf_import(
        &mut self,
        import_materials: &[ImportedMaterial],
        import_textures: &[ImportedTexture],
    ) {
        for texture in import_textures {
            self.textures
                .entry(texture.key.clone())
                .and_modify(|entry| {
                    entry.width = texture.width;
                    entry.height = texture.height;
                    entry.data = texture.data.clone();
                    entry.gpu_srgb = None;
                    entry.gpu_linear = None;
                })
                .or_insert_with(|| TextureEntry {
                    width: texture.width,
                    height: texture.height,
                    data: texture.data.clone(),
                    gpu_srgb: None,
                    gpu_linear: None,
                });
        }

        for material in import_materials {
            let definition = MaterialDefinition {
                key: material.key.clone(),
                label: material.label.clone(),
                base_color_factor: material.base_color_factor,
                metallic_factor: material.metallic_factor,
                roughness_factor: material.roughness_factor,
                emissive_factor: material.emissive_factor,
                base_color_texture: material.base_color_texture.clone(),
                metallic_roughness_texture: material.metallic_roughness_texture.clone(),
                normal_texture: material.normal_texture.clone(),
                emissive_texture: material.emissive_texture.clone(),
                source: material.source.clone(),
            };
            if let Some(mut entry) = self.materials.remove(&material.key) {
                self.bump_texture_refs(&entry.definition, -1);
                entry.definition = definition;
                entry.gpu = None;
                self.bump_texture_refs(&entry.definition, 1);
                self.materials.insert(material.key.clone(), entry);
            } else {
                self.bump_texture_refs(&definition, 1);
                self.materials.insert(
                    material.key.clone(),
                    MaterialEntry { definition, gpu: None, ref_count: 0, permanent: false },
                );
            }
        }
    }

    pub fn default_key(&self) -> &str {
        &self.default_material
    }

    pub fn has(&self, key: &str) -> bool {
        self.materials.contains_key(key)
    }

    pub fn material_source(&self, key: &str) -> Option<&str> {
        self.materials.get(key).and_then(|entry| entry.definition.source.as_deref())
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.materials.keys().map(|k| k.as_str())
    }

    pub fn retain(&mut self, key: &str) -> Result<()> {
        let entry = self.materials.get_mut(key).ok_or_else(|| anyhow!("Material '{key}' not registered"))?;
        entry.ref_count = entry.ref_count.saturating_add(1);
        Ok(())
    }

    pub fn release(&mut self, key: &str) {
        let mut remove_entry = false;
        if let Some(entry) = self.materials.get_mut(key) {
            if entry.ref_count > 0 {
                entry.ref_count -= 1;
            }
            if entry.ref_count == 0 {
                entry.gpu = None;
                if !entry.permanent {
                    remove_entry = true;
                }
            }
        }
        if remove_entry {
            if let Some(entry) = self.materials.remove(key) {
                self.bump_texture_refs(&entry.definition, -1);
            }
        }
    }

    pub fn ref_count(&self, key: &str) -> Option<usize> {
        self.materials.get(key).map(|entry| entry.ref_count)
    }

    pub fn definition(&self, key: &str) -> Option<&MaterialDefinition> {
        self.materials.get(key).map(|entry| &entry.definition)
    }

    pub fn prepare_material_gpu(&mut self, key: &str, renderer: &mut Renderer) -> Result<Arc<MaterialGpu>> {
        let definition = {
            let entry =
                self.materials.get_mut(key).ok_or_else(|| anyhow!("Material '{key}' not registered"))?;
            if let Some(gpu) = &entry.gpu {
                return Ok(gpu.clone());
            }
            entry.definition.clone()
        };

        let layout = renderer.material_bind_group_layout()?;
        let device = renderer.device()?;
        let queue = renderer.queue()?;

        let sampler = self.ensure_sampler(device);
        self.ensure_default_textures(device, queue)?;
        let (default_base, default_mr, default_normal, default_emissive) = {
            let defaults = self.default_textures.as_ref().expect("default textures initialized");
            (
                defaults.base_color.clone(),
                defaults.metallic_roughness.clone(),
                defaults.normal.clone(),
                defaults.emissive.clone(),
            )
        };

        let base_color_texture = if let Some(binding) = definition.base_color_texture.as_ref() {
            self.ensure_texture_gpu(&binding.texture_key, true, device, queue)?
        } else {
            default_base
        };
        let metallic_roughness_texture = if let Some(binding) = definition.metallic_roughness_texture.as_ref()
        {
            self.ensure_texture_gpu(&binding.texture_key, false, device, queue)?
        } else {
            default_mr
        };
        let normal_texture_binding = definition.normal_texture.as_ref();
        let normal_texture = if let Some(binding) = normal_texture_binding {
            self.ensure_texture_gpu(&binding.texture_key, false, device, queue)?
        } else {
            default_normal
        };
        let emissive_texture = if let Some(binding) = definition.emissive_texture.as_ref() {
            self.ensure_texture_gpu(&binding.texture_key, true, device, queue)?
        } else {
            default_emissive
        };

        let normal_scale = normal_texture_binding.map(|binding| binding.scale).unwrap_or(1.0);
        let uniform = MaterialUniform {
            base_color_factor: definition.base_color_factor,
            emissive_factor: [
                definition.emissive_factor[0],
                definition.emissive_factor[1],
                definition.emissive_factor[2],
                1.0,
            ],
            params: [definition.metallic_factor, definition.roughness_factor, normal_scale, 0.0],
            texture_flags: [
                definition.base_color_texture.is_some() as u32 as f32,
                definition.metallic_roughness_texture.is_some() as u32 as f32,
                definition.normal_texture.is_some() as u32 as f32,
                definition.emissive_texture.is_some() as u32 as f32,
            ],
        };

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Material Uniform Buffer"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Material Bind Group"),
            layout: layout.as_ref(),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(base_color_texture.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(metallic_roughness_texture.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(normal_texture.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(emissive_texture.view()),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(sampler.as_ref()),
                },
            ],
        });

        let gpu = Arc::new(MaterialGpu {
            bind_group: Arc::new(bind_group),
            uniform_buffer: Arc::new(uniform_buffer),
            base_color: base_color_texture,
            metallic_roughness: metallic_roughness_texture,
            normal: normal_texture,
            emissive: emissive_texture,
        });
        if let Some(entry) = self.materials.get_mut(key) {
            entry.gpu = Some(gpu.clone());
        }
        Ok(gpu)
    }

    fn ensure_sampler(&mut self, device: &wgpu::Device) -> Arc<wgpu::Sampler> {
        if let Some(sampler) = &self.sampler {
            return sampler.clone();
        }
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Material Sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let sampler = Arc::new(sampler);
        self.sampler = Some(sampler.clone());
        sampler
    }

    fn ensure_default_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<()> {
        if self.default_textures.is_some() {
            return Ok(());
        }
        let make_texture = |data: [u8; 4],
                            format: wgpu::TextureFormat|
         -> (wgpu::Texture, wgpu::TextureView) {
            let (pixel_data, padded_row_bytes) = Self::prepare_texture_upload(&data, 1, 1);
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Material Default Texture"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixel_data.as_ref(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row_bytes),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            (texture, view)
        };

        let (base_tex, base_view) = make_texture([255, 255, 255, 255], wgpu::TextureFormat::Rgba8UnormSrgb);
        let (metal_tex, metal_view) = make_texture([255, 255, 255, 255], wgpu::TextureFormat::Rgba8Unorm);
        let (normal_tex, normal_view) = make_texture([128, 128, 255, 255], wgpu::TextureFormat::Rgba8Unorm);
        let (emissive_tex, emissive_view) = make_texture([0, 0, 0, 255], wgpu::TextureFormat::Rgba8UnormSrgb);

        self.default_textures = Some(DefaultTextures {
            base_color: Arc::new(GpuTexture::new(base_tex, base_view, true)),
            metallic_roughness: Arc::new(GpuTexture::new(metal_tex, metal_view, false)),
            normal: Arc::new(GpuTexture::new(normal_tex, normal_view, false)),
            emissive: Arc::new(GpuTexture::new(emissive_tex, emissive_view, true)),
        });
        Ok(())
    }

    fn ensure_texture_gpu(
        &mut self,
        key: &str,
        srgb: bool,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Arc<GpuTexture>> {
        let entry = self
            .textures
            .get_mut(key)
            .ok_or_else(|| anyhow!("Texture '{key}' not registered for materials"))?;
        let cache = if srgb { &mut entry.gpu_srgb } else { &mut entry.gpu_linear };
        if let Some(texture) = cache {
            return Ok(texture.clone());
        }

        let format = if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Material Texture"),
            size: wgpu::Extent3d { width: entry.width, height: entry.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let (pixel_data, padded_row_bytes) =
            Self::prepare_texture_upload(&entry.data, entry.width, entry.height);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            pixel_data.as_ref(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row_bytes),
                rows_per_image: Some(entry.height),
            },
            wgpu::Extent3d { width: entry.width, height: entry.height, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let gpu_texture = Arc::new(GpuTexture::new(texture, view, srgb));
        *cache = Some(gpu_texture.clone());
        Ok(gpu_texture)
    }

    fn bump_texture_refs(&mut self, definition: &MaterialDefinition, delta: isize) {
        if delta == 0 {
            return;
        }
        for binding in Self::texture_bindings(definition) {
            self.adjust_texture_ref(&binding.texture_key, delta);
        }
    }

    fn texture_bindings(definition: &MaterialDefinition) -> impl Iterator<Item = &MaterialTextureBinding> {
        [
            definition.base_color_texture.as_ref(),
            definition.metallic_roughness_texture.as_ref(),
            definition.normal_texture.as_ref(),
            definition.emissive_texture.as_ref(),
        ]
        .into_iter()
        .flatten()
    }

    fn adjust_texture_ref(&mut self, key: &str, delta: isize) {
        if delta == 0 {
            return;
        }
        if delta > 0 {
            let entry = self.texture_material_refs.entry(key.to_string()).or_insert(0);
            *entry = entry.saturating_add(delta as usize);
        } else {
            let dec = (-delta) as usize;
            let mut remove_entry = false;
            if let Some(entry) = self.texture_material_refs.get_mut(key) {
                if *entry <= dec {
                    remove_entry = true;
                } else {
                    *entry -= dec;
                }
            }
            if remove_entry {
                self.texture_material_refs.remove(key);
                self.textures.remove(key);
            }
        }
    }

    fn padded_bytes_per_row(width: u32) -> u32 {
        let unpadded = width.saturating_mul(4);
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u32;
        let remainder = unpadded % align;
        if remainder == 0 { unpadded } else { unpadded + align - remainder }
    }

    fn prepare_texture_upload<'a>(
        data: &'a [u8],
        width: u32,
        height: u32,
    ) -> (Cow<'a, [u8]>, u32) {
        let row_bytes = width.saturating_mul(4);
        let padded_row_bytes = Self::padded_bytes_per_row(width);
        if padded_row_bytes == row_bytes {
            (Cow::Borrowed(data), row_bytes)
        } else {
            let mut padded = vec![0u8; (padded_row_bytes.saturating_mul(height)) as usize];
            for row in 0..height {
                let src_start = (row_bytes * row) as usize;
                let dst_start = (padded_row_bytes * row) as usize;
                let src_end = src_start + row_bytes as usize;
                if src_end <= data.len() && dst_start + row_bytes as usize <= padded.len() {
                    padded[dst_start..dst_start + row_bytes as usize]
                        .copy_from_slice(&data[src_start..src_end]);
                }
            }
            (Cow::Owned(padded), padded_row_bytes)
        }
    }
}

impl Default for MaterialRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl MaterialGpu {
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        self.bind_group.as_ref()
    }
}

impl GpuTexture {
    fn new(texture: wgpu::Texture, view: wgpu::TextureView, srgb: bool) -> Self {
        Self { texture: Arc::new(texture), view: Arc::new(view), srgb }
    }

    fn view(&self) -> &wgpu::TextureView {
        self.view.as_ref()
    }
}
