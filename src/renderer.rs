mod egui_pass;
mod light_clusters;
mod mesh_pass;
mod shadow_pass;
mod sprite_pass;

use crate::camera3d::Camera3D;
use crate::config::WindowConfig;
use crate::ecs::{InstanceData, MeshLightingInfo};
use crate::environment::EnvironmentGpu;
use crate::material_registry::MaterialGpu;
use crate::mesh::{Mesh, MeshBounds, MeshVertex};
use anyhow::{anyhow, Context, Result};
use glam::{Mat4, Vec3, Vec4};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window};

// egui
use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};
pub use self::light_clusters::LightClusterMetrics;
use self::light_clusters::{LightClusterParams, LightClusterPass, LightClusterScratch};
use self::mesh_pass::{MeshDrawData, MeshFrameData, MeshPass, MeshPipelineResources, PaletteUploadStats};
use self::shadow_pass::{ShadowPass, ShadowPassParams};
use self::sprite_pass::SpritePass;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const MAX_SKIN_JOINTS: usize = 256;
const SKINNING_CACHE_HEADROOM: usize = 4;
pub const MAX_SHADOW_CASCADES: usize = 4;
const LIGHT_CLUSTER_TILE_SIZE: u32 = 192;
const LIGHT_CLUSTER_Z_SLICES: u32 = 8;
pub const LIGHT_CLUSTER_MAX_LIGHTS: usize = 256;
const LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER: usize = 64;
const LIGHT_CLUSTER_RECORD_STRIDE_WORDS: u32 = 2;
const LIGHT_CLUSTER_CACHE_QUANTIZE: f32 = 1e-3;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Default)]
struct PointLightGpu {
    position_radius: [f32; 4],
    color_intensity: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Default)]
struct ClusterRecordGpu {
    offset: u32,
    count: u32,
    _pad0: u32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable, Default)]
struct ClusterConfigUniform {
    viewport: [f32; 4],
    depth_params: [f32; 4],
    grid_dims: [u32; 4],
    stats: [u32; 4],
    data_meta: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ClusterLightUniform {
    config: ClusterConfigUniform,
    lights: [PointLightGpu; LIGHT_CLUSTER_MAX_LIGHTS],
}

#[derive(Clone, Copy, Debug)]
pub struct RenderViewport {
    pub origin: (f32, f32),
    pub size: (f32, f32),
}

#[derive(Clone, Debug)]
pub struct SpriteBatch {
    pub atlas: Arc<str>,
    pub range: Range<u32>,
    pub view: Arc<wgpu::TextureView>,
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
    pub bounds: MeshBounds,
}

#[derive(Clone)]
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

struct HeadlessTarget {
    texture: wgpu::Texture,
}

struct RendererEnvironmentState {
    bind_group: Arc<wgpu::BindGroup>,
    mip_count: u32,
    intensity: f32,
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
    pub shadow_cascade_count: u32,
    pub shadow_resolution: u32,
    pub shadow_split_lambda: f32,
    pub shadow_pcf_radius: f32,
    pub point_lights: Vec<ScenePointLight>,
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
            shadow_cascade_count: MAX_SHADOW_CASCADES as u32,
            shadow_resolution: 2048,
            shadow_split_lambda: 0.6,
            shadow_pcf_radius: 1.25,
            point_lights: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ScenePointLight {
    pub position: Vec3,
    pub color: Vec3,
    pub radius: f32,
    pub intensity: f32,
}

impl ScenePointLight {
    pub fn new(position: Vec3, color: Vec3, radius: f32, intensity: f32) -> Self {
        Self { position, color, radius: radius.max(0.0), intensity: intensity.max(0.0) }
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

    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    mesh_pass: MeshPass,
    shadow_pass: ShadowPass,
    light_clusters: LightClusterPass,
    light_cluster_scratch: LightClusterScratch,
    lighting: SceneLightingState,
    environment_state: Option<RendererEnvironmentState>,
    sprite_pass: SpritePass,
    present_modes: Vec<wgpu::PresentMode>,
    gpu_timer: GpuTimer,
    skinning_limit_warnings: HashSet<usize>,
    sprite_bind_groups: Vec<(Range<u32>, Arc<wgpu::BindGroup>)>,
    #[cfg(test)]
    resize_invocations: usize,
    headless_target: Option<HeadlessTarget>,
    #[cfg(test)]
    surface_error_injector: Option<wgpu::SurfaceError>,
    palette_stats_frame: PaletteUploadStats,
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
            depth_texture: None,
            depth_view: None,
            mesh_pass: MeshPass::new(),
            shadow_pass: ShadowPass::default(),
            light_clusters: LightClusterPass::default(),
            light_cluster_scratch: LightClusterScratch::default(),
            lighting: SceneLightingState::default(),
            environment_state: None,
            sprite_pass: SpritePass::new(),
            present_modes: Vec::new(),
            gpu_timer: GpuTimer::default(),
            skinning_limit_warnings: HashSet::new(),
            sprite_bind_groups: Vec::new(),
            #[cfg(test)]
            resize_invocations: 0,
            headless_target: None,
            #[cfg(test)]
            surface_error_injector: None,
            palette_stats_frame: PaletteUploadStats::default(),
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
        self.shadow_pass.mark_dirty();
    }

    pub fn mark_shadow_settings_dirty(&mut self) {
        self.shadow_pass.mark_dirty();
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
        let mut required_limits = adapter.limits();
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
        self.sprite_pass.clear_bind_cache();
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let config = self.config.as_ref().context("Surface configuration missing")?;
        self.sprite_pass
            .init_pipeline_with_atlas(device, config.format, atlas_view, sampler)
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

        let frame_draw_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mesh Frame+Draw BGL"),
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
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
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

        let light_cluster_bgl = Arc::new(device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Light Cluster BGL"),
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
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
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
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                frame_draw_bgl.as_ref(),
                skinning_bgl.as_ref(),
                material_bgl.as_ref(),
                shadow_bgl.as_ref(),
                environment_bgl.as_ref(),
                light_cluster_bgl.as_ref(),
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
            frame_draw_bgl: frame_draw_bgl.clone(),
            skinning_bgl: skinning_bgl.clone(),
            material_bgl: material_bgl.clone(),
            environment_bgl: environment_bgl.clone(),
            light_cluster_bgl: light_cluster_bgl.clone(),
        });
        self.mesh_pass.frame_buffer = Some(frame_buf);
        self.mesh_pass.draw_buffer = Some(draw_buf);
        self.mesh_pass.frame_draw_bind_group = None;
        self.mesh_pass.skinning_identity_buffer = None;
        self.mesh_pass.skinning_identity_bind_group = None;
        self.shadow_pass.set_sample_layout(shadow_bgl);
        self.light_clusters.set_layout(light_cluster_bgl);
        self.light_clusters.invalidate_cache();
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
        Ok(GpuMesh {
            vertex_buffer,
            index_buffer,
            index_count: mesh.indices.len() as u32,
            bounds: mesh.bounds.clone(),
        })
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
        let (environment_mip_count, environment_intensity) = {
            let env = self.environment_state.as_ref().context("Environment state not configured")?;
            (env.mip_count, env.intensity)
        };
        let device = self.device()?.clone();
        let vp_size = PhysicalSize::new(
            viewport.size.0.max(1.0).round() as u32,
            viewport.size.1.max(1.0).round() as u32,
        );
        let view_proj = camera.view_projection(vp_size);
        let view_matrix = camera.view_matrix();
        let lighting_dir = self.lighting.direction.normalize_or_zero();
        let mesh_resources = self.mesh_pass.resources.as_ref().context("Mesh pipeline not initialized")?;
        let depth_view = self.depth_view.as_ref().context("Depth texture missing")?;
        let queue = self.queue()?.clone();
        self.light_clusters.prepare(LightClusterParams {
            device: &device,
            queue: &queue,
            camera,
            viewport: vp_size,
            lighting: &self.lighting,
            scratch: &mut self.light_cluster_scratch,
        })?;
        let frame_data = MeshFrameData {
            view_proj: view_proj.to_cols_array_2d(),
            view: view_matrix.to_cols_array_2d(),
            camera_pos: [camera.position.x, camera.position.y, camera.position.z, 1.0],
            light_dir: [lighting_dir.x, lighting_dir.y, lighting_dir.z, 0.0],
            light_color: [self.lighting.color.x, self.lighting.color.y, self.lighting.color.z, 1.0],
            ambient_color: [self.lighting.ambient.x, self.lighting.ambient.y, self.lighting.ambient.z, 1.0],
            exposure_params: [
                self.lighting.exposure,
                environment_mip_count.max(1) as f32,
                environment_intensity,
                0.0,
            ],
            cascade_splits: self.shadow_pass.cascade_splits(),
        };

        if self.mesh_pass.frame_buffer.is_none() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mesh Frame Buffer"),
                size: std::mem::size_of::<MeshFrameData>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.mesh_pass.frame_buffer = Some(buffer);
            self.mesh_pass.frame_draw_bind_group = None;
        }
        let frame_buffer = self.mesh_pass.frame_buffer.as_ref().context("Mesh frame buffer missing")?.clone();

        if self.mesh_pass.draw_buffer.is_none() {
            debug_assert_eq!(std::mem::size_of::<MeshDrawData>(), 112);
            let draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mesh Draw Buffer"),
                size: std::mem::size_of::<MeshDrawData>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.mesh_pass.draw_buffer = Some(draw_buf);
            self.mesh_pass.frame_draw_bind_group = None;
        }
        let draw_buffer = self.mesh_pass.draw_buffer.as_ref().context("Mesh draw buffer missing")?.clone();

        if self.mesh_pass.frame_draw_bind_group.is_none() {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mesh Frame+Draw BG"),
                layout: mesh_resources.frame_draw_bgl.as_ref(),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: frame_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: draw_buffer.as_entire_binding() },
                ],
            });
            self.mesh_pass.frame_draw_bind_group = Some(bind_group);
        }

        queue.write_buffer(&frame_buffer, 0, bytemuck::bytes_of(&frame_data));

        let frame_draw_bind_group =
            self.mesh_pass.frame_draw_bind_group.as_ref().context("Mesh frame/draw bind group missing")?;
        if self.shadow_pass.sample_bind_group().is_none() {
            let lighting_clone = self.lighting.clone();
            self.shadow_pass.ensure_sample_bind_group(&lighting_clone, &device, &queue)?;
        }
        let shadow_bind_group =
            self.shadow_pass.sample_bind_group().context("Shadow sample bind group missing")?;
        let environment_bind_group =
            self.environment_state.as_ref().context("Environment state not configured")?.bind_group.as_ref();
        let light_cluster_bind_group =
            self.light_clusters.bind_group().context("Light cluster bind group missing")?;

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

        pass.set_bind_group(0, frame_draw_bind_group, &[]);
        pass.set_bind_group(3, shadow_bind_group, &[]);
        pass.set_bind_group(4, environment_bind_group, &[]);
        pass.set_bind_group(5, light_cluster_bind_group, &[]);

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
                let upload_start = Instant::now();
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.mesh_pass.palette_staging));
                let elapsed_ms = upload_start.elapsed().as_secs_f32() * 1000.0;
                self.palette_stats_frame.record(joint_count, elapsed_ms);
                let bind_group = &self.mesh_pass.skinning_palette_bind_groups[slot];
                pass.set_bind_group(1, bind_group, &[]);
            } else {
                pass.set_bind_group(1, skinning_identity_bind_group, &[]);
            }
            pass.set_bind_group(2, draw.material.bind_group(), &[]);
            pass.set_vertex_buffer(0, draw.mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(draw.mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..draw.mesh.index_count, 0, 0..1);
        }

        Self::trim_skinning_cache(
            &mut self.mesh_pass.skinning_palette_buffers,
            &mut self.mesh_pass.skinning_palette_bind_groups,
            self.mesh_pass.skinning_cursor,
        );

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

    fn prepare_shadow_map(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        draws: &[MeshDraw],
        camera: &Camera3D,
        viewport: RenderViewport,
    ) -> Result<()> {
        self.init_mesh_pipeline()?;
        let device = self.device()?.clone();
        let queue = self.queue()?.clone();
        self.shadow_pass.prepare(ShadowPassParams {
            encoder,
            draws,
            camera,
            viewport,
            lighting: &self.lighting,
            device: &device,
            queue: &queue,
            skinning_limit_warnings: &mut self.skinning_limit_warnings,
            palette_stats: &mut self.palette_stats_frame,
        })
    }

    pub fn take_palette_upload_metrics(&mut self) -> PaletteUploadStats {
        let stats = self.palette_stats_frame;
        self.palette_stats_frame = PaletteUploadStats::default();
        stats
    }

    pub fn light_cluster_metrics(&self) -> &LightClusterMetrics {
        self.light_clusters.metrics()
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
        } else if let Some(target) = self.headless_target.as_ref() {
            let view = target.texture.create_view(&wgpu::TextureViewDescriptor::default());
            Ok(SurfaceFrame::headless(view))
        } else {
            Err(anyhow!("Surface not initialized"))
        }
    }

    #[cfg(test)]
    pub fn resize_invocations_for_test(&self) -> usize {
        self.resize_invocations
    }

    pub fn prepare_headless_render_target(&mut self) -> Result<()> {
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
        }
        self.headless_target = None;
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

    pub fn clear_sprite_bind_cache(&mut self) {
        self.sprite_pass.clear_bind_cache();
    }

    pub fn invalidate_sprite_bind_group(&mut self, atlas: &str) {
        self.sprite_pass.invalidate_bind_group(atlas);
    }

    fn trim_skinning_cache(
        buffers: &mut Vec<wgpu::Buffer>,
        bind_groups: &mut Vec<wgpu::BindGroup>,
        active_slots: usize,
    ) {
        let desired = active_slots.saturating_add(SKINNING_CACHE_HEADROOM);
        if buffers.len() > desired {
            buffers.truncate(desired);
        }
        if bind_groups.len() > desired {
            bind_groups.truncate(desired);
        }
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

    fn cull_mesh_draws<'a>(
        &self,
        draws: &[MeshDraw<'a>],
        camera: &Camera3D,
        viewport: RenderViewport,
    ) -> Vec<MeshDraw<'a>> {
        if draws.is_empty() {
            return Vec::new();
        }
        let vp_size = PhysicalSize::new(
            viewport.size.0.max(1.0).round() as u32,
            viewport.size.1.max(1.0).round() as u32,
        );
        let view_proj = camera.view_projection(vp_size);
        let planes = Self::extract_frustum_planes(view_proj);
        let mut visible = Vec::with_capacity(draws.len());
        for draw in draws {
            let (center, radius) = Self::transform_bounds(draw.model, &draw.mesh.bounds);
            if radius <= 0.0 || Self::sphere_in_frustum(center, radius, &planes) {
                visible.push(draw.clone());
            }
        }
        visible
    }

    fn transform_bounds(model: Mat4, bounds: &MeshBounds) -> (Vec3, f32) {
        let center = model.transform_point3(bounds.center);
        let scale_x = model.x_axis.truncate().length();
        let scale_y = model.y_axis.truncate().length();
        let scale_z = model.z_axis.truncate().length();
        let max_scale = scale_x.max(scale_y).max(scale_z).max(0.0001);
        let radius = bounds.radius * max_scale;
        (center, radius)
    }

    fn sphere_in_frustum(center: Vec3, radius: f32, planes: &[Vec4; 6]) -> bool {
        for plane in planes {
            let normal = plane.truncate();
            let distance = normal.dot(center) + plane.w;
            if distance < -radius {
                return false;
            }
        }
        true
    }

    fn extract_frustum_planes(matrix: Mat4) -> [Vec4; 6] {
        let m = matrix.to_cols_array();
        let row = |i: usize| Vec4::new(m[i], m[i + 4], m[i + 8], m[i + 12]);
        let row0 = row(0);
        let row1 = row(1);
        let row2 = row(2);
        let row3 = row(3);
        let mut planes = [row3 + row0, row3 - row0, row3 + row1, row3 - row1, row3 + row2, row3 - row2];
        for plane in &mut planes {
            let normal = plane.truncate();
            let length = normal.length();
            if length > 0.0 {
                *plane /= length;
            }
        }
        planes
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
        self.palette_stats_frame = PaletteUploadStats::default();
        self.light_clusters.reset_metrics();
        let frame = self.acquire_surface_frame()?;
        let device = self.device.as_ref().context("GPU device not initialized")?.clone();
        let queue = self.queue.as_ref().context("GPU queue not initialized")?.clone();
        self.sprite_pass.write_globals(&queue, sprite_view_proj)?;
        let view = frame.view();
        let encoder_label =
            format!("Frame Encoder (sprites={}, meshes={})", instances.len(), mesh_draws.len());
        let mut encoder = device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(encoder_label.as_str()) });
        self.sprite_pass.upload_instances(&device, &mut encoder, instances)?;
        self.gpu_timer.begin_frame();
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::FrameStart);

        self.sprite_bind_groups.clear();
        for batch in sprite_batches {
            match self
                .sprite_pass
                .sprite_bind_group(&device, batch.atlas.as_ref(), &batch.view, sampler)
            {
                Ok(bind_group) => self.sprite_bind_groups.push((batch.range.clone(), bind_group)),
                Err(err) => {
                    eprintln!(
                        "Failed to prepare sprite bind group for atlas '{}': {err:?}",
                        batch.atlas.as_ref()
                    );
                }
            }
        }

        let clear_color = wgpu::Color { r: 0.05, g: 0.06, b: 0.1, a: 1.0 };
        let mut culled_mesh_draws: Vec<MeshDraw> = Vec::new();
        let mut mesh_draw_slice: &[MeshDraw] = culled_mesh_draws.as_slice();
        if let Some(camera) = mesh_camera {
            culled_mesh_draws = self.cull_mesh_draws(mesh_draws, camera, viewport);
            mesh_draw_slice = culled_mesh_draws.as_slice();
        }
        let mut sprite_load_op = wgpu::LoadOp::Clear(clear_color);
        if let Some(camera) = mesh_camera {
            if !mesh_draw_slice.is_empty() {
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::ShadowStart);
                self.prepare_shadow_map(&mut encoder, mesh_draw_slice, camera, viewport)?;
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::ShadowEnd);
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::MeshStart);
                self.encode_mesh_pass(&mut encoder, view, viewport, mesh_draw_slice, camera, clear_color)?;
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
            self.sprite_pass
                .encode_pass(&mut pass, viewport, self.size, instances, &self.sprite_bind_groups)?;
        }
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::SpriteEnd);
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::FrameEnd);

        self.sprite_pass.finish_uploads();
        queue.submit(std::iter::once(encoder.finish()));
        let _ = device.poll(wgpu::PollType::Poll);
        self.sprite_pass.recall_uploads();
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
        egui_pass::render(&mut self.gpu_timer, device, queue, painter, paint_jobs, screen, frame)
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
    fn mesh_draw_data_layout() {
        assert_eq!(std::mem::size_of::<MeshDrawData>(), 112);
    }
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
        renderer.prepare_headless_render_target().expect("headless target");
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
        renderer.prepare_headless_render_target().expect("headless target reinit");
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
        let mut required_limits = adapter.limits();
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
                required_limits: adapter.limits(),
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
