use crate::camera3d::Camera3D;
use crate::config::WindowConfig;
use crate::ecs::{InstanceData, MeshLightingInfo};
use crate::environment::EnvironmentGpu;
use crate::material_registry::MaterialGpu;
use crate::mesh::{Mesh, MeshVertex};
use anyhow::{anyhow, Context, Result};
use glam::{Mat4, Vec3};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
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
const MAX_SKIN_JOINTS: usize = 256;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshFrameData {
    view_proj: [[f32; 4]; 4],
    camera_pos: [f32; 4],
    light_dir: [f32; 4],
    light_color: [f32; 4],
    ambient_color: [f32; 4],
    exposure_params: [f32; 4],
    padding: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshDrawData {
    model: [[f32; 4]; 4],
    base_color: [f32; 4],
    emissive: [f32; 4],
    material_params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowUniform {
    light_view_proj: [[f32; 4]; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowDrawUniform {
    model: [[f32; 4]; 4],
    joint_count: u32,
    _padding: [u32; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct RenderViewport {
    pub origin: (f32, f32),
    pub size: (f32, f32),
}

#[derive(Clone, Debug)]
pub struct SpriteBatch {
    pub atlas: String,
    pub range: Range<u32>,
    pub view: Arc<wgpu::TextureView>,
}

struct SpriteBindCacheEntry {
    view: Arc<wgpu::TextureView>,
    sampler_id: u64,
    bind_group: Arc<wgpu::BindGroup>,
}

#[derive(Debug, Clone)]
pub struct GpuPassTiming {
    pub label: &'static str,
    pub duration_ms: f32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
enum GpuTimestampLabel {
    FrameStart,
    ShadowStart,
    ShadowEnd,
    MeshStart,
    MeshEnd,
    SpriteStart,
    SpriteEnd,
    FrameEnd,
    EguiStart,
    EguiEnd,
}

#[derive(Copy, Clone, Debug)]
struct GpuTimestampMark {
    label: GpuTimestampLabel,
    index: u32,
}

#[derive(Clone)]
struct GpuTimer {
    supported: bool,
    timestamp_period: f32,
    max_queries: u32,
    query_set: Option<wgpu::QuerySet>,
    query_buffer: Option<wgpu::Buffer>,
    readback_buffer: Option<wgpu::Buffer>,
    marks: Vec<GpuTimestampMark>,
    pending_query_count: u32,
    latest: Vec<GpuPassTiming>,
    frame_active: bool,
    next_query: u32,
}

impl Default for GpuTimer {
    fn default() -> Self {
        Self {
            supported: false,
            timestamp_period: 0.0,
            max_queries: 32,
            query_set: None,
            query_buffer: None,
            readback_buffer: None,
            marks: Vec::new(),
            pending_query_count: 0,
            latest: Vec::new(),
            frame_active: false,
            next_query: 0,
        }
    }
}

impl GpuTimer {
    fn configure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, supported: bool) {
        self.supported = supported;
        if !supported {
            self.timestamp_period = 0.0;
            self.query_set = None;
            self.query_buffer = None;
            self.readback_buffer = None;
            self.marks.clear();
            self.pending_query_count = 0;
            self.latest.clear();
            self.frame_active = false;
            self.next_query = 0;
            return;
        }
        self.timestamp_period = queue.get_timestamp_period();
        if self.query_set.is_none() {
            self.query_set = Some(device.create_query_set(&wgpu::QuerySetDescriptor {
                label: Some("gpu-timer"),
                ty: wgpu::QueryType::Timestamp,
                count: self.max_queries,
            }));
        }
        if self.query_buffer.is_none() {
            let size = self.max_queries as u64 * std::mem::size_of::<u64>() as u64;
            self.query_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu-timer-buffer"),
                size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
            self.readback_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu-timer-readback"),
                size,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
    }

    fn begin_frame(&mut self) {
        if !self.supported {
            return;
        }
        self.next_query = 0;
        self.marks.clear();
        self.pending_query_count = 0;
        self.frame_active = true;
    }

    fn write_timestamp(&mut self, encoder: &mut wgpu::CommandEncoder, label: GpuTimestampLabel) {
        if !self.supported || !self.frame_active {
            return;
        }
        if self.next_query >= self.max_queries {
            return;
        }
        if let Some(query_set) = self.query_set.as_ref() {
            encoder.write_timestamp(query_set, self.next_query);
            self.marks.push(GpuTimestampMark { label, index: self.next_query });
            self.next_query += 1;
        }
    }

    fn finish_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.supported || !self.frame_active {
            return;
        }
        if self.next_query == 0 {
            self.frame_active = false;
            return;
        }
        if let (Some(query_set), Some(buffer)) = (self.query_set.as_ref(), self.query_buffer.as_ref()) {
            encoder.resolve_query_set(query_set, 0..self.next_query, buffer, 0);
            if let Some(readback) = self.readback_buffer.as_ref() {
                let byte_len = self.next_query as u64 * std::mem::size_of::<u64>() as u64;
                encoder.copy_buffer_to_buffer(buffer, 0, readback, 0, byte_len);
            }
            self.pending_query_count = self.next_query;
        }
        self.frame_active = false;
    }

    fn collect_results(&mut self, device: &wgpu::Device) {
        if !self.supported || self.pending_query_count == 0 {
            return;
        }
        let buffer = match self.readback_buffer.as_ref() {
            Some(buffer) => buffer,
            None => return,
        };
        let byte_len = self.pending_query_count as usize * std::mem::size_of::<u64>();
        let slice = buffer.slice(0..byte_len as u64);
        let (sender, receiver) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        match receiver.recv() {
            Ok(Ok(())) => {}
            _ => {
                return;
            }
        }
        let data = slice.get_mapped_range();
        let mut timestamps: Vec<u64> = Vec::with_capacity(self.pending_query_count as usize);
        for chunk in data.chunks_exact(std::mem::size_of::<u64>()) {
            let mut bytes = [0u8; 8];
            bytes.copy_from_slice(chunk);
            timestamps.push(u64::from_le_bytes(bytes));
        }
        drop(data);
        buffer.unmap();

        let mut value_map: HashMap<GpuTimestampLabel, u64> = HashMap::new();
        for mark in &self.marks {
            if let Some(value) = timestamps.get(mark.index as usize) {
                value_map.insert(mark.label, *value);
            }
        }

        self.latest.clear();
        let nanos_per_tick = self.timestamp_period as f64;
        let mut push_pass = |label: &'static str, start: GpuTimestampLabel, end: GpuTimestampLabel| {
            if let (Some(s), Some(e)) = (value_map.get(&start), value_map.get(&end)) {
                if e > s {
                    let duration_ms = ((*e - *s) as f64 * nanos_per_tick) / 1_000_000.0;
                    self.latest.push(GpuPassTiming { label, duration_ms: duration_ms as f32 });
                }
            }
        };

        push_pass("Shadow pass", GpuTimestampLabel::ShadowStart, GpuTimestampLabel::ShadowEnd);
        push_pass("Mesh pass", GpuTimestampLabel::MeshStart, GpuTimestampLabel::MeshEnd);
        push_pass("Sprite pass", GpuTimestampLabel::SpriteStart, GpuTimestampLabel::SpriteEnd);
        push_pass("Frame (pre-egui)", GpuTimestampLabel::FrameStart, GpuTimestampLabel::FrameEnd);
        push_pass("Egui pass", GpuTimestampLabel::EguiStart, GpuTimestampLabel::EguiEnd);
        if value_map.contains_key(&GpuTimestampLabel::EguiEnd) {
            push_pass("Frame (with egui)", GpuTimestampLabel::FrameStart, GpuTimestampLabel::EguiEnd);
        }

        self.pending_query_count = 0;
        self.marks.clear();
    }

    fn take_latest(&mut self) -> Vec<GpuPassTiming> {
        if self.latest.is_empty() {
            Vec::new()
        } else {
            std::mem::take(&mut self.latest)
        }
    }
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
    pub lighting: MeshLightingInfo,
    pub material: Arc<MaterialGpu>,
    pub casts_shadows: bool,
    pub skin_palette: Option<Arc<[Mat4]>>,
}

pub struct SurfaceFrame {
    view: wgpu::TextureView,
    surface: Option<wgpu::SurfaceTexture>,
}

impl SurfaceFrame {
    fn new(surface: wgpu::SurfaceTexture) -> Self {
        let view = surface.texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self { view, surface: Some(surface) }
    }

    #[cfg(test)]
    fn headless(view: wgpu::TextureView) -> Self {
        Self { view, surface: None }
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn present(mut self) {
        if let Some(surface) = self.surface.take() {
            surface.present();
        }
    }
}

#[cfg(test)]
struct HeadlessTarget {
    texture: wgpu::Texture,
}

struct MeshPipelineResources {
    pipeline: wgpu::RenderPipeline,
    frame_bgl: Arc<wgpu::BindGroupLayout>,
    draw_bgl: Arc<wgpu::BindGroupLayout>,
    skinning_bgl: Arc<wgpu::BindGroupLayout>,
    material_bgl: Arc<wgpu::BindGroupLayout>,
    environment_bgl: Arc<wgpu::BindGroupLayout>,
}

#[derive(Default)]
struct MeshPass {
    resources: Option<MeshPipelineResources>,
    frame_buffer: Option<wgpu::Buffer>,
    draw_buffer: Option<wgpu::Buffer>,
    frame_bind_group: Option<wgpu::BindGroup>,
    draw_bind_group: Option<wgpu::BindGroup>,
    skinning_identity_buffer: Option<wgpu::Buffer>,
    skinning_identity_bind_group: Option<wgpu::BindGroup>,
    skinning_palette_buffers: Vec<wgpu::Buffer>,
    skinning_palette_bind_groups: Vec<wgpu::BindGroup>,
    palette_staging: Vec<[f32; 16]>,
    skinning_cursor: usize,
}

struct RendererEnvironmentState {
    bind_group: Arc<wgpu::BindGroup>,
    mip_count: u32,
    intensity: f32,
}

struct ShadowPipelineResources {
    pipeline: wgpu::RenderPipeline,
    skinning_bgl: Arc<wgpu::BindGroupLayout>,
}

struct ShadowPass {
    resources: Option<ShadowPipelineResources>,
    uniform_buffer: Option<wgpu::Buffer>,
    frame_bind_group: Option<wgpu::BindGroup>,
    draw_buffer: Option<wgpu::Buffer>,
    draw_bind_group: Option<wgpu::BindGroup>,
    skinning_identity_buffer: Option<wgpu::Buffer>,
    skinning_identity_bind_group: Option<wgpu::BindGroup>,
    skinning_palette_buffers: Vec<wgpu::Buffer>,
    skinning_palette_bind_groups: Vec<wgpu::BindGroup>,
    palette_staging: Vec<[f32; 16]>,
    skinning_cursor: usize,
    map_texture: Option<wgpu::Texture>,
    map_view: Option<wgpu::TextureView>,
    sampler: Option<wgpu::Sampler>,
    sample_layout: Option<Arc<wgpu::BindGroupLayout>>,
    sample_bind_group: Option<wgpu::BindGroup>,
    resolution: u32,
    dirty: bool,
}

impl Default for ShadowPass {
    fn default() -> Self {
        Self {
            resources: None,
            uniform_buffer: None,
            frame_bind_group: None,
            draw_buffer: None,
            draw_bind_group: None,
            skinning_identity_buffer: None,
            skinning_identity_bind_group: None,
            skinning_palette_buffers: Vec::new(),
            skinning_palette_bind_groups: Vec::new(),
            palette_staging: Vec::new(),
            skinning_cursor: 0,
            map_texture: None,
            map_view: None,
            sampler: None,
            sample_layout: None,
            sample_bind_group: None,
            resolution: 2048,
            dirty: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SceneLightingState {
    pub direction: Vec3,
    pub color: Vec3,
    pub ambient: Vec3,
    pub exposure: f32,
    pub environment_intensity: f32,
    pub shadow_distance: f32,
    pub shadow_bias: f32,
    pub shadow_strength: f32,
}

impl Default for SceneLightingState {
    fn default() -> Self {
        Self {
            direction: Vec3::new(0.4, 0.8, 0.35).normalize(),
            color: Vec3::new(1.05, 0.98, 0.92),
            ambient: Vec3::splat(0.03),
            exposure: 1.0,
            environment_intensity: 1.0,
            shadow_distance: 35.0,
            shadow_bias: 0.002,
            shadow_strength: 1.0,
        }
    }
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
    mesh_pass: MeshPass,
    shadow_pass: ShadowPass,
    lighting: SceneLightingState,
    environment_state: Option<RendererEnvironmentState>,

    sprite_bind_cache: HashMap<String, SpriteBindCacheEntry>,
    present_modes: Vec<wgpu::PresentMode>,
    gpu_timer: GpuTimer,
    skinning_limit_warnings: HashSet<usize>,
    #[cfg(test)]
    resize_invocations: usize,
    #[cfg(test)]
    headless_target: Option<HeadlessTarget>,
    #[cfg(test)]
    surface_error_injector: Option<wgpu::SurfaceError>,
}

const DEFAULT_PRESENT_MODES: [wgpu::PresentMode; 1] = [wgpu::PresentMode::Fifo];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceErrorAction {
    Reconfigure,
    Retry,
    OutOfMemory,
    Unknown,
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
            mesh_pass: MeshPass::default(),
            shadow_pass: ShadowPass::default(),
            lighting: SceneLightingState::default(),
            environment_state: None,
            sprite_bind_cache: HashMap::new(),
            present_modes: Vec::new(),
            gpu_timer: GpuTimer::default(),
            skinning_limit_warnings: HashSet::new(),
            #[cfg(test)]
            resize_invocations: 0,
            #[cfg(test)]
            headless_target: None,
            #[cfg(test)]
            surface_error_injector: None,
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
        } else {
            attrs = attrs.with_maximized(true);
        }
        let window = Arc::new(event_loop.create_window(attrs).context("Failed to create window")?);
        if !self.fullscreen {
            window.set_maximized(true);
        }
        pollster::block_on(self.init_wgpu(&window))?;
        if !self.fullscreen {
            let maximized_size = window.inner_size();
            if maximized_size.width > 0 && maximized_size.height > 0 && maximized_size != self.size {
                self.resize(maximized_size);
            }
        }
        self.window = Some(window);
        Ok(())
    }

    pub fn set_lighting(&mut self, direction: Vec3, color: Vec3, ambient: Vec3, exposure: f32) {
        self.lighting.direction = direction;
        self.lighting.color = color;
        self.lighting.ambient = ambient;
        self.lighting.exposure = exposure.max(0.001);
        self.shadow_pass.dirty = true;
    }

    pub fn mark_shadow_settings_dirty(&mut self) {
        self.shadow_pass.dirty = true;
    }

    pub fn set_environment(&mut self, environment: &EnvironmentGpu, intensity: f32) -> Result<()> {
        if self.mesh_pass.resources.is_none() {
            self.init_mesh_pipeline()?;
        }
        let layout = self.environment_bind_group_layout()?;
        let device = self.device()?.clone();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Environment Bind Group"),
            layout: layout.as_ref(),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(environment.diffuse_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(environment.specular_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(environment.brdf_view()),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(environment.sampler()),
                },
            ],
        });
        self.environment_state = Some(RendererEnvironmentState {
            bind_group: Arc::new(bind_group),
            mip_count: environment.specular_mip_count().max(1),
            intensity: intensity.max(0.0),
        });
        self.lighting.environment_intensity = intensity.max(0.0);
        Ok(())
    }

    pub fn set_environment_intensity(&mut self, intensity: f32) {
        if let Some(state) = self.environment_state.as_mut() {
            state.intensity = intensity.max(0.0);
        }
        self.lighting.environment_intensity = intensity.max(0.0);
    }

    pub fn environment_parameters(&self) -> Option<(u32, f32)> {
        self.environment_state.as_ref().map(|state| (state.mip_count, state.intensity))
    }

    pub fn lighting(&self) -> &SceneLightingState {
        &self.lighting
    }

    pub fn lighting_mut(&mut self) -> &mut SceneLightingState {
        &mut self.lighting
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
        let adapter_features = adapter.features();
        let supports_timestamp = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let supports_encoder_queries =
            adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let gpu_timing_supported = supports_timestamp && supports_encoder_queries;
        let mut required_features = wgpu::Features::empty();
        if supports_timestamp {
            required_features |= wgpu::Features::TIMESTAMP_QUERY;
        }
        if supports_encoder_queries {
            required_features |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        }
        let mut required_limits =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        required_limits.max_bind_groups = required_limits.max_bind_groups.max(6);
        required_limits.max_storage_buffers_per_shader_stage =
            required_limits.max_storage_buffers_per_shader_stage.max(1);
        let device_desc = wgpu::DeviceDescriptor {
            label: Some("Device"),
            required_features,
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
        if let (Some(device), Some(queue)) = (self.device.as_ref(), self.queue.as_ref()) {
            self.gpu_timer.configure(device, queue, gpu_timing_supported);
        }
        self.config = Some(config);
        self.depth_texture = Some(depth_texture);
        self.depth_view = Some(depth_view);
        self.present_modes = caps.present_modes.clone();
        Ok(())
    }

    pub fn init_sprite_pipeline_with_atlas(
        &mut self,
        atlas_view: wgpu::TextureView,
        sampler: wgpu::Sampler,
    ) -> Result<()> {
        self.clear_sprite_bind_cache();
        let device = self.device.as_ref().context("GPU device not initialized")?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Sprite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/shaders/sprite_batch.wgsl").into()),
        });

        let globals_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Globals BGL"),
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
        let device = self.device()?.clone();

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mesh Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../assets/shaders/mesh_basic.wgsl").into()),
        });

        let frame_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mesh Frame BGL"),
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
        }));
        let draw_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mesh Draw BGL"),
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
        }));
        let skinning_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mesh Skinning BGL"),
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
        }));
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesh Frame Buffer"),
            size: std::mem::size_of::<MeshFrameData>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mesh Draw Buffer"),
            size: std::mem::size_of::<MeshDrawData>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let material_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Material BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        }));

        let environment_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Environment BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        }));

        let shadow_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow Sample BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Depth,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        }));

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mesh Pipeline Layout"),
            bind_group_layouts: &[
                frame_bgl.as_ref(),
                draw_bgl.as_ref(),
                skinning_bgl.as_ref(),
                material_bgl.as_ref(),
                shadow_bgl.as_ref(),
                environment_bgl.as_ref(),
            ],
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

        self.mesh_pass.resources = Some(MeshPipelineResources {
            pipeline,
            frame_bgl: frame_bgl.clone(),
            draw_bgl: draw_bgl.clone(),
            skinning_bgl: skinning_bgl.clone(),
            material_bgl: material_bgl.clone(),
            environment_bgl: environment_bgl.clone(),
        });
        self.mesh_pass.frame_buffer = Some(frame_buf);
        self.mesh_pass.draw_buffer = Some(draw_buf);
        self.mesh_pass.frame_bind_group = None;
        self.mesh_pass.draw_bind_group = None;
        self.mesh_pass.skinning_identity_buffer = None;
        self.mesh_pass.skinning_identity_bind_group = None;
        self.shadow_pass.sample_layout = Some(shadow_bgl);
        self.shadow_pass.sample_bind_group = None;
        Ok(())
    }

    pub fn create_gpu_mesh(&self, mesh: &Mesh) -> Result<GpuMesh> {
        let device = self.device()?.clone();
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
        if self.mesh_pass.resources.is_none() {
            self.init_mesh_pipeline()?;
        }
        if self.depth_texture.is_none() {
            self.recreate_depth_texture()?;
        }
        let environment_state =
            self.environment_state.as_ref().context("Environment state not configured")?;
        let mesh_resources = self.mesh_pass.resources.as_ref().context("Mesh pipeline not initialized")?;
        let depth_view = self.depth_view.as_ref().context("Depth texture missing")?;
        let device = self.device()?.clone();
        let vp_size = PhysicalSize::new(
            viewport.size.0.max(1.0).round() as u32,
            viewport.size.1.max(1.0).round() as u32,
        );
        let view_proj = camera.view_projection(vp_size);
        let lighting_dir = self.lighting.direction.normalize_or_zero();
        let frame_data = MeshFrameData {
            view_proj: view_proj.to_cols_array_2d(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            light_dir: [lighting_dir.x, lighting_dir.y, lighting_dir.z, 0.0],
            light_color: [self.lighting.color.x, self.lighting.color.y, self.lighting.color.z, 1.0],
            ambient_color: [self.lighting.ambient.x, self.lighting.ambient.y, self.lighting.ambient.z, 1.0],
            exposure_params: [
                self.lighting.exposure,
                environment_state.mip_count.max(1) as f32,
                environment_state.intensity,
                0.0,
            ],
            padding: [0.0; 4],
        };

        if self.mesh_pass.frame_buffer.is_none() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mesh Frame Buffer"),
                size: std::mem::size_of::<MeshFrameData>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.mesh_pass.frame_buffer = Some(buffer);
            self.mesh_pass.frame_bind_group = None;
        }
        let frame_buffer = self.mesh_pass.frame_buffer.as_ref().context("Mesh frame buffer missing")?.clone();

        if self.mesh_pass.draw_buffer.is_none() {
            let draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mesh Draw Buffer"),
                size: std::mem::size_of::<MeshDrawData>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.mesh_pass.draw_buffer = Some(draw_buf);
            self.mesh_pass.draw_bind_group = None;
        }
        let draw_buffer = self.mesh_pass.draw_buffer.as_ref().context("Mesh draw buffer missing")?.clone();

        if self.mesh_pass.frame_bind_group.is_none() {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mesh Frame BG"),
                layout: mesh_resources.frame_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() }],
            });
            self.mesh_pass.frame_bind_group = Some(bind_group);
        }
        if self.mesh_pass.draw_bind_group.is_none() {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mesh Draw BG"),
                layout: mesh_resources.draw_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: draw_buffer.as_entire_binding() }],
            });
            self.mesh_pass.draw_bind_group = Some(bind_group);
        }

        let queue = self.queue()?.clone();
        queue.write_buffer(&frame_buffer, 0, bytemuck::bytes_of(&frame_data));

        let frame_bind_group =
            self.mesh_pass.frame_bind_group.as_ref().context("Mesh frame bind group missing")?;
        let draw_bind_group =
            self.mesh_pass.draw_bind_group.as_ref().context("Mesh draw bind group missing")?;
        let shadow_bind_group =
            self.shadow_pass.sample_bind_group.as_ref().context("Shadow sample bind group missing")?;
        let environment_bind_group = environment_state.bind_group.as_ref();

        if self.mesh_pass.skinning_identity_buffer.is_none() {
            let identity = Mat4::IDENTITY.to_cols_array();
            let palette: Vec<[f32; 16]> = vec![identity; MAX_SKIN_JOINTS];
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Mesh Skinning Identity Buffer"),
                contents: bytemuck::cast_slice(&palette),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            self.mesh_pass.skinning_identity_buffer = Some(buffer);
            self.mesh_pass.skinning_identity_bind_group = None;
        }
        if self.mesh_pass.skinning_identity_bind_group.is_none() {
            let buffer = self
                .mesh_pass
                .skinning_identity_buffer
                .as_ref()
                .context("Mesh skinning identity buffer missing")?;
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mesh Skinning Identity BG"),
                layout: mesh_resources.skinning_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            self.mesh_pass.skinning_identity_bind_group = Some(bind_group);
        }
        let skinning_identity_bind_group = self
            .mesh_pass
            .skinning_identity_bind_group
            .as_ref()
            .context("Mesh skinning identity bind group missing")?;

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
        pass.set_pipeline(&mesh_resources.pipeline);
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

        pass.set_bind_group(0, frame_bind_group, &[]);
        pass.set_bind_group(4, shadow_bind_group, &[]);
        pass.set_bind_group(5, environment_bind_group, &[]);

        self.mesh_pass.skinning_cursor = 0;
        let identity_cols = Mat4::IDENTITY.to_cols_array();
        if self.mesh_pass.palette_staging.len() != MAX_SKIN_JOINTS {
            self.mesh_pass.palette_staging.clear();
            self.mesh_pass.palette_staging.resize(MAX_SKIN_JOINTS, identity_cols);
        }
        for draw in draws {
            let base_color = draw.lighting.base_color;
            let emissive = draw.lighting.emissive.unwrap_or(Vec3::ZERO);
            let metallic = draw.lighting.metallic.clamp(0.0, 1.0);
            let roughness = draw.lighting.roughness.clamp(0.04, 1.0);
            let palette_len = draw.skin_palette.as_ref().map(|palette| palette.len()).unwrap_or(0);
            if palette_len > MAX_SKIN_JOINTS && self.skinning_limit_warnings.insert(palette_len) {
                eprintln!(
                    "[renderer] Skin palette has {} joints; only the first {} will be uploaded.",
                    palette_len, MAX_SKIN_JOINTS
                );
            }
            let joint_count = palette_len.min(MAX_SKIN_JOINTS);
            let draw_data = MeshDrawData {
                model: draw.model.to_cols_array_2d(),
                base_color: [base_color.x, base_color.y, base_color.z, 1.0],
                emissive: [emissive.x, emissive.y, emissive.z, 0.0],
                material_params: [
                    metallic,
                    roughness,
                    if draw.lighting.receive_shadows { 1.0 } else { 0.0 },
                    joint_count as f32,
                ],
            };
            queue.write_buffer(&draw_buffer, 0, bytemuck::bytes_of(&draw_data));
            pass.set_bind_group(1, draw_bind_group, &[]);
            if joint_count > 0 {
                {
                    let staging = &mut self.mesh_pass.palette_staging;
                    for slot in staging.iter_mut() {
                        *slot = identity_cols;
                    }
                    if let Some(palette) = draw.skin_palette.as_ref() {
                        for (dst, mat) in staging.iter_mut().zip(palette.iter()).take(joint_count) {
                            *dst = mat.to_cols_array();
                        }
                    }
                }
                let slot = self.mesh_pass.skinning_cursor;
                self.mesh_pass.skinning_cursor += 1;
                while self.mesh_pass.skinning_palette_buffers.len() <= slot {
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Mesh Skinning Palette Buffer"),
                        size: (MAX_SKIN_JOINTS * std::mem::size_of::<[f32; 16]>()) as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Mesh Skinning Palette BG"),
                        layout: mesh_resources.skinning_bgl.as_ref(),
                        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
                    });
                    self.mesh_pass.skinning_palette_buffers.push(buffer);
                    self.mesh_pass.skinning_palette_bind_groups.push(bind_group);
                }
                let buffer = &self.mesh_pass.skinning_palette_buffers[slot];
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.mesh_pass.palette_staging));
                let bind_group = &self.mesh_pass.skinning_palette_bind_groups[slot];
                pass.set_bind_group(2, bind_group, &[]);
            } else {
                pass.set_bind_group(2, skinning_identity_bind_group, &[]);
            }
            pass.set_bind_group(3, draw.material.bind_group(), &[]);
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
    pub fn material_bind_group_layout(&mut self) -> Result<Arc<wgpu::BindGroupLayout>> {
        if self.mesh_pass.resources.is_none() {
            self.init_mesh_pipeline()?;
        }
        let resources = self.mesh_pass.resources.as_ref().context("Mesh pipeline not initialized")?;
        Ok(resources.material_bgl.clone())
    }
    pub fn environment_bind_group_layout(&mut self) -> Result<Arc<wgpu::BindGroupLayout>> {
        if self.mesh_pass.resources.is_none() {
            self.init_mesh_pipeline()?;
        }
        let resources = self.mesh_pass.resources.as_ref().context("Mesh pipeline not initialized")?;
        Ok(resources.environment_bgl.clone())
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

    fn recreate_shadow_map(&mut self) -> Result<()> {
        let device = self.device()?.clone();
        let resolution = self.shadow_pass.resolution.max(1);
        let extent = wgpu::Extent3d { width: resolution, height: resolution, depth_or_array_layers: 1 };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shadow Map"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.shadow_pass.map_texture = Some(texture);
        self.shadow_pass.map_view = Some(view);
        self.shadow_pass.sample_bind_group = None;
        self.shadow_pass.dirty = true;
        Ok(())
    }

    fn ensure_shadow_resources(&mut self) -> Result<()> {
        let device = self.device()?.clone();
        if self.shadow_pass.resources.is_none() {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Shadow Shader"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../assets/shaders/mesh_shadow.wgsl").into()),
            });

            let frame_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Shadow Frame BGL"),
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
            }));

            let draw_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Shadow Draw BGL"),
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
            }));

            let skinning_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Shadow Skinning BGL"),
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
            }));

            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Shadow Pipeline Layout"),
                bind_group_layouts: &[frame_bgl.as_ref(), draw_bgl.as_ref(), skinning_bgl.as_ref()],
                push_constant_ranges: &[],
            });

            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Shadow Pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[MeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    unclipped_depth: false,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    conservative: false,
                    strip_index_format: None,
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

            self.shadow_pass.resources = Some(ShadowPipelineResources { pipeline, skinning_bgl });
            self.shadow_pass.skinning_identity_buffer = None;
            self.shadow_pass.skinning_identity_bind_group = None;

            let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shadow Uniform Buffer"),
                size: std::mem::size_of::<ShadowUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.shadow_pass.uniform_buffer = Some(uniform_buffer);

            let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shadow Draw Buffer"),
                size: std::mem::size_of::<ShadowDrawUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.shadow_pass.draw_buffer = Some(draw_buffer);

            let uniform_buffer_ref =
                self.shadow_pass.uniform_buffer.as_ref().context("Shadow uniform buffer missing")?;
            let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow Frame BG"),
                layout: frame_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer_ref.as_entire_binding(),
                }],
            });
            self.shadow_pass.frame_bind_group = Some(frame_bind_group);

            let draw_buffer_ref =
                self.shadow_pass.draw_buffer.as_ref().context("Shadow draw buffer missing")?;
            let draw_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow Draw BG"),
                layout: draw_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: draw_buffer_ref.as_entire_binding(),
                }],
            });
            self.shadow_pass.draw_bind_group = Some(draw_bind_group);

            self.shadow_pass.dirty = true;
        }

        if self.shadow_pass.map_texture.is_none() || self.shadow_pass.map_view.is_none() {
            self.recreate_shadow_map()?;
        }

        if self.shadow_pass.sampler.is_none() {
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Shadow Sampler"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Nearest,
                lod_min_clamp: 0.0,
                lod_max_clamp: 0.0,
                compare: Some(wgpu::CompareFunction::LessEqual),
                anisotropy_clamp: 1,
                border_color: None,
            });
            self.shadow_pass.sampler = Some(sampler);
        }

        if self.shadow_pass.sample_bind_group.is_none() {
            if let (Some(layout), Some(buffer), Some(view), Some(sampler)) = (
                self.shadow_pass.sample_layout.as_ref(),
                self.shadow_pass.uniform_buffer.as_ref(),
                self.shadow_pass.map_view.as_ref(),
                self.shadow_pass.sampler.as_ref(),
            ) {
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Shadow Sample BG"),
                    layout: layout.as_ref(),
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                });
                self.shadow_pass.sample_bind_group = Some(bind_group);
            }
        }

        Ok(())
    }

    fn write_shadow_uniform(&mut self, matrix: Mat4, strength: f32) -> Result<()> {
        let queue = self.queue()?;
        let buffer = self.shadow_pass.uniform_buffer.as_ref().context("Shadow uniform buffer missing")?;
        let bias = self.lighting.shadow_bias.clamp(0.00001, 0.05);
        let data = ShadowUniform {
            light_view_proj: matrix.to_cols_array_2d(),
            params: [bias, strength.clamp(0.0, 1.0), 0.0, 0.0],
        };
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&data));
        self.shadow_pass.dirty = false;
        Ok(())
    }

    fn prepare_shadow_map(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        draws: &[MeshDraw],
        camera: &Camera3D,
    ) -> Result<()> {
        self.init_mesh_pipeline()?;
        self.ensure_shadow_resources()?;
        let device = self.device()?.clone();
        let shadow_strength = self.lighting.shadow_strength.clamp(0.0, 1.0);
        let casters: Vec<&MeshDraw> = draws.iter().filter(|draw| draw.casts_shadows).collect();
        if casters.is_empty() || shadow_strength <= 0.0 {
            self.write_shadow_uniform(Mat4::IDENTITY, 0.0)?;
            return Ok(());
        }

        let mut light_dir = self.lighting.direction.normalize_or_zero();
        if light_dir.length_squared() < 1e-4 {
            light_dir = Vec3::new(0.4, 0.8, 0.35).normalize();
        }

        let focus = camera.target;
        let distance = self.lighting.shadow_distance.max(1.0);
        let light_pos = focus - light_dir * distance;
        let mut up = Vec3::new(0.0, 1.0, 0.0);
        if up.dot(light_dir).abs() > 0.95 {
            up = Vec3::new(1.0, 0.0, 0.0);
        }
        let view = Mat4::look_at_rh(light_pos, focus, up);
        let half = distance;
        let near = 0.1;
        let far = distance * 4.0;
        let proj = Mat4::orthographic_rh(-half, half, -half, half, near, far);
        let light_matrix = proj * view;
        self.write_shadow_uniform(light_matrix, shadow_strength)?;

        let resources = self.shadow_pass.resources.as_ref().context("Shadow pipeline resources missing")?;
        let view = self.shadow_pass.map_view.as_ref().context("Shadow map view missing")?;
        let frame_bg =
            self.shadow_pass.frame_bind_group.as_ref().context("Shadow frame bind group missing")?;
        let draw_bg = self.shadow_pass.draw_bind_group.as_ref().context("Shadow draw bind group missing")?;
        let draw_buffer = self.shadow_pass.draw_buffer.as_ref().context("Shadow draw buffer missing")?;
        let queue = self.queue()?.clone();

        if self.shadow_pass.skinning_identity_buffer.is_none() {
            let identity = Mat4::IDENTITY.to_cols_array();
            let palette: Vec<[f32; 16]> = vec![identity; MAX_SKIN_JOINTS];
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Shadow Skinning Identity Buffer"),
                contents: bytemuck::cast_slice(&palette),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            self.shadow_pass.skinning_identity_buffer = Some(buffer);
            self.shadow_pass.skinning_identity_bind_group = None;
        }
        if self.shadow_pass.skinning_identity_bind_group.is_none() {
            let buffer = self
                .shadow_pass
                .skinning_identity_buffer
                .as_ref()
                .context("Shadow skinning identity buffer missing")?;
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow Skinning Identity BG"),
                layout: resources.skinning_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            self.shadow_pass.skinning_identity_bind_group = Some(bind_group);
        }
        let shadow_skinning_identity = self
            .shadow_pass
            .skinning_identity_bind_group
            .as_ref()
            .context("Shadow skinning identity bind group missing")?;

        let resolution = self.shadow_pass.resolution.max(1);
        self.shadow_pass.skinning_cursor = 0;
        let identity_cols = Mat4::IDENTITY.to_cols_array();
        if self.shadow_pass.palette_staging.len() != MAX_SKIN_JOINTS {
            self.shadow_pass.palette_staging.clear();
            self.shadow_pass.palette_staging.resize(MAX_SKIN_JOINTS, identity_cols);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Shadow Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&resources.pipeline);
            let res_f = resolution as f32;
            pass.set_viewport(0.0, 0.0, res_f, res_f, 0.0, 1.0);
            pass.set_scissor_rect(0, 0, resolution, resolution);
            pass.set_bind_group(0, frame_bg, &[]);

            for draw in casters {
                let palette_len = draw.skin_palette.as_ref().map(|palette| palette.len()).unwrap_or(0);
                if palette_len > MAX_SKIN_JOINTS && self.skinning_limit_warnings.insert(palette_len) {
                    eprintln!(
                        "[renderer] Skin palette has {} joints; only the first {} will be uploaded.",
                        palette_len, MAX_SKIN_JOINTS
                    );
                }
                let joint_count = palette_len.min(MAX_SKIN_JOINTS);
                let draw_uniform = ShadowDrawUniform {
                    model: draw.model.to_cols_array_2d(),
                    joint_count: joint_count as u32,
                    _padding: [0; 3],
                };
                queue.write_buffer(draw_buffer, 0, bytemuck::bytes_of(&draw_uniform));
                pass.set_bind_group(1, draw_bg, &[]);
                if joint_count > 0 {
                    {
                        let staging = &mut self.shadow_pass.palette_staging;
                        for slot in staging.iter_mut() {
                            *slot = identity_cols;
                        }
                        if let Some(palette) = draw.skin_palette.as_ref() {
                            for (dst, mat) in staging.iter_mut().zip(palette.iter()).take(joint_count) {
                                *dst = mat.to_cols_array();
                            }
                        }
                    }
                    let slot = self.shadow_pass.skinning_cursor;
                    self.shadow_pass.skinning_cursor += 1;
                    while self.shadow_pass.skinning_palette_buffers.len() <= slot {
                        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("Shadow Skinning Palette Buffer"),
                            size: (MAX_SKIN_JOINTS * std::mem::size_of::<[f32; 16]>()) as u64,
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Shadow Skinning Palette BG"),
                            layout: resources.skinning_bgl.as_ref(),
                            entries: &[wgpu::BindGroupEntry {
                                binding: 0,
                                resource: buffer.as_entire_binding(),
                            }],
                        });
                        self.shadow_pass.skinning_palette_buffers.push(buffer);
                        self.shadow_pass.skinning_palette_bind_groups.push(bind_group);
                    }
                    let buffer = &self.shadow_pass.skinning_palette_buffers[slot];
                    queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.shadow_pass.palette_staging));
                    let bind_group = &self.shadow_pass.skinning_palette_bind_groups[slot];
                    pass.set_bind_group(2, bind_group, &[]);
                } else {
                    pass.set_bind_group(2, shadow_skinning_identity, &[]);
                }
                pass.set_vertex_buffer(0, draw.mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(draw.mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.mesh.index_count, 0, 0..1);
            }
        }

        Ok(())
    }

    pub fn window(&self) -> Option<&Window> {
        self.window.as_deref()
    }

    fn acquire_surface_frame(&mut self) -> Result<SurfaceFrame> {
        #[cfg(test)]
        if let Some(err) = self.surface_error_injector.take() {
            return Err(self.handle_surface_error(&err));
        }
        if let Some(surface) = self.surface.as_ref() {
            match surface.get_current_texture() {
                Ok(frame) => Ok(SurfaceFrame::new(frame)),
                Err(err) => Err(self.handle_surface_error(&err)),
            }
        } else {
            #[cfg(test)]
            {
                if let Some(target) = self.headless_target.as_ref() {
                    let view = target.texture.create_view(&wgpu::TextureViewDescriptor::default());
                    return Ok(SurfaceFrame::headless(view));
                }
            }
            Err(anyhow!("Surface not initialized"))
        }
    }

    #[cfg(test)]
    pub fn resize_invocations_for_test(&self) -> usize {
        self.resize_invocations
    }

    #[cfg(test)]
    pub fn prepare_headless_render_target_for_test(&mut self) -> Result<()> {
        let device = self.device()?;
        if self.size.width == 0 || self.size.height == 0 {
            return Err(anyhow!("Headless render target requires non-zero dimensions"));
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Headless Render Target"),
            size: wgpu::Extent3d {
                width: self.size.width,
                height: self.size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        self.headless_target = Some(HeadlessTarget { texture });
        Ok(())
    }

    #[cfg(test)]
    pub fn inject_surface_error_for_test(&mut self, error: wgpu::SurfaceError) {
        self.surface_error_injector = Some(error);
    }

    pub fn vsync_enabled(&self) -> bool {
        self.vsync
    }

    pub fn set_vsync(&mut self, enabled: bool) -> Result<()> {
        if self.vsync == enabled {
            return Ok(());
        }
        self.vsync = enabled;
        self.reconfigure_present_mode()
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
        #[cfg(test)]
        {
            self.resize_invocations = self.resize_invocations.saturating_add(1);
            self.headless_target = None;
        }
        if new_size.width > 0 && new_size.height > 0 {
            if let Some(config) = self.config.as_mut() {
                config.width = new_size.width;
                config.height = new_size.height;
                if let Err(err) = self.configure_surface() {
                    eprintln!("Surface resize failed: {err:?}");
                }
            }
            if let Err(err) = self.recreate_depth_texture() {
                eprintln!("Depth texture resize failed: {err:?}");
            }
        }
    }

    fn ensure_instance_capacity(&mut self, count: usize) -> Result<()> {
        let device = self.device.as_ref().context("GPU device not initialized")?;
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

    fn sprite_bind_group(
        &mut self,
        atlas: &str,
        view: &Arc<wgpu::TextureView>,
        sampler: &wgpu::Sampler,
    ) -> Result<Arc<wgpu::BindGroup>> {
        let sampler_id = sampler as *const wgpu::Sampler as usize as u64;
        if let Some(entry) = self.sprite_bind_cache.get(atlas) {
            if Arc::ptr_eq(&entry.view, view) && entry.sampler_id == sampler_id {
                return Ok(entry.bind_group.clone());
            }
        }

        let device = self.device.as_ref().context("GPU device not initialized")?;
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

        self.sprite_bind_cache.insert(
            atlas.to_string(),
            SpriteBindCacheEntry { view: view.clone(), sampler_id, bind_group: bind_group.clone() },
        );

        Ok(bind_group)
    }

    pub fn clear_sprite_bind_cache(&mut self) {
        self.sprite_bind_cache.clear();
    }

    pub fn invalidate_sprite_bind_group(&mut self, atlas: &str) {
        self.sprite_bind_cache.remove(atlas);
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
        sprite_batches: &[SpriteBatch],
        sampler: &wgpu::Sampler,
        sprite_view_proj: Mat4,
        viewport: RenderViewport,
        mesh_draws: &[MeshDraw],
        mesh_camera: Option<&Camera3D>,
    ) -> Result<SurfaceFrame> {
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

        let frame = self.acquire_surface_frame()?;
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let view = frame.view();
        let encoder_label =
            format!("Frame Encoder (sprites={}, meshes={})", instances.len(), mesh_draws.len());
        let mut encoder = device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(encoder_label.as_str()) });
        self.gpu_timer.begin_frame();
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::FrameStart);

        let mut sprite_bind_groups: Vec<(Range<u32>, Arc<wgpu::BindGroup>)> = Vec::new();
        for batch in sprite_batches {
            match self.sprite_bind_group(&batch.atlas, &batch.view, sampler) {
                Ok(bind_group) => sprite_bind_groups.push((batch.range.clone(), bind_group)),
                Err(err) => {
                    eprintln!("Failed to prepare sprite bind group for atlas '{}': {err:?}", batch.atlas);
                }
            }
        }

        let clear_color = wgpu::Color { r: 0.05, g: 0.06, b: 0.1, a: 1.0 };
        let mut sprite_load_op = wgpu::LoadOp::Clear(clear_color);
        if let Some(camera) = mesh_camera {
            if !mesh_draws.is_empty() {
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::ShadowStart);
                self.prepare_shadow_map(&mut encoder, mesh_draws, camera)?;
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::ShadowEnd);
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::MeshStart);
                self.encode_mesh_pass(&mut encoder, view, viewport, mesh_draws, camera, clear_color)?;
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::MeshEnd);
                sprite_load_op = wgpu::LoadOp::Load;
            }
        }

        {
            self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::SpriteStart);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Sprite Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
            let vertex_buffer = self.vertex_buffer.as_ref().context("Vertex buffer missing")?;
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
        }
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::SpriteEnd);
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::FrameEnd);

        if let Some(queue) = self.queue.as_ref() {
            queue.submit(std::iter::once(encoder.finish()));
        } else {
            return Err(anyhow!("GPU queue not initialized"));
        }
        Ok(frame)
    }

    fn handle_surface_error(&mut self, error: &wgpu::SurfaceError) -> anyhow::Error {
        match Self::surface_error_action(error) {
            SurfaceErrorAction::Reconfigure => {
                self.resize(self.size);
                anyhow!("Surface lost or outdated; reconfigured surface")
            }
            SurfaceErrorAction::Retry => anyhow!("Surface acquisition timed out"),
            SurfaceErrorAction::OutOfMemory => anyhow!("Surface out of memory"),
            SurfaceErrorAction::Unknown => anyhow!("Surface reported an unknown error"),
        }
    }

    fn surface_error_action(error: &wgpu::SurfaceError) -> SurfaceErrorAction {
        match error {
            wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated => SurfaceErrorAction::Reconfigure,
            wgpu::SurfaceError::Timeout => SurfaceErrorAction::Retry,
            wgpu::SurfaceError::OutOfMemory => SurfaceErrorAction::OutOfMemory,
            wgpu::SurfaceError::Other => SurfaceErrorAction::Unknown,
        }
    }

    fn configure_surface(&mut self) -> Result<()> {
        let surface = self.surface.as_ref().context("Surface not initialized")?;
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let config = self.config.as_mut().context("Surface configuration missing")?;
        surface.configure(device, config);
        Ok(())
    }

    fn reconfigure_present_mode(&mut self) -> Result<()> {
        if self.surface.is_none() {
            // Nothing to reconfigure yet; init_wgpu will respect the new flag.
            return Ok(());
        }
        let modes: &[wgpu::PresentMode] = if self.present_modes.is_empty() {
            &DEFAULT_PRESENT_MODES
        } else {
            self.present_modes.as_slice()
        };
        let present_mode = self.select_present_mode(modes);
        {
            let config = self.config.as_mut().context("Surface configuration missing")?;
            config.present_mode = present_mode;
        }
        self.configure_surface()
    }

    pub fn render_egui(
        &mut self,
        painter: &mut EguiRenderer,
        paint_jobs: &[egui::ClippedPrimitive],
        screen: &ScreenDescriptor,
        frame: SurfaceFrame,
    ) -> Result<()> {
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let queue = self.queue.as_ref().context("GPU queue not initialized")?;
        let view = frame.view();

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Egui Encoder") });
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::EguiStart);
        let mut extra_cmd = painter.update_buffers(device, queue, &mut encoder, paint_jobs, screen);

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Egui Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
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
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::EguiEnd);
        self.gpu_timer.finish_frame(&mut encoder);
        extra_cmd.push(encoder.finish());
        queue.submit(extra_cmd.into_iter());
        self.gpu_timer.collect_results(device);
        frame.present();
        Ok(())
    }

    pub fn gpu_timing_supported(&self) -> bool {
        self.gpu_timer.supported
    }

    pub fn take_gpu_timings(&mut self) -> Vec<GpuPassTiming> {
        self.gpu_timer.take_latest()
    }
}

#[cfg(test)]
mod surface_tests {
    use super::*;
    use crate::config::WindowConfig;
    use pollster::block_on;

    #[test]
    fn present_mode_respects_vsync_flag() {
        let cfg = WindowConfig::default();
        let mut renderer = block_on(Renderer::new(&cfg));
        renderer.vsync = false;
        let modes = vec![wgpu::PresentMode::Immediate, wgpu::PresentMode::Fifo];
        assert_eq!(renderer.select_present_mode(&modes), wgpu::PresentMode::Immediate);

        let mut vsync_renderer = block_on(Renderer::new(&cfg));
        vsync_renderer.vsync = true;
        assert_eq!(vsync_renderer.select_present_mode(&modes), wgpu::PresentMode::Fifo);
    }

    #[test]
    fn surface_error_action_matches_variants() {
        assert_eq!(
            Renderer::surface_error_action(&wgpu::SurfaceError::Lost),
            SurfaceErrorAction::Reconfigure
        );
        assert_eq!(
            Renderer::surface_error_action(&wgpu::SurfaceError::Outdated),
            SurfaceErrorAction::Reconfigure
        );
        assert_eq!(Renderer::surface_error_action(&wgpu::SurfaceError::Timeout), SurfaceErrorAction::Retry);
        assert_eq!(
            Renderer::surface_error_action(&wgpu::SurfaceError::OutOfMemory),
            SurfaceErrorAction::OutOfMemory
        );
        assert_eq!(Renderer::surface_error_action(&wgpu::SurfaceError::Other), SurfaceErrorAction::Unknown);
    }

    #[test]
    fn surface_loss_triggers_resize_attempt_even_without_surface() {
        let mut renderer = block_on(Renderer::new(&WindowConfig::default()));
        assert_eq!(renderer.resize_invocations_for_test(), 0);
        let _ = renderer.handle_surface_error(&wgpu::SurfaceError::Lost);
        assert_eq!(renderer.resize_invocations_for_test(), 1);
    }

    #[test]
    fn headless_render_recovers_from_surface_loss() {
        let window_config =
            WindowConfig { title: "Headless".into(), width: 64, height: 64, vsync: false, fullscreen: false };
        let mut renderer = block_on(Renderer::new(&window_config));
        block_on(renderer.init_headless_for_test()).expect("init headless");
        let (pipeline_sampler, draw_sampler, atlas_view) = {
            let device = renderer.device().expect("device");
            let queue = renderer.queue().expect("queue");
            let atlas_texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Test Atlas"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &atlas_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &[255, 255, 255, 255],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4), rows_per_image: Some(1) },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
            let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
            let pipeline_sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
            let draw_sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
            (pipeline_sampler, draw_sampler, atlas_view)
        };
        renderer.init_sprite_pipeline_with_atlas(atlas_view, pipeline_sampler).expect("init sprite pipeline");
        renderer.prepare_headless_render_target_for_test().expect("headless target");
        let viewport = RenderViewport {
            origin: (0.0, 0.0),
            size: (window_config.width as f32, window_config.height as f32),
        };

        let render_once = |renderer: &mut Renderer| -> anyhow::Result<()> {
            let frame =
                renderer.render_frame(&[], &[], &draw_sampler, Mat4::IDENTITY, viewport, &[], None)?;
            frame.present();
            Ok(())
        };

        render_once(&mut renderer).expect("initial render");
        renderer.inject_surface_error_for_test(wgpu::SurfaceError::Lost);
        let err = render_once(&mut renderer).expect_err("surface loss should bubble");
        assert!(err.to_string().contains("Surface lost"));
        assert!(renderer.resize_invocations_for_test() >= 1);
        renderer.prepare_headless_render_target_for_test().expect("headless target reinit");
        render_once(&mut renderer).expect("render after recovery");
    }
}

impl Renderer {
    pub async fn init_headless_for_test(&mut self) -> Result<()> {
        if self.device.is_some() {
            return Ok(());
        }
        let instance = wgpu::Instance::default();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .context("Failed to request headless adapter")?;
        let adapter_features = adapter.features();
        let supports_timestamp = adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY);
        let supports_encoder_queries =
            adapter_features.contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);
        let gpu_timing_supported = supports_timestamp && supports_encoder_queries;
        let mut required_features = wgpu::Features::empty();
        if supports_timestamp {
            required_features |= wgpu::Features::TIMESTAMP_QUERY;
        }
        if supports_encoder_queries {
            required_features |= wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        }
        let mut required_limits =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        required_limits.max_bind_groups = required_limits.max_bind_groups.max(6);
        required_limits.max_storage_buffers_per_shader_stage =
            required_limits.max_storage_buffers_per_shader_stage.max(1);
        let device_desc = wgpu::DeviceDescriptor {
            label: Some("Headless Device"),
            required_features,
            required_limits,
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::default(),
        };
        let (device, queue) =
            adapter.request_device(&device_desc).await.context("Failed to request headless device")?;
        self.device = Some(device);
        self.queue = Some(queue);
        if let (Some(device), Some(queue)) = (self.device.as_ref(), self.queue.as_ref()) {
            self.gpu_timer.configure(device, queue, gpu_timing_supported);
        }
        if self.config.is_none() {
            self.config = Some(wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format: wgpu::TextureFormat::Bgra8UnormSrgb,
                width: self.size.width.max(1),
                height: self.size.height.max(1),
                present_mode: wgpu::PresentMode::Fifo,
                alpha_mode: wgpu::CompositeAlphaMode::Opaque,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            });
        }
        if self.depth_texture.is_none() {
            self.recreate_depth_texture()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod depth_texture_tests {
    use super::*;
    use pollster::block_on;
    use winit::dpi::PhysicalSize;

    #[test]
    fn depth_texture_respects_size() {
        block_on(async {
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
