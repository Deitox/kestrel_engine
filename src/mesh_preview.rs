use crate::camera3d::{Camera3D, OrbitCamera};
use crate::App;
use glam::{EulerRot, Quat, Vec2, Vec3};
use rand::Rng;

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

pub(crate) fn focus_mesh_selection(app: &mut App, center: Vec3) {
    app.mesh_frustum_focus = center;
    app.mesh_frustum_distance = (app.mesh_camera.position - center).length().max(0.1);
    app.mesh_orbit.target = center;
    app.mesh_orbit.radius = app.mesh_frustum_distance;
    app.mesh_camera = app.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
    app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
    app.mesh_status = Some("Framed selection in 3D viewport.".to_string());
}

pub(crate) fn update_mesh_camera(app: &mut App, dt: f32) {
    match app.mesh_control_mode {
        MeshControlMode::Disabled => {
            app.mesh_freefly_velocity = Vec3::ZERO;
            app.mesh_freefly_rot_velocity = Vec3::ZERO;
            let auto_delta = Vec2::new(0.25 * dt, 0.12 * dt);
            app.mesh_orbit.orbit(auto_delta);
            app.mesh_camera =
                app.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
            app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
        }
        MeshControlMode::Orbit => {
            app.mesh_freefly_velocity = Vec3::ZERO;
            app.mesh_freefly_rot_velocity = Vec3::ZERO;
            let (dx, dy) = app.input.mouse_delta;
            if app.input.right_held() && (dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON) {
                let sensitivity = 0.008;
                app.mesh_orbit.orbit(Vec2::new(dx * sensitivity, dy * sensitivity));
            }
            if app.input.wheel.abs() > 0.0 && !app.mesh_frustum_lock {
                let sensitivity = 0.12;
                let factor = (app.input.wheel * sensitivity).exp();
                app.mesh_orbit.zoom(factor);
                app.input.wheel = 0.0;
            }
            app.mesh_camera =
                app.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
            app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
        }
        MeshControlMode::Freefly => {
            let dt = dt.max(1e-6);
            let mut target_rot = Vec3::ZERO;
            if app.input.right_held() {
                let sensitivity = 0.008;
                target_rot.x = app.input.mouse_delta.0 * sensitivity / dt;
                target_rot.y = app.input.mouse_delta.1 * sensitivity / dt;
            }
            let roll_raw =
                (app.input.freefly_roll_right() as i32 - app.input.freefly_roll_left() as i32) as f32;
            if roll_raw.abs() > 0.0 {
                target_rot.z = roll_raw * 2.5;
            }
            let angular_lerp = 1.0 - (-dt * 14.0).exp();
            app.mesh_freefly_rot_velocity = app.mesh_freefly_rot_velocity.lerp(target_rot, angular_lerp);
            app.mesh_freefly.yaw += app.mesh_freefly_rot_velocity.x * dt;
            app.mesh_freefly.pitch = (app.mesh_freefly.pitch + app.mesh_freefly_rot_velocity.y * dt)
                .clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
            app.mesh_freefly.roll += app.mesh_freefly_rot_velocity.z * dt;
            app.mesh_freefly.roll = crate::wrap_angle(app.mesh_freefly.roll);

            let mut direction = Vec3::ZERO;
            let forward = app.mesh_freefly.forward().normalize_or_zero();
            let right = app.mesh_freefly.right().normalize_or_zero();
            let up = app.mesh_freefly.up().normalize_or_zero();

            if !app.mesh_frustum_lock {
                if app.input.freefly_forward() {
                    direction += forward;
                }
                if app.input.freefly_backward() {
                    direction -= forward;
                }
                if app.input.freefly_right() {
                    direction += right;
                }
                if app.input.freefly_left() {
                    direction -= right;
                }
                if app.input.freefly_ascend() {
                    direction += up;
                }
                if app.input.freefly_descend() {
                    direction -= up;
                }
            }

            let boost = if app.input.freefly_boost() { 3.0 } else { 1.0 };
            let target_velocity = if direction.length_squared() > 0.0 {
                direction.normalize_or_zero() * app.mesh_freefly_speed * boost
            } else {
                Vec3::ZERO
            };
            let velocity_lerp = 1.0 - (-dt * 10.0).exp();
            app.mesh_freefly_velocity = app.mesh_freefly_velocity.lerp(target_velocity, velocity_lerp);
            app.mesh_freefly.position += app.mesh_freefly_velocity * dt;

            if !app.mesh_frustum_lock && app.input.wheel.abs() > 0.0 {
                let factor = (1.0 + app.input.wheel * 0.06).clamp(0.2, 5.0);
                app.mesh_freefly_speed = (app.mesh_freefly_speed * factor).clamp(0.1, 200.0);
                app.mesh_status = Some(format!("Free-fly speed: {:.2}", app.mesh_freefly_speed));
                app.input.wheel = 0.0;
            }

            app.mesh_camera = app.mesh_freefly.to_camera();

            if app.mesh_frustum_lock {
                let direction = (app.mesh_frustum_focus - app.mesh_camera.position).normalize_or_zero();
                if direction.length_squared() > 0.0 {
                    app.mesh_camera.target = app.mesh_camera.position + direction * app.mesh_frustum_distance;
                }
                app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
            } else {
                app.mesh_orbit.target = app.mesh_camera.target;
                app.mesh_orbit.radius = (app.mesh_camera.target - app.mesh_camera.position).length().max(0.1);
            }
        }
    }

    if app.mesh_frustum_lock {
        let focus = compute_focus_point(app);
        if focus.length_squared() > 0.0 {
            app.mesh_frustum_focus = focus;
        }
        let direction = (app.mesh_frustum_focus - app.mesh_camera.position).normalize_or_zero();
        if direction.length_squared() > 0.0 {
            app.mesh_camera.target = app.mesh_camera.position + direction * app.mesh_frustum_distance;
            if app.mesh_control_mode == MeshControlMode::Freefly {
                app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
            } else {
                app.mesh_orbit.target = app.mesh_frustum_focus;
                app.mesh_orbit.radius = app.mesh_frustum_distance.max(0.1);
                app.mesh_camera =
                    app.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
                app.mesh_camera.target = focus;
            }
        }
    } else {
        match app.mesh_control_mode {
            MeshControlMode::Orbit | MeshControlMode::Disabled => {
                app.mesh_frustum_focus = app.mesh_orbit.target;
                app.mesh_frustum_distance = app.mesh_orbit.radius;
            }
            MeshControlMode::Freefly => {
                app.mesh_frustum_focus = app.mesh_camera.target;
                app.mesh_frustum_distance =
                    (app.mesh_frustum_focus - app.mesh_camera.position).length().max(0.1);
            }
        }
    }
}

pub(crate) fn set_mesh_control_mode(app: &mut App, mode: MeshControlMode) {
    if app.mesh_control_mode == mode {
        return;
    }
    app.mesh_freefly_velocity = Vec3::ZERO;
    app.mesh_freefly_rot_velocity = Vec3::ZERO;
    match mode {
        MeshControlMode::Disabled | MeshControlMode::Orbit => {
            sync_orbit_from_camera_pose(app);
            app.mesh_camera =
                app.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
            app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
        }
        MeshControlMode::Freefly => {
            app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
            app.mesh_camera = app.mesh_freefly.to_camera();
        }
    }
    app.mesh_control_mode = mode;
    app.mesh_status = Some(mode.status_message().to_string());
    app.input.wheel = 0.0;
    app.input.mouse_delta = (0.0, 0.0);
    if app.mesh_frustum_lock {
        app.mesh_frustum_distance = (app.mesh_camera.position - app.mesh_frustum_focus).length().max(0.1);
    }
}

pub(crate) fn set_viewport_camera_mode(app: &mut App, mode: super::ViewportCameraMode) {
    if app.viewport_camera_mode == mode {
        return;
    }
    app.viewport_camera_mode = mode;
    if mode == super::ViewportCameraMode::Perspective3D && app.mesh_control_mode == MeshControlMode::Disabled
    {
        app.mesh_control_mode = MeshControlMode::Orbit;
        app.mesh_camera =
            app.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
        app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
        app.mesh_status = Some(app.mesh_control_mode.status_message().to_string());
    }
}

pub(crate) fn set_frustum_lock(app: &mut App, enabled: bool) {
    if app.mesh_frustum_lock == enabled {
        return;
    }
    if enabled {
        let focus = compute_focus_point(app);
        app.mesh_frustum_focus = focus;
        app.mesh_frustum_distance = (app.mesh_camera.position - focus).length().max(0.1);
        if app.mesh_control_mode == MeshControlMode::Freefly {
            let direction = (focus - app.mesh_freefly.position).normalize_or_zero();
            if direction.length_squared() > 0.0 {
                app.mesh_freefly.yaw = direction.x.atan2(direction.z);
                app.mesh_freefly.pitch = direction
                    .y
                    .asin()
                    .clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
            }
        }
        app.mesh_status = Some("Frustum lock enabled (wheel adjusts focus distance).".to_string());
    } else {
        app.mesh_status = Some("Frustum lock disabled.".to_string());
        app.mesh_frustum_distance = app.mesh_orbit.radius;
    }
    app.mesh_frustum_lock = enabled;
    app.mesh_freefly_velocity = Vec3::ZERO;
    app.mesh_freefly_rot_velocity = Vec3::ZERO;
}

pub(crate) fn compute_focus_point(app: &App) -> Vec3 {
    if let Some(entity) = app.selected_entity {
        if let Some(info) = app.ecs.entity_info(entity) {
            if let Some(mesh_tx) = info.mesh_transform {
                return mesh_tx.translation;
            }
            return Vec3::new(info.translation.x, info.translation.y, 0.0);
        }
    }
    app.mesh_orbit.target
}

pub(crate) fn sync_orbit_from_camera_pose(app: &mut App) {
    let target = app.mesh_orbit.target;
    let mut offset = app.mesh_camera.position - target;
    if offset.length_squared() < 1e-5 {
        offset = Vec3::new(0.0, 0.0, app.mesh_orbit.radius.max(0.1));
    }
    let radius = offset.length().max(0.1);
    let yaw = offset.x.atan2(offset.z);
    let pitch = (offset.y / radius).clamp(-1.0, 1.0).asin();
    app.mesh_orbit.radius = radius;
    app.mesh_orbit.yaw_radians = yaw;
    app.mesh_orbit.pitch_radians =
        pitch.clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
}

pub(crate) fn handle_mesh_control_input(app: &mut App) {
    if app.input.take_mesh_toggle() {
        let next = app.mesh_control_mode.next();
        set_mesh_control_mode(app, next);
    }
    if app.input.take_frustum_lock_toggle() {
        let next = !app.mesh_frustum_lock;
        set_frustum_lock(app, next);
    }
}

pub(crate) fn reset_mesh_camera(app: &mut App) {
    let radius = app.mesh_orbit.radius;
    app.mesh_orbit = OrbitCamera::new(app.mesh_orbit.target, radius);
    app.mesh_camera = app.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
    app.mesh_freefly = FreeflyController::from_camera(&app.mesh_camera);
    app.mesh_freefly_velocity = Vec3::ZERO;
    app.mesh_freefly_rot_velocity = Vec3::ZERO;
    app.mesh_freefly.roll = 0.0;
    if app.mesh_control_mode == MeshControlMode::Freefly {
        app.mesh_camera = app.mesh_freefly.to_camera();
    }
    if app.mesh_frustum_lock {
        app.mesh_frustum_focus = compute_focus_point(app);
        app.mesh_frustum_distance = (app.mesh_camera.position - app.mesh_frustum_focus).length().max(0.1);
    } else {
        app.mesh_frustum_distance = app.mesh_orbit.radius;
    }
    app.mesh_status = Some("Mesh camera reset.".to_string());
}

pub(crate) fn set_preview_mesh(app: &mut App, new_key: String) {
    if new_key == app.preview_mesh_key {
        return;
    }
    let source_path = app.mesh_registry.mesh_source(&new_key).map(|path| path.to_string_lossy().into_owned());
    match app.mesh_registry.retain_mesh(&new_key, source_path.as_deref()) {
        Ok(()) => {
            let previous = std::mem::replace(&mut app.preview_mesh_key, new_key.clone());
            app.persistent_meshes.insert(new_key.clone());
            if app.persistent_meshes.remove(&previous) {
                app.mesh_registry.release_mesh(&previous);
            }
            app.mesh_status = Some(format!("Preview mesh: {}", new_key));
            if let Err(err) = app.mesh_registry.ensure_gpu(&app.preview_mesh_key, &mut app.renderer) {
                app.mesh_status = Some(format!("Mesh upload failed: {err}"));
            }
        }
        Err(err) => {
            app.mesh_status = Some(format!("Mesh '{}' unavailable: {err}", new_key));
        }
    }
}

pub(crate) fn spawn_mesh_entity(app: &mut App, mesh_key: &str) {
    if let Err(err) = app.mesh_registry.ensure_mesh(mesh_key, None) {
        app.mesh_status = Some(format!("Mesh '{}' unavailable: {err}", mesh_key));
        return;
    }
    if let Err(err) = app.mesh_registry.ensure_gpu(mesh_key, &mut app.renderer) {
        app.mesh_status = Some(format!("Failed to upload mesh '{}': {err}", mesh_key));
        return;
    }
    let mut rng = rand::thread_rng();
    let position = Vec3::new(rng.gen_range(-1.2..1.2), rng.gen_range(-0.6..0.8), rng.gen_range(-1.0..1.0));
    let scale = Vec3::splat(0.6);
    let entity = app.ecs.spawn_mesh_entity(mesh_key, position, scale);
    if let Some(subsets) = app.mesh_registry.mesh_subsets(mesh_key) {
        if let Some(material) = subsets.iter().find_map(|subset| subset.material.clone()) {
            app.ecs.set_mesh_material(entity, Some(material));
        }
    }
    app.selected_entity = Some(entity);
    app.mesh_status = Some(format!("Spawned mesh '{}' as entity {:?}", mesh_key, entity));
}

pub(crate) fn mesh_camera_forward(app: &App) -> Vec3 {
    (app.mesh_camera.target - app.mesh_camera.position).normalize_or_zero()
}
