use bevy_ecs::prelude::Entity;
use glam::{Quat, Vec2, Vec3};

pub(crate) const GIZMO_TRANSLATE_RADIUS_PX: f32 = 18.0;
pub(crate) const GIZMO_SCALE_INNER_RADIUS_PX: f32 = 20.0;
pub(crate) const GIZMO_SCALE_OUTER_RADIUS_PX: f32 = 32.0;
pub(crate) const GIZMO_SCALE_AXIS_LENGTH_PX: f32 = 44.0;
pub(crate) const GIZMO_SCALE_AXIS_THICKNESS_PX: f32 = 8.0;
pub(crate) const GIZMO_SCALE_AXIS_DEADZONE_PX: f32 = 10.0;
pub(crate) const GIZMO_SCALE_HANDLE_SIZE_PX: f32 = 12.0;
pub(crate) const GIZMO_ROTATE_INNER_RADIUS_PX: f32 = 38.0;
pub(crate) const GIZMO_ROTATE_OUTER_RADIUS_PX: f32 = 52.0;
pub(crate) const SCALE_MIN_RATIO: f32 = 0.05;
pub(crate) const SCALE_MAX_RATIO: f32 = 20.0;
pub(crate) const SCALE_SNAP_STEP: f32 = 0.1;
pub(crate) const TRANSLATE_SNAP_STEP: f32 = 0.05;
pub(crate) const ROTATE_SNAP_STEP_RADIANS: f32 = 15.0_f32.to_radians();

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GizmoMode {
    Translate,
    Scale,
    Rotate,
}

impl Default for GizmoMode {
    fn default() -> Self {
        GizmoMode::Translate
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum GizmoInteraction {
    Translate {
        entity: Entity,
        offset: Vec2,
        start_translation: Vec2,
        start_pointer: Vec2,
        axis_lock: Option<Axis2>,
    },
    Translate3D {
        entity: Entity,
        offset: Vec3,
        plane_origin: Vec3,
        plane_normal: Vec3,
    },
    Rotate {
        entity: Entity,
        start_rotation: f32,
        start_angle: f32,
    },
    Rotate3D {
        entity: Entity,
        axis: Vec3,
        start_rotation: Quat,
        start_vector: Vec3,
    },
    Scale {
        entity: Entity,
        start_scale: Vec2,
        handle: ScaleHandle,
    },
    Scale3D {
        entity: Entity,
        start_scale: Vec3,
        start_distance: f32,
        plane_normal: Vec3,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Axis2 {
    X,
    Y,
}

impl Axis2 {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Axis2::X => "X axis",
            Axis2::Y => "Y axis",
        }
    }

    pub(crate) fn vector(self) -> Vec2 {
        match self {
            Axis2::X => Vec2::X,
            Axis2::Y => Vec2::Y,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ScaleHandle {
    Uniform { start_distance: f32 },
    Axis { axis: Axis2, start_extent: f32 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScaleHandleKind {
    Uniform,
    Axis(Axis2),
}

impl ScaleHandle {
    pub(crate) fn kind(self) -> ScaleHandleKind {
        match self {
            ScaleHandle::Uniform { .. } => ScaleHandleKind::Uniform,
            ScaleHandle::Axis { axis, .. } => ScaleHandleKind::Axis(axis),
        }
    }
}

pub(crate) fn apply_scale_ratio(ratio: f32, snap: bool) -> f32 {
    let clamped = ratio.clamp(SCALE_MIN_RATIO, SCALE_MAX_RATIO);
    if snap {
        let snapped = (clamped / SCALE_SNAP_STEP).round() * SCALE_SNAP_STEP;
        snapped.clamp(SCALE_MIN_RATIO, SCALE_MAX_RATIO)
    } else {
        clamped
    }
}

pub(crate) fn detect_scale_handle(
    pointer_world: Vec2,
    pointer_viewport: Vec2,
    center_world: Vec2,
    center_viewport: Vec2,
    shift: bool,
) -> Option<(ScaleHandleKind, ScaleHandle)> {
    let rel_view = pointer_viewport - center_viewport;
    let dist = pointer_viewport.distance(center_viewport);
    let axis_half = GIZMO_SCALE_AXIS_THICKNESS_PX * 0.5;
    let axis_length = GIZMO_SCALE_AXIS_LENGTH_PX;
    let deadzone = GIZMO_SCALE_AXIS_DEADZONE_PX;
    let mut kind = None;
    if rel_view.x.abs() >= deadzone && rel_view.x.abs() <= axis_length && rel_view.y.abs() <= axis_half {
        kind = Some(if shift { ScaleHandleKind::Uniform } else { ScaleHandleKind::Axis(Axis2::X) });
    } else if rel_view.y.abs() >= deadzone && rel_view.y.abs() <= axis_length && rel_view.x.abs() <= axis_half
    {
        kind = Some(if shift { ScaleHandleKind::Uniform } else { ScaleHandleKind::Axis(Axis2::Y) });
    } else if dist >= GIZMO_SCALE_INNER_RADIUS_PX && dist <= GIZMO_SCALE_OUTER_RADIUS_PX {
        kind = Some(ScaleHandleKind::Uniform);
    }
    let kind = kind?;
    let delta_world = pointer_world - center_world;
    match kind {
        ScaleHandleKind::Uniform => {
            let distance = delta_world.length();
            if distance > f32::EPSILON {
                Some((kind, ScaleHandle::Uniform { start_distance: distance }))
            } else {
                None
            }
        }
        ScaleHandleKind::Axis(axis) => {
            let extent = delta_world.dot(axis.vector()).abs();
            if extent > f32::EPSILON {
                Some((kind, ScaleHandle::Axis { axis, start_extent: extent }))
            } else {
                None
            }
        }
    }
}
