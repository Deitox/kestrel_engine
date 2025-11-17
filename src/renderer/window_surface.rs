use crate::config::WindowConfig;
use anyhow::{anyhow, Context, Result};
use std::sync::Arc;
use winit::dpi::PhysicalSize;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window};

use super::DEPTH_FORMAT;

const DEFAULT_PRESENT_MODES: [wgpu::PresentMode; 1] = [wgpu::PresentMode::Fifo];

#[derive(Debug)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceErrorAction {
    Reconfigure,
    Retry,
    OutOfMemory,
    Unknown,
}

pub struct WindowSurface {
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
    present_modes: Vec<wgpu::PresentMode>,
    headless_target: Option<HeadlessTarget>,
    gpu_timing_supported: bool,
    #[cfg(test)]
    resize_invocations: usize,
    #[cfg(test)]
    surface_error_injector: Option<wgpu::SurfaceError>,
}

impl WindowSurface {
    pub fn new(window_cfg: &WindowConfig) -> Self {
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
            present_modes: Vec::new(),
            headless_target: None,
            gpu_timing_supported: false,
            #[cfg(test)]
            resize_invocations: 0,
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

    pub fn device_and_queue(&self) -> Result<(&wgpu::Device, &wgpu::Queue)> {
        Ok((self.device()?, self.queue()?))
    }

    pub fn device(&self) -> Result<&wgpu::Device> {
        self.device.as_ref().context("GPU device not initialized")
    }

    pub fn queue(&self) -> Result<&wgpu::Queue> {
        self.queue.as_ref().context("GPU queue not initialized")
    }

    pub fn depth_view(&self) -> Result<&wgpu::TextureView> {
        self.depth_view.as_ref().context("Depth texture missing")
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

    pub fn window(&self) -> Option<&Window> {
        self.window.as_deref()
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

    pub fn acquire_surface_frame(&mut self) -> Result<SurfaceFrame> {
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

    pub fn ensure_depth_texture(&mut self) -> Result<()> {
        if self.depth_texture.is_some() {
            return Ok(());
        }
        self.recreate_depth_texture()
    }

    pub fn gpu_timing_supported(&self) -> bool {
        self.gpu_timing_supported
    }

    #[cfg(test)]
    pub fn resize_invocations_for_test(&self) -> usize {
        self.resize_invocations
    }

    #[cfg(test)]
    pub fn inject_surface_error_for_test(&mut self, error: wgpu::SurfaceError) {
        self.surface_error_injector = Some(error);
    }

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
        self.gpu_timing_supported = supports_timestamp && supports_encoder_queries;
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

    pub fn handle_surface_error(&mut self, error: &wgpu::SurfaceError) -> anyhow::Error {
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

    fn configure_surface(&mut self) -> Result<()> {
        let surface = self.surface.as_ref().context("Surface not initialized")?;
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let config = self.config.as_mut().context("Surface configuration missing")?;
        surface.configure(device, config);
        Ok(())
    }

    fn recreate_depth_texture(&mut self) -> Result<()> {
        let device = self.device.as_ref().context("GPU device not initialized")?;
        let (depth_texture, depth_view) = create_depth_texture(device, self.size)?;
        self.depth_texture = Some(depth_texture);
        self.depth_view = Some(depth_view);
        Ok(())
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

    fn choose_surface_format(formats: &[wgpu::TextureFormat]) -> wgpu::TextureFormat {
        formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(formats[0])
    }

    pub fn reconfigure_present_mode(&mut self) -> Result<()> {
        if self.surface.is_none() {
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
        self.gpu_timing_supported = supports_timestamp && supports_encoder_queries;
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

        let (depth_texture, depth_view) = create_depth_texture(&device, size)?;

        self.surface = Some(surface);
        self.device = Some(device);
        self.queue = Some(queue);
        self.config = Some(config);
        self.depth_texture = Some(depth_texture);
        self.depth_view = Some(depth_view);
        self.present_modes = caps.present_modes.clone();
        Ok(())
    }

    fn surface_error_action(error: &wgpu::SurfaceError) -> SurfaceErrorAction {
        match error {
            wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated => SurfaceErrorAction::Reconfigure,
            wgpu::SurfaceError::Timeout => SurfaceErrorAction::Retry,
            wgpu::SurfaceError::OutOfMemory => SurfaceErrorAction::OutOfMemory,
            wgpu::SurfaceError::Other => SurfaceErrorAction::Unknown,
        }
    }
}

pub(super) fn create_depth_texture(
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

#[cfg(test)]
mod tests {
    use super::*;
    use pollster::block_on;

    #[test]
    fn present_mode_respects_vsync_flag() {
        let cfg = WindowConfig::default();
        let surface = WindowSurface::new(&cfg);
        let modes = vec![wgpu::PresentMode::Immediate, wgpu::PresentMode::Fifo];
        assert_eq!(surface.select_present_mode(&modes), wgpu::PresentMode::Immediate);

        let mut vsync_surface = WindowSurface::new(&cfg);
        vsync_surface.vsync = true;
        assert_eq!(vsync_surface.select_present_mode(&modes), wgpu::PresentMode::Fifo);
    }

    #[test]
    fn surface_error_action_matches_variants() {
        assert_eq!(
            WindowSurface::surface_error_action(&wgpu::SurfaceError::Lost),
            SurfaceErrorAction::Reconfigure
        );
        assert_eq!(
            WindowSurface::surface_error_action(&wgpu::SurfaceError::Outdated),
            SurfaceErrorAction::Reconfigure
        );
        assert_eq!(
            WindowSurface::surface_error_action(&wgpu::SurfaceError::Timeout),
            SurfaceErrorAction::Retry
        );
        assert_eq!(
            WindowSurface::surface_error_action(&wgpu::SurfaceError::OutOfMemory),
            SurfaceErrorAction::OutOfMemory
        );
        assert_eq!(
            WindowSurface::surface_error_action(&wgpu::SurfaceError::Other),
            SurfaceErrorAction::Unknown
        );
    }

    #[test]
    fn surface_loss_triggers_resize_attempt_even_without_surface() {
        let mut surface = WindowSurface::new(&WindowConfig::default());
        #[cfg(test)]
        assert_eq!(surface.resize_invocations_for_test(), 0);
        let _ = surface.handle_surface_error(&wgpu::SurfaceError::Lost);
        #[cfg(test)]
        assert_eq!(surface.resize_invocations_for_test(), 1);
    }

    #[test]
    fn headless_render_recovers_from_surface_loss() {
        let window_config =
            WindowConfig { title: "Headless".into(), width: 64, height: 64, vsync: false, fullscreen: false };
        let mut surface = block_on(async {
            let mut s = WindowSurface::new(&window_config);
            s.init_headless_for_test().await.expect("init headless");
            s
        });

        surface.prepare_headless_render_target().expect("headless target");
        surface.inject_surface_error_for_test(wgpu::SurfaceError::Lost);
        let err = surface.acquire_surface_frame().expect_err("surface loss should bubble");
        assert!(err.to_string().contains("Surface lost"));
        #[cfg(test)]
        assert!(surface.resize_invocations_for_test() >= 1);
    }
}
