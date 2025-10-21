use crate::camera3d::Camera3D;
use crate::config::WindowConfig;
use crate::ecs::InstanceData;
use crate::mesh::{Mesh, MeshVertex};
use anyhow::{anyhow, Context, Result};
use glam::Mat4;
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window};

// egui
use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    proj: [[f32; 4]; 4],
}

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshGlobals {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
}

#[derive(Clone, Copy, Debug)]
pub struct RenderViewport {
    pub origin: (f32, f32),
    pub size: (f32, f32),
}

#[derive(Debug)]
pub struct GpuMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

pub struct MeshDraw<'a> {
    pub mesh: &'a GpuMesh,
    pub model: Mat4,
}

struct MeshPipelineResources {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
}

pub struct Renderer {
    surface: Option<wgpu::Surface<'static>>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    config: Option<wgpu::SurfaceConfiguration>,
    size: PhysicalSize<u32>,
    window: Option<Arc<Window>>,
    title: String,
    vsync: bool,
    fullscreen: bool,

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

    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    mesh_pipeline: Option<MeshPipelineResources>,
}

impl Renderer {
    pub async fn new(window_cfg: &WindowConfig) -> Self {
        Self {
            surface: None,
            device: None,
            queue: None,
            config: None,
            size: PhysicalSize::new(window_cfg.width, window_cfg.height),
            window: None,
            title: window_cfg.title.clone(),
            vsync: window_cfg.vsync,
            fullscreen: window_cfg.fullscreen,
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
            depth_texture: None,
            depth_view: None,
            mesh_pipeline: None,
        }
    }

    pub fn ensure_window(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if self.window.is_some() {
            return Ok(());
        }
        let mut attrs =
            Window::default_attributes().with_title(self.title.clone()).with_inner_size(self.size);
        if self.fullscreen {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = Arc::new(event_loop.create_window(attrs).context("Failed to create window")?);
        pollster::block_on(self.init_wgpu(&window))?;
        self.window = Some(window);
        Ok(())
    }

    fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> wgpu::TextureFormat {
        formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(formats[0])
    }

    fn select_present_mode(&self, modes: &[wgpu::PresentMode]) -> wgpu::PresentMode {
        if self.vsync {
            wgpu::PresentMode::Fifo
        } else {
            modes
                .iter()
                .copied()
                .find(|mode| *mode != wgpu::PresentMode::Fifo)
                .unwrap_or(wgpu::PresentMode::Fifo)
        }
    }

    async fn init_wgpu(&mut self, window: &Arc<Window>) -> Result<()> {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).context("Failed to create WGPU surface")?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("Failed to request WGPU adapter")?;
        let required_limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let device_desc = wgpu::DeviceDescriptor {
            label: Some("Device"),
            required_features: wgpu::Features::empty(),
            required_limits,
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::default(),
        };
        let (device, queue) =
            adapter.request_device(&device_desc).await.context("Failed to request WGPU device")?;

        let caps = surface.get_capabilities(&adapter);
        let format = Self::choose_surface_format(&caps.formats);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: self.select_present_mode(&caps.present_modes),
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let (depth_texture, depth_view) = Self::create_depth_texture(&device, size)?;

        self.surface = Some(surface);
        self.device = Some(device);
        self.queue = Some(queue);
        self.config = Some(config);
        self.depth_texture = Some(depth_texture);
        self.depth_view = Some(depth_view);
        Ok(())
    }

    pub fn init_sprite_pipeline_with_atlas(
        &mut self,
        atlas_view: wgpu::TextureView,
        sampler: wgpu::Sampler,
    ) -> Result<()> {
        let device = self.device.as_ref().context("GPU device not initialized")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sprite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/shaders/sprite_batch.wgsl").into()),
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Globals BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Globals Buffer"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Globals BG"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture BGL"),
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
            label: Some("Texture BG"),
            layout: &texture_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        // Unit quad
        let vertices: [[f32; 5]; 4] = [
            [-0.5, 0.5, 0.0, 0.0, 0.0],
            [0.5, 0.5, 0.0, 1.0, 0.0],
            [0.5, -0.5, 0.0, 1.0, 1.0],
            [-0.5, -0.5, 0.0, 0.0, 1.0],
        ];
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("VB"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("IB"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
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
                            wgpu::VertexAttribute {
                                shader_location: 7,
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 80,
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
                    format: self.config.as_ref().context("Surface configuration missing")?.format,
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
        self.vertex_buffer = Some(vb);
        self.index_buffer = Some(ib);
        self.globals_bgl = Some(globals_bgl);
        self.globals_buf = Some(globals_buf);
        self.globals_bg = Some(globals_bg);
        self.texture_bgl = Some(texture_bgl);
        self.texture_bg = Some(texture_bg);
        Ok(())
    }

    pub fn init_mesh_pipeline(&mut self) -> Result<()> {
        if self.depth_texture.is_none() {
            self.recreate_depth_texture()?;
        }
        let device = self.device()?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mesh Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/shaders/mesh_basic.wgsl").into()),
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mesh Globals BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesh Globals Buffer"),
            size: std::mem::size_of::<MeshGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Mesh Globals BG"),
            layout: &globals_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mesh Pipeline Layout"),
            bind_group_layouts: &[&globals_bgl],
            push_constant_ranges: &[],
        });

        let mesh_vertex_layout = MeshVertex::layout();
        let color_target = Some(wgpu::ColorTargetState {
            format: self.surface_format()?,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Mesh Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[mesh_vertex_layout],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[color_target],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: Some(wgpu::Face::Back),
                front_face: wgpu::FrontFace::Ccw,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        self.mesh_pipeline = Some(MeshPipelineResources { pipeline, globals_buf, globals_bg });
        Ok(())
    }

    pub fn create_gpu_mesh(&self, mesh: &Mesh) -> Result<GpuMesh> {
        let device = self.device()?;
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mesh Vertex Buffer"),
            contents: bytemuck::cast_slice(&mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mesh Index Buffer"),
            contents: bytemuck::cast_slice(&mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Ok(GpuMesh { vertex_buffer, index_buffer, index_count: mesh.indices.len() as u32 })
    }

    pub fn encode_mesh_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        color_target: &wgpu::TextureView,
        viewport: RenderViewport,
        draws: &[MeshDraw],
        camera: &Camera3D,
        clear_color: wgpu::Color,
    ) -> Result<()> {
        if draws.is_empty() {
            return Ok(());
        }
        if self.mesh_pipeline.is_none() {
            self.init_mesh_pipeline()?;
        }
        if self.depth_texture.is_none() {
            self.recreate_depth_texture()?;
        }
        let mesh_pipeline = self.mesh_pipeline.as_ref().context("Mesh pipeline not initialized")?;
        let depth_view = self.depth_view.as_ref().context("Depth texture missing")?;
        let queue = self.queue()?;
        let vp_size = PhysicalSize::new(
            viewport.size.0.max(1.0).round() as u32,
            viewport.size.1.max(1.0).round() as u32,
        );
        let view_proj = camera.view_projection(vp_size);

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Mesh Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear_color), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&mesh_pipeline.pipeline);
        let mut sc_x = viewport.origin.0.max(0.0).floor() as u32;
        let mut sc_y = viewport.origin.1.max(0.0).floor() as u32;
        let mut sc_w = viewport.size.0.max(1.0).floor() as u32;
        let mut sc_h = viewport.size.1.max(1.0).floor() as u32;
        let limit_w = self.size.width.max(1);
        let limit_h = self.size.height.max(1);
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

        pass.set_viewport(
            viewport.origin.0,
            viewport.origin.1,
            viewport.size.0.max(1.0),
            viewport.size.1.max(1.0),
            0.0,
            1.0,
        );
        pass.set_scissor_rect(sc_x, sc_y, sc_w, sc_h);

        for draw in draws {
            let globals =
                MeshGlobals { view_proj: view_proj.to_cols_array_2d(), model: draw.model.to_cols_array_2d() };
            queue.write_buffer(&mesh_pipeline.globals_buf, 0, bytemuck::bytes_of(&globals));
            pass.set_bind_group(0, &mesh_pipeline.globals_bg, &[]);
            pass.set_vertex_buffer(0, draw.mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(draw.mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..draw.mesh.index_count, 0, 0..1);
        }

        Ok(())
    }

    pub fn device_and_queue(&self) -> Result<(&wgpu::Device, &wgpu::Queue)> {
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let queue = self.queue.as_ref().context("GPU queue not initialized")?;
        Ok((device, queue))
    }
    pub fn device(&self) -> Result<&wgpu::Device> {
        self.device.as_ref().context("GPU device not initialized")
    }
    pub fn queue(&self) -> Result<&wgpu::Queue> {
        self.queue.as_ref().context("GPU queue not initialized")
    }
    pub fn surface_format(&self) -> Result<wgpu::TextureFormat> {
        Ok(self.config.as_ref().context("Surface configuration missing")?.format)
    }
    pub fn size(&self) -> PhysicalSize<u32> {
        self.size
    }
    pub fn pixels_per_point(&self) -> f32 {
        1.0
    }

    fn recreate_depth_texture(&mut self) -> Result<()> {
        let depth_sources = {
            let device = self.device.as_ref().context("GPU device not initialized")?;
            Self::create_depth_texture(device, self.size)?
        };
        let (depth_texture, depth_view) = depth_sources;
        self.depth_texture = Some(depth_texture);
        self.depth_view = Some(depth_view);
        Ok(())
    }

    pub fn window(&self) -> Option<&Window> {
        self.window.as_deref()
    }
    pub fn aspect_ratio(&self) -> f32 {
        if self.size.height == 0 {
            1.0
        } else {
            self.size.width as f32 / self.size.height as f32
        }
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.size = new_size;
        if new_size.width > 0 && new_size.height > 0 {
            if let (Some(surface), Some(device), Some(config)) =
                (&self.surface, &self.device, &mut self.config)
            {
                config.width = new_size.width;
                config.height = new_size.height;
                surface.configure(device, config);
            }
            if let Err(err) = self.recreate_depth_texture() {
                eprintln!("Depth texture resize failed: {err:?}");
            }
        }
    }

    fn ensure_instance_capacity(&mut self, count: usize) -> Result<()> {
        let device = self.device.as_ref().context("GPU device not initialized")?;
        if self.instance_capacity >= count {
            return Ok(());
        }
        let mut new_cap = self.instance_capacity.max(256);
        while new_cap < count {
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

    fn create_depth_texture(
        device: &wgpu::Device,
        size: PhysicalSize<u32>,
    ) -> Result<(wgpu::Texture, wgpu::TextureView)> {
        let extent =
            wgpu::Extent3d { width: size.width.max(1), height: size.height.max(1), depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok((texture, view))
    }

    pub fn render_frame(
        &mut self,
        instances: &[InstanceData],
        sprite_view_proj: Mat4,
        viewport: RenderViewport,
        mesh_draws: &[MeshDraw],
        mesh_camera: Option<&Camera3D>,
    ) -> Result<()> {
        {
            let queue = self.queue.as_ref().context("GPU queue not initialized")?;
            let globals = self.globals_buf.as_ref().context("Globals buffer missing")?;
            queue.write_buffer(
                globals,
                0,
                bytemuck::bytes_of(&Globals { proj: sprite_view_proj.to_cols_array_2d() }),
            );
        }

        self.ensure_instance_capacity(instances.len())?;

        let byte_data = bytemuck::cast_slice(instances);
        {
            let instance_buffer = self.instance_buffer.as_ref().context("Instance buffer missing")?;
            let queue = self.queue.as_ref().context("GPU queue not initialized")?;
            queue.write_buffer(instance_buffer, 0, byte_data);
        }

        let surface = self.surface.as_ref().context("Surface not initialized")?;
        let device = self.device.as_ref().context("GPU device not initialized")?;

        let frame = surface.get_current_texture().context("Acquiring swapchain texture")?;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Encoder") });

        let clear_color = wgpu::Color { r: 0.05, g: 0.06, b: 0.1, a: 1.0 };
        let mut sprite_load_op = wgpu::LoadOp::Clear(clear_color);
        if let Some(camera) = mesh_camera {
            if !mesh_draws.is_empty() {
                self.encode_mesh_pass(&mut encoder, &view, viewport, mesh_draws, camera, clear_color)?;
                sprite_load_op = wgpu::LoadOp::Load;
            }
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Sprite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: sprite_load_op, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(self.pipeline.as_ref().context("Sprite pipeline missing")?);
            pass.set_bind_group(0, self.globals_bg.as_ref().context("Globals bind group missing")?, &[]);
            pass.set_bind_group(1, self.texture_bg.as_ref().context("Texture bind group missing")?, &[]);
            pass.set_vertex_buffer(
                0,
                self.vertex_buffer.as_ref().context("Vertex buffer missing")?.slice(..),
            );
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
            let limit_w = self.size.width.max(1);
            let limit_h = self.size.height.max(1);
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
                self.index_buffer.as_ref().context("Index buffer missing")?.slice(..),
                wgpu::IndexFormat::Uint16,
            );
            pass.draw_indexed(0..6, 0, 0..(instances.len() as u32));
        }

        if let Some(queue) = self.queue.as_ref() {
            queue.submit(std::iter::once(encoder.finish()));
        } else {
            return Err(anyhow!("GPU queue not initialized"));
        }
        frame.present();
        Ok(())
    }

    pub fn render_egui(
        &mut self,
        painter: &mut EguiRenderer,
        paint_jobs: &[egui::ClippedPrimitive],
        screen: &ScreenDescriptor,
    ) -> Result<()> {
        let surface = self.surface.as_ref().context("Surface not initialized")?;
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let queue = self.queue.as_ref().context("GPU queue not initialized")?;
        let frame = surface.get_current_texture().context("Acquiring swapchain texture")?;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Egui Encoder") });
        let mut extra_cmd = painter.update_buffers(device, queue, &mut encoder, paint_jobs, screen);

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Egui Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.06, b: 0.1, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            let pass = unsafe {
                std::mem::transmute::<&mut wgpu::RenderPass<'_>, &mut wgpu::RenderPass<'static>>(&mut pass)
            };
            painter.render(pass, paint_jobs, screen);
        }

        extra_cmd.push(encoder.finish());
        queue.submit(extra_cmd.into_iter());
        frame.present();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalSize;

    #[test]
    fn depth_texture_respects_size() {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await
                .expect("adapter");
            let device_desc = wgpu::DeviceDescriptor {
                label: Some("depth-test-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                experimental_features: wgpu::ExperimentalFeatures::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::default(),
            };
            let (device, _) = adapter.request_device(&device_desc).await.expect("device");
            let size = PhysicalSize::new(321, 123);
            let (texture, view) = Renderer::create_depth_texture(&device, size).expect("depth texture");
            let extent = texture.size();
            assert_eq!(extent.width, 321);
            assert_eq!(extent.height, 123);
            assert_eq!(texture.dimension(), wgpu::TextureDimension::D2);
            let _ = view;
        });
    }
}
