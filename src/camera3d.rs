use glam::{Mat4, Quat, Vec2, Vec3};
use winit::dpi::PhysicalSize;

const DEFAULT_UP: Vec3 = Vec3::Y;

/// Simple perspective camera intended for upcoming 3D tooling.
#[derive(Debug, Clone)]
pub struct Camera3D {
    pub position: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera3D {
    pub fn new(position: Vec3, target: Vec3, fov_y_radians: f32, near: f32, far: f32) -> Self {
        Self { position, target, up: DEFAULT_UP, fov_y_radians, near, far }
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }

    pub fn projection_matrix(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh_gl(self.fov_y_radians, aspect.max(0.0001), self.near, self.far)
    }

    pub fn view_projection(&self, viewport: PhysicalSize<u32>) -> Mat4 {
        let aspect = if viewport.height > 0 {
            viewport.width as f32 / viewport.height as f32
        } else {
            1.0
        };
        self.projection_matrix(aspect) * self.view_matrix()
    }

    /// Generates a world-space ray originating from the camera through a screen-space position.
    pub fn screen_ray(&self, screen: Vec2, viewport: PhysicalSize<u32>) -> Option<(Vec3, Vec3)> {
        if viewport.width == 0 || viewport.height == 0 {
            return None;
        }
        let ndc_x = (2.0 * screen.x / viewport.width as f32) - 1.0;
        let ndc_y = 1.0 - (2.0 * screen.y / viewport.height as f32);
        let clip = Vec3::new(ndc_x, ndc_y, 1.0);
        let aspect = viewport.width as f32 / viewport.height as f32;

        let proj = self.projection_matrix(aspect);
        let inv_proj = proj.inverse();
        let view = self.view_matrix();
        let inv_view = view.inverse();

        let eye_dir = inv_proj * clip.extend(1.0);
        let world_dir = (inv_view * Vec3::new(eye_dir.x, eye_dir.y, eye_dir.z).extend(0.0)).truncate();
        let dir = world_dir.normalize();
        Some((self.position, dir))
    }
}

/// Orbit-style controller storing yaw/pitch around a target.
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub radius: f32,
    pub yaw_radians: f32,
    pub pitch_radians: f32,
}

impl OrbitCamera {
    pub fn new(target: Vec3, radius: f32) -> Self {
        Self { target, radius: radius.max(0.01), yaw_radians: 0.0, pitch_radians: 0.0 }
    }

    pub fn to_camera(&self, fov_y_radians: f32, near: f32, far: f32) -> Camera3D {
        let rotation = Quat::from_euler(glam::EulerRot::YXZ, self.yaw_radians, self.pitch_radians, 0.0);
        let offset = rotation * Vec3::new(0.0, 0.0, self.radius);
        let position = self.target + offset;
        Camera3D::new(position, self.target, fov_y_radians, near, far)
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.yaw_radians += delta.x;
        self.pitch_radians = (self.pitch_radians + delta.y).clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.radius = (self.radius * factor).clamp(0.1, 10_000.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera3d_view_projection_is_finite() {
        let camera = Camera3D::new(Vec3::new(0.0, 1.0, 5.0), Vec3::ZERO, 60.0_f32.to_radians(), 0.1, 1000.0);
        let vp = camera.view_projection(PhysicalSize::new(1280, 720));
        assert!(!vp.to_cols_array().iter().any(|v| v.is_nan() || v.is_infinite()));
    }

    #[test]
    fn orbit_camera_orbits_target() {
        let mut orbit = OrbitCamera::new(Vec3::ZERO, 5.0);
        orbit.orbit(Vec2::new(0.5, 0.25));
        let camera = orbit.to_camera(45.0f32.to_radians(), 0.1, 500.0);
        assert!(camera.position.distance(Vec3::ZERO) > 1.0);
        assert!(camera.position.distance(Vec3::ZERO) < 10.0);
    }
}
