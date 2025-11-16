use glam::{Mat4, Vec2, Vec3, Vec4};
use winit::dpi::PhysicalSize;

#[derive(Debug, Clone)]
pub struct Camera2D {
    pub position: Vec2,
    pub zoom: f32,
    base_half_height: f32,
    zoom_limits: (f32, f32),
}

impl Camera2D {
    pub fn new(base_half_height: f32) -> Self {
        Self { position: Vec2::ZERO, zoom: 1.0, base_half_height, zoom_limits: (0.25, 5.0) }
    }

    pub fn set_zoom_limits(&mut self, min: f32, max: f32) {
        debug_assert!(min > 0.0 && max > min);
        self.zoom_limits = (min, max);
        self.zoom = self.zoom.clamp(min, max);
    }

    pub fn apply_scroll_zoom(&mut self, scroll_delta: f32) {
        let multiplier = (scroll_delta * 0.1).exp();
        self.zoom = (self.zoom * multiplier).clamp(self.zoom_limits.0, self.zoom_limits.1);
    }

    pub fn set_zoom(&mut self, zoom: f32) {
        self.zoom = zoom.clamp(self.zoom_limits.0, self.zoom_limits.1);
    }

    pub fn view_projection(&self, size: PhysicalSize<u32>) -> Mat4 {
        let aspect = Self::aspect(size);
        let half_height = self.base_half_height / self.zoom;
        let half_width = half_height * aspect;
        let proj = Mat4::orthographic_rh_gl(-half_width, half_width, -half_height, half_height, -1.0, 1.0);
        let view = Mat4::from_translation(Vec3::new(-self.position.x, -self.position.y, 0.0));
        proj * view
    }

    pub fn pan_screen_delta(&mut self, delta: Vec2, size: PhysicalSize<u32>) {
        if let Some((half_width, half_height)) = self.half_extents(size) {
            if size.width == 0 || size.height == 0 {
                return;
            }
            let world_width = half_width * 2.0;
            let world_height = half_height * 2.0;
            let offset = Vec2::new(
                delta.x / size.width as f32 * world_width,
                -delta.y / size.height as f32 * world_height,
            );
            self.position -= offset;
        }
    }

    pub fn screen_to_world(&self, screen: Vec2, size: PhysicalSize<u32>) -> Option<Vec2> {
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let inv = self.view_projection(size).inverse();
        let ndc_x = (screen.x / size.width as f32) * 2.0 - 1.0;
        let ndc_y = 1.0 - (screen.y / size.height as f32) * 2.0;
        let clip = Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
        let world = inv * clip;
        if world.w.abs() <= f32::EPSILON {
            return None;
        }
        let world = world / world.w;
        Some(Vec2::new(world.x, world.y))
    }

    pub fn world_to_screen_pixels(&self, world: Vec2, size: PhysicalSize<u32>) -> Option<Vec2> {
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let clip = self.view_projection(size) * Vec4::new(world.x, world.y, 0.0, 1.0);
        if clip.w.abs() <= f32::EPSILON {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        let x = (ndc.x + 1.0) * 0.5 * size.width as f32;
        let y = (1.0 - ndc.y) * 0.5 * size.height as f32;
        Some(Vec2::new(x, y))
    }

    pub fn world_rect_to_screen_bounds(
        &self,
        min: Vec2,
        max: Vec2,
        size: PhysicalSize<u32>,
    ) -> Option<(Vec2, Vec2)> {
        let corners = [
            Vec2::new(min.x, min.y),
            Vec2::new(min.x, max.y),
            Vec2::new(max.x, min.y),
            Vec2::new(max.x, max.y),
        ];
        let mut min_screen = Vec2::splat(f32::INFINITY);
        let mut max_screen = Vec2::splat(f32::NEG_INFINITY);
        for corner in corners {
            let screen = self.world_to_screen_pixels(corner, size)?;
            min_screen = min_screen.min(screen);
            max_screen = max_screen.max(screen);
        }
        Some((min_screen, max_screen))
    }

    pub fn half_extents(&self, size: PhysicalSize<u32>) -> Option<(f32, f32)> {
        if size.width == 0 || size.height == 0 {
            return None;
        }
        let aspect = Self::aspect(size);
        let half_height = self.base_half_height / self.zoom;
        let half_width = half_height * aspect;
        Some((half_width, half_height))
    }

    fn aspect(size: PhysicalSize<u32>) -> f32 {
        if size.height == 0 {
            1.0
        } else {
            size.width as f32 / size.height as f32
        }
    }
}
