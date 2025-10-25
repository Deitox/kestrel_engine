use super::{App, ViewportCameraMode};
use crate::ecs::EntityInfo;
use crate::gizmo;
use crate::gizmo::{
    Axis2, GizmoInteraction, GizmoMode, ScaleHandle, ScaleHandleKind, GIZMO_ROTATE_INNER_RADIUS_PX,
    GIZMO_ROTATE_OUTER_RADIUS_PX, GIZMO_SCALE_OUTER_RADIUS_PX, GIZMO_TRANSLATE_RADIUS_PX,
    ROTATE_SNAP_STEP_RADIANS, TRANSLATE_SNAP_STEP,
};
use crate::mesh_preview::MeshControlMode;
use crate::wrap_angle;

use glam::{EulerRot, Quat, Vec2, Vec3};
use winit::dpi::PhysicalSize;

pub(crate) struct GizmoUpdate {
    pub hovered_scale_kind: Option<ScaleHandleKind>,
}

impl App {
    pub(crate) fn update_gizmo_interactions(
        &mut self,
        viewport_size: PhysicalSize<u32>,
        cursor_world_2d: Option<Vec2>,
        cursor_viewport: Option<Vec2>,
        cursor_ray: Option<(Vec3, Vec3)>,
        cursor_in_viewport: bool,
        mesh_center_world: Option<Vec3>,
        gizmo_center_viewport: Option<Vec2>,
        selected_info: &Option<EntityInfo>,
    ) -> GizmoUpdate {
        let mesh_control_mode = self
            .mesh_preview_plugin()
            .map(|plugin| plugin.mesh_control_mode())
            .unwrap_or(MeshControlMode::Disabled);
        if self.viewport_camera_mode == ViewportCameraMode::Ortho2D
            && mesh_control_mode == MeshControlMode::Disabled
        {
            if let Some(delta) = self.input.consume_wheel_delta() {
                self.camera.apply_scroll_zoom(delta);
            }

            if self.input.right_held() {
                let (dx, dy) = self.input.mouse_delta;
                if dx.abs() > f32::EPSILON || dy.abs() > f32::EPSILON {
                    self.camera.pan_screen_delta(Vec2::new(dx, dy), viewport_size);
                }
            }
        }

        let shift_held = self.input.shift_held();
        let hovered_scale_kind = if self.viewport_camera_mode == ViewportCameraMode::Ortho2D
            && self.gizmo_mode == GizmoMode::Scale
        {
            if let (Some(info), Some(center_viewport), Some(pointer_viewport), Some(pointer_world)) =
                (selected_info.as_ref(), gizmo_center_viewport, cursor_viewport, cursor_world_2d)
            {
                gizmo::detect_scale_handle(
                    pointer_world,
                    pointer_viewport,
                    info.translation,
                    center_viewport,
                    shift_held,
                )
                .map(|(kind, _)| kind)
            } else {
                None
            }
        } else {
            None
        };

        let mut gizmo_click_consumed = false;
        if self.input.take_left_click() {
            if let Some(entity) = self.selected_entity {
                match self.viewport_camera_mode {
                    ViewportCameraMode::Perspective3D => {
                        if let Some(center_world) = mesh_center_world {
                            match self.gizmo_mode {
                                GizmoMode::Translate => {
                                    if let Some((ray_origin, ray_dir)) = cursor_ray {
                                        let plane_normal = self.mesh_camera_forward();
                                        if plane_normal.length_squared() > f32::EPSILON {
                                            if let Some(hit) = App::intersect_ray_plane(
                                                ray_origin,
                                                ray_dir,
                                                center_world,
                                                plane_normal,
                                            ) {
                                                let offset = center_world - hit;
                                                self.gizmo_interaction =
                                                    Some(GizmoInteraction::Translate3D {
                                                        entity,
                                                        offset,
                                                        plane_origin: center_world,
                                                        plane_normal,
                                                    });
                                                gizmo_click_consumed = true;
                                                self.inspector_status = None;
                                            }
                                        }
                                    }
                                }
                                GizmoMode::Scale => {
                                    if let (
                                        Some(center_viewport),
                                        Some(pointer_viewport),
                                        Some((ray_origin, ray_dir)),
                                    ) = (gizmo_center_viewport, cursor_viewport, cursor_ray)
                                    {
                                        let dist = pointer_viewport.distance(center_viewport);
                                        if dist <= GIZMO_SCALE_OUTER_RADIUS_PX {
                                            let plane_normal = self.mesh_camera_forward();
                                            if let Some(hit) = App::intersect_ray_plane(
                                                ray_origin,
                                                ray_dir,
                                                center_world,
                                                plane_normal,
                                            ) {
                                                let start_vec = hit - center_world;
                                                let start_distance = start_vec.length();
                                                if start_distance > f32::EPSILON {
                                                    let start_scale = selected_info
                                                        .as_ref()
                                                        .and_then(|info| {
                                                            info.mesh_transform.as_ref().map(|tx| tx.scale)
                                                        })
                                                        .unwrap_or(Vec3::splat(1.0));
                                                    self.gizmo_interaction =
                                                        Some(GizmoInteraction::Scale3D {
                                                            entity,
                                                            start_scale,
                                                            start_distance,
                                                            plane_normal,
                                                        });
                                                    gizmo_click_consumed = true;
                                                    self.inspector_status = None;
                                                }
                                            }
                                        }
                                    }
                                }
                                GizmoMode::Rotate => {
                                    if let (
                                        Some(center_viewport),
                                        Some(pointer_viewport),
                                        Some((ray_origin, ray_dir)),
                                    ) = (gizmo_center_viewport, cursor_viewport, cursor_ray)
                                    {
                                        let dist = pointer_viewport.distance(center_viewport);
                                        if dist >= GIZMO_ROTATE_INNER_RADIUS_PX
                                            && dist <= GIZMO_ROTATE_OUTER_RADIUS_PX
                                        {
                                            let plane_normal = self.mesh_camera_forward();
                                            if let Some(hit) = App::intersect_ray_plane(
                                                ray_origin,
                                                ray_dir,
                                                center_world,
                                                plane_normal,
                                            ) {
                                                let start_vec = hit - center_world;
                                                if start_vec.length_squared() > f32::EPSILON {
                                                    let start_rotation = selected_info
                                                        .as_ref()
                                                        .and_then(|info| {
                                                            info.mesh_transform.as_ref().map(|tx| tx.rotation)
                                                        })
                                                        .unwrap_or(Quat::IDENTITY);
                                                    self.gizmo_interaction =
                                                        Some(GizmoInteraction::Rotate3D {
                                                            entity,
                                                            axis: plane_normal.normalize_or_zero(),
                                                            start_rotation,
                                                            start_vector: start_vec,
                                                        });
                                                    gizmo_click_consumed = true;
                                                    self.inspector_status = None;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    ViewportCameraMode::Ortho2D => match self.gizmo_mode {
                        GizmoMode::Translate => {
                            if let (Some(center_viewport), Some(pointer_viewport)) =
                                (gizmo_center_viewport, cursor_viewport)
                            {
                                let dist = pointer_viewport.distance(center_viewport);
                                if dist <= GIZMO_TRANSLATE_RADIUS_PX {
                                    if let Some(pointer_world) = cursor_world_2d {
                                        let offset = selected_info
                                            .as_ref()
                                            .map(|info| info.translation - pointer_world)
                                            .unwrap_or(Vec2::ZERO);
                                        if let Some(info) = selected_info.as_ref() {
                                            self.gizmo_interaction = Some(GizmoInteraction::Translate {
                                                entity,
                                                offset,
                                                start_translation: info.translation,
                                                start_pointer: pointer_world,
                                                axis_lock: None,
                                            });
                                        }
                                        gizmo_click_consumed = true;
                                        self.inspector_status = None;
                                    }
                                }
                            }
                        }
                        GizmoMode::Scale => {
                            if let (
                                Some(pointer_world),
                                Some(pointer_viewport),
                                Some(info),
                                Some(center_viewport),
                            ) = (
                                cursor_world_2d,
                                cursor_viewport,
                                selected_info.as_ref(),
                                gizmo_center_viewport,
                            ) {
                                if let Some((_kind, handle)) = gizmo::detect_scale_handle(
                                    pointer_world,
                                    pointer_viewport,
                                    info.translation,
                                    center_viewport,
                                    shift_held,
                                ) {
                                    self.gizmo_interaction = Some(GizmoInteraction::Scale {
                                        entity,
                                        start_scale: info.scale,
                                        handle,
                                    });
                                    gizmo_click_consumed = true;
                                    self.inspector_status = None;
                                }
                            }
                        }
                        GizmoMode::Rotate => {
                            if let (Some(center_viewport), Some(pointer_viewport)) =
                                (gizmo_center_viewport, cursor_viewport)
                            {
                                let dist = pointer_viewport.distance(center_viewport);
                                if dist >= GIZMO_ROTATE_INNER_RADIUS_PX
                                    && dist <= GIZMO_ROTATE_OUTER_RADIUS_PX
                                {
                                    if let (Some(pointer_world), Some(info)) =
                                        (cursor_world_2d, selected_info.as_ref())
                                    {
                                        let center = info.translation;
                                        let vec = pointer_world - center;
                                        if vec.length_squared() > f32::EPSILON {
                                            let start_angle = vec.y.atan2(vec.x);
                                            self.gizmo_interaction = Some(GizmoInteraction::Rotate {
                                                entity,
                                                start_rotation: info.rotation,
                                                start_angle,
                                            });
                                            gizmo_click_consumed = true;
                                            self.inspector_status = None;
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }

            if !gizmo_click_consumed {
                match self.viewport_camera_mode {
                    ViewportCameraMode::Perspective3D => {
                        if let Some((ray_origin, ray_dir)) = cursor_ray {
                            let mut picked =
                                self.ecs.pick_entity_3d(ray_origin, ray_dir, &self.mesh_registry);
                            if picked.is_none() {
                                if let Some(hit) =
                                    App::intersect_ray_plane(ray_origin, ray_dir, Vec3::ZERO, Vec3::Z)
                                {
                                    picked = self.ecs.pick_entity(hit.truncate());
                                }
                            }
                            self.selected_entity = picked;
                            if self.selected_entity.is_some() {
                                self.inspector_status = None;
                            }
                        } else if cursor_in_viewport {
                            self.selected_entity = None;
                            self.inspector_status = None;
                        }
                    }
                    ViewportCameraMode::Ortho2D => {
                        if let Some(world) = cursor_world_2d {
                            self.selected_entity = self.ecs.pick_entity(world);
                            self.inspector_status = None;
                        } else if cursor_in_viewport {
                            self.selected_entity = None;
                            self.inspector_status = None;
                        }
                    }
                }
                if cursor_in_viewport {
                    self.gizmo_interaction = None;
                }
            }
        }

        if self.selected_entity.is_none() {
            self.gizmo_interaction = None;
        }

        if let Some(interaction) = self.gizmo_interaction.as_mut() {
            let mut keep_active = true;
            match interaction {
                GizmoInteraction::Translate {
                    entity,
                    offset,
                    start_translation,
                    start_pointer,
                    axis_lock,
                } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some(pointer_world) = cursor_world_2d {
                        if self.ecs.entity_exists(*entity) {
                            let mut current_axis = None;
                            if self.input.shift_held() {
                                let delta = pointer_world - *start_pointer;
                                if delta.length_squared() > f32::EPSILON {
                                    current_axis = Some(if delta.x.abs() >= delta.y.abs() {
                                        Axis2::X
                                    } else {
                                        Axis2::Y
                                    });
                                }
                            }
                            *axis_lock = current_axis;
                            let mut translation = if let Some(axis) = current_axis {
                                let delta = pointer_world - *start_pointer;
                                let mut result = *start_translation;
                                match axis {
                                    Axis2::X => result.x += delta.x,
                                    Axis2::Y => result.y += delta.y,
                                }
                                result
                            } else {
                                pointer_world + *offset
                            };
                            if self.input.ctrl_held() {
                                match current_axis {
                                    Some(Axis2::X) => {
                                        translation.x = (translation.x / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                    }
                                    Some(Axis2::Y) => {
                                        translation.y = (translation.y / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                    }
                                    None => {
                                        translation.x = (translation.x / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                        translation.y = (translation.y / TRANSLATE_SNAP_STEP).round()
                                            * TRANSLATE_SNAP_STEP;
                                    }
                                }
                            }
                            self.ecs.set_translation(*entity, translation);
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Translate3D { entity, offset, plane_origin, plane_normal } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some((ray_origin, ray_dir)) = cursor_ray {
                        if let Some(hit) =
                            App::intersect_ray_plane(ray_origin, ray_dir, *plane_origin, *plane_normal)
                        {
                            if self.ecs.entity_exists(*entity) {
                                let mut translation = hit + *offset;
                                if self.input.ctrl_held() {
                                    translation.x =
                                        (translation.x / TRANSLATE_SNAP_STEP).round() * TRANSLATE_SNAP_STEP;
                                    translation.y =
                                        (translation.y / TRANSLATE_SNAP_STEP).round() * TRANSLATE_SNAP_STEP;
                                    translation.z =
                                        (translation.z / TRANSLATE_SNAP_STEP).round() * TRANSLATE_SNAP_STEP;
                                }
                                self.ecs.set_mesh_translation(*entity, translation);
                                self.ecs.set_translation(*entity, translation.truncate());
                            } else {
                                keep_active = false;
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Rotate { entity, start_rotation, start_angle } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some(pointer_world) = cursor_world_2d {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let vec = pointer_world - info.translation;
                            if vec.length_squared() > f32::EPSILON {
                                let current_angle = vec.y.atan2(vec.x);
                                let mut delta = wrap_angle(current_angle - *start_angle);
                                if self.input.ctrl_held() {
                                    delta =
                                        (delta / ROTATE_SNAP_STEP_RADIANS).round() * ROTATE_SNAP_STEP_RADIANS;
                                }
                                self.ecs.set_rotation(*entity, *start_rotation + delta);
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Rotate3D { entity, axis, start_rotation, start_vector } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some((ray_origin, ray_dir)) = cursor_ray {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let center = info
                                .mesh_transform
                                .as_ref()
                                .map(|tx| tx.translation)
                                .unwrap_or(Vec3::new(info.translation.x, info.translation.y, 0.0));
                            if let Some(hit) = App::intersect_ray_plane(ray_origin, ray_dir, center, *axis) {
                                let start_vec = start_vector.normalize_or_zero();
                                let current_vec = (hit - center).normalize_or_zero();
                                if start_vec.length_squared() > f32::EPSILON
                                    && current_vec.length_squared() > f32::EPSILON
                                {
                                    let axis_norm = axis.normalize_or_zero();
                                    if axis_norm.length_squared() > f32::EPSILON {
                                        let dot = start_vec.dot(current_vec).clamp(-1.0, 1.0);
                                        let cross = start_vec.cross(current_vec);
                                        let sin = cross.dot(axis_norm);
                                        let mut delta = sin.atan2(dot);
                                        if self.input.ctrl_held() {
                                            delta = (delta / ROTATE_SNAP_STEP_RADIANS).round()
                                                * ROTATE_SNAP_STEP_RADIANS;
                                        }
                                        if delta.abs() > f32::EPSILON {
                                            let quat =
                                                Quat::from_axis_angle(axis_norm, delta) * *start_rotation;
                                            let (x, y, z) = quat.to_euler(EulerRot::XYZ);
                                            self.ecs.set_mesh_rotation_euler(*entity, Vec3::new(x, y, z));
                                        }
                                    }
                                }
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Scale { entity, start_scale, handle } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some(pointer_world) = cursor_world_2d {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let center = info.translation;
                            let mut new_scale = *start_scale;
                            let snap = self.input.ctrl_held();
                            match handle {
                                ScaleHandle::Uniform { start_distance } => {
                                    let delta = pointer_world - center;
                                    let len_sq = delta.length_squared();
                                    if len_sq > f32::EPSILON && *start_distance > f32::EPSILON {
                                        let distance = len_sq.sqrt();
                                        let ratio =
                                            gizmo::apply_scale_ratio(distance / *start_distance, snap);
                                        new_scale = Vec2::new(
                                            (start_scale.x * ratio).max(0.01),
                                            (start_scale.y * ratio).max(0.01),
                                        );
                                    }
                                }
                                ScaleHandle::Axis { axis, start_extent } => {
                                    let axis_vec = axis.vector();
                                    let extent = (pointer_world - center).dot(axis_vec).abs();
                                    if extent > f32::EPSILON && *start_extent > f32::EPSILON {
                                        let ratio = gizmo::apply_scale_ratio(extent / *start_extent, snap);
                                        match axis {
                                            Axis2::X => {
                                                new_scale.x = (start_scale.x * ratio).max(0.01);
                                                if self.input.shift_held() {
                                                    new_scale.y = (start_scale.y * ratio).max(0.01);
                                                }
                                            }
                                            Axis2::Y => {
                                                new_scale.y = (start_scale.y * ratio).max(0.01);
                                                if self.input.shift_held() {
                                                    new_scale.x = (start_scale.x * ratio).max(0.01);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            if new_scale != *start_scale {
                                self.ecs.set_scale(*entity, new_scale);
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
                GizmoInteraction::Scale3D { entity, start_scale, start_distance, plane_normal } => {
                    if !self.input.left_held() {
                        keep_active = false;
                    } else if let Some((ray_origin, ray_dir)) = cursor_ray {
                        if let Some(info) = self.ecs.entity_info(*entity) {
                            let center = info
                                .mesh_transform
                                .as_ref()
                                .map(|tx| tx.translation)
                                .unwrap_or(Vec3::new(info.translation.x, info.translation.y, 0.0));
                            if let Some(hit) =
                                App::intersect_ray_plane(ray_origin, ray_dir, center, *plane_normal)
                            {
                                let distance = (hit - center).length();
                                if distance > f32::EPSILON && *start_distance > f32::EPSILON {
                                    let ratio = gizmo::apply_scale_ratio(
                                        distance / *start_distance,
                                        self.input.ctrl_held(),
                                    );
                                    let mut new_scale = Vec3::new(
                                        (start_scale.x * ratio).max(0.01),
                                        (start_scale.y * ratio).max(0.01),
                                        (start_scale.z * ratio).max(0.01),
                                    );
                                    if self.input.shift_held() {
                                        let uniform = new_scale.x.max(new_scale.y).max(new_scale.z);
                                        new_scale = Vec3::splat(uniform);
                                    }
                                    if (new_scale - *start_scale).length_squared() > f32::EPSILON {
                                        self.ecs.set_mesh_scale(*entity, new_scale);
                                        self.ecs.set_scale(*entity, Vec2::new(new_scale.x, new_scale.y));
                                    }
                                }
                            }
                        } else {
                            keep_active = false;
                        }
                    } else {
                        keep_active = false;
                    }
                }
            }
            if !keep_active {
                self.gizmo_interaction = None;
            }
        }
        GizmoUpdate { hovered_scale_kind }
    }
}
