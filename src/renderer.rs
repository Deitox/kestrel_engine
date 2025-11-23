#[cfg(feature = "editor")]
mod egui_pass;
mod light_clusters;
mod mesh_pass;
mod shadow_pass;
mod sprite_pass;
mod window_surface;

use crate::camera3d::Camera3D;
use crate::config::WindowConfig;
use crate::ecs::{InstanceData, MeshLightingInfo};
use crate::environment::EnvironmentGpu;
use crate::material_registry::MaterialGpu;
use crate::mesh::{Mesh, MeshBounds, MeshVertex};
use anyhow::{Context, Result};
use glam::{Mat4, Vec3, Vec4};
#[cfg(feature = "editor")]
use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

// egui
pub use self::light_clusters::LightClusterMetrics;
use self::light_clusters::{LightClusterParams, LightClusterPass, LightClusterScratch};
use self::mesh_pass::{MeshDrawData, MeshFrameData, MeshPass, MeshPipelineResources, PaletteUploadStats};
use self::shadow_pass::{ShadowPass, ShadowPassParams};
use self::sprite_pass::SpritePass;
pub use self::window_surface::SurfaceFrame;
use self::window_surface::WindowSurface;
#[cfg(feature = "editor")]
use egui_wgpu::{Renderer as EguiRenderer, ScreenDescriptor};

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
const GPU_TIMER_MAX_QUERIES: u32 = 128;
const GPU_TIMER_READBACK_RING: usize = 3;

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
    #[cfg(feature = "editor")]
    EguiStart,
    #[cfg(feature = "editor")]
    EguiEnd,
}

#[derive(Copy, Clone, Debug)]
#[cfg_attr(not(feature = "editor"), allow(dead_code))]
struct GpuTimestampMark {
    label: GpuTimestampLabel,
    index: u32,
}

#[derive(Default)]
struct GpuTimerReadback {
    pending_query_count: u32,
    marks: Vec<GpuTimestampMark>,
    byte_len: u64,
    receiver: Option<std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

struct GpuTimer {
    supported: bool,
    requested_enabled: bool,
    enabled: bool,
    timestamp_period: f32,
    max_queries: u32,
    query_set: Option<wgpu::QuerySet>,
    query_buffer: Option<wgpu::Buffer>,
    readback_buffers: Vec<wgpu::Buffer>,
    readback_states: Vec<GpuTimerReadback>,
    readback_cursor: usize,
    readback_buffer_size: u64,
    marks: Vec<GpuTimestampMark>,
    query_overflowed: bool,
    latest: Vec<GpuPassTiming>,
    frame_active: bool,
    next_query: u32,
}

impl Default for GpuTimer {
    fn default() -> Self {
        Self {
            supported: false,
            requested_enabled: false,
            enabled: false,
            timestamp_period: 0.0,
            max_queries: GPU_TIMER_MAX_QUERIES,
            query_set: None,
            query_buffer: None,
            readback_buffers: Vec::new(),
            readback_states: Vec::new(),
            readback_cursor: 0,
            readback_buffer_size: 0,
            marks: Vec::new(),
            query_overflowed: false,
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
            self.enabled = false;
            self.timestamp_period = 0.0;
            self.query_set = None;
            self.query_buffer = None;
            self.readback_buffers.clear();
            self.readback_states.clear();
            self.readback_cursor = 0;
            self.readback_buffer_size = 0;
            self.marks.clear();
            self.query_overflowed = false;
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
        let required_size = self.max_queries as u64 * std::mem::size_of::<u64>() as u64;
        let needs_rebuild = self.query_buffer.is_none() || self.readback_buffer_size != required_size;
        if needs_rebuild {
            self.query_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gpu-timer-buffer"),
                size: required_size,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }));
            self.readback_buffers.clear();
            self.readback_states.clear();
            for _ in 0..GPU_TIMER_READBACK_RING {
                self.readback_buffers.push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gpu-timer-readback"),
                    size: required_size,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
                self.readback_states.push(GpuTimerReadback::default());
            }
            self.readback_buffer_size = required_size;
            self.readback_cursor = 0;
        }
        self.enabled = self.requested_enabled && self.supported;
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.requested_enabled = enabled;
        self.enabled = enabled && self.supported;
        if !self.enabled {
            self.marks.clear();
            self.readback_states.iter_mut().for_each(|state| *state = GpuTimerReadback::default());
            self.query_overflowed = false;
            self.latest.clear();
            self.frame_active = false;
            self.next_query = 0;
        }
    }

    fn begin_frame(&mut self) {
        if !self.supported || !self.enabled {
            return;
        }
        self.next_query = 0;
        self.marks.clear();
        self.query_overflowed = false;
        self.frame_active = true;
    }

    fn write_timestamp(&mut self, encoder: &mut wgpu::CommandEncoder, label: GpuTimestampLabel) {
        if !self.supported || !self.enabled || !self.frame_active {
            return;
        }
        if self.next_query >= self.max_queries {
            self.query_overflowed = true;
            return;
        }
        if let Some(query_set) = self.query_set.as_ref() {
            encoder.write_timestamp(query_set, self.next_query);
            self.marks.push(GpuTimestampMark { label, index: self.next_query });
            self.next_query += 1;
        }
    }

    #[cfg(feature = "editor")]
    fn finish_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.supported || !self.enabled || !self.frame_active {
            return;
        }
        if self.next_query == 0 {
            self.frame_active = false;
            return;
        }
        if let (Some(query_set), Some(buffer)) = (self.query_set.as_ref(), self.query_buffer.as_ref()) {
            encoder.resolve_query_set(query_set, 0..self.next_query, buffer, 0);
            let byte_len = self.next_query as u64 * std::mem::size_of::<u64>() as u64;
            if let (Some(readback), Some(state)) = (
                self.readback_buffers.get(self.readback_cursor),
                self.readback_states.get_mut(self.readback_cursor),
            ) {
                if state.pending_query_count > 0 || state.receiver.is_some() {
                    state.receiver = None;
                    state.pending_query_count = 0;
                    state.byte_len = 0;
                    state.marks.clear();
                }
                encoder.copy_buffer_to_buffer(buffer, 0, readback, 0, byte_len);
                state.pending_query_count = self.next_query;
                state.byte_len = byte_len;
                state.marks = std::mem::take(&mut self.marks);
                self.readback_cursor = (self.readback_cursor + 1) % self.readback_buffers.len().max(1);
            }
        }
        if self.query_overflowed {
            eprintln!(
                "[renderer] GPU timer exceeded max queries ({}); dropping extra timestamps for this frame.",
                self.max_queries
            );
        }
        self.frame_active = false;
    }

    #[cfg(feature = "editor")]
    fn collect_results(&mut self, device: &wgpu::Device) {
        if !self.supported || !self.enabled {
            return;
        }
        for idx in 0..self.readback_states.len() {
            let state = &mut self.readback_states[idx];
            if state.pending_query_count == 0 && state.receiver.is_none() {
                continue;
            }
            if state.pending_query_count == 0 || state.byte_len == 0 {
                state.receiver = None;
                state.marks.clear();
                continue;
            }
            if state.receiver.is_none() {
                let Some(buffer) = self.readback_buffers.get(idx) else { continue };
                let slice = buffer.slice(0..state.byte_len);
                let (sender, receiver) = std::sync::mpsc::channel();
                slice.map_async(wgpu::MapMode::Read, move |result| {
                    let _ = sender.send(result);
                });
                state.receiver = Some(receiver);
            }
            let Some(receiver) = state.receiver.as_ref() else { continue };
            match receiver.try_recv() {
                Ok(Ok(())) => {
                    let Some(buffer) = self.readback_buffers.get(idx) else {
                        state.receiver = None;
                        state.pending_query_count = 0;
                        state.byte_len = 0;
                        state.marks.clear();
                        continue;
                    };
                    let data = buffer.slice(0..state.byte_len).get_mapped_range();
                    let mut timestamps: Vec<u64> = Vec::with_capacity(state.pending_query_count as usize);
                    for chunk in data.chunks_exact(std::mem::size_of::<u64>()) {
                        let mut bytes = [0u8; 8];
                        bytes.copy_from_slice(chunk);
                        timestamps.push(u64::from_le_bytes(bytes));
                    }
                    drop(data);
                    buffer.unmap();

                    let mut value_map: HashMap<GpuTimestampLabel, u64> = HashMap::new();
                    for mark in &state.marks {
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
                    #[cfg(feature = "editor")]
                    {
                        push_pass("Egui pass", GpuTimestampLabel::EguiStart, GpuTimestampLabel::EguiEnd);
                        if value_map.contains_key(&GpuTimestampLabel::EguiEnd) {
                            push_pass("Frame (with egui)", GpuTimestampLabel::FrameStart, GpuTimestampLabel::EguiEnd);
                        }
                    }

                    state.receiver = None;
                    state.pending_query_count = 0;
                    state.byte_len = 0;
                    state.marks.clear();
                }
                Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    if let Some(buffer) = self.readback_buffers.get(idx) {
                        buffer.unmap();
                    }
                    state.receiver = None;
                    state.pending_query_count = 0;
                    state.byte_len = 0;
                    state.marks.clear();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    let _ = device.poll(wgpu::PollType::Poll);
                }
            }
        }
    }

    fn take_latest(&mut self) -> Vec<GpuPassTiming> {
        if self.latest.is_empty() || !self.enabled {
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
    window_surface: WindowSurface,
    mesh_pass: MeshPass,
    shadow_pass: ShadowPass,
    light_clusters: LightClusterPass,
    light_cluster_scratch: LightClusterScratch,
    lighting: SceneLightingState,
    environment_state: Option<RendererEnvironmentState>,
    sprite_pass: SpritePass,
    gpu_timer: GpuTimer,
    skinning_limit_warnings: HashSet<usize>,
    sprite_bind_groups: Vec<(Range<u32>, Arc<wgpu::BindGroup>)>,
    palette_stats_frame: PaletteUploadStats,
    culled_mesh_indices: Vec<usize>,
}

impl Renderer {
    pub async fn new(window_cfg: &WindowConfig) -> Self {
        Self {
            window_surface: WindowSurface::new(window_cfg),
            mesh_pass: MeshPass::new(),
            shadow_pass: ShadowPass::new(),
            light_clusters: LightClusterPass::new(),
            light_cluster_scratch: LightClusterScratch::default(),
            lighting: SceneLightingState::default(),
            environment_state: None,
            sprite_pass: SpritePass::new(),
            gpu_timer: GpuTimer::default(),
            skinning_limit_warnings: HashSet::new(),
            sprite_bind_groups: Vec::new(),
            palette_stats_frame: PaletteUploadStats::default(),
            culled_mesh_indices: Vec::new(),
        }
    }

    pub fn ensure_window(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        self.window_surface.ensure_window(event_loop)?;
        if let Ok((device, queue)) = self.window_surface.device_and_queue() {
            let supported = self.window_surface.gpu_timing_supported();
            self.gpu_timer.configure(device, queue, supported);
        }
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

    pub fn init_sprite_pipeline_with_atlas(
        &mut self,
        atlas_view: wgpu::TextureView,
        sampler: wgpu::Sampler,
    ) -> Result<()> {
        self.sprite_pass.clear_bind_cache();
        let device = self.window_surface.device()?;
        let format = self.window_surface.surface_format()?;
        self.sprite_pass.init_pipeline_with_atlas(device, format, atlas_view, sampler)
    }

    pub fn init_mesh_pipeline(&mut self) -> Result<()> {
        self.window_surface.ensure_depth_texture()?;
        let device = self.window_surface.device()?.clone();

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
        visible_indices: Option<&[usize]>,
        camera: &Camera3D,
        clear_color: wgpu::Color,
    ) -> Result<()> {
        let visible_count = visible_indices.map(|idx| idx.len()).unwrap_or(draws.len());
        if visible_count == 0 {
            return Ok(());
        }
        if self.mesh_pass.resources.is_none() {
            self.init_mesh_pipeline()?;
        }
        self.window_surface.ensure_depth_texture()?;
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
        let frame_draw_layout = mesh_resources.frame_draw_bgl.clone();
        let skinning_layout = mesh_resources.skinning_bgl.clone();
        let pipeline = mesh_resources.pipeline.clone();
        let depth_view = self.window_surface.depth_view()?;
        let queue = self.queue()?.clone();
        let skinned_draws = if let Some(indices) = visible_indices {
            indices
                .iter()
                .filter(|&&idx| draws.get(idx).map_or(false, |d| d.skin_palette.is_some()))
                .count()
        } else {
            draws.iter().filter(|d| d.skin_palette.is_some()).count()
        };
        let palette_target = skinned_draws.saturating_add(SKINNING_CACHE_HEADROOM);
        Self::ensure_skinning_palette_capacity(
            &mut self.mesh_pass,
            &device,
            skinning_layout.as_ref(),
            palette_target,
        );
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
                layout: frame_draw_layout.as_ref(),
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
                layout: skinning_layout.as_ref(),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            self.mesh_pass.skinning_identity_bind_group = Some(bind_group);
        }
        let skinning_identity_bind_group = self
            .mesh_pass
            .skinning_identity_bind_group
            .as_ref()
            .context("Mesh skinning identity bind group missing")?
            .clone();

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
        pass.set_pipeline(&pipeline);
        let mut sc_x = viewport.origin.0.max(0.0).floor() as u32;
        let mut sc_y = viewport.origin.1.max(0.0).floor() as u32;
        let mut sc_w = viewport.size.0.max(1.0).floor() as u32;
        let mut sc_h = viewport.size.1.max(1.0).floor() as u32;
        let surface_size = self.window_surface.size();
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
        let draw_iter: Box<dyn Iterator<Item = &MeshDraw>> = if let Some(indices) = visible_indices {
            Box::new(indices.iter().filter_map(move |&idx| draws.get(idx)))
        } else {
            Box::new(draws.iter())
        };
        for draw in draw_iter {
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
                let upload_len = joint_count.max(1);
                {
                    let staging = &mut self.mesh_pass.palette_staging;
                    if staging.len() != upload_len {
                        staging.clear();
                        staging.resize(upload_len, identity_cols);
                    } else {
                        for slot in staging.iter_mut() {
                            *slot = identity_cols;
                        }
                    }
                    if let Some(palette) = draw.skin_palette.as_ref() {
                        for (dst, mat) in staging.iter_mut().zip(palette.iter()).take(joint_count) {
                            *dst = mat.to_cols_array();
                        }
                    }
                }
                let slot = self.mesh_pass.skinning_cursor;
                self.mesh_pass.skinning_cursor += 1;
                if self.mesh_pass.skinning_palette_buffers.len() <= slot {
                    Self::ensure_skinning_palette_capacity(
                        &mut self.mesh_pass,
                        &device,
                        skinning_layout.as_ref(),
                        slot + 1,
                    );
                }
                let buffer = &self.mesh_pass.skinning_palette_buffers[slot];
                let upload_start = Instant::now();
                let upload_slice = &self.mesh_pass.palette_staging[..upload_len];
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(upload_slice));
                let elapsed_ms = upload_start.elapsed().as_secs_f32() * 1000.0;
                self.palette_stats_frame.record(joint_count, elapsed_ms);
                let bind_group = &self.mesh_pass.skinning_palette_bind_groups[slot];
                pass.set_bind_group(1, bind_group, &[]);
            } else {
                pass.set_bind_group(1, &skinning_identity_bind_group, &[]);
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
        self.window_surface.device_and_queue()
    }
    pub fn device(&self) -> Result<&wgpu::Device> {
        self.window_surface.device()
    }
    pub fn queue(&self) -> Result<&wgpu::Queue> {
        self.window_surface.queue()
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
        self.window_surface.surface_format()
    }
    pub fn size(&self) -> PhysicalSize<u32> {
        self.window_surface.size()
    }
    pub fn pixels_per_point(&self) -> f32 {
        self.window_surface.pixels_per_point()
    }

    fn prepare_shadow_map(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        draws: &[MeshDraw],
        indices: Option<&[usize]>,
        camera: &Camera3D,
        viewport: RenderViewport,
    ) -> Result<()> {
        self.init_mesh_pipeline()?;
        let device = self.device()?.clone();
        let queue = self.queue()?.clone();
        self.shadow_pass.prepare(ShadowPassParams {
            encoder,
            draws,
            visible_indices: indices,
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
        self.window_surface.window()
    }

    #[cfg(test)]
    pub fn resize_invocations_for_test(&self) -> usize {
        self.window_surface.resize_invocations_for_test()
    }

    pub fn prepare_headless_render_target(&mut self) -> Result<()> {
        self.window_surface.prepare_headless_render_target()
    }

    #[cfg(test)]
    pub fn inject_surface_error_for_test(&mut self, error: wgpu::SurfaceError) {
        self.window_surface.inject_surface_error_for_test(error);
    }

    pub fn vsync_enabled(&self) -> bool {
        self.window_surface.vsync_enabled()
    }

    pub fn set_vsync(&mut self, enabled: bool) -> Result<()> {
        self.window_surface.set_vsync(enabled)
    }

    pub fn aspect_ratio(&self) -> f32 {
        self.window_surface.aspect_ratio()
    }

    pub fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.window_surface.resize(new_size);
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

    fn ensure_skinning_palette_capacity(
        mesh_pass: &mut MeshPass,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        required: usize,
    ) {
        if required == 0 || mesh_pass.skinning_palette_buffers.len() >= required {
            return;
        }
        let mut target = mesh_pass.skinning_palette_buffers.len().max(1);
        while target < required {
            target = target.saturating_mul(2);
        }
        while mesh_pass.skinning_palette_buffers.len() < target {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Mesh Skinning Palette Buffer"),
                size: (MAX_SKIN_JOINTS * std::mem::size_of::<[f32; 16]>()) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mesh Skinning Palette BG"),
                layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            mesh_pass.skinning_palette_buffers.push(buffer);
            mesh_pass.skinning_palette_bind_groups.push(bind_group);
        }
    }

    fn cull_mesh_draw_indices(
        &mut self,
        draws: &[MeshDraw<'_>],
        camera: &Camera3D,
        viewport: RenderViewport,
    ) -> usize {
        self.culled_mesh_indices.clear();
        if draws.is_empty() {
            return 0;
        }
        let vp_size = PhysicalSize::new(
            viewport.size.0.max(1.0).round() as u32,
            viewport.size.1.max(1.0).round() as u32,
        );
        let view_proj = camera.view_projection(vp_size);
        let planes = Self::extract_frustum_planes(view_proj);
        for (idx, draw) in draws.iter().enumerate() {
            let (center, radius) = Self::transform_bounds(draw.model, &draw.mesh.bounds);
            if radius <= 0.0 || Self::sphere_in_frustum(center, radius, &planes) {
                self.culled_mesh_indices.push(idx);
            }
        }
        self.culled_mesh_indices.len()
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

    #[allow(clippy::too_many_arguments)]
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
        let frame = self.window_surface.acquire_surface_frame()?;
        let device = self.device()?.clone();
        let queue = self.queue()?.clone();
        self.sprite_pass.write_globals(&queue, sprite_view_proj)?;
        let view = frame.view();
        let encoder_label =
            format!("Frame Encoder (sprites={}, meshes={})", instances.len(), mesh_draws.len());
        let mut encoder = device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(encoder_label.as_str()) });
        self.sprite_pass.upload_instances(&device, &queue, instances)?;
        self.gpu_timer.begin_frame();
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::FrameStart);

        self.sprite_bind_groups.clear();
        for batch in sprite_batches {
            match self.sprite_pass.sprite_bind_group(&device, batch.atlas.as_ref(), &batch.view, sampler) {
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
        let mut visible_mesh_count = mesh_draws.len();
        let mut mesh_indices: Option<&[usize]> = None;
        if let Some(camera) = mesh_camera {
            visible_mesh_count = self.cull_mesh_draw_indices(mesh_draws, camera, viewport);
            if visible_mesh_count > 0 {
                mesh_indices = Some(&self.culled_mesh_indices);
            }
        }
        let mesh_indices_owned: Option<Vec<usize>> = mesh_indices.map(|idx| idx.to_vec());
        let mut sprite_load_op = wgpu::LoadOp::Clear(clear_color);
        if let Some(camera) = mesh_camera {
            if visible_mesh_count > 0 {
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::ShadowStart);
                self.prepare_shadow_map(
                    &mut encoder,
                    mesh_draws,
                    mesh_indices_owned.as_deref(),
                    camera,
                    viewport,
                )?;
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::ShadowEnd);
                self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::MeshStart);
                self.encode_mesh_pass(
                    &mut encoder,
                    view,
                    viewport,
                    mesh_draws,
                    mesh_indices_owned.as_deref(),
                    camera,
                    clear_color,
                )?;
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
            self.sprite_pass.encode_pass(
                &mut pass,
                viewport,
                self.window_surface.size(),
                instances,
                &self.sprite_bind_groups,
            )?;
        }
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::SpriteEnd);
        self.gpu_timer.write_timestamp(&mut encoder, GpuTimestampLabel::FrameEnd);

        queue.submit(std::iter::once(encoder.finish()));
        let _ = device.poll(wgpu::PollType::Poll);
        Ok(frame)
    }

    #[cfg(feature = "editor")]
    pub fn render_egui(
        &mut self,
        painter: &mut EguiRenderer,
        paint_jobs: &[egui::ClippedPrimitive],
        screen: &ScreenDescriptor,
        frame: SurfaceFrame,
    ) -> Result<()> {
        let device = self.device()?.clone();
        let queue = self.queue()?.clone();
        egui_pass::render(&mut self.gpu_timer, &device, &queue, painter, paint_jobs, screen, frame)
    }

    pub fn gpu_timing_supported(&self) -> bool {
        self.gpu_timer.supported
    }

    pub fn set_gpu_timing_enabled(&mut self, enabled: bool) {
        self.gpu_timer.set_enabled(enabled);
    }

    pub fn gpu_timing_enabled(&self) -> bool {
        self.gpu_timer.enabled
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

#[cfg(all(test, feature = "editor"))]
mod pass_tests {
    use super::*;
    use crate::ecs::MeshLightingInfo;
    use crate::material_registry::MaterialRegistry;
    use crate::mesh::Mesh;
    use egui_wgpu::RendererOptions;
    use glam::Vec3;
    use pollster::block_on;

    fn test_window_config() -> WindowConfig {
        WindowConfig { title: "PassTests".into(), width: 96, height: 64, vsync: false, fullscreen: false }
    }

    fn create_headless_renderer() -> Renderer {
        let cfg = test_window_config();
        let mut renderer = block_on(Renderer::new(&cfg));
        block_on(renderer.init_headless_for_test()).expect("init headless");
        renderer
    }

    fn create_test_atlas(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> (wgpu::TextureView, wgpu::Sampler, wgpu::Sampler) {
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
        (atlas_view, pipeline_sampler, draw_sampler)
    }

    #[test]
    fn cull_mesh_draws_respects_camera_frustum() {
        let mut renderer = create_headless_renderer();
        let mesh = Mesh::cube(1.0);
        let gpu_mesh = renderer.create_gpu_mesh(&mesh).expect("gpu mesh");
        let mut registry = MaterialRegistry::new();
        let default_key = registry.default_key().to_string();
        let material = registry.prepare_material_gpu(&default_key, &mut renderer).expect("material gpu");
        let lighting = MeshLightingInfo::default();
        let visible_draw = MeshDraw {
            mesh: &gpu_mesh,
            model: Mat4::IDENTITY,
            lighting: lighting.clone(),
            material: material.clone(),
            casts_shadows: true,
            skin_palette: None,
        };
        let hidden_draw = MeshDraw {
            mesh: &gpu_mesh,
            model: Mat4::from_translation(Vec3::new(10_000.0, 0.0, 0.0)),
            lighting,
            material,
            casts_shadows: true,
            skin_palette: None,
        };
        let draws = vec![visible_draw.clone(), hidden_draw];
        let camera = Camera3D::new(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, 60f32.to_radians(), 0.1, 500.0);
        let viewport = RenderViewport { origin: (0.0, 0.0), size: (96.0, 64.0) };
        let count = renderer.cull_mesh_draw_indices(&draws, &camera, viewport);
        assert_eq!(count, 1);
        let first =
            renderer.culled_mesh_indices.get(0).and_then(|&idx| draws.get(idx)).map(|draw| draw.model);
        assert!(first.is_some_and(|model| model == visible_draw.model));
    }

    #[test]
    fn headless_render_collects_gpu_timings() {
        let mut renderer = create_headless_renderer();
        renderer.set_gpu_timing_enabled(true);
        renderer.prepare_headless_render_target().expect("headless target");
        let device = renderer.device().expect("device").clone();
        let queue = renderer.queue().expect("queue").clone();
        let (atlas_view, pipeline_sampler, draw_sampler) = create_test_atlas(&device, &queue);
        renderer.init_sprite_pipeline_with_atlas(atlas_view, pipeline_sampler).expect("sprite pipeline");
        let cfg = test_window_config();
        let viewport = RenderViewport { origin: (0.0, 0.0), size: (cfg.width as f32, cfg.height as f32) };
        let frame = renderer
            .render_frame(&[], &[], &draw_sampler, Mat4::IDENTITY, viewport, &[], None)
            .expect("render frame");
        let format = renderer.surface_format().expect("format");
        let mut egui_renderer = EguiRenderer::new(&device, format, RendererOptions::default());
        let screen = ScreenDescriptor {
            size_in_pixels: [cfg.width, cfg.height],
            pixels_per_point: renderer.pixels_per_point(),
        };
        renderer.render_egui(&mut egui_renderer, &[], &screen, frame).expect("render egui");
        let timings = renderer.take_gpu_timings();
        if renderer.gpu_timing_supported() {
            assert!(!timings.is_empty());
        }
    }

    #[test]
    fn light_cluster_metrics_track_visible_lights() {
        let mut renderer = create_headless_renderer();
        renderer.init_mesh_pipeline().expect("mesh pipeline");
        let device = renderer.device().expect("device").clone();
        let queue = renderer.queue().expect("queue").clone();
        let mut lighting = SceneLightingState::default();
        lighting.point_lights = vec![
            ScenePointLight::new(Vec3::new(0.0, 2.0, 0.0), Vec3::splat(1.0), 5.0, 2.0),
            ScenePointLight::new(Vec3::new(2.0, 1.0, -3.0), Vec3::splat(0.8), 3.0, 1.5),
        ];
        let camera = Camera3D::new(Vec3::new(0.0, 0.0, 8.0), Vec3::ZERO, 60f32.to_radians(), 0.1, 100.0);
        let viewport = PhysicalSize::new(test_window_config().width, test_window_config().height);
        let mut scratch = LightClusterScratch::default();
        renderer.light_clusters.reset_metrics();
        renderer
            .light_clusters
            .prepare(LightClusterParams {
                device: &device,
                queue: &queue,
                camera: &camera,
                viewport,
                lighting: &lighting,
                scratch: &mut scratch,
            })
            .expect("prepare light clusters");
        let metrics = renderer.light_clusters.metrics();
        assert_eq!(metrics.total_lights, lighting.point_lights.len() as u32);
        assert!(metrics.visible_lights > 0);
        assert!(metrics.active_clusters > 0);
        assert!(metrics.grid_dims.iter().all(|dim| *dim > 0));
    }
}

impl Renderer {
    pub async fn init_headless_for_test(&mut self) -> Result<()> {
        self.window_surface.init_headless_for_test().await?;
        if let Ok((device, queue)) = self.window_surface.device_and_queue() {
            let supported = self.window_surface.gpu_timing_supported();
            self.gpu_timer.configure(device, queue, supported);
        }
        Ok(())
    }
}

#[cfg(test)]
mod depth_texture_tests {
    use super::window_surface::create_depth_texture;
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
            let (texture, view) = create_depth_texture(&device, size).expect("depth texture");
            let extent = texture.size();
            assert_eq!(extent.width, 321);
            assert_eq!(extent.height, 123);
            assert_eq!(texture.dimension(), wgpu::TextureDimension::D2);
            let _ = view;
        });
    }
}
