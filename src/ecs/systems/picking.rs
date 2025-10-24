use crate::ecs::types::Transform3D;
use crate::mesh::MeshBounds;
use glam::{Mat4, Vec3};

pub fn ray_sphere_intersection(origin: Vec3, dir: Vec3, center: Vec3, radius: f32) -> Option<f32> {
    let oc = origin - center;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let discriminant = b * b - c;
    if discriminant < 0.0 {
        return None;
    }
    let sqrt_d = discriminant.sqrt();
    let mut t = -b - sqrt_d;
    if t < 0.0 {
        t = -b + sqrt_d;
    }
    if t < 0.0 {
        return None;
    }
    Some(t)
}

pub fn ray_hit_obb(origin: Vec3, dir: Vec3, transform: &Transform3D, bounds: &MeshBounds) -> Option<f32> {
    if !transform.scale.is_finite() {
        return None;
    }
    let min_scale = 0.0001;
    let scale = Vec3::new(
        transform.scale.x.abs().max(min_scale),
        transform.scale.y.abs().max(min_scale),
        transform.scale.z.abs().max(min_scale),
    );
    let world = Mat4::from_scale_rotation_translation(scale, transform.rotation, transform.translation);
    let inv = world.inverse();
    if !matrix_is_finite(&inv) {
        return None;
    }
    let origin_local = inv.transform_point3(origin);
    let dir_local = inv.transform_vector3(dir);
    if dir_local.length_squared() <= f32::EPSILON {
        return None;
    }
    let dir_local = dir_local.normalize();
    let (t_local, hit_local) = ray_aabb_intersection(origin_local, dir_local, bounds.min, bounds.max)?;
    if t_local < 0.0 {
        return None;
    }
    let hit_world = world.transform_point3(hit_local);
    let distance = (hit_world - origin).length();
    Some(distance)
}

pub fn matrix_is_finite(mat: &Mat4) -> bool {
    mat.to_cols_array().iter().all(|v| v.is_finite())
}

pub fn ray_aabb_intersection(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<(f32, Vec3)> {
    let mut t_min: f32 = 0.0;
    let mut t_max: f32 = f32::INFINITY;
    let origin_arr = origin.to_array();
    let dir_arr = dir.to_array();
    let min_arr = min.to_array();
    let max_arr = max.to_array();
    for i in 0..3 {
        let o = origin_arr[i];
        let d = dir_arr[i];
        let min_axis = min_arr[i];
        let max_axis = max_arr[i];
        if d.abs() < 1e-6 {
            if o < min_axis || o > max_axis {
                return None;
            }
        } else {
            let inv_d = 1.0 / d;
            let mut t1 = (min_axis - o) * inv_d;
            let mut t2 = (max_axis - o) * inv_d;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            t_min = t_min.max(t1);
            t_max = t_max.min(t2);
            if t_min > t_max {
                return None;
            }
        }
    }
    if t_max < 0.0 {
        return None;
    }
    let t_hit = if t_min >= 0.0 { t_min } else { t_max };
    let hit = origin + dir * t_hit;
    Some((t_hit, hit))
}
