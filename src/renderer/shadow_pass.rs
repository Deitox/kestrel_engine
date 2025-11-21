use anyhow::{Context, Result};
use glam::{Mat4, Vec3, Vec4};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;

use super::{
    mesh_pass::PaletteUploadStats, Camera3D, MeshDraw, RenderViewport, SceneLightingState, DEPTH_FORMAT,
    MAX_SHADOW_CASCADES, MAX_SKIN_JOINTS, SKINNING_CACHE_HEADROOM,
};

struct ShadowPipelineResources {
    pipeline: wgpu::RenderPipeline,
    skinning_bgl: Arc<wgpu::BindGroupLayout>,
}

#[derive(Default)]
pub struct ShadowPass {
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
    cascade_views: Vec<wgpu::TextureView>,
    sampler: Option<wgpu::Sampler>,
    sample_layout: Option<Arc<wgpu::BindGroupLayout>>,
    sample_bind_group: Option<wgpu::BindGroup>,
    resolution: u32,
    cascade_matrices: [Mat4; MAX_SHADOW_CASCADES],
    cascade_splits: [f32; MAX_SHADOW_CASCADES],
    cascade_count: usize,
    dirty: bool,
}

pub struct ShadowPassParams<'a> {
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub draws: &'a [MeshDraw<'a>],
    pub camera: &'a Camera3D,
    pub viewport: RenderViewport,
    pub lighting: &'a SceneLightingState,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub skinning_limit_warnings: &'a mut HashSet<usize>,
    pub palette_stats: &'a mut PaletteUploadStats,
}

impl ShadowPass {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_sample_layout(&mut self, layout: Arc<wgpu::BindGroupLayout>) {
        self.sample_layout = Some(layout);
        self.sample_bind_group = None;
    }

    pub fn sample_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.sample_bind_group.as_ref()
    }

    pub fn cascade_splits(&self) -> [f32; MAX_SHADOW_CASCADES] {
        self.cascade_splits
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn ensure_sample_bind_group(
        &mut self,
        lighting: &SceneLightingState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<()> {
        self.ensure_resources(device)?;
        self.sync_config(lighting, device)?;
        if self.sample_bind_group.is_none() {
            let matrices = self.cascade_matrices;
            let cascade_count = self.cascade_count;
            self.write_shadow_uniform(queue, lighting, &matrices, 0.0, cascade_count, 0)?;
        }
        Ok(())
    }

    pub fn prepare(&mut self, params: ShadowPassParams<'_>) -> Result<()> {
        self.ensure_resources(params.device)?;
        self.sync_config(params.lighting, params.device)?;

        let shadow_strength = params.lighting.shadow_strength.clamp(0.0, 1.0);
        let casters: Vec<&MeshDraw> = params.draws.iter().filter(|draw| draw.casts_shadows).collect();
        if casters.is_empty() || shadow_strength <= 0.0 {
            self.cascade_matrices = [Mat4::IDENTITY; MAX_SHADOW_CASCADES];
            self.cascade_splits = [0.0; MAX_SHADOW_CASCADES];
            let matrices = self.cascade_matrices;
            let cascade_count = self.cascade_count;
            self.write_shadow_uniform(params.queue, params.lighting, &matrices, 0.0, cascade_count, 0)?;
            return Ok(());
        }

        let mut light_dir = params.lighting.direction.normalize_or_zero();
        if light_dir.length_squared() < 1e-4 {
            light_dir = Vec3::new(0.4, 0.8, 0.35).normalize();
        }
        let viewport_size = PhysicalSize::new(
            params.viewport.size.0.max(1.0).round() as u32,
            params.viewport.size.1.max(1.0).round() as u32,
        );
        let aspect = if viewport_size.height > 0 {
            viewport_size.width as f32 / viewport_size.height as f32
        } else {
            1.0
        };
        let splits = compute_cascade_splits(params.camera, params.lighting);
        let mut prev_split = params.camera.near;
        for (idx, split) in splits.iter().enumerate().take(self.cascade_count) {
            let cascade_far = split.max(prev_split + 0.01);
            self.cascade_matrices[idx] = build_cascade_matrix(
                params.camera,
                aspect,
                prev_split,
                cascade_far,
                light_dir,
                params.lighting,
            );
            prev_split = cascade_far;
        }
        for idx in self.cascade_count..MAX_SHADOW_CASCADES {
            self.cascade_matrices[idx] = Mat4::IDENTITY;
        }
        self.cascade_splits = splits;

        let (pipeline, skinning_bgl) = {
            let resources = self.resources.as_ref().context("Shadow pipeline resources missing")?;
            (resources.pipeline.clone(), resources.skinning_bgl.clone())
        };
        let frame_bg = self.frame_bind_group.as_ref().context("Shadow frame bind group missing")?.clone();
        let draw_bg = self.draw_bind_group.as_ref().context("Shadow draw bind group missing")?.clone();
        let draw_buffer = self.draw_buffer.as_ref().context("Shadow draw buffer missing")?.clone();

        if self.skinning_identity_buffer.is_none() {
            let identity = Mat4::IDENTITY.to_cols_array();
            let palette: Vec<[f32; 16]> = vec![identity; MAX_SKIN_JOINTS];
            let buffer = params.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Shadow Skinning Identity Buffer"),
                contents: bytemuck::cast_slice(&palette),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            self.skinning_identity_buffer = Some(buffer);
            self.skinning_identity_bind_group = None;
        }
        if self.skinning_identity_bind_group.is_none() {
            let buffer =
                self.skinning_identity_buffer.as_ref().context("Shadow skinning identity buffer missing")?;
            let bind_group = params.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow Skinning Identity BG"),
                layout: skinning_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() }],
            });
            self.skinning_identity_bind_group = Some(bind_group);
        }
        let shadow_skinning_identity = self
            .skinning_identity_bind_group
            .as_ref()
            .context("Shadow skinning identity bind group missing")?
            .clone();

        let resolution = self.resolution.max(1);
        self.skinning_cursor = 0;
        let identity_cols = Mat4::IDENTITY.to_cols_array();
        if self.palette_staging.len() != MAX_SKIN_JOINTS {
            self.palette_staging.clear();
            self.palette_staging.resize(MAX_SKIN_JOINTS, identity_cols);
        }

        for cascade_index in 0..self.cascade_count {
            let layer_view =
                self.cascade_views.get(cascade_index).cloned().context("Shadow cascade view missing")?;
            let matrices = self.cascade_matrices;
            let cascade_count = self.cascade_count;
            self.write_shadow_uniform(
                params.queue,
                params.lighting,
                &matrices,
                shadow_strength,
                cascade_count,
                cascade_index,
            )?;
            let mut pass = params.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Shadow Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &layer_view,
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
            let res_f = resolution as f32;
            pass.set_viewport(0.0, 0.0, res_f, res_f, 0.0, 1.0);
            pass.set_scissor_rect(0, 0, resolution, resolution);
            pass.set_bind_group(0, &frame_bg, &[]);

            for draw in &casters {
                let palette_len = draw.skin_palette.as_ref().map(|palette| palette.len()).unwrap_or(0);
                if palette_len > MAX_SKIN_JOINTS && params.skinning_limit_warnings.insert(palette_len) {
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
                params.queue.write_buffer(&draw_buffer, 0, bytemuck::bytes_of(&draw_uniform));
                pass.set_bind_group(1, &draw_bg, &[]);
                if joint_count > 0 {
                    for slot in self.palette_staging.iter_mut() {
                        *slot = identity_cols;
                    }
                    if let Some(palette) = draw.skin_palette.as_ref() {
                        for (dst, mat) in
                            self.palette_staging.iter_mut().zip(palette.iter()).take(joint_count)
                        {
                            *dst = mat.to_cols_array();
                        }
                    }
                    let slot = self.skinning_cursor;
                    self.skinning_cursor += 1;
                    while self.skinning_palette_buffers.len() <= slot {
                        let buffer = params.device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("Shadow Skinning Palette Buffer"),
                            size: (MAX_SKIN_JOINTS * std::mem::size_of::<[f32; 16]>()) as u64,
                            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        });
                        let bind_group = params.device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("Shadow Skinning Palette BG"),
                            layout: skinning_bgl.as_ref(),
                            entries: &[wgpu::BindGroupEntry {
                                binding: 0,
                                resource: buffer.as_entire_binding(),
                            }],
                        });
                        self.skinning_palette_buffers.push(buffer);
                        self.skinning_palette_bind_groups.push(bind_group);
                    }
                    let buffer = &self.skinning_palette_buffers[slot];
                    let upload_start = Instant::now();
                    params.queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.palette_staging));
                    let elapsed_ms = upload_start.elapsed().as_secs_f32() * 1000.0;
                    params.palette_stats.record(joint_count, elapsed_ms);
                    let bind_group = &self.skinning_palette_bind_groups[slot];
                    pass.set_bind_group(2, bind_group, &[]);
                } else {
                    pass.set_bind_group(2, &shadow_skinning_identity, &[]);
                }
                pass.set_vertex_buffer(0, draw.mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(draw.mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.mesh.index_count, 0, 0..1);
            }
        }

        trim_skinning_cache(
            &mut self.skinning_palette_buffers,
            &mut self.skinning_palette_bind_groups,
            self.skinning_cursor,
        );

        Ok(())
    }

    fn ensure_resources(&mut self, device: &wgpu::Device) -> Result<()> {
        if self.resources.is_none() {
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Shadow Shader"),
                source: wgpu::ShaderSource::Wgsl(
                    include_str!("../../assets/shaders/mesh_shadow.wgsl").into(),
                ),
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
                    buffers: &[crate::mesh::MeshVertex::layout()],
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                },
                fragment: None,
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
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

            self.resources = Some(ShadowPipelineResources { pipeline, skinning_bgl });
            self.skinning_identity_buffer = None;
            self.skinning_identity_bind_group = None;

            let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shadow Uniform Buffer"),
                size: std::mem::size_of::<ShadowUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.uniform_buffer = Some(uniform_buffer);

            let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow Frame BG"),
                layout: frame_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self
                        .uniform_buffer
                        .as_ref()
                        .context("Shadow uniform buffer missing")?
                        .as_entire_binding(),
                }],
            });
            self.frame_bind_group = Some(frame_bind_group);

            let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Shadow Draw Buffer"),
                size: std::mem::size_of::<ShadowDrawUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.draw_buffer = Some(draw_buffer);

            let draw_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Shadow Draw BG"),
                layout: draw_bgl.as_ref(),
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self
                        .draw_buffer
                        .as_ref()
                        .context("Shadow draw buffer missing")?
                        .as_entire_binding(),
                }],
            });
            self.draw_bind_group = Some(draw_bind_group);

            self.dirty = true;
        }

        if self.map_texture.is_none() || self.map_view.is_none() {
            self.recreate_shadow_map(device)?;
        }

        if self.sampler.is_none() {
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
            self.sampler = Some(sampler);
        }

        if self.sample_bind_group.is_none() {
            if let (Some(layout), Some(buffer), Some(view), Some(sampler)) = (
                self.sample_layout.as_ref(),
                self.uniform_buffer.as_ref(),
                self.map_view.as_ref(),
                self.sampler.as_ref(),
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
                self.sample_bind_group = Some(bind_group);
            }
        }

        Ok(())
    }

    fn sync_config(&mut self, lighting: &SceneLightingState, device: &wgpu::Device) -> Result<()> {
        let desired_cascades = lighting.shadow_cascade_count.clamp(1, MAX_SHADOW_CASCADES as u32) as usize;
        let desired_resolution = lighting.shadow_resolution.clamp(256, 8192);
        let mut needs_recreate = false;
        if self.cascade_count != desired_cascades {
            self.cascade_count = desired_cascades;
            needs_recreate = true;
        }
        if self.resolution != desired_resolution {
            self.resolution = desired_resolution;
            needs_recreate = true;
        }
        if needs_recreate {
            self.recreate_shadow_map(device)?;
        }
        Ok(())
    }

    fn recreate_shadow_map(&mut self, device: &wgpu::Device) -> Result<()> {
        let resolution = self.resolution.max(1);
        let cascade_layers = self.cascade_count.max(1);
        let extent = wgpu::Extent3d {
            width: resolution,
            height: resolution,
            depth_or_array_layers: cascade_layers as u32,
        };
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("Shadow Map Array View"),
            format: Some(DEPTH_FORMAT),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_mip_level: 0,
            mip_level_count: None,
            base_array_layer: 0,
            array_layer_count: None,
            ..Default::default()
        });
        let mut layer_views = Vec::with_capacity(cascade_layers);
        for layer in 0..cascade_layers {
            layer_views.push(texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("Shadow Map Cascade Layer"),
                format: Some(DEPTH_FORMAT),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: 0,
                mip_level_count: None,
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..Default::default()
            }));
        }
        self.map_texture = Some(texture);
        self.map_view = Some(view);
        self.cascade_views = layer_views;
        self.sample_bind_group = None;
        self.dirty = true;
        Ok(())
    }

    fn write_shadow_uniform(
        &mut self,
        queue: &wgpu::Queue,
        lighting: &SceneLightingState,
        matrices: &[Mat4; MAX_SHADOW_CASCADES],
        strength: f32,
        cascade_count: usize,
        active_cascade: usize,
    ) -> Result<()> {
        if !self.dirty && active_cascade == 0 {
            return Ok(());
        }
        let buffer = self.uniform_buffer.as_ref().context("Shadow uniform buffer missing")?;
        let bias = lighting.shadow_bias.clamp(0.00001, 0.05);
        let clamped_count = cascade_count.clamp(1, MAX_SHADOW_CASCADES);
        let mut gpu_matrices = [[[0.0f32; 4]; 4]; MAX_SHADOW_CASCADES];
        for (dst, src) in gpu_matrices.iter_mut().zip(matrices.iter()) {
            *dst = src.to_cols_array_2d();
        }
        let inv_resolution = 1.0 / self.resolution.max(1) as f32;
        let base_radius = lighting.shadow_pcf_radius.max(0.0);
        let mut cascade_params = [[0.0f32; 4]; MAX_SHADOW_CASCADES];
        for (idx, params) in cascade_params.iter_mut().enumerate() {
            let cascade_factor = 1.0 + (idx as f32 * 0.35);
            params[0] = inv_resolution;
            params[1] = (base_radius * cascade_factor).max(0.0);
        }
        let params = [
            bias,
            strength.clamp(0.0, 1.0),
            clamped_count as f32,
            active_cascade.min(clamped_count - 1) as f32,
        ];
        let data = ShadowUniform { light_view_proj: gpu_matrices, params, cascade_params };
        queue.write_buffer(buffer, 0, bytemuck::bytes_of(&data));
        if active_cascade + 1 == clamped_count {
            self.dirty = false;
        }
        Ok(())
    }
}

const SHADOW_CASCADE_EPS: f32 = 0.01;

#[allow(clippy::needless_range_loop)]
fn compute_cascade_splits(camera: &Camera3D, lighting: &SceneLightingState) -> [f32; MAX_SHADOW_CASCADES] {
    let safe_near = camera.near.max(0.01);
    let mut target_far = (safe_near + lighting.shadow_distance).min(camera.far);
    if target_far <= safe_near {
        target_far = safe_near + SHADOW_CASCADE_EPS;
    }
    let range = (target_far - safe_near).max(SHADOW_CASCADE_EPS);
    let mut splits = [target_far; MAX_SHADOW_CASCADES];
    let mut cascade_count = lighting.shadow_cascade_count.max(1) as usize;
    if range < SHADOW_CASCADE_EPS * 4.0 {
        cascade_count = cascade_count.min(2);
    }
    let lambda = lighting.shadow_split_lambda.clamp(0.0, 1.0);
    for cascade in 0..cascade_count {
        let p = (cascade + 1) as f32 / cascade_count as f32;
        let uniform_split = safe_near + range * p;
        let log_split = safe_near * (target_far / safe_near).powf(p);
        let split = uniform_split + (log_split - uniform_split) * lambda;
        splits[cascade] = split.min(target_far);
    }
    for idx in 1..MAX_SHADOW_CASCADES {
        if splits[idx] <= splits[idx - 1] {
            splits[idx] = splits[idx - 1] + SHADOW_CASCADE_EPS;
        }
    }
    splits[MAX_SHADOW_CASCADES - 1] = target_far;
    splits
}

fn build_cascade_matrix(
    camera: &Camera3D,
    aspect: f32,
    near: f32,
    far: f32,
    light_dir: Vec3,
    lighting: &SceneLightingState,
) -> Mat4 {
    let corners = frustum_corners(camera, aspect, near, far);
    let mut center = Vec3::ZERO;
    for corner in &corners {
        center += *corner;
    }
    center /= corners.len() as f32;
    let mut up = Vec3::Y;
    if up.dot(light_dir).abs() > 0.95 {
        up = Vec3::X;
    }
    let distance = (far - near).max(1.0);
    let eye = center - light_dir * (distance + lighting.shadow_distance * 0.5);
    let view = Mat4::look_at_rh(eye, center, up);
    let mut min = Vec3::splat(f32::MAX);
    let mut max = Vec3::splat(f32::MIN);
    for corner in corners {
        let light_space = view.transform_point3(corner);
        min = min.min(light_space);
        max = max.max(light_space);
    }
    let padding = 10.0;
    min -= Vec3::splat(padding);
    max += Vec3::splat(padding);
    Mat4::orthographic_rh(min.x, max.x, min.y, max.y, min.z - padding, max.z + padding) * view
}

fn frustum_corners(camera: &Camera3D, aspect: f32, near: f32, far: f32) -> [Vec3; 8] {
    let proj = Mat4::perspective_rh_gl(camera.fov_y_radians, aspect.max(0.0001), near, far);
    let view = camera.view_matrix();
    let inv = (proj * view).inverse();
    let mut corners = [Vec3::ZERO; 8];
    let mut idx = 0;
    for &x in &[-1.0, 1.0] {
        for &y in &[-1.0, 1.0] {
            for &z in &[-1.0, 1.0] {
                let clip = Vec4::new(x, y, z, 1.0);
                let world = inv * clip;
                corners[idx] = world.truncate() / world.w;
                idx += 1;
            }
        }
    }
    corners
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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowUniform {
    light_view_proj: [[[f32; 4]; 4]; MAX_SHADOW_CASCADES],
    params: [f32; 4],
    cascade_params: [[f32; 4]; MAX_SHADOW_CASCADES],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ShadowDrawUniform {
    model: [[f32; 4]; 4],
    joint_count: u32,
    _padding: [u32; 3],
}
