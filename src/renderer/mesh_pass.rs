use std::sync::Arc;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct MeshFrameData {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub camera_pos: [f32; 4],
    pub light_dir: [f32; 4],
    pub light_color: [f32; 4],
    pub ambient_color: [f32; 4],
    pub exposure_params: [f32; 4],
    pub cascade_splits: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct MeshDrawData {
    pub model: [[f32; 4]; 4],
    pub base_color: [f32; 4],
    pub emissive: [f32; 4],
    pub material_params: [f32; 4],
}

#[derive(Default)]
pub(super) struct MeshPass {
    pub resources: Option<MeshPipelineResources>,
    pub frame_buffer: Option<wgpu::Buffer>,
    pub draw_buffer: Option<wgpu::Buffer>,
    pub frame_draw_bind_group: Option<wgpu::BindGroup>,
    pub skinning_identity_buffer: Option<wgpu::Buffer>,
    pub skinning_identity_bind_group: Option<wgpu::BindGroup>,
    pub skinning_palette_buffers: Vec<wgpu::Buffer>,
    pub skinning_palette_bind_groups: Vec<wgpu::BindGroup>,
    pub palette_staging: Vec<[f32; 16]>,
    pub palette_hashes: Vec<u64>,
    pub skinning_cursor: usize,
}

impl MeshPass {
    pub fn new() -> Self {
        Self::default()
    }
}

pub(super) struct MeshPipelineResources {
    pub pipeline: wgpu::RenderPipeline,
    pub frame_draw_bgl: Arc<wgpu::BindGroupLayout>,
    pub skinning_bgl: Arc<wgpu::BindGroupLayout>,
    pub material_bgl: Arc<wgpu::BindGroupLayout>,
    pub environment_bgl: Arc<wgpu::BindGroupLayout>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PaletteUploadStats {
    pub calls: u32,
    pub joints_uploaded: u32,
    pub total_cpu_ms: f32,
}

impl PaletteUploadStats {
    pub fn record(&mut self, joints: usize, cpu_ms: f32) {
        if joints == 0 {
            return;
        }
        self.calls = self.calls.saturating_add(1);
        self.joints_uploaded = self.joints_uploaded.saturating_add(joints as u32);
        self.total_cpu_ms += cpu_ms;
    }
}
