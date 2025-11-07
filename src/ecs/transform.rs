use super::{Children, Parent, Transform, Transform3D, WorldTransform, WorldTransform3D};
use crate::ecs::profiler::SystemProfiler;
use bevy_ecs::prelude::*;
use glam::Mat4;
use smallvec::SmallVec;

#[derive(Resource, Default)]
pub struct TransformPropagationScratch {
    pub stack: SmallVec<[(Entity, Mat4); 128]>,
    pub visited: VisitTracker,
}

#[derive(Resource, Clone, Copy)]
pub struct TransformPropagationStats {
    pub mode: TransformPropagationMode,
    pub total_entities: u32,
    pub root_entities: u32,
    pub processed_entities: u32,
    pub max_stack_size: u32,
}

impl Default for TransformPropagationStats {
    fn default() -> Self {
        Self {
            mode: TransformPropagationMode::Flat,
            total_entities: 0,
            root_entities: 0,
            processed_entities: 0,
            max_stack_size: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum TransformPropagationMode {
    Flat,
    Hierarchy,
}

impl Default for TransformPropagationMode {
    fn default() -> Self {
        Self::Flat
    }
}

#[derive(Default)]
pub struct VisitTracker {
    marks: Vec<u32>,
    generation: u32,
}

impl VisitTracker {
    fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.marks.fill(0);
            self.generation = 1;
        }
    }

    fn mark(&mut self, entity: Entity) {
        let index = entity.index() as usize;
        if index >= self.marks.len() {
            self.marks.resize(index + 1, 0);
        }
        self.marks[index] = self.generation;
    }

    fn is_marked(&self, entity: Entity) -> bool {
        let index = entity.index() as usize;
        index < self.marks.len() && self.marks[index] == self.generation
    }
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
    parents: Query<(), With<Parent>>,
    mut scratch: ResMut<TransformPropagationScratch>,
    mut stats: ResMut<TransformPropagationStats>,
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

    if parents.is_empty() {
        let mut total = 0u32;
        for (_entity, transform2d, transform3d, _children, mut world) in nodes.iter_mut() {
            world.0 = compose_local(transform2d, transform3d);
            total += 1;
        }
        stats.mode = TransformPropagationMode::Flat;
        stats.total_entities = total;
        stats.root_entities = total;
        stats.processed_entities = total;
        stats.max_stack_size = 0;
        scratch.stack.clear();
        scratch.visited.clear();
        return;
    }

    let mut stack = std::mem::take(&mut scratch.stack);
    let mut visited = std::mem::take(&mut scratch.visited);

    stack.clear();
    visited.clear();
    let mut processed = 0u32;
    let mut root_count = 0u32;
    let mut max_stack = 0usize;

    for root in roots.iter() {
        if let Ok((entity, transform2d, transform3d, children, mut world)) = nodes.get_mut(root) {
            let local = compose_local(transform2d, transform3d);
            world.0 = local;
            visited.mark(entity);
            processed += 1;
            root_count += 1;
            let world_mat = world.0;
            if let Some(children) = children {
                for &child in children.0.iter().rev() {
                    stack.push((child, world_mat));
                    max_stack = max_stack.max(stack.len());
                }
            }
        }
    }

    while let Some((entity, parent_world)) = stack.pop() {
        if let Ok((current, transform2d, transform3d, children, mut world)) = nodes.get_mut(entity) {
            if visited.is_marked(current) {
                continue;
            }
            let local = compose_local(transform2d, transform3d);
            let world_mat = parent_world * local;
            world.0 = world_mat;
            visited.mark(current);
            processed += 1;
            if let Some(children) = children {
                for &child in children.0.iter().rev() {
                    stack.push((child, world_mat));
                    max_stack = max_stack.max(stack.len());
                }
            }
        }
    }

    let visited_ref = &visited;
    for (entity, transform2d, transform3d, _, mut world) in nodes.iter_mut() {
        if !visited_ref.is_marked(entity) {
            world.0 = compose_local(transform2d, transform3d);
        }
    }

    scratch.stack = stack;
    scratch.visited = visited;
    stats.mode = TransformPropagationMode::Hierarchy;
    stats.total_entities = processed;
    stats.root_entities = root_count;
    stats.processed_entities = processed;
    stats.max_stack_size = max_stack as u32;
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
