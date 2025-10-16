use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;
use glam::Mat4;
use crate::ecs::InstanceData;

// egui
use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals { proj: [[f32; 4]; 4], }

pub struct Renderer {
    surface: Option<wgpu::Surface<'static>>,
    device: Option<wgpu::Device>,
    queue: Option<wgpu::Queue>,
    config: Option<wgpu::SurfaceConfiguration>,
    size: PhysicalSize<u32>,
    window: Option<Arc<Window>>,

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
}

impl Renderer {
    pub async fn new() -> Self {
        Self {
            surface: None, device: None, queue: None, config: None,
            size: PhysicalSize::new(1280, 720),
            window: None,
            pipeline: None, vertex_buffer: None, index_buffer: None,
            globals_buf: None, globals_bg: None, globals_bgl: None,
            texture_bg: None, texture_bgl: None,
            instance_buffer: None, instance_capacity: 0,
        }
    }

    pub fn ensure_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let window = Arc::new(event_loop.create_window(
            Window::default_attributes().with_title("Kestrel Engine - Milestone 5")
                                       .with_inner_size(self.size)
        ).expect("Failed to create window"));
        pollster::block_on(self.init_wgpu(&window));
        self.window = Some(window);
    }

    fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> wgpu::TextureFormat {
        formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(formats[0])
    }

    async fn init_wgpu(&mut self, window: &Arc<Window>) {
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("surface");
        let adapter = instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            }
        ).await.expect("adapter");
        let required_limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        let device_desc = wgpu::DeviceDescriptor {
            label: Some("Device"),
            required_features: wgpu::Features::empty(),
            required_limits,
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::default(),
        };
        let (device, queue) = adapter.request_device(&device_desc).await.expect("device");

        let caps = surface.get_capabilities(&adapter);
        let format = Self::choose_surface_format(&caps.formats);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        self.surface = Some(surface);
        self.device = Some(device);
        self.queue = Some(queue);
        self.config = Some(config);
    }

    pub fn init_sprite_pipeline_with_atlas(&mut self, atlas_view: wgpu::TextureView, sampler: wgpu::Sampler) {
        let device = self.device.as_ref().unwrap();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sprite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/shaders/sprite_batch.wgsl").into())
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Globals BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None
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
                    binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let texture_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Texture BG"),
            layout: &texture_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        // Unit quad
        let vertices: [[f32;5];4] = [
            [-0.5,  0.5, 0.0, 0.0, 0.0],
            [ 0.5,  0.5, 0.0, 1.0, 0.0],
            [ 0.5, -0.5, 0.0, 1.0, 1.0],
            [-0.5, -0.5, 0.0, 0.0, 1.0],
        ];
        let indices: [u16;6] = [0,1,2, 0,2,3];
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
                        array_stride: std::mem::size_of::<[f32;5]>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute { shader_location: 0, format: wgpu::VertexFormat::Float32x3, offset: 0 },
                            wgpu::VertexAttribute { shader_location: 1, format: wgpu::VertexFormat::Float32x2, offset: 12 },
                        ],
                    },
                    wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<InstanceData>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &[
                            wgpu::VertexAttribute { shader_location: 2, format: wgpu::VertexFormat::Float32x4, offset: 0 },
                            wgpu::VertexAttribute { shader_location: 3, format: wgpu::VertexFormat::Float32x4, offset: 16 },
                            wgpu::VertexAttribute { shader_location: 4, format: wgpu::VertexFormat::Float32x4, offset: 32 },
                            wgpu::VertexAttribute { shader_location: 5, format: wgpu::VertexFormat::Float32x4, offset: 48 },
                            wgpu::VertexAttribute { shader_location: 6, format: wgpu::VertexFormat::Float32x4, offset: 64 },
                        ],
                    },
                ],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.config.as_ref().unwrap().format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
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
    }

    pub fn device_and_queue(&self) -> (&wgpu::Device, &wgpu::Queue) { (self.device.as_ref().unwrap(), self.queue.as_ref().unwrap()) }
    pub fn device(&self) -> &wgpu::Device { self.device.as_ref().unwrap() }
    pub fn queue(&self) -> &wgpu::Queue { self.queue.as_ref().unwrap() }
    pub fn surface_format(&self) -> wgpu::TextureFormat { self.config.as_ref().unwrap().format }
    pub fn size(&self) -> PhysicalSize<u32> { self.size }
    pub fn pixels_per_point(&self) -> f32 { 1.0 }

    pub fn window(&self) -> Option<&Window> { self.window.as_deref() }
    pub fn aspect_ratio(&self) -> f32 { if self.size.height == 0 { 1.0 } else { self.size.width as f32 / self.size.height as f32 } }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.size = new_size;
        if new_size.width > 0 && new_size.height > 0 {
            if let (Some(surface), Some(device), Some(config)) = (&self.surface, &self.device, &mut self.config) {
                config.width = new_size.width; config.height = new_size.height;
                surface.configure(device, config);
            }
        }
    }

    fn ensure_instance_capacity(&mut self, count: usize) {
        let device = self.device.as_ref().unwrap();
        if self.instance_capacity >= count { return; }
        let mut new_cap = self.instance_capacity.max(256);
        while new_cap < count { new_cap *= 2; }
        let buf_size = (new_cap * std::mem::size_of::<InstanceData>()) as u64;
        let new_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Instance Buffer"),
            size: buf_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer = Some(new_buf);
        self.instance_capacity = new_cap;
    }

    pub fn render_batch(&mut self, instances: &[InstanceData], view_proj: Mat4) -> Result<(), wgpu::SurfaceError> {
        {
            let queue = self.queue.as_ref().unwrap();
            queue.write_buffer(
                self.globals_buf.as_ref().unwrap(),
                0,
                bytemuck::bytes_of(&Globals { proj: view_proj.to_cols_array_2d() }),
            );
        }

        self.ensure_instance_capacity(instances.len());

        {
            let queue = self.queue.as_ref().unwrap();
            let byte_data = bytemuck::cast_slice(instances);
            queue.write_buffer(self.instance_buffer.as_ref().unwrap(), 0, byte_data);
        }

        let surface = self.surface.as_ref().unwrap();
        let device = self.device.as_ref().unwrap();
        let queue = self.queue.as_ref().unwrap();

        let frame = surface.get_current_texture()?;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Encoder") });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Sprite Pass"),
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
            pass.set_pipeline(self.pipeline.as_ref().unwrap());
            pass.set_bind_group(0, self.globals_bg.as_ref().unwrap(), &[]);
            pass.set_bind_group(1, self.texture_bg.as_ref().unwrap(), &[]);
            pass.set_vertex_buffer(0, self.vertex_buffer.as_ref().unwrap().slice(..));
            pass.set_vertex_buffer(1, self.instance_buffer.as_ref().unwrap().slice(..));
            pass.set_index_buffer(self.index_buffer.as_ref().unwrap().slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..6, 0, 0..(instances.len() as u32));
        }

        queue.submit(std::iter::once(encoder.finish()));
        frame.present();
        Ok(())
    }

    pub fn render_egui(&mut self, painter: &mut EguiRenderer, paint_jobs: &[egui::ClippedPrimitive], screen: &ScreenDescriptor) -> Result<(), wgpu::SurfaceError> {
        let surface = self.surface.as_ref().unwrap();
        let device = self.device.as_ref().unwrap();
        let queue = self.queue.as_ref().unwrap();
        let frame = surface.get_current_texture()?;
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Egui Encoder") });
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
