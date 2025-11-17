use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Mat4;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;

use super::{InstanceData, RenderViewport};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    proj: [[f32; 4]; 4],
}

struct SpriteBindCacheEntry {
    view: Arc<wgpu::TextureView>,
    sampler_id: u64,
    bind_group: Arc<wgpu::BindGroup>,
}

pub struct SpritePass {
    pipeline: Option<wgpu::RenderPipeline>,
    vertex_buffer: Option<wgpu::Buffer>,
    index_buffer: Option<wgpu::Buffer>,
    globals_buf: Option<wgpu::Buffer>,
    globals_bg: Option<wgpu::BindGroup>,
    globals_bgl: Option<wgpu::BindGroupLayout>,
    texture_bg: Option<wgpu::BindGroup>,
    texture_bgl: Option<wgpu::BindGroupLayout>,
    instance_buffer: Option<wgpu::Buffer>,
    instance_capacity: usize,
    bind_cache: HashMap<String, SpriteBindCacheEntry>,
}

impl Default for SpritePass {
    fn default() -> Self {
        Self {
            pipeline: None,
            vertex_buffer: None,
            index_buffer: None,
            globals_buf: None,
            globals_bg: None,
            globals_bgl: None,
            texture_bg: None,
            texture_bgl: None,
            instance_buffer: None,
            instance_capacity: 0,
            bind_cache: HashMap::new(),
        }
    }
}

impl SpritePass {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn init_pipeline_with_atlas(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        atlas_view: wgpu::TextureView,
        sampler: wgpu::Sampler,
    ) -> Result<()> {
        self.bind_cache.clear();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sprite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../assets/shaders/sprite_batch.wgsl").into()),
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite Globals BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sprite Globals Buffer"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Globals BG"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sprite Texture BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Texture BG"),
            layout: &texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        let vertices: [[f32; 5]; 4] = [
            [-0.5, 0.5, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.0, 1.0, 0.0],
            [0.5, -0.5, 0.0, 1.0, 1.0],
            [-0.5, -0.5, 0.0, 0.0, 1.0],
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite VB"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Sprite IB"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Sprite Pipeline Layout"),
            bind_group_layouts: &[&globals_bgl, &texture_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Sprite Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<[f32; 5]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                shader_location: 0,
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                            },
                            wgpu::VertexAttribute {
                                shader_location: 1,
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 12,
                            },
                        ],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<InstanceData>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute {
                                shader_location: 2,
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 0,
                            },
                            wgpu::VertexAttribute {
                                shader_location: 3,
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 16,
                            },
                            wgpu::VertexAttribute {
                                shader_location: 4,
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 32,
                            },
                            wgpu::VertexAttribute {
                                shader_location: 5,
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 48,
                            },
                            wgpu::VertexAttribute {
                                shader_location: 6,
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 64,
                            },
                        ],
                    },
                ],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        self.pipeline = Some(pipeline);
        self.vertex_buffer = Some(vertex_buffer);
        self.index_buffer = Some(index_buffer);
        self.globals_bgl = Some(globals_bgl);
        self.globals_buf = Some(globals_buf);
        self.globals_bg = Some(globals_bg);
        self.texture_bgl = Some(texture_bgl);
        self.texture_bg = Some(texture_bg);
        Ok(())
    }

    pub fn clear_bind_cache(&mut self) {
        self.bind_cache.clear();
    }

    pub fn invalidate_bind_group(&mut self, atlas: &str) {
        self.bind_cache.remove(atlas);
    }

    pub fn write_globals(&self, queue: &wgpu::Queue, sprite_view_proj: Mat4) -> Result<()> {
        let globals = self.globals_buf.as_ref().context("Sprite globals buffer missing")?;
        queue.write_buffer(globals, 0, bytemuck::bytes_of(&Globals { proj: sprite_view_proj.to_cols_array_2d() }));
        Ok(())
    }

    pub fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        instances: &[InstanceData],
    ) -> Result<()> {
        self.ensure_instance_capacity(device, instances.len())?;
        let instance_buffer = self.instance_buffer.as_ref().context("Instance buffer missing")?;
        queue.write_buffer(instance_buffer, 0, bytemuck::cast_slice(instances));
        Ok(())
    }

    pub fn sprite_bind_group(
        &mut self,
        device: &wgpu::Device,
        atlas: &str,
        view: &Arc<wgpu::TextureView>,
        sampler: &wgpu::Sampler,
    ) -> Result<Arc<wgpu::BindGroup>> {
        let sampler_id = sampler as *const wgpu::Sampler as usize as u64;
        if let Some(entry) = self.bind_cache.get(atlas) {
            if Arc::ptr_eq(&entry.view, view) && entry.sampler_id == sampler_id {
                return Ok(entry.bind_group.clone());
            }
        }

        let layout = self.texture_bgl.as_ref().context("Texture bind group layout missing")?;
        let bind_group = Arc::new(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sprite Atlas Bind Group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view.as_ref()),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            ],
        }));

        self.bind_cache.insert(
            atlas.to_string(),
            SpriteBindCacheEntry { view: view.clone(), sampler_id, bind_group: bind_group.clone() },
        );

        Ok(bind_group)
    }

    pub fn encode_pass(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        viewport: RenderViewport,
        surface_size: PhysicalSize<u32>,
        instances: &[InstanceData],
        sprite_bind_groups: &[(Range<u32>, Arc<wgpu::BindGroup>)],
    ) -> Result<()> {
        pass.set_pipeline(self.pipeline.as_ref().context("Sprite pipeline missing")?);
        pass.set_bind_group(0, self.globals_bg.as_ref().context("Sprite globals bind group missing")?, &[]);
        let vertex_buffer = self.vertex_buffer.as_ref().context("Sprite vertex buffer missing")?;
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        let instance_buffer = self.instance_buffer.as_ref().context("Instance buffer missing")?;
        pass.set_vertex_buffer(1, instance_buffer.slice(..));
        let (vp_x, vp_y) = viewport.origin;
        let (vp_w, vp_h) = viewport.size;
        let vp_w = vp_w.max(1.0);
        let vp_h = vp_h.max(1.0);
        pass.set_viewport(vp_x, vp_y, vp_w, vp_h, 0.0, 1.0);
        let mut sc_x = vp_x.max(0.0).floor() as u32;
        let mut sc_y = vp_y.max(0.0).floor() as u32;
        let mut sc_w = vp_w.floor() as u32;
        let mut sc_h = vp_h.floor() as u32;
        if sc_w == 0 {
            sc_w = 1;
        }
        if sc_h == 0 {
            sc_h = 1;
        }
        let limit_w = surface_size.width.max(1);
        let limit_h = surface_size.height.max(1);
        if sc_x >= limit_w {
            sc_x = limit_w.saturating_sub(1);
        }
        if sc_y >= limit_h {
            sc_y = limit_h.saturating_sub(1);
        }
        let avail_w = limit_w.saturating_sub(sc_x).max(1);
        let avail_h = limit_h.saturating_sub(sc_y).max(1);
        sc_w = sc_w.min(avail_w);
        sc_h = sc_h.min(avail_h);
        pass.set_scissor_rect(sc_x, sc_y, sc_w, sc_h);
        pass.set_index_buffer(
            self.index_buffer.as_ref().context("Sprite index buffer missing")?.slice(..),
            wgpu::IndexFormat::Uint16,
        );

        if sprite_bind_groups.is_empty() {
            if !instances.is_empty() {
                if let Some(bg) = self.texture_bg.as_ref() {
                    pass.set_bind_group(1, bg, &[]);
                    pass.draw_indexed(0..6, 0, 0..(instances.len() as u32));
                }
            }
        } else {
            for (range, bind_group) in sprite_bind_groups.iter() {
                pass.set_bind_group(1, bind_group.as_ref(), &[]);
                pass.draw_indexed(0..6, 0, range.clone());
            }
        }
        Ok(())
    }

    fn ensure_instance_capacity(&mut self, device: &wgpu::Device, count: usize) -> Result<()> {
        let required = count.max(1);
        if self.instance_capacity >= required && self.instance_buffer.is_some() {
            return Ok(());
        }
        let mut new_cap = self.instance_capacity.max(256);
        if new_cap == 0 {
            new_cap = 256;
        }
        while new_cap < required {
            new_cap *= 2;
        }
        let buf_size = (new_cap * std::mem::size_of::<InstanceData>()) as u64;
        let new_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Instance Buffer"),
            size: buf_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer = Some(new_buf);
        self.instance_capacity = new_cap;
        Ok(())
    }
}
