use super::{Children, Parent, Transform, Transform3D, WorldTransform, WorldTransform3D};
use crate::ecs::profiler::SystemProfiler;
use bevy_ecs::prelude::*;
use glam::Mat4;
use smallvec::SmallVec;
use std::collections::HashSet;

#[derive(Resource, Default)]
pub struct TransformPropagationScratch {
    pub stack: SmallVec<[(Entity, Mat4); 128]>,
    pub visited: HashSet<Entity>,
}

pub fn sys_propagate_scene_transforms(
    mut profiler: ResMut<SystemProfiler>,
    mut nodes: Query<(
        Entity,
        Option<&Transform>,
        Option<&Transform3D>,
        Option<&Children>,
        &mut WorldTransform,
    )>,
    roots: Query<Entity, (With<WorldTransform>, Without<Parent>)>,
    mut scratch: ResMut<TransformPropagationScratch>,
) {
    let _span = profiler.scope("sys_propagate_scene_transforms");
    fn compose_local(transform2d: Option<&Transform>, transform3d: Option<&Transform3D>) -> Mat4 {
        if let Some(t3d) = transform3d {
            Mat4::from_scale_rotation_translation(t3d.scale, t3d.rotation, t3d.translation)
        } else if let Some(t2d) = transform2d {
            mat_from_transform(*t2d)
        } else {
            Mat4::IDENTITY
        }
    }

    let mut stack = std::mem::take(&mut scratch.stack);
    let mut visited = std::mem::take(&mut scratch.visited);

    stack.clear();
    visited.clear();

    for root in roots.iter() {
        if let Ok((entity, transform2d, transform3d, children, mut world)) = nodes.get_mut(root) {
            let local = compose_local(transform2d, transform3d);
            world.0 = local;
            visited.insert(entity);
            let world_mat = world.0;
            if let Some(children) = children {
                for &child in children.0.iter().rev() {
                    stack.push((child, world_mat));
                }
            }
        }
    }

    while let Some((entity, parent_world)) = stack.pop() {
        if let Ok((current, transform2d, transform3d, children, mut world)) = nodes.get_mut(entity) {
            if visited.contains(&current) {
                continue;
            }
            let local = compose_local(transform2d, transform3d);
            let world_mat = parent_world * local;
            world.0 = world_mat;
            visited.insert(current);
            if let Some(children) = children {
                for &child in children.0.iter().rev() {
                    stack.push((child, world_mat));
                }
            }
        }
    }

    let visited_ref = &visited;
    for (entity, transform2d, transform3d, _, mut world) in nodes.iter_mut() {
        if !visited_ref.contains(&entity) {
            world.0 = compose_local(transform2d, transform3d);
        }
    }

    scratch.stack = stack;
    scratch.visited = visited;
}

pub fn sys_sync_world3d(
    mut profiler: ResMut<SystemProfiler>,
    mut query: Query<(&WorldTransform, &mut WorldTransform3D)>,
) {
    let _span = profiler.scope("sys_sync_world3d");
    for (world, mut world3d) in &mut query {
        world3d.0 = world.0;
    }
}

fn mat_from_transform(t: Transform) -> Mat4 {
    let (sx, sy) = (t.scale.x, t.scale.y);
    let (s, c) = t.rotation.sin_cos();
    Mat4::from_cols_array(&[
        c * sx,
        s * sx,
        0.0,
        0.0,
        -s * sy,
        c * sy,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
        t.translation.x,
        t.translation.y,
        0.0,
        1.0,
    ])
}
