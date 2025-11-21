use crate::camera3d::{Camera3D, OrbitCamera};
use crate::ecs::EntityInfo;
use crate::plugins::{EnginePlugin, PluginContext};
use crate::scene::{
    SceneFreeflyCamera, SceneOrbitCamera, ScenePreviewCamera, ScenePreviewCameraMode, Vec3Data,
};
use crate::wrap_angle;
use anyhow::Result;
use bevy_ecs::prelude::Entity;
use glam::{EulerRot, Mat4, Quat, Vec2, Vec3};
use rand::Rng;
use std::any::Any;
use std::collections::HashSet;

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

impl From<MeshControlMode> for ScenePreviewCameraMode {
    fn from(mode: MeshControlMode) -> Self {
        match mode {
            MeshControlMode::Disabled => ScenePreviewCameraMode::Disabled,
            MeshControlMode::Orbit => ScenePreviewCameraMode::Orbit,
            MeshControlMode::Freefly => ScenePreviewCameraMode::Freefly,
        }
    }
}

impl From<ScenePreviewCameraMode> for MeshControlMode {
    fn from(mode: ScenePreviewCameraMode) -> Self {
        match mode {
            ScenePreviewCameraMode::Disabled => MeshControlMode::Disabled,
            ScenePreviewCameraMode::Orbit => MeshControlMode::Orbit,
            ScenePreviewCameraMode::Freefly => MeshControlMode::Freefly,
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

impl Default for FreeflyController {
    fn default() -> Self {
        Self { position: Vec3::ZERO, yaw: 0.0, pitch: 0.0, roll: 0.0 }
    }
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

pub struct MeshPreviewPlugin {
    preview_mesh_key: String,
    mesh_orbit: OrbitCamera,
    mesh_camera: Camera3D,
    mesh_model: Mat4,
    mesh_angle: f32,
    mesh_control_mode: MeshControlMode,
    mesh_freefly: FreeflyController,
    mesh_freefly_speed: f32,
    mesh_freefly_velocity: Vec3,
    mesh_freefly_rot_velocity: Vec3,
    mesh_frustum_lock: bool,
    mesh_frustum_focus: Vec3,
    mesh_frustum_distance: f32,
    mesh_status: Option<String>,
    persistent_meshes: HashSet<String>,
    persistent_materials: HashSet<String>,
}

impl Default for MeshPreviewPlugin {
    fn default() -> Self {
        let mesh_orbit = OrbitCamera::new(Vec3::ZERO, 5.0);
        let mesh_camera = mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
        let mesh_freefly = FreeflyController::from_camera(&mesh_camera);
        Self {
            preview_mesh_key: String::new(),
            mesh_orbit,
            mesh_camera,
            mesh_model: Mat4::IDENTITY,
            mesh_angle: 0.0,
            mesh_control_mode: MeshControlMode::Disabled,
            mesh_freefly,
            mesh_freefly_speed: 4.0,
            mesh_freefly_velocity: Vec3::ZERO,
            mesh_freefly_rot_velocity: Vec3::ZERO,
            mesh_frustum_lock: false,
            mesh_frustum_focus: Vec3::ZERO,
            mesh_frustum_distance: 5.0,
            mesh_status: None,
            persistent_meshes: HashSet::new(),
            persistent_materials: HashSet::new(),
        }
    }
}

impl MeshPreviewPlugin {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn preview_mesh_key(&self) -> &str {
        &self.preview_mesh_key
    }

    pub fn mesh_model(&self) -> &Mat4 {
        &self.mesh_model
    }

    pub fn mesh_camera(&self) -> &Camera3D {
        &self.mesh_camera
    }

    pub fn mesh_control_mode(&self) -> MeshControlMode {
        self.mesh_control_mode
    }

    pub fn mesh_orbit(&self) -> &OrbitCamera {
        &self.mesh_orbit
    }

    pub fn mesh_freefly_speed(&self) -> f32 {
        self.mesh_freefly_speed
    }

    pub fn mesh_frustum_lock(&self) -> bool {
        self.mesh_frustum_lock
    }

    pub fn mesh_status(&self) -> Option<&str> {
        self.mesh_status.as_deref()
    }

    pub fn set_status<S: Into<String>>(&mut self, message: S) {
        self.mesh_status = Some(message.into());
    }

    pub fn persistent_meshes(&self) -> &HashSet<String> {
        &self.persistent_meshes
    }

    pub fn persistent_materials(&self) -> &HashSet<String> {
        &self.persistent_materials
    }

    pub fn mesh_camera_forward(&self) -> Vec3 {
        (self.mesh_camera.target - self.mesh_camera.position).normalize_or_zero()
    }

    pub fn ensure_preview_gpu(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        let (mesh_registry, renderer) = ctx.mesh_registry_and_renderer()?;
        if let Err(err) = mesh_registry.ensure_gpu(self.preview_mesh_key(), renderer) {
            eprintln!("Failed to upload preview mesh '{}': {err:?}", self.preview_mesh_key());
            self.mesh_status = Some(format!("Mesh upload failed: {err}"));
        }
        Ok(())
    }

    pub fn capture_preview_camera(&self) -> ScenePreviewCamera {
        ScenePreviewCamera {
            mode: ScenePreviewCameraMode::from(self.mesh_control_mode),
            orbit: SceneOrbitCamera {
                target: Vec3Data::from(self.mesh_orbit.target),
                radius: self.mesh_orbit.radius,
                yaw: self.mesh_orbit.yaw_radians,
                pitch: self.mesh_orbit.pitch_radians,
            },
            freefly: SceneFreeflyCamera {
                position: Vec3Data::from(self.mesh_freefly.position),
                yaw: self.mesh_freefly.yaw,
                pitch: self.mesh_freefly.pitch,
                roll: self.mesh_freefly.roll,
                speed: self.mesh_freefly_speed,
            },
            frustum_lock: self.mesh_frustum_lock,
            frustum_focus: Vec3Data::from(self.mesh_frustum_focus),
            frustum_distance: self.mesh_frustum_distance,
        }
    }

    pub fn apply_preview_camera(&mut self, preview: &ScenePreviewCamera) {
        self.mesh_orbit.target = Vec3::from(preview.orbit.target.clone());
        self.mesh_orbit.radius = preview.orbit.radius.max(0.1);
        self.mesh_orbit.yaw_radians = preview.orbit.yaw;
        self.mesh_orbit.pitch_radians = preview
            .orbit
            .pitch
            .clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
        self.mesh_freefly.position = Vec3::from(preview.freefly.position.clone());
        self.mesh_freefly.yaw = preview.freefly.yaw;
        self.mesh_freefly.pitch = preview.freefly.pitch;
        self.mesh_freefly.roll = preview.freefly.roll;
        self.mesh_freefly_speed = preview.freefly.speed.max(0.01);
        self.mesh_frustum_lock = preview.frustum_lock;
        self.mesh_frustum_focus = Vec3::from(preview.frustum_focus.clone());
        self.mesh_frustum_distance = preview.frustum_distance.max(0.1);
        self.mesh_freefly_velocity = Vec3::ZERO;
        self.mesh_freefly_rot_velocity = Vec3::ZERO;

        let mode = MeshControlMode::from(preview.mode);
        self.mesh_control_mode = mode;
        match mode {
            MeshControlMode::Disabled | MeshControlMode::Orbit => {
                self.mesh_camera =
                    self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
                self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
            }
            MeshControlMode::Freefly => {
                self.mesh_camera = self.mesh_freefly.to_camera();
            }
        }
        self.mesh_status = Some(mode.status_message().to_string());
    }

    pub fn focus_selection_with_info(&mut self, info: &EntityInfo) -> bool {
        if let Some(mesh_tx) = info.mesh_transform.as_ref() {
            self.focus_mesh_center(mesh_tx.translation);
            true
        } else {
            self.mesh_status = Some("Centered 2D camera on selection.".to_string());
            true
        }
    }

    pub fn snap_frustum_to_selection(&mut self, selection: Option<&EntityInfo>, fallback_target: Vec3) {
        let focus = selection
            .and_then(|info| info.mesh_transform.as_ref().map(|tx| tx.translation))
            .or_else(|| selection.map(|info| Vec3::new(info.translation.x, info.translation.y, 0.0)))
            .unwrap_or(fallback_target);
        self.focus_mesh_center(focus);
        self.mesh_status = Some("Frustum focus updated.".to_string());
    }

    pub fn set_mesh_control_mode(
        &mut self,
        ctx: &mut PluginContext<'_>,
        mode: MeshControlMode,
    ) -> Result<()> {
        if self.mesh_control_mode == mode {
            return Ok(());
        }
        self.mesh_freefly_velocity = Vec3::ZERO;
        self.mesh_freefly_rot_velocity = Vec3::ZERO;
        match mode {
            MeshControlMode::Disabled | MeshControlMode::Orbit => {
                self.sync_orbit_from_camera_pose();
                self.mesh_camera =
                    self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
                self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
            }
            MeshControlMode::Freefly => {
                self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
                self.mesh_camera = self.mesh_freefly.to_camera();
            }
        }
        self.mesh_control_mode = mode;
        self.mesh_status = Some(mode.status_message().to_string());
        {
            let input = ctx.input_mut()?;
            input.wheel = 0.0;
            input.mouse_delta = (0.0, 0.0);
        }
        if self.mesh_frustum_lock {
            self.mesh_frustum_distance =
                (self.mesh_camera.position - self.mesh_frustum_focus).length().max(0.1);
        }
        Ok(())
    }

    pub fn set_frustum_lock(&mut self, ctx: &mut PluginContext<'_>, enabled: bool) -> Result<()> {
        if self.mesh_frustum_lock == enabled {
            return Ok(());
        }
        if enabled {
            let focus = self.compute_focus_point(ctx)?;
            self.mesh_frustum_focus = focus;
            self.mesh_frustum_distance = (self.mesh_camera.position - focus).length().max(0.1);
            if self.mesh_control_mode == MeshControlMode::Freefly {
                let direction = (focus - self.mesh_freefly.position).normalize_or_zero();
                if direction.length_squared() > 0.0 {
                    self.mesh_freefly.yaw = direction.x.atan2(direction.z);
                    self.mesh_freefly.pitch = direction
                        .y
                        .asin()
                        .clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
                }
            }
            self.mesh_status = Some("Frustum lock enabled (wheel adjusts focus distance).".to_string());
        } else {
            self.mesh_status = Some("Frustum lock disabled.".to_string());
            self.mesh_frustum_distance = self.mesh_orbit.radius;
        }
        self.mesh_frustum_lock = enabled;
        self.mesh_freefly_velocity = Vec3::ZERO;
        self.mesh_freefly_rot_velocity = Vec3::ZERO;
        Ok(())
    }

    pub fn reset_mesh_camera(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        let radius = self.mesh_orbit.radius;
        self.mesh_orbit = OrbitCamera::new(self.mesh_orbit.target, radius);
        self.mesh_camera =
            self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
        self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
        self.mesh_freefly_velocity = Vec3::ZERO;
        self.mesh_freefly_rot_velocity = Vec3::ZERO;
        self.mesh_freefly.roll = 0.0;
        if self.mesh_control_mode == MeshControlMode::Freefly {
            self.mesh_camera = self.mesh_freefly.to_camera();
        }
        if self.mesh_frustum_lock {
            self.mesh_frustum_focus = self.compute_focus_point(ctx)?;
            self.mesh_frustum_distance =
                (self.mesh_camera.position - self.mesh_frustum_focus).length().max(0.1);
        } else {
            self.mesh_frustum_distance = self.mesh_orbit.radius;
        }
        self.mesh_status = Some("Mesh camera reset.".to_string());
        Ok(())
    }

    pub fn set_preview_mesh(
        &mut self,
        ctx: &mut PluginContext<'_>,
        scene_material_refs: &HashSet<String>,
        new_key: String,
    ) -> Result<()> {
        if new_key == self.preview_mesh_key {
            return Ok(());
        }
        let source_path =
            ctx.mesh_registry()?.mesh_source(&new_key).map(|path| path.to_string_lossy().into_owned());
        let retain_result = {
            let (mesh_registry, material_registry) = ctx.mesh_registry_and_materials()?;
            mesh_registry.retain_mesh(&new_key, source_path.as_deref(), material_registry)
        };
        match retain_result {
            Ok(()) => {
                let previous = std::mem::replace(&mut self.preview_mesh_key, new_key.clone());
                let previous_materials: Vec<String> = ctx
                    .mesh_registry()?
                    .mesh_subsets(&previous)
                    .map(|subs| subs.iter().filter_map(|subset| subset.material.clone()).collect())
                    .unwrap_or_default();
                let new_materials: Vec<String> = ctx
                    .mesh_registry()?
                    .mesh_subsets(&new_key)
                    .map(|subs| subs.iter().filter_map(|subset| subset.material.clone()).collect())
                    .unwrap_or_default();

                self.persistent_meshes.insert(new_key.clone());
                if self.persistent_meshes.remove(&previous) {
                    ctx.mesh_registry_mut()?.release_mesh(&previous);
                }

                {
                    let material_registry = ctx.material_registry_mut()?;
                    for material in &new_materials {
                        if let Err(err) = material_registry.retain(material) {
                            self.mesh_status = Some(format!("Material retain failed: {err}"));
                        } else {
                            self.persistent_materials.insert(material.clone());
                        }
                    }

                    for material in previous_materials {
                        if self.persistent_materials.remove(&material)
                            && !scene_material_refs.contains(&material)
                        {
                            material_registry.release(&material);
                        }
                    }
                }

                self.mesh_status = Some(format!("Preview mesh: {}", new_key));
                let (mesh_registry, renderer) = ctx.mesh_registry_and_renderer()?;
                if let Err(err) = mesh_registry.ensure_gpu(&self.preview_mesh_key, renderer) {
                    self.mesh_status = Some(format!("Mesh upload failed: {err}"));
                }
            }
            Err(err) => {
                self.mesh_status = Some(format!("Mesh '{}' unavailable: {err}", new_key));
            }
        }
        Ok(())
    }

    pub fn spawn_mesh_entity(
        &mut self,
        ctx: &mut PluginContext<'_>,
        mesh_key: &str,
    ) -> Result<Option<Entity>> {
        {
            let (mesh_registry, material_registry) = ctx.mesh_registry_and_materials()?;
            if let Err(err) = mesh_registry.ensure_mesh(mesh_key, None, material_registry) {
                self.mesh_status = Some(format!("Mesh '{}' unavailable: {err}", mesh_key));
                return Ok(None);
            }
        }
        {
            let (mesh_registry, renderer) = ctx.mesh_registry_and_renderer()?;
            if let Err(err) = mesh_registry.ensure_gpu(mesh_key, renderer) {
                self.mesh_status = Some(format!("Failed to upload mesh '{}': {err}", mesh_key));
                return Ok(None);
            }
        }
        let mut rng = rand::thread_rng();
        let position =
            Vec3::new(rng.gen_range(-1.2..1.2), rng.gen_range(-0.6..0.8), rng.gen_range(-1.0..1.0));
        let scale = Vec3::splat(0.6);
        let entity = ctx.ecs_mut()?.spawn_mesh_entity(mesh_key, position, scale);
        let subset_copy =
            ctx.mesh_registry()?.mesh_subsets(mesh_key).map(|subs| subs.to_vec()).unwrap_or_default();
        if let Some(material) = subset_copy.iter().find_map(|subset| subset.material.clone()) {
            ctx.ecs_mut()?.set_mesh_material(entity, Some(material));
        }
        self.mesh_status = Some(format!("Spawned mesh '{}' as entity {:?}", mesh_key, entity));
        Ok(Some(entity))
    }

    fn ensure_preview_assets(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        {
            let (mesh_registry, material_registry) = ctx.mesh_registry_and_materials()?;
            mesh_registry.retain_mesh(&self.preview_mesh_key, None, material_registry)?;
        }
        self.persistent_meshes.insert(self.preview_mesh_key.clone());
        let subset_copies = ctx
            .mesh_registry()?
            .mesh_subsets(&self.preview_mesh_key)
            .map(|subs| subs.to_vec())
            .unwrap_or_default();
        for subset in subset_copies {
            if let Some(material_key) = subset.material.as_ref() {
                ctx.material_registry_mut()?.retain(material_key)?;
                self.persistent_materials.insert(material_key.clone());
            }
        }
        Ok(())
    }

    fn handle_mesh_control_input(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        let mesh_toggle = { ctx.input_mut()?.take_mesh_toggle() };
        if mesh_toggle {
            let next = self.mesh_control_mode.next();
            self.set_mesh_control_mode(ctx, next)?;
        }
        let frustum_toggle = { ctx.input_mut()?.take_frustum_lock_toggle() };
        if frustum_toggle {
            let next = !self.mesh_frustum_lock;
            self.set_frustum_lock(ctx, next)?;
        }
        Ok(())
    }

    fn update_mesh_camera(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        match self.mesh_control_mode {
            MeshControlMode::Disabled => {
                self.mesh_freefly_velocity = Vec3::ZERO;
                self.mesh_freefly_rot_velocity = Vec3::ZERO;
                let auto_delta = Vec2::new(0.25 * dt, 0.12 * dt);
                self.mesh_orbit.orbit(auto_delta);
                self.mesh_camera =
                    self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
                self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
            }
            MeshControlMode::Orbit => {
                self.mesh_freefly_velocity = Vec3::ZERO;
                self.mesh_freefly_rot_velocity = Vec3::ZERO;
                let (dx, dy, right_held) = {
                    let input = ctx.input()?;
                    (input.mouse_delta.0, input.mouse_delta.1, input.right_mouse_held())
                };
                if right_held && (dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON) {
                    let sensitivity = 0.008;
                    self.mesh_orbit.orbit(Vec2::new(dx * sensitivity, dy * sensitivity));
                }
                let wheel = { ctx.input()?.wheel };
                if wheel.abs() > 0.0 && !self.mesh_frustum_lock {
                    let sensitivity = 0.12;
                    let factor = (wheel * sensitivity).exp();
                    self.mesh_orbit.zoom(factor);
                    ctx.input_mut()?.wheel = 0.0;
                }
                self.mesh_camera =
                    self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
                self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
            }
            MeshControlMode::Freefly => {
                let dt = dt.max(1e-6);
                #[derive(Clone, Copy)]
                struct FreeflySnapshot {
                    mouse_delta: (f32, f32),
                    right_held: bool,
                    roll_right: bool,
                    roll_left: bool,
                    forward: bool,
                    backward: bool,
                    right: bool,
                    left: bool,
                    up: bool,
                    down: bool,
                    boost: bool,
                    wheel: f32,
                }
                let snapshot = {
                    let input = ctx.input()?;
                    FreeflySnapshot {
                        mouse_delta: input.mouse_delta,
                        right_held: input.right_mouse_held(),
                        roll_right: input.freefly_roll_right(),
                        roll_left: input.freefly_roll_left(),
                        forward: input.freefly_forward(),
                        backward: input.freefly_backward(),
                        right: input.freefly_right(),
                        left: input.freefly_left(),
                        up: input.freefly_ascend(),
                        down: input.freefly_descend(),
                        boost: input.freefly_boost(),
                        wheel: input.wheel,
                    }
                };

                let mut target_rot = Vec3::ZERO;
                if snapshot.right_held {
                    let sensitivity = 0.008;
                    target_rot.x = snapshot.mouse_delta.0 * sensitivity / dt;
                    target_rot.y = snapshot.mouse_delta.1 * sensitivity / dt;
                }
                let roll_raw = (snapshot.roll_right as i32 - snapshot.roll_left as i32) as f32;
                if roll_raw.abs() > 0.0 {
                    target_rot.z = roll_raw * 2.5;
                }
                let angular_lerp = 1.0 - (-dt * 14.0).exp();
                self.mesh_freefly_rot_velocity =
                    self.mesh_freefly_rot_velocity.lerp(target_rot, angular_lerp);
                self.mesh_freefly.yaw += self.mesh_freefly_rot_velocity.x * dt;
                self.mesh_freefly.pitch = (self.mesh_freefly.pitch + self.mesh_freefly_rot_velocity.y * dt)
                    .clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
                self.mesh_freefly.roll += self.mesh_freefly_rot_velocity.z * dt;
                self.mesh_freefly.roll = wrap_angle(self.mesh_freefly.roll);

                let mut direction = Vec3::ZERO;
                let forward = self.mesh_freefly.forward().normalize_or_zero();
                let right = self.mesh_freefly.right().normalize_or_zero();
                let up = self.mesh_freefly.up().normalize_or_zero();

                if !self.mesh_frustum_lock {
                    if snapshot.forward {
                        direction += forward;
                    }
                    if snapshot.backward {
                        direction -= forward;
                    }
                    if snapshot.right {
                        direction += right;
                    }
                    if snapshot.left {
                        direction -= right;
                    }
                    if snapshot.up {
                        direction += up;
                    }
                    if snapshot.down {
                        direction -= up;
                    }
                }

                let boost = if snapshot.boost { 3.0 } else { 1.0 };
                let target_velocity = if direction.length_squared() > 0.0 {
                    direction.normalize_or_zero() * self.mesh_freefly_speed * boost
                } else {
                    Vec3::ZERO
                };
                let velocity_lerp = 1.0 - (-dt * 10.0).exp();
                self.mesh_freefly_velocity = self.mesh_freefly_velocity.lerp(target_velocity, velocity_lerp);
                self.mesh_freefly.position += self.mesh_freefly_velocity * dt;

                if !self.mesh_frustum_lock && snapshot.wheel.abs() > 0.0 {
                    let factor = (1.0 + snapshot.wheel * 0.06).clamp(0.2, 5.0);
                    self.mesh_freefly_speed = (self.mesh_freefly_speed * factor).clamp(0.1, 200.0);
                    self.mesh_status = Some(format!("Free-fly speed: {:.2}", self.mesh_freefly_speed));
                    ctx.input_mut()?.wheel = 0.0;
                }

                self.mesh_camera = self.mesh_freefly.to_camera();

                if self.mesh_frustum_lock {
                    let direction = (self.mesh_frustum_focus - self.mesh_camera.position).normalize_or_zero();
                    if direction.length_squared() > 0.0 {
                        self.mesh_camera.target =
                            self.mesh_camera.position + direction * self.mesh_frustum_distance;
                    }
                    self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
                } else {
                    self.mesh_orbit.target = self.mesh_camera.target;
                    self.mesh_orbit.radius =
                        (self.mesh_camera.target - self.mesh_camera.position).length().max(0.1);
                }
            }
        }

        if self.mesh_frustum_lock {
            let focus = self.compute_focus_point(ctx)?;
            if focus.length_squared() > 0.0 {
                self.mesh_frustum_focus = focus;
            }
            let direction = (self.mesh_frustum_focus - self.mesh_camera.position).normalize_or_zero();
            if direction.length_squared() > 0.0 {
                self.mesh_camera.target = self.mesh_camera.position + direction * self.mesh_frustum_distance;
                if self.mesh_control_mode == MeshControlMode::Freefly {
                    self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
                } else {
                    self.mesh_orbit.target = self.mesh_frustum_focus;
                    self.mesh_orbit.radius = self.mesh_frustum_distance.max(0.1);
                    self.mesh_camera =
                        self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
                    self.mesh_camera.target = focus;
                }
            }
        } else {
            match self.mesh_control_mode {
                MeshControlMode::Orbit | MeshControlMode::Disabled => {
                    self.mesh_frustum_focus = self.mesh_orbit.target;
                    self.mesh_frustum_distance = self.mesh_orbit.radius;
                }
                MeshControlMode::Freefly => {
                    self.mesh_frustum_focus = self.mesh_camera.target;
                    self.mesh_frustum_distance =
                        (self.mesh_frustum_focus - self.mesh_camera.position).length().max(0.1);
                }
            }
        }
        Ok(())
    }

    fn focus_mesh_center(&mut self, center: Vec3) {
        self.mesh_frustum_focus = center;
        self.mesh_frustum_distance = (self.mesh_camera.position - center).length().max(0.1);
        self.mesh_orbit.target = center;
        self.mesh_orbit.radius = self.mesh_frustum_distance;
        self.mesh_camera =
            self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
        self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
        self.mesh_status = Some("Framed selection in 3D viewport.".to_string());
    }

    fn compute_focus_point(&self, ctx: &PluginContext<'_>) -> Result<Vec3> {
        if let Some(entity) = ctx.selected_entity() {
            if let Some(info) = ctx.ecs()?.entity_info(entity) {
                if let Some(mesh_tx) = info.mesh_transform {
                    return Ok(mesh_tx.translation);
                }
                return Ok(Vec3::new(info.translation.x, info.translation.y, 0.0));
            }
        }
        Ok(self.mesh_orbit.target)
    }

    fn sync_orbit_from_camera_pose(&mut self) {
        let target = self.mesh_orbit.target;
        let mut offset = self.mesh_camera.position - target;
        if offset.length_squared() < 1e-5 {
            offset = Vec3::new(0.0, 0.0, self.mesh_orbit.radius.max(0.1));
        }
        let radius = offset.length().max(0.1);
        let yaw = offset.x.atan2(offset.z);
        let pitch = (offset.y / radius).clamp(-1.0, 1.0).asin();
        self.mesh_orbit.radius = radius;
        self.mesh_orbit.yaw_radians = yaw;
        self.mesh_orbit.pitch_radians =
            pitch.clamp(-std::f32::consts::FRAC_PI_2 + 0.01, std::f32::consts::FRAC_PI_2 - 0.01);
    }
}

impl EnginePlugin for MeshPreviewPlugin {
    fn name(&self) -> &'static str {
        "mesh_preview"
    }

    fn version(&self) -> &'static str {
        "1.0.0"
    }

    fn build(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        if self.preview_mesh_key.is_empty() {
            self.preview_mesh_key = ctx.mesh_registry()?.default_key().to_string();
        }
        self.mesh_orbit = OrbitCamera::new(Vec3::ZERO, 5.0);
        self.mesh_camera =
            self.mesh_orbit.to_camera(MESH_CAMERA_FOV_RADIANS, MESH_CAMERA_NEAR, MESH_CAMERA_FAR);
        self.mesh_frustum_focus = self.mesh_orbit.target;
        self.mesh_frustum_distance = self.mesh_orbit.radius;
        self.mesh_freefly = FreeflyController::from_camera(&self.mesh_camera);
        self.mesh_angle = 0.0;
        self.mesh_model = Mat4::IDENTITY;
        self.mesh_status =
            Some(format!("Preview mesh: {} - press M to cycle camera control", self.preview_mesh_key));
        self.ensure_preview_assets(ctx)?;
        Ok(())
    }

    fn update(&mut self, ctx: &mut PluginContext<'_>, dt: f32) -> Result<()> {
        self.mesh_angle = (self.mesh_angle + dt * 0.5) % (std::f32::consts::TAU);
        self.mesh_model = Mat4::from_rotation_y(self.mesh_angle);
        self.handle_mesh_control_input(ctx)?;
        self.update_mesh_camera(ctx, dt)?;
        Ok(())
    }

    fn shutdown(&mut self, ctx: &mut PluginContext<'_>) -> Result<()> {
        for mesh in std::mem::take(&mut self.persistent_meshes) {
            ctx.mesh_registry_mut()?.release_mesh(&mesh);
        }
        for material in std::mem::take(&mut self.persistent_materials) {
            ctx.material_registry_mut()?.release(&material);
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
