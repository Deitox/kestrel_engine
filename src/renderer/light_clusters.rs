use anyhow::{Context, Result};
#[cfg(test)]
use glam::Vec3;
use glam::{Mat4, Vec4};
use std::sync::Arc;
use winit::dpi::PhysicalSize;

use super::{
    Camera3D, ClusterConfigUniform, ClusterLightUniform, ClusterRecordGpu, PointLightGpu, SceneLightingState,
    ScenePointLight, LIGHT_CLUSTER_CACHE_QUANTIZE, LIGHT_CLUSTER_MAX_LIGHTS,
    LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER, LIGHT_CLUSTER_RECORD_STRIDE_WORDS, LIGHT_CLUSTER_TILE_SIZE,
    LIGHT_CLUSTER_Z_SLICES,
};

#[derive(Clone, Copy, Debug, Default)]
struct LightClusterCache {
    viewport: PhysicalSize<u32>,
    view_key: [i32; 16],
    proj_key: [i32; 16],
    lights_hash: u64,
    metrics: LightClusterMetrics,
    valid: bool,
}

impl LightClusterCache {
    fn matches(&self, viewport: PhysicalSize<u32>, view: Mat4, proj: Mat4, lights_hash: u64) -> bool {
        self.valid
            && self.viewport == viewport
            && self.view_key == quantize_matrix(view)
            && self.proj_key == quantize_matrix(proj)
            && self.lights_hash == lights_hash
    }

    fn update(
        &mut self,
        viewport: PhysicalSize<u32>,
        view: Mat4,
        proj: Mat4,
        lights_hash: u64,
        metrics: LightClusterMetrics,
    ) {
        self.viewport = viewport;
        self.view_key = quantize_matrix(view);
        self.proj_key = quantize_matrix(proj);
        self.lights_hash = lights_hash;
        self.metrics = metrics;
        self.valid = true;
    }

    fn invalidate(&mut self) {
        self.valid = false;
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct LightClusterSpan {
    light_index: u32,
    start_x: u32,
    end_x: u32,
    start_y: u32,
    end_y: u32,
    start_z: u32,
    end_z: u32,
}

#[derive(Default)]
pub struct LightClusterScratch {
    spans: Vec<LightClusterSpan>,
    cluster_counts: Vec<u16>,
    cluster_write_offsets: Vec<u16>,
    cluster_records: Vec<ClusterRecordGpu>,
    cluster_indices: Vec<u32>,
    cluster_data_words: Vec<u32>,
    gpu_lights: Vec<PointLightGpu>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LightClusterMetrics {
    pub total_lights: u32,
    pub visible_lights: u32,
    pub grid_dims: [u32; 3],
    pub active_clusters: u32,
    pub total_clusters: u32,
    pub average_lights_per_cluster: f32,
    pub max_lights_per_cluster: u32,
    pub overflow_clusters: u32,
    pub light_assignments: u32,
    pub tile_size_px: u32,
    pub truncated_lights: u32,
}

impl LightClusterMetrics {
    pub fn culled_lights(&self) -> u32 {
        self.total_lights.saturating_sub(self.visible_lights)
    }
}

#[derive(Default)]
pub struct LightClusterPass {
    layout: Option<Arc<wgpu::BindGroupLayout>>,
    uniform_buffer: Option<wgpu::Buffer>,
    storage_buffer: Option<wgpu::Buffer>,
    bind_group: Option<wgpu::BindGroup>,
    storage_capacity_words: usize,
    metrics: LightClusterMetrics,
    grid_dims: [u32; 3],
    tile_size_px: u32,
    cache: LightClusterCache,
}

pub struct LightClusterParams<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub camera: &'a Camera3D,
    pub viewport: PhysicalSize<u32>,
    pub lighting: &'a SceneLightingState,
    pub scratch: &'a mut LightClusterScratch,
}

struct LightClusterBuildData<'a> {
    uniform: ClusterLightUniform,
    cluster_data_words: &'a [u32],
    metrics: LightClusterMetrics,
}

impl LightClusterPass {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_layout(&mut self, layout: Arc<wgpu::BindGroupLayout>) {
        self.layout = Some(layout);
        self.bind_group = None;
    }

    pub fn bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.bind_group.as_ref()
    }

    pub fn metrics(&self) -> &LightClusterMetrics {
        &self.metrics
    }

    pub fn reset_metrics(&mut self) {
        self.metrics = LightClusterMetrics::default();
    }

    pub fn invalidate_cache(&mut self) {
        self.cache.invalidate();
    }

    pub fn prepare(&mut self, params: LightClusterParams<'_>) -> Result<()> {
        let layout = self.layout.as_ref().context("Light cluster layout missing")?.clone();
        let view = params.camera.view_matrix();
        let aspect = if params.viewport.height > 0 {
            params.viewport.width as f32 / params.viewport.height as f32
        } else {
            1.0
        };
        let proj = params.camera.projection_matrix(aspect);
        let light_hash = hash_point_lights(&params.lighting.point_lights);
        if self.cache.matches(params.viewport, view, proj, light_hash) && self.bind_group.is_some() {
            self.metrics = self.cache.metrics;
            return Ok(());
        }

        let build_data = build_light_cluster_data(
            &params.lighting.point_lights,
            params.camera,
            params.viewport,
            view,
            proj,
            params.scratch,
        );
        if build_data.metrics.truncated_lights > 0 && self.metrics.truncated_lights == 0 {
            eprintln!(
                "[renderer] {} point light(s) exceeded the clustered lighting budget (max {}). Extra lights will be ignored.",
                build_data.metrics.truncated_lights,
                LIGHT_CLUSTER_MAX_LIGHTS
            );
        }
        self.update_resources(params.device, params.queue, &layout, &build_data)?;
        self.cache.update(params.viewport, view, proj, light_hash, build_data.metrics);
        self.metrics = build_data.metrics;
        Ok(())
    }

    fn update_resources<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &Arc<wgpu::BindGroupLayout>,
        data: &LightClusterBuildData<'a>,
    ) -> Result<()> {
        if self.uniform_buffer.is_none() {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Light Cluster Uniform"),
                size: std::mem::size_of::<ClusterLightUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.uniform_buffer = Some(buffer);
            self.bind_group = None;
        }
        if self.storage_buffer.is_none() || self.storage_capacity_words < data.cluster_data_words.len() {
            let capacity = data.cluster_data_words.len().max(1);
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Light Cluster Storage"),
                size: (capacity * std::mem::size_of::<u32>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.storage_buffer = Some(buffer);
            self.storage_capacity_words = capacity;
            self.bind_group = None;
        }

        let uniform_buffer = self.uniform_buffer.as_ref().context("Light cluster uniform missing")?;
        queue.write_buffer(uniform_buffer, 0, bytemuck::bytes_of(&data.uniform));

        let storage_buffer = self.storage_buffer.as_ref().context("Light cluster storage missing")?;
        queue.write_buffer(storage_buffer, 0, bytemuck::cast_slice(data.cluster_data_words));

        if self.bind_group.is_none() {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Light Cluster Bind Group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uniform_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: storage_buffer.as_entire_binding() },
                ],
            });
            self.bind_group = Some(bind_group);
        }

        self.metrics = data.metrics;
        self.grid_dims = data.metrics.grid_dims;
        self.tile_size_px = data.metrics.tile_size_px;
        Ok(())
    }
}

fn build_light_cluster_data<'a>(
    lights: &[ScenePointLight],
    camera: &Camera3D,
    viewport: PhysicalSize<u32>,
    view: Mat4,
    proj: Mat4,
    scratch: &'a mut LightClusterScratch,
) -> LightClusterBuildData<'a> {
    let width = viewport.width.max(1);
    let height = viewport.height.max(1);
    let grid_x = width.div_ceil(LIGHT_CLUSTER_TILE_SIZE).max(1);
    let grid_y = height.div_ceil(LIGHT_CLUSTER_TILE_SIZE).max(1);
    let grid_z = LIGHT_CLUSTER_Z_SLICES.max(1);
    let total_clusters = grid_x.saturating_mul(grid_y).saturating_mul(grid_z).max(1);
    let aspect = if height > 0 { width as f32 / height as f32 } else { 1.0 };
    let near = camera.near;
    let far = camera.far.max(near + 0.0001);
    let depth_range = (far - near).max(0.0001);
    let inv_depth_range = 1.0 / depth_range;
    let view_proj = proj * view;
    let frustum_planes = super::Renderer::extract_frustum_planes(view_proj);
    let width_f = width as f32;
    let height_f = height as f32;
    let viewport_inv_width = if width == 0 { 0.0 } else { 1.0 / width as f32 };
    let viewport_inv_height = if height == 0 { 0.0 } else { 1.0 / height as f32 };
    let half_width = width_f * 0.5;
    let half_height = height_f * 0.5;
    let half_fov = (camera.fov_y_radians * 0.5).max(0.001);
    let focal_y = 1.0 / half_fov.tan();
    let focal_x = focal_y / aspect.max(0.001);

    let mut uniform = ClusterLightUniform {
        config: ClusterConfigUniform {
            viewport: [width as f32, height as f32, viewport_inv_width, viewport_inv_height],
            depth_params: [near, far, inv_depth_range, 0.0],
        grid_dims: [grid_x, grid_y, grid_z, total_clusters],
            stats: [0, LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER as u32, LIGHT_CLUSTER_TILE_SIZE, 0],
            data_meta: [0, LIGHT_CLUSTER_RECORD_STRIDE_WORDS, 0, 0],
        },
        lights: [PointLightGpu::default(); LIGHT_CLUSTER_MAX_LIGHTS],
    };

    scratch.spans.clear();
    scratch.gpu_lights.clear();
    scratch.cluster_counts.clear();
    scratch.cluster_counts.resize(total_clusters as usize, 0);
    scratch.cluster_write_offsets.clear();
    scratch.cluster_write_offsets.resize(total_clusters as usize, 0);
    scratch.cluster_records.clear();
    scratch.cluster_indices.clear();
    scratch.cluster_data_words.clear();

    let mut overflow_clusters = 0u32;
    let mut truncated_lights = 0u32;

    for light in lights {
        if scratch.gpu_lights.len() >= LIGHT_CLUSTER_MAX_LIGHTS {
            truncated_lights = truncated_lights.saturating_add(1);
            continue;
        }
        let radius = light.radius.max(0.01);
        if !super::Renderer::sphere_in_frustum(light.position, radius, &frustum_planes) {
            continue;
        }
        let world_pos = light.position.extend(1.0);
        let view_pos = view * world_pos;
        let view_vec = view_pos.truncate();
        let depth = -view_vec.z;
        if depth <= 0.0 || depth + radius <= near || depth - radius >= far {
            continue;
        }
        let clip_pos = proj * Vec4::new(view_vec.x, view_vec.y, view_vec.z, 1.0);
        if clip_pos.w.abs() < 1e-5 {
            continue;
        }
        let ndc = clip_pos.truncate() / clip_pos.w;
        let screen_x = (ndc.x * 0.5 + 0.5) * width_f;
        let screen_y = (1.0 - (ndc.y * 0.5 + 0.5)) * height_f;
        let screen_radius_x = ((radius / depth) * focal_x * half_width).abs().max(1.0);
        let screen_radius_y = ((radius / depth) * focal_y * half_height).abs().max(1.0);

        let min_screen_x = screen_x - screen_radius_x;
        let max_screen_x = screen_x + screen_radius_x;
        let min_screen_y = screen_y - screen_radius_y;
        let max_screen_y = screen_y + screen_radius_y;

        if max_screen_x < 0.0 || min_screen_x > width_f || max_screen_y < 0.0 || min_screen_y > height_f {
            continue;
        }

        let min_norm_x = (min_screen_x / width_f).clamp(0.0, 1.0);
        let max_norm_x = (max_screen_x / width_f).clamp(0.0, 1.0);
        let min_norm_y = (min_screen_y / height_f).clamp(0.0, 1.0);
        let max_norm_y = (max_screen_y / height_f).clamp(0.0, 1.0);
        let depth_min = (depth - radius).max(near);
        let depth_max = (depth + radius).min(far);
        if depth_max <= near {
            continue;
        }
        let min_norm_z = ((depth_min - near) * inv_depth_range).clamp(0.0, 1.0);
        let max_norm_z = ((depth_max - near) * inv_depth_range).clamp(0.0, 1.0);

        let start_x = cluster_start_index(min_norm_x, grid_x);
        let end_x = cluster_end_index(max_norm_x, grid_x);
        let start_y = cluster_start_index(min_norm_y, grid_y);
        let end_y = cluster_end_index(max_norm_y, grid_y);
        let start_z = cluster_start_index(min_norm_z, grid_z);
        let end_z = cluster_end_index(max_norm_z, grid_z);
        if start_x > end_x || start_y > end_y || start_z > end_z {
            continue;
        }

        let light_index = scratch.gpu_lights.len() as u32;
        scratch.gpu_lights.push(PointLightGpu {
            position_radius: [light.position.x, light.position.y, light.position.z, light.radius],
            color_intensity: [light.color.x, light.color.y, light.color.z, light.intensity],
        });

        for z in start_z..=end_z {
            for y in start_y..=end_y {
                for x in start_x..=end_x {
                    let idx = cluster_flat_index(x, y, z, grid_x, grid_y);
                    let count = &mut scratch.cluster_counts[idx];
                    if (*count as usize) >= LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER {
                        overflow_clusters = overflow_clusters.saturating_add(1);
                        continue;
                    }
                    *count = count.saturating_add(1);
                }
            }
        }

        scratch.spans.push(LightClusterSpan { light_index, start_x, end_x, start_y, end_y, start_z, end_z });
    }

    scratch.cluster_records.reserve(scratch.cluster_counts.len());
    let mut indices_total = 0u32;
    for &count in &scratch.cluster_counts {
        let limited = count.min(LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER as u16) as u32;
        scratch.cluster_records.push(ClusterRecordGpu {
            offset: indices_total,
            count: limited,
            ..Default::default()
        });
        indices_total += limited;
    }

    scratch.cluster_indices.clear();
    scratch.cluster_indices.resize(indices_total as usize, 0);
    scratch.cluster_write_offsets.iter_mut().for_each(|v| *v = 0);
    for span in &scratch.spans {
        for z in span.start_z..=span.end_z {
            for y in span.start_y..=span.end_y {
                for x in span.start_x..=span.end_x {
                    let idx = cluster_flat_index(x, y, z, grid_x, grid_y);
                    let record = &scratch.cluster_records[idx];
                    if (record.count as usize) >= LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER {
                        continue;
                    }
                    let offset = &mut scratch.cluster_write_offsets[idx];
                    if (*offset as usize) >= LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER {
                        continue;
                    }
                    let write_index = (record.offset + *offset as u32) as usize;
                    if write_index < scratch.cluster_indices.len() {
                        scratch.cluster_indices[write_index] = span.light_index;
                    }
                    *offset = offset.saturating_add(1);
                }
            }
        }
    }

    scratch.cluster_data_words.clear();
    scratch.cluster_data_words.reserve(
        scratch.cluster_records.len() * LIGHT_CLUSTER_RECORD_STRIDE_WORDS as usize
            + scratch.cluster_indices.len(),
    );
    for record in &scratch.cluster_records {
        scratch.cluster_data_words.push(record.offset);
        scratch.cluster_data_words.push(record.count);
        scratch.cluster_data_words.push(0);
        scratch.cluster_data_words.push(0);
    }
    scratch.cluster_data_words.extend(&scratch.cluster_indices);

    uniform.config.stats = [
        scratch.gpu_lights.len() as u32,
        LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER as u32,
        LIGHT_CLUSTER_TILE_SIZE,
        overflow_clusters,
    ];
    uniform.config.data_meta = [
        scratch.cluster_records.len() as u32,
        LIGHT_CLUSTER_RECORD_STRIDE_WORDS,
        scratch.cluster_records.len() as u32 * LIGHT_CLUSTER_RECORD_STRIDE_WORDS,
        scratch.cluster_indices.len() as u32,
    ];

    let mut gpu_lights = [PointLightGpu::default(); LIGHT_CLUSTER_MAX_LIGHTS];
    for (dst, src) in gpu_lights.iter_mut().zip(scratch.gpu_lights.iter()) {
        *dst = *src;
    }
    uniform.lights = gpu_lights;

    let metrics = LightClusterMetrics {
        total_lights: lights.len() as u32,
        visible_lights: scratch.gpu_lights.len() as u32,
        grid_dims: [grid_x, grid_y, grid_z],
        active_clusters: scratch.cluster_counts.iter().filter(|count| **count > 0).count() as u32,
        total_clusters,
        average_lights_per_cluster: if total_clusters > 0 {
            scratch.spans.len() as f32 / total_clusters as f32
        } else {
            0.0
        },
        max_lights_per_cluster: scratch
            .cluster_counts
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .min(LIGHT_CLUSTER_MAX_LIGHTS_PER_CLUSTER as u16) as u32,
        overflow_clusters,
        light_assignments: scratch.cluster_counts.iter().map(|count| *count as u32).sum(),
        tile_size_px: LIGHT_CLUSTER_TILE_SIZE,
        truncated_lights,
    };

    LightClusterBuildData { uniform, cluster_data_words: &scratch.cluster_data_words, metrics }
}

fn cluster_start_index(norm: f32, count: u32) -> u32 {
    if count <= 1 {
        return 0;
    }
    let value = (norm * count as f32).floor();
    value.clamp(0.0, (count - 1) as f32) as u32
}

fn cluster_end_index(norm: f32, count: u32) -> u32 {
    if count <= 1 {
        return 0;
    }
    let value = (norm * count as f32).ceil() as i32 - 1;
    value.clamp(0, count as i32 - 1) as u32
}

fn cluster_flat_index(x: u32, y: u32, z: u32, grid_x: u32, grid_y: u32) -> usize {
    (z as usize * grid_x as usize * grid_y as usize) + (y as usize * grid_x as usize) + x as usize
}

fn hash_point_lights(lights: &[ScenePointLight]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET ^ (lights.len() as u64);
    for light in lights {
        for value in [
            light.position.x.to_bits(),
            light.position.y.to_bits(),
            light.position.z.to_bits(),
            light.color.x.to_bits(),
            light.color.y.to_bits(),
            light.color.z.to_bits(),
            light.radius.to_bits(),
            light.intensity.to_bits(),
        ] {
            hash ^= value as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    hash
}

fn quantize_matrix(mat: Mat4) -> [i32; 16] {
    let mut key = [0i32; 16];
    let cols = mat.to_cols_array();
    for (dst, value) in key.iter_mut().zip(cols.iter()) {
        *dst = (value / LIGHT_CLUSTER_CACHE_QUANTIZE).round() as i32;
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_light_cluster_data_counts_visible_lights() {
        let camera = Camera3D::new(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, 60.0_f32.to_radians(), 0.1, 100.0);
        let viewport = PhysicalSize::new(640, 480);
        let view = camera.view_matrix();
        let proj = camera.projection_matrix(viewport.width as f32 / viewport.height as f32);
        let mut scratch = LightClusterScratch::default();
        let lights = vec![
            ScenePointLight::new(Vec3::ZERO, Vec3::splat(1.0), 4.0, 1.0),
            ScenePointLight::new(Vec3::new(50.0, 0.0, 0.0), Vec3::splat(1.0), 2.0, 1.0),
        ];
        let data = build_light_cluster_data(&lights, &camera, viewport, view, proj, &mut scratch);
        assert_eq!(data.metrics.total_lights, 2);
        assert!(data.metrics.visible_lights >= 1);
        assert!(data.metrics.total_clusters > 0);
    }
}
