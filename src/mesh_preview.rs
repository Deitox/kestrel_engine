use crate::camera3d::Camera3D;
use glam::{EulerRot, Quat, Vec3};

pub(crate) const GIZMO_3D_AXIS_LENGTH_SCALE: f32 = 0.2;
pub(crate) const GIZMO_3D_AXIS_MIN: f32 = 0.1;
pub(crate) const GIZMO_3D_AXIS_MAX: f32 = 5.0;
pub(crate) const MESH_CAMERA_FOV_RADIANS: f32 = 60.0_f32.to_radians();
pub(crate) const MESH_CAMERA_NEAR: f32 = 0.1;
pub(crate) const MESH_CAMERA_FAR: f32 = 100.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeshControlMode {
    Disabled,
    Orbit,
    Freefly,
}

impl Default for MeshControlMode {
    fn default() -> Self {
        MeshControlMode::Disabled
    }
}

impl MeshControlMode {
    pub(crate) fn next(self) -> Self {
        match self {
            MeshControlMode::Disabled => MeshControlMode::Orbit,
            MeshControlMode::Orbit => MeshControlMode::Freefly,
            MeshControlMode::Freefly => MeshControlMode::Disabled,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            MeshControlMode::Disabled => "Disabled",
            MeshControlMode::Orbit => "Orbit",
            MeshControlMode::Freefly => "Free-fly",
        }
    }

    pub(crate) fn status_message(self) -> &'static str {
        match self {
            MeshControlMode::Disabled => "Scripted orbit animates the camera (press M to switch modes).",
            MeshControlMode::Orbit => {
                "Orbit control enabled (right-drag to orbit, scroll to zoom, L toggles frustum lock)."
            }
            MeshControlMode::Freefly => {
                "Free-fly enabled (RMB + WASD/QE to move, Z/C to roll, Shift to boost, L locks frustum)."
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct FreeflyController {
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) roll: f32,
}

impl FreeflyController {
    pub(crate) fn from_camera(camera: &Camera3D) -> Self {
        let forward = (camera.target - camera.position).normalize_or_zero();
        let yaw = forward.x.atan2(forward.z);
        let pitch =
            forward.y.asin().clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
        let roll = 0.0;
        Self { position: camera.position, yaw, pitch, roll }
    }

    pub(crate) fn orientation(&self) -> Quat {
        Quat::from_euler(EulerRot::YXZ, self.yaw, self.pitch, self.roll)
    }

    pub(crate) fn forward(&self) -> Vec3 {
        self.orientation() * Vec3::new(0.0, 0.0, -1.0)
    }

    pub(crate) fn right(&self) -> Vec3 {
        self.orientation() * Vec3::new(1.0, 0.0, 0.0)
    }

    pub(crate) fn up(&self) -> Vec3 {
        self.orientation() * Vec3::Y
    }

    pub(crate) fn to_camera(&self) -> Camera3D {
        let forward = self.forward();
        let mut camera = Camera3D::new(
            self.position,
            self.position + forward,
            MESH_CAMERA_FOV_RADIANS,
            MESH_CAMERA_NEAR,
            MESH_CAMERA_FAR,
        );
        let up = self.up().normalize_or_zero();
        camera.up = if up.length_squared() > 0.0 { up } else { Vec3::Y };
        camera
    }
}
